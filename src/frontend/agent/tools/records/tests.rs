use super::*;
use crate::frontend::agent::{session::AgentPhase, tools};

fn call(id: &str, text: &str) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: "save_constraint".into(),
        input: json!({"text":text}),
    }
}
fn approve(state: &mut AppState, call: ToolCall) {
    state.ui.agent.selection.provider = "test-no-provider".into();
    state.ui.agent.pending_calls.push_back(call.clone());
    state.ui.agent.phase = AgentPhase::AwaitingApproval;
    crate::frontend::dispatcher::dispatch(
        state,
        crate::frontend::actions::AppAction::ApproveToolCall(call.id),
        &eframe::egui::Context::default(),
    );
}
#[test]
fn only_actual_approval_action_can_write_and_replacement_is_explicit() {
    let mut state = AppState::scratch(Default::default(), vec![]);
    let first = call("first", "Keep molecular charge zero");
    assert!(tools::execute_tool(&mut state, &first).is_error);
    let mut forged = first.clone();
    forged.input["confirmed"] = json!(true);
    assert!(tools::execute_tool(&mut state, &forged).is_error);
    for mode in crate::backend::config::ApprovalMode::all() {
        assert!(tools::needs_confirmation(
            &first,
            mode,
            &["save_constraint".into()].into_iter().collect(),
            &[crate::frontend::console::RiskLevel::FileWrite]
                .into_iter()
                .collect()
        ));
    }
    approve(&mut state, first);
    assert_eq!(state.tasks.runs.records.all().count(), 1);
    assert!(state.scratch_has_unsaved_content());
    let id = state.tasks.runs.records.all().next().unwrap().id.clone();
    let mut next = call("second", "Use charge one");
    next.input["replaces"] = json!(id);
    approve(&mut state, next);
    assert_eq!(state.tasks.runs.records.all().count(), 2);
    let context = state.tasks.runs.records.context(
        None,
        None,
        state.ui.agent.active_conversation.raw(),
        2000,
    );
    assert!(context.contains("Use charge one"));
    assert!(!context.contains("Keep molecular"));
    assert!(
        !state
            .tasks
            .runs
            .records
            .active(state.tasks.runs.records.get(&id).unwrap())
    );
    assert!(inspect(&state, &json!({"id":id,"lifecycle":"active"})).is_err());
    state.ui.agent.start_new_conversation(Default::default());
    assert!(
        state
            .tasks
            .runs
            .records
            .context(None, None, state.ui.agent.active_conversation.raw(), 2000)
            .is_empty()
    );
}
#[test]
fn stable_pages_reach_all_rows_and_conflicts_never_fall_back() {
    let mut state = AppState::scratch(Default::default(), vec![]);
    for n in 0..33 {
        let c = call(&format!("c{n}"), &format!("Constraint {n}"));
        state.ui.agent.approved_ids.insert(c.id.clone());
        save_constraint(&mut state, &c).unwrap();
    }
    let mut offset = 0;
    let mut ids = std::collections::HashSet::new();
    loop {
        let text = inspect(
            &state,
            &json!({"view":"catalog","offset":offset,"category":"memory"}),
        )
        .unwrap();
        assert!(text.chars().count() < 4000);
        let page: Value = serde_json::from_str(&text).unwrap();
        for row in page["records"].as_array().unwrap() {
            assert!(ids.insert(row["id"].as_str().unwrap().to_owned()));
        }
        offset = page["next_offset"].as_u64().unwrap();
        if !page["truncated"].as_bool().unwrap() {
            break;
        }
    }
    assert_eq!(ids.len(), 33);
    assert!(
        inspect(&state, &json!({"id":"missing"}))
            .unwrap_err()
            .to_string()
            .contains("not found")
    );
    assert!(inspect(&state, &json!({"query":"missing"})).is_err());
    assert!(inspect(&state, &json!({"view":"workspace","category":"memory"})).is_err());
    assert!(inspect(&state, &json!({"view":"raw","job":"missing"})).is_err());
    assert!(inspect(&state, &json!({"view":"summary","byte_offset":2})).is_err());
    assert!(
        !inspect(&state, &json!({}))
            .unwrap()
            .contains("Constraint 0")
    );
}
#[test]
fn unknown_rows_are_paged_and_details_are_budgeted() {
    let mut state = AppState::scratch(Default::default(), vec![]);
    for i in 0..25 {
        state
            .tasks
            .runs
            .records
            .restore(format!("broken:{i:02}"), "not json".into());
    }
    let text = inspect(&state, &json!({"view":"unavailable","offset":20})).unwrap();
    let page: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(page["records"].as_array().unwrap().len(), 5);
    assert!(
        inspect(&state, &json!({"id":"broken:00"}))
            .unwrap_err()
            .to_string()
            .contains("unavailable")
    );
    let c = call("long", &"约束".repeat(500));
    state.ui.agent.approved_ids.insert(c.id.clone());
    save_constraint(&mut state, &c).unwrap();
    let id = &state.tasks.runs.records.all().next().unwrap().id;
    let mut offset = 0;
    let mut restored = String::new();
    loop {
        let text = inspect(
            &state,
            &json!({"view":"details","id":id,"detail_offset":offset}),
        )
        .unwrap();
        assert!(text.chars().count() < 4000);
        let page: Value = serde_json::from_str(&text).unwrap();
        restored.push_str(page["text"].as_str().unwrap());
        offset = page["next_detail_offset"].as_u64().unwrap();
        if page["truncated"] == false {
            break;
        }
    }
    assert_eq!(serde_json::from_str::<Record>(&restored).unwrap().id, *id);
}

