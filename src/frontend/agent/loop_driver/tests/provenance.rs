use super::*;
use crate::backend::tasks::{TaskStatus, task_controller_by_id};
use crate::frontend::dispatcher::{bind_task_inputs, ensure_task_run_dir};

fn add_input(state: &mut AppState, title: &str) -> u64 {
    let mut structure = crate::domain::Structure::empty();
    structure.title = title.to_string();
    state.entries.add_entry(
        structure,
        None,
        std::path::PathBuf::from(format!("{title}.xyz")),
    )
}

fn assert_stopped(state: &AppState, count: usize) {
    assert!(state.ui.agent.pending_calls.is_empty());
    assert!(state.ui.agent.approved_ids.is_empty());
    assert!(state.ui.agent.approval_inputs.is_none());
    let message = state
        .ui
        .agent
        .history
        .iter()
        .rev()
        .find(|m| m.role == Role::Tool)
        .unwrap();
    assert_eq!(message.content.len(), count);
    let mut ids = std::collections::HashSet::new();
    for block in &message.content {
        if let ContentBlock::ToolResult {
            tool_use_id,
            is_error,
            ..
        } = block
        {
            assert!(ids.insert(tool_use_id));
            assert!(*is_error);
        } else {
            panic!("expected tool result");
        }
    }
}

#[test]
fn implicit_input_switch_and_revision_change_stop_approved_compute() {
    for change_revision in [false, true] {
        let mut state = AppState::scratch(Default::default(), Vec::new());
        let first = add_input(&mut state, "first");
        let ctx = egui::Context::default();
        handle_turn_result(
            &mut state,
            Ok(turn_with_tools(&["qm energy", "status"])),
            &ctx,
        );
        assert_eq!(
            state.ui.agent.approval_inputs.as_ref().unwrap().1[0].entry_id,
            first
        );
        if change_revision {
            state.entries.bump_active_revision();
        } else {
            add_input(&mut state, "second");
        }
        approve_tool_call(&mut state, "call_1", &ctx);
        assert_stopped(&state, 2);
        assert!(last_tool_results(&state)["call_1"].contains("inputs changed"));
        assert!(state.jobs.agent_jobs.is_empty());
        assert!(state.tasks.tasks.is_empty());
    }
}

#[test]
fn product_revision_is_part_of_approval() {
    let mut state = AppState::scratch(Default::default(), Vec::new());
    let product = add_input(&mut state, "product");
    add_input(&mut state, "reactant");
    let ctx = egui::Context::default();
    handle_turn_result(
        &mut state,
        Ok(turn_with_tool(&format!("qm ts --product {product}"))),
        &ctx,
    );
    let inputs = &state.ui.agent.approval_inputs.as_ref().unwrap().1;
    assert_eq!(inputs.len(), 2);
    assert_eq!(inputs[1].role, "product");
    state.entries.entry_mut(product).unwrap().revision += 1;
    approve_tool_call(&mut state, "call_1", &ctx);
    assert_stopped(&state, 1);
    assert!(last_tool_results(&state)["call_1"].contains("inputs changed"));
}

#[test]
fn explicit_docking_inputs_ignore_unrelated_active_entry() {
    let mut state = AppState::scratch(Default::default(), Vec::new());
    let receptor = add_input(&mut state, "receptor");
    let ligand = add_input(&mut state, "ligand");
    let ctx = egui::Context::default();
    handle_turn_result(
        &mut state,
        Ok(turn_with_tool(&format!(
            "dock --receptor {receptor} --ligand {ligand}"
        ))),
        &ctx,
    );
    add_input(&mut state, "unrelated");
    let call = state.ui.agent.pending_calls.front().unwrap();
    assert_eq!(
        heavy_inputs(&state, call).unwrap().unwrap(),
        state.ui.agent.approval_inputs.as_ref().unwrap().1
    );
    approve_tool_call(&mut state, "call_1", &ctx);
    assert!(!last_tool_results(&state)["call_1"].contains("inputs changed"));
}

