use super::*;
use crate::backend::run_attempt::{Placement, RunAttempt, RunGraph};
use crate::engines::qm::{QmCalculation, QmEngine, QmJob, QmKind, QmMethod, QmOutcome, QmRequest};
use crate::io::llm::types::ToolCall;
use serde_json::{Value, json};

fn outcome(report: &str, energy: f64) -> QmOutcome {
    QmOutcome {
        energy_hartree: energy,
        converged: true,
        optimized_structure: None,
        summary: report.into(),
        scf_trace: vec![-0.5, energy],
        opt_trace: vec![],
        frequencies: vec![40.0, 80.0],
    }
}
fn query(state: &mut AppState, input: Value) -> Value {
    let result = crate::frontend::agent::tools::execute_tool(
        state,
        &ToolCall {
            id: "read".into(),
            name: "inspect".into(),
            input,
        },
    );
    assert!(!result.is_error, "{}", result.content);
    serde_json::from_str(&result.content).unwrap()
}
fn snapshot(state: &AppState, project: &ProjectSession) -> ProjectSnapshot {
    ProjectSnapshot {
        name: project.name.clone(),
        project_id: project.project_id.as_ref().unwrap().as_str().into(),
        entries: state.entries.clone(),
        tasks: state.tasks.clone(),
        materializations: state.materializations.clone(),
        view: state.project_view_settings(),
        history: state.history.clone(),
        assistant: state.ui.agent.project_snapshot(),
        warnings: vec![],
    }
}
#[test]
fn attempts_retain_exact_reports_inputs_and_facts_across_first_save_and_save_as() {
    let root = std::env::temp_dir().join(format!("silicolab-evidence-{}", uuid::Uuid::new_v4()));
    let mut state = AppState::scratch(Default::default(), vec![]);
    let task = state
        .tasks
        .create_task_run(*task_controller_by_id("qm-energy").unwrap());
    state.tasks.set_run_dir(task, root.join("scratch-run"));
    let input = state
        .entries
        .add_entry(Structure::new("original", vec![]), None, PathBuf::new());
    state.tasks.task_run_mut(task).unwrap().source_entry_id = Some(input);
    let first = state
        .tasks
        .runs
        .begin_execution(task, Placement::Local, None, 1);
    let request = QmJob::molecular(
        QmEngine::Hartree,
        QmRequest {
            structure: state.structure().clone(),
            method: QmMethod::Hf,
            basis: "sto-3g".into(),
            charge: -1,
            multiplicity: 2,
            kind: QmKind::SinglePoint,
            options: Default::default(),
            ts: None,
        },
    );
    capture_qm_input(&mut state, &first.to_string(), request);
    let report = "important method warning; 证据\n".repeat(1200);
    let cx = JobContext {
        job_id: Some(first),
        task_run_id: Some(task),
    };
    apply_qm_outcome(&mut state, &cx, outcome(&report, -1.0)).unwrap();
    let fact_id = format!("qm:{first}");
    let original =
        serde_json::to_string(&state.tasks.runs.records.get(&fact_id).unwrap().content).unwrap();
    assert!(apply_qm_outcome(&mut state, &cx, outcome("conflicting report", -99.0)).is_err());
    assert_eq!(
        serde_json::to_string(&state.tasks.runs.records.get(&fact_id).unwrap().content).unwrap(),
        original
    );
    let mut attempts = state.tasks.runs.attempts().to_vec();
    attempts.push(RunAttempt {
        run_attempt_id: 2,
        task_run_id: task,
        attempt_no: 2,
        created_at_ms: 2,
        finished_at_ms: None,
    });
    let records = std::mem::take(&mut state.tasks.runs.records);
    state.tasks.runs = RunGraph::from_rows(attempts, state.tasks.runs.executions().to_vec());
    state.tasks.runs.records = records;
    let second = state
        .tasks
        .runs
        .begin_execution(task, Placement::Local, None, 2);
    apply_qm_outcome(
        &mut state,
        &JobContext {
            job_id: Some(second),
            task_run_id: Some(task),
        },
        outcome("second report", -2.0),
    )
    .unwrap();
    state
        .entries
        .add_entry(Structure::new("now active", vec![]), None, PathBuf::new());
    let input_record = state
        .tasks
        .runs
        .records
        .get(&format!("input:{first}"))
        .unwrap();
    let Content::QmInput(job) = &input_record.content else {
        panic!("input record")
    };
    let QmCalculation::Molecular(request) = &job.calculation else {
        panic!("molecular")
    };
    assert_eq!(request.charge, -1);
    assert_eq!(request.basis, "sto-3g");
    assert_eq!(request.structure.title, "original");
    assert_eq!(input_record.input_entries, vec![input]);
    let mut offset = 0;
    let mut restored = String::new();
    loop {
        let page = query(
            &mut state,
            json!({"view":"raw","id":fact_id,"artifact":"report","byte_offset":offset}),
        );
        restored.push_str(page["text"].as_str().unwrap());
        offset = page["next_offset"].as_u64().unwrap();
        if page["truncated"] == false {
            break;
        }
    }
    assert_eq!(restored, report);
    let second_page = query(
        &mut state,
        json!({"view":"raw","id":format!("qm:{second}"),"artifact":"report"}),
    );
    assert_eq!(second_page["text"], "second report\n");
    let p1 = crate::backend::project::create_project(&root, "first").unwrap();
    let mut saved = snapshot(&state, &p1);
    let mut retry = saved.clone();
    crate::backend::records::artifacts::copy_registered_runs(&mut saved.tasks, &p1.root).unwrap();
    crate::backend::records::artifacts::copy_registered_runs(&mut retry.tasks, &p1.root).unwrap();
    crate::backend::project::save_project(&p1, &saved, true).unwrap();
    crate::backend::project::save_project(&p1, &saved, true).unwrap();
    let loaded = crate::backend::storage::load_project_snapshot(&p1).unwrap();
    assert_eq!(loaded.tasks.runs.records.all().count(), 3);
    let p2 = crate::backend::project::create_project(&root, "copy").unwrap();
    let mut copied = loaded;
    crate::backend::records::artifacts::copy_registered_runs(&mut copied.tasks, &p2.root).unwrap();
    crate::backend::project::save_project(&p2, &copied, true).unwrap();
    let loaded = crate::backend::storage::load_project_snapshot(&p2).unwrap();
    assert_ne!(p1.project_id, p2.project_id);
    assert!(
        loaded
            .tasks
            .task_run(task)
            .unwrap()
            .run_dir
            .as_ref()
            .unwrap()
            .starts_with(&p2.root)
    );
    let mut reopened = AppState::new(
        Structure::empty(),
        None,
        WorkspaceSession::Project(p2),
        Default::default(),
        vec![],
        Some(loaded),
    );
    assert_eq!(
        query(&mut reopened, json!({"view":"summary","id":fact_id}))["records"][0]["content"]["value"]
            ["energy_hartree"],
        -1.0
    );
    let page = query(
        &mut reopened,
        json!({"view":"raw","id":fact_id,"artifact":"report","byte_offset":480}),
    );
    assert!(page["truncated"].as_bool().unwrap());
    let p3 = crate::backend::project::create_project(&root, "empty").unwrap();
    let loaded = crate::backend::storage::load_project_snapshot(&p3).unwrap();
    let other = AppState::new(
        Structure::empty(),
        None,
        WorkspaceSession::Project(p3),
        Default::default(),
        vec![],
        Some(loaded),
    );
    assert_eq!(other.tasks.runs.records.all().count(), 0);
    std::fs::remove_dir_all(root).unwrap();
}