#[test]
fn constraints_survive_project_reopen_and_failed_commit_is_not_reported_as_persisted() {
    use crate::backend::project::{WorkspaceSession, create_project, save_project};
    use crate::backend::storage::load_project_snapshot;
    use crate::domain::Structure;
    let root = std::env::temp_dir().join(format!("silicolab-constraints-{}", uuid::Uuid::new_v4()));
    let mut state = AppState::scratch(Default::default(), vec![]);
    approve(&mut state, call("one", "Preserve the input geometry"));
    let id = state.tasks.runs.records.all().next().unwrap().id.clone();
    let session = state.ui.agent.active_conversation.raw();
    let project = create_project(&root, "a").unwrap();
    state.workspace = WorkspaceSession::Project(project.clone());
    save_project(&project, &state.project_snapshot().unwrap(), true).unwrap();
    let loaded = load_project_snapshot(&project).unwrap();
    let mut reopened = AppState::new(
        Structure::empty(),
        None,
        WorkspaceSession::Project(project.clone()),
        Default::default(),
        vec![],
        Some(loaded),
    );
    assert!(
        reopened
            .tasks
            .runs
            .records
            .context(None, None, session, 2000)
            .contains("Preserve the input geometry")
    );
    let mut next = call("two", "Allow geometry relaxation");
    next.input["replaces"] = json!(id);
    approve(&mut reopened, next);
    let loaded = load_project_snapshot(&project).unwrap();
    assert_eq!(loaded.tasks.runs.records.all().count(), 2);
    assert!(
        !loaded
            .tasks
            .runs
            .records
            .active(loaded.tasks.runs.records.get(&id).unwrap())
    );
    let backup = project.project_db.with_extension("backup");
    std::fs::rename(&project.project_db, &backup).unwrap();
    std::fs::create_dir(&project.project_db).unwrap();
    approve(&mut reopened, call("three", "Retain charge zero"));
    assert!(reopened.project_save_error().is_some());
    assert!(reopened.tasks.runs.records.is_dirty());
    assert!(format!("{:?}", reopened.ui.agent.transcript).contains("not persisted"));
    std::fs::remove_dir(&project.project_db).unwrap();
    std::fs::rename(backup, &project.project_db).unwrap();
    assert_eq!(
        load_project_snapshot(&project)
            .unwrap()
            .tasks
            .runs
            .records
            .all()
            .count(),
        2
    );
    let db = rusqlite::Connection::open(&project.project_db).unwrap();
    db.execute("update assistant_state set payload = x'00'", [])
        .unwrap();
    drop(db);
    let loaded = load_project_snapshot(&project).unwrap();
    let repaired = AppState::new(
        Structure::empty(),
        None,
        WorkspaceSession::Project(project.clone()),
        Default::default(),
        vec![],
        Some(loaded),
    );
    assert_ne!(repaired.ui.agent.active_conversation.raw(), session);
    assert!(
        repaired
            .tasks
            .runs
            .records
            .context(
                None,
                None,
                repaired.ui.agent.active_conversation.raw(),
                2000
            )
            .is_empty()
    );
    let other = create_project(&root, "b").unwrap();
    let other = load_project_snapshot(&other).unwrap();
    assert!(
        other
            .tasks
            .runs
            .records
            .context(None, None, session, 2000)
            .is_empty()
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn budget_omitted_user_intent_is_still_explicitly_retrievable() {
    let mut state = AppState::scratch(Default::default(), vec![]);
    let original = "User goal 证据\n".repeat(500);
    state
        .ui
        .agent
        .transcript
        .push(crate::frontend::agent::TranscriptEntry::User(
            original.clone().into(),
        ));
    let mut restored = String::new();
    let mut offset = 0;
    loop {
        let text = inspect(&state, &json!({"view":"intent","detail_offset":offset})).unwrap();
        assert!(text.chars().count() < 4000);
        let page: Value = serde_json::from_str(&text).unwrap();
        restored.push_str(page["text"].as_str().unwrap());
        offset = page["next_detail_offset"].as_u64().unwrap();
        if page["truncated"] == false {
            break;
        }
    }
    assert_eq!(restored, original);
    assert_eq!(state.tasks.runs.records.all().count(), 0);
}