#[test]
fn preparation_failure_keeps_binding_and_initial_manifest_errors_propagate() {
    let mut state = AppState::scratch(Default::default(), Vec::new());
    let input = add_input(&mut state, "input");
    let controller = *task_controller_by_id("qm-energy").unwrap();
    let task = state.tasks.create_task_run(controller);
    let reference = crate::frontend::entry_ref::primary_input(&state).unwrap();
    bind_task_inputs(&mut state, task, vec![reference.clone()]).unwrap();
    let run_dir = ensure_task_run_dir(&mut state, task, controller.kind, None).unwrap();
    std::fs::remove_file(run_dir.join("manifest.json")).unwrap();
    std::fs::create_dir(run_dir.join("manifest.json")).unwrap();
    add_input(&mut state, "other");
    assert!(ensure_task_run_dir(&mut state, task, controller.kind, None).is_err());
    let task = state.tasks.task_run(task).unwrap();
    assert_eq!(task.source_entry_id, Some(input));
    assert_eq!(task.inputs, Some(vec![reference]));
    std::fs::remove_dir_all(run_dir).unwrap();
}

#[test]
fn rerunning_panel_creates_a_distinct_bound_run() {
    let mut state = AppState::scratch(Default::default(), Vec::new());
    add_input(&mut state, "input");
    let controller = *task_controller_by_id("qm-energy").unwrap();
    let original = state.tasks.create_task_run(controller);
    state.tasks.open_panel(original);
    let input = crate::frontend::entry_ref::primary_input(&state).unwrap();
    let first = crate::frontend::dispatcher::prepare_compute_run(
        &mut state,
        controller.panel,
        vec![input.clone()],
        None,
    )
    .unwrap();
    state.tasks.mark_status(original, TaskStatus::Completed);
    crate::frontend::dispatcher::run_task(&mut state, original);
    assert_eq!(
        state.tasks.task_run(original).unwrap().status,
        TaskStatus::Completed
    );
    let second = crate::frontend::dispatcher::prepare_compute_run(
        &mut state,
        controller.panel,
        vec![input],
        None,
    )
    .unwrap();
    assert_ne!(first, second);
    assert_ne!(state.active_task_run, Some(original));
    assert_eq!(
        state.tasks.task_run(original).unwrap().status,
        TaskStatus::Completed
    );
    std::fs::remove_dir_all(first).unwrap();
    std::fs::remove_dir_all(second).unwrap();
}

#[test]
fn deleted_input_and_failed_activation_stop_batch_without_side_effects() {
    let ctx = egui::Context::default();
    let mut state = AppState::scratch(Default::default(), Vec::new());
    let id = add_input(&mut state, "A");
    handle_turn_result(
        &mut state,
        Ok(turn_with_tools(&["qm energy", "status"])),
        &ctx,
    );
    assert!(state.entries.delete_entry(id));
    approve_tool_call(&mut state, "call_1", &ctx);
    assert_stopped(&state, 2);
    assert!(state.jobs.agent_jobs.is_empty());

    state.config.assistant.approval_mode = crate::backend::config::ApprovalMode::AutoSafe;
    handle_turn_result(
        &mut state,
        Ok(turn_with_tools(&["activate #999999", "qm energy"])),
        &ctx,
    );
    if state.ui.agent.phase == AgentPhase::AwaitingApproval {
        approve_tool_call(&mut state, "call_1", &ctx);
    }
    assert_stopped(&state, 2);
    assert!(last_tool_results(&state)["call_2"].contains("Not executed"));
    assert!(state.tasks.tasks.is_empty());
}

#[test]
fn busy_heavy_tool_stops_later_read_only_call() {
    let mut state = AppState::scratch(Default::default(), Vec::new());
    add_input(&mut state, "A");
    let (_sender, receiver) = std::sync::mpsc::channel();
    let conversation = state.ui.agent.active_conversation;
    state
        .jobs
        .agent_jobs
        .push(super::fake_qm_job(1, conversation, receiver));
    let ctx = egui::Context::default();
    handle_turn_result(
        &mut state,
        Ok(turn_with_tools(&["qm energy", "status"])),
        &ctx,
    );
    approve_tool_call(&mut state, "call_1", &ctx);
    assert_stopped(&state, 2);
    assert!(last_tool_results(&state)["call_1"].contains("already running"));
    assert!(last_tool_results(&state)["call_2"].contains("Not executed"));
    assert_eq!(state.jobs.agent_jobs.len(), 1);
}