fn qm_state() -> (AppState, JobContext, PathBuf) {
    let root = std::env::temp_dir().join(format!("silicolab-evidence-{}", uuid::Uuid::new_v4()));
    let mut state = AppState::scratch(Default::default(), vec![]);
    let task = state
        .tasks
        .create_task_run(*task_controller_by_id("qm-energy").unwrap());
    state.tasks.set_run_dir(task, root.clone());
    let job = state
        .tasks
        .runs
        .begin_execution(task, Placement::Local, None, 1);
    (
        state,
        JobContext {
            job_id: Some(job),
            task_run_id: Some(task),
        },
        root,
    )
}

#[test]
fn quarantined_evidence_protects_files_and_does_not_materialize() {
    let (mut state, cx, root) = qm_state();
    let job = cx.job_id.unwrap().to_string();
    let dir = root.join("jobs").join(&job);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(QM_OUTPUT_FILE), "future report").unwrap();
    state
        .tasks
        .runs
        .records
        .restore(format!("qm:{job}"), "future data".into());
    let mut incoming = outcome("replacement", -1.0);
    incoming.optimized_structure = Some(Structure::empty());
    let error = apply_qm_outcome(&mut state, &cx, incoming).unwrap_err();
    assert!(error.to_string().contains("unavailable"));
    assert!(
        state
            .tasks
            .runs
            .execution(&job)
            .unwrap()
            .qm_result
            .is_none()
    );
    assert_eq!(
        state.tasks.runs.records.unavailable[&format!("qm:{job}")].0,
        "future data"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join(QM_OUTPUT_FILE)).unwrap(),
        "future report"
    );
    assert!(state.entries.records.is_empty());
    assert!(!state.materializations.contains(&job));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn replay_checks_facts_independently_of_restored_status_and_repairs_identical_results() {
    use crate::backend::storage::{create_project_schema, load_run_graph, write_run_graph};
    for status_json in [Some("damaged status"), None] {
        let (mut state, cx, root) = qm_state();
        let job = cx.job_id.unwrap().to_string();
        let original = outcome("original report", -1.0);
        save_qm_execution(&mut state, &job, &original).unwrap();
        let db = rusqlite::Connection::open_in_memory().unwrap();
        create_project_schema(&db).unwrap();
        write_run_graph(&db, &state.tasks.runs).unwrap();
        db.execute(
            "update job_executions set qm_result_json = ?1",
            [status_json],
        )
        .unwrap();
        state.tasks.runs = load_run_graph(&db).unwrap();
        let record =
            serde_json::to_string(state.tasks.runs.records.get(&format!("qm:{job}")).unwrap())
                .unwrap();
        let dir = root.join("jobs").join(&job);
        let series_path = dir.join(crate::backend::runs::SERIES_FILE);
        let series = std::fs::read(&series_path).unwrap();
        let mut conflicting = outcome("conflicting report", -99.0);
        conflicting.optimized_structure = Some(Structure::empty());
        let error = apply_qm_outcome(&mut state, &cx, conflicting).unwrap_err();
        assert!(error.to_string().contains("Conflicting"));
        assert_eq!(
            std::fs::read_to_string(dir.join(QM_OUTPUT_FILE)).unwrap(),
            "original report\n"
        );
        assert_eq!(std::fs::read(series_path).unwrap(), series);
        assert_eq!(
            serde_json::to_string(state.tasks.runs.records.get(&format!("qm:{job}")).unwrap())
                .unwrap(),
            record
        );
        assert!(state.entries.records.is_empty());
        assert!(!state.materializations.contains(&job));
        write_run_graph(&db, &state.tasks.runs).unwrap();
        let preserved: Option<String> = db
            .query_row("select qm_result_json from job_executions", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(preserved.as_deref(), status_json);
        apply_qm_outcome(&mut state, &cx, original.clone()).unwrap();
        assert!(state.tasks.runs.unavailable_qm_results.is_empty());
        assert!(
            state
                .tasks
                .runs
                .execution(&job)
                .unwrap()
                .qm_result
                .as_ref()
                .unwrap()
                .artifacts_complete()
        );
        apply_qm_outcome(&mut state, &cx, original).unwrap();
        assert_eq!(state.materializations.len(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn local_rerun_invalidates_task_thumbnail() {
    let (mut state, cx, root) = qm_state();
    let task = cx.task_run_id.unwrap();
    apply_qm_outcome(&mut state, &cx, outcome("first", -1.0)).unwrap();
    let first = task_chart_thumbnail(&mut state, task).unwrap();
    assert_eq!(first.series[0].points.last().unwrap()[1], -1.0);
    let second = state
        .tasks
        .runs
        .begin_execution(task, Placement::Local, None, 2);
    let cx = JobContext {
        job_id: Some(second),
        task_run_id: Some(task),
    };
    apply_qm_outcome(&mut state, &cx, outcome("second", -2.0)).unwrap();
    let chart = task_chart_thumbnail(&mut state, task).unwrap();
    assert_eq!(chart.series[0].points.last().unwrap()[1], -2.0);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn partial_artifact_failures_use_independent_registrations_without_legacy_fallback() {
    use crate::backend::run_attempt::ArtifactStatus;
    use crate::backend::runs::SERIES_FILE;
    for report_fails in [true, false] {
        let (mut state, cx, root) = qm_state();
        let task = cx.task_run_id.unwrap();
        let job = cx.job_id.unwrap().to_string();
        let entry = state
            .entries
            .add_entry(Structure::empty(), None, PathBuf::new());
        state.tasks.set_source_entry_id(task, Some(entry));
        let dir = root.join("jobs").join(&job);
        let failed_file = if report_fails {
            QM_OUTPUT_FILE
        } else {
            SERIES_FILE
        };
        std::fs::create_dir_all(dir.join(failed_file)).unwrap();
        std::fs::write(root.join(QM_OUTPUT_FILE), "unrelated legacy report").unwrap();
        crate::backend::runs::save_qm_series_file(
            &root,
            &crate::backend::runs::QmSeries::from_outcome(&outcome("legacy", -99.0)),
        )
        .unwrap();
        apply_qm_outcome(&mut state, &cx, outcome("current report", -1.0)).unwrap();
        state.tasks.mark_status(task, TaskStatus::Completed);
        let result = state
            .tasks
            .runs
            .execution(&job)
            .unwrap()
            .qm_result
            .as_ref()
            .unwrap();
        assert_eq!(result.report == ArtifactStatus::Saved, !report_fails);
        assert_eq!(result.series == ArtifactStatus::Saved, report_fails);
        assert_eq!(entry_chart_available(&mut state, entry), report_fails);
        assert_eq!(
            task_chart_thumbnail(&mut state, task).is_some(),
            report_fails
        );
        show_qm_output(&mut state, entry);
        if report_fails {
            assert!(state.ui.text_viewer.is_none());
            assert_eq!(
                entry_series_path(&state, entry).unwrap().1,
                dir.join(SERIES_FILE)
            );
        } else {
            assert_eq!(
                state.ui.text_viewer.as_ref().unwrap().text,
                "current report\n"
            );
            assert!(entry_series_path(&state, entry).is_none());
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