#[test]
fn agent_directory_failure_records_failed_run_and_never_spawns_worker() {
    use crate::backend::project::{ProjectSession, WorkspaceSession};
    let root =
        std::env::temp_dir().join(format!("silicolab-start-failure-{}", uuid::Uuid::new_v4()));
    let project = ProjectSession::from_root(root.clone(), "failure".into());
    crate::backend::storage::initialize_project_databases(&project).unwrap();
    std::fs::write(root.join("runs"), "not a directory").unwrap();
    let mut state = AppState::scratch(Default::default(), Vec::new());
    state.workspace = WorkspaceSession::Project(project);
    let structure = crate::domain::Structure::new(
        "helium",
        vec![crate::domain::Atom {
            element: "He".into(),
            position: nalgebra::Point3::origin(),
            charge: 0.0,
        }],
    );
    let id = state
        .entries
        .add_entry(structure, None, root.join("helium.xyz"));
    let ctx = egui::Context::default();
    handle_turn_result(
        &mut state,
        Ok(turn_with_tools(&[
            "qm energy --method hf --basis sto-3g",
            "md simulate --time 1",
        ])),
        &ctx,
    );
    approve_tool_call(&mut state, "call_1", &ctx);
    assert_stopped(&state, 2);
    assert!(state.jobs.agent_jobs.is_empty());
    assert_eq!(state.tasks.tasks.len(), 1);
    let task = &state.tasks.tasks[0];
    assert_eq!(task.status, TaskStatus::Failed);
    assert_eq!(task.source_entry_id, Some(id));
    assert!(state.tasks.runs.executions().is_empty());
    assert!(last_tool_results(&state)["call_2"].contains("Not executed"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn gui_manifest_failure_keeps_form_and_never_spawns_qm_worker() {
    let mut state = AppState::scratch(Default::default(), Vec::new());
    let structure = crate::domain::Structure::new(
        "helium",
        vec![crate::domain::Atom {
            element: "He".into(),
            position: nalgebra::Point3::origin(),
            charge: 0.0,
        }],
    );
    state
        .entries
        .add_entry(structure, None, std::path::PathBuf::from("helium.xyz"));
    let controller = *task_controller_by_id("qm-energy").unwrap();
    let task = state.tasks.create_task_run(controller);
    state.tasks.open_panel(task);
    let dir = std::env::temp_dir().join(format!(
        "silicolab-manifest-failure-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(dir.join("manifest.json")).unwrap();
    state.tasks.set_run_dir(task, dir.clone());
    let prompt = crate::frontend::state::QmPrompt::new(crate::engines::qm::QmKind::SinglePoint);
    state.ui.pending_qm = Some(prompt);
    crate::frontend::dispatcher::start_pending_qm(&mut state);
    assert!(!state.jobs.qm_running());
    assert!(state.tasks.runs.executions().is_empty());
    assert_eq!(
        state.tasks.task_run(task).unwrap().status,
        TaskStatus::Failed
    );
    assert!(state.ui.pending_qm.is_some());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn activation_before_compute_snapshots_the_new_head_input() {
    let mut state = AppState::scratch(Default::default(), Vec::new());
    let a = add_input(&mut state, "A");
    state
        .entries
        .entry_mut(a)
        .unwrap()
        .structure
        .atoms
        .push(crate::domain::Atom {
            element: "He".into(),
            position: nalgebra::Point3::origin(),
            charge: 0.0,
        });
    add_input(&mut state, "B");
    let ctx = egui::Context::default();
    handle_turn_result(
        &mut state,
        Ok(turn_with_tools(&[
            &format!("activate #{a}"),
            "qm energy --method hf --basis sto-3g",
        ])),
        &ctx,
    );
    if state.ui.agent.pending_calls.front().unwrap().id == "call_1" {
        approve_tool_call(&mut state, "call_1", &ctx);
    }
    assert_eq!(state.ui.agent.phase, AgentPhase::AwaitingApproval);
    assert_eq!(
        state.ui.agent.approval_inputs.as_ref().unwrap().1[0].entry_id,
        a
    );
    assert_eq!(state.ui.agent.pending_calls.front().unwrap().id, "call_2");
    approve_tool_call(&mut state, "call_2", &ctx);
    assert!(!last_tool_results(&state)["call_2"].contains("inputs changed"));
    assert_eq!(state.jobs.agent_jobs.len(), 1);
    assert_eq!(state.tasks.tasks[0].source_entry_id, Some(a));
    let conversation = state.ui.agent.active_conversation;
    cancel_conversation_jobs(&mut state, conversation);
}
