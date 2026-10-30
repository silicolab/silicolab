use std::{collections::BTreeMap, fs, path::PathBuf};

use super::{CliScriptRequest, parse_cli_script_request, parse_named_script_args, run_cli_script};
use crate::backend::project::{create_project, open_project};

#[test]
fn parses_script_request_with_named_args() {
    let args = vec![
        "examples/demo.sls".to_string(),
        "--output".to_string(),
        "out/demo.png".to_string(),
        "--dry-run".to_string(),
        "--width=1200".to_string(),
    ];
    let request = parse_cli_script_request(&args)
        .unwrap()
        .expect("script request should parse");

    assert_eq!(request.script_path.to_string_lossy(), "examples/demo.sls");
    assert_eq!(request.script_args.get("output").unwrap(), "out/demo.png");
    assert_eq!(request.script_args.get("dry_run").unwrap(), "true");
    assert_eq!(request.script_args.get("width").unwrap(), "1200");
}

#[test]
fn rejects_unexpected_positional_script_args() {
    let error = parse_named_script_args(&["plain".to_string()]).expect_err("should fail");
    assert!(error.to_string().contains("unexpected positional argument"));
}

#[test]
fn parses_project_flag_before_script() {
    let args = ["--project", "proj", "run.sls", "--basis", "sto-3g"].map(String::from);
    let request = parse_cli_script_request(&args)
        .unwrap()
        .expect("script request should parse");
    assert_eq!(request.project, Some(PathBuf::from("proj")));
    assert_eq!(request.script_path, PathBuf::from("run.sls"));
    assert_eq!(request.script_args.get("basis").unwrap(), "sto-3g");

    let error = parse_cli_script_request(&["--project".to_string()]).unwrap_err();
    assert!(error.to_string().contains("project directory"), "{error}");
}

#[test]
fn project_runs_persist_across_invocations() {
    let root = PathBuf::from("target/test-cli-project");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let project = create_project(&root, "demo").unwrap();
    fs::write(root.join("h2.xyz"), "2\nh2\nH 0 0 0\nH 0 0 0.74\n").unwrap();
    let script = root.join("step.sls");
    fs::write(
        &script,
        format!(
            "open {}\nqm energy --method rhf --basis sto-3g\n",
            root.join("h2.xyz").display()
        ),
    )
    .unwrap();
    let request = CliScriptRequest {
        project: Some(project.root.clone()),
        strict: false,
        script_path: script,
        script_args: BTreeMap::new(),
    };

    run_cli_script(&request).unwrap();
    run_cli_script(&request).unwrap();

    let (_, snapshot) = open_project(&project.root).unwrap();
    assert_eq!(snapshot.entries.records.len(), 2);
    assert_eq!(snapshot.tasks.tasks.len(), 2);
    assert!(snapshot.warnings.is_empty(), "{:?}", snapshot.warnings);
}

#[test]
fn project_open_in_another_session_is_refused() {
    let root = PathBuf::from("target/test-cli-locked");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let project = create_project(&root, "demo").unwrap();
    let script = root.join("noop.sls");
    fs::write(&script, "# nothing\n").unwrap();
    let (held, _) = crate::backend::housekeeping::acquire_lock(&project).unwrap();
    let request = CliScriptRequest {
        project: Some(project.root.clone()),
        strict: false,
        script_path: script,
        script_args: BTreeMap::new(),
    };

    let error = run_cli_script(&request).unwrap_err();
    assert!(format!("{error:#}").contains("already open"), "{error:#}");
    held.release();
    run_cli_script(&request).unwrap();
}

#[test]
fn status_reports_results_made_stale_by_a_later_edit() {
    let root = PathBuf::from("target/test-cli-status");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let project = create_project(&root, "demo").unwrap();
    fs::write(root.join("o.xyz"), "1\no\nO 0 0 0\n").unwrap();
    let run = |name: &str, body: String| {
        let script = root.join(name);
        fs::write(&script, body).unwrap();
        run_cli_script(&CliScriptRequest {
            project: Some(project.root.clone()),
            strict: false,
            script_path: script,
            script_args: BTreeMap::new(),
        })
        .unwrap();
    };
    let status = || -> serde_json::Value {
        let args = ["--project".to_string(), project.root.display().to_string()];
        serde_json::from_str(&super::run_cli_status(&args).unwrap()).unwrap()
    };

    run(
        "energy.sls",
        format!(
            "open {}\nhydrogen add\nqm energy --method rhf --basis sto-3g\n",
            root.join("o.xyz").display()
        ),
    );
    let report = status();
    assert_eq!(report["summary"]["stale"], 0, "{report:#}");
    assert_eq!(report["project"]["open_elsewhere"], false);
    let energy = report["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["task"] == "qm-energy")
        .unwrap();
    assert_eq!(energy["freshness"], "current", "{report:#}");

    run("edit.sls", "delete chain A\nhydrogen add\n".to_string());
    let report = status();
    let energy = report["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["task"] == "qm-energy")
        .unwrap();
    assert_eq!(energy["freshness"], "stale", "{report:#}");
    assert_eq!(energy["stale_reasons"][0]["kind"], "input_edited");
}

#[test]
fn status_never_writes_even_to_an_unmigrated_project() {
    let root = PathBuf::from("target/test-cli-status-readonly");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let project = create_project(&root, "demo").unwrap();
    fs::write(root.join("o.xyz"), "1\no\nO 0 0 0\n").unwrap();
    let script = root.join("energy.sls");
    fs::write(
        &script,
        format!(
            "open {}\nqm energy --method rhf --basis sto-3g\n",
            root.join("o.xyz").display()
        ),
    )
    .unwrap();
    run_cli_script(&CliScriptRequest {
        project: Some(project.root.clone()),
        strict: false,
        script_path: script,
        script_args: BTreeMap::new(),
    })
    .unwrap();
    // Simulate a project last saved before result revisions existed.
    rusqlite::Connection::open(&project.project_db)
        .unwrap()
        .execute("alter table task_runs drop column result_revision", [])
        .unwrap();
    let before = fs::read(&project.project_db).unwrap();

    let args = ["--project".to_string(), project.root.display().to_string()];
    let report: serde_json::Value =
        serde_json::from_str(&super::run_cli_status(&args).unwrap()).unwrap();

    assert_eq!(fs::read(&project.project_db).unwrap(), before);
    assert_eq!(report["tasks"][0]["freshness"], "current", "{report:#}");
    assert!(report["tasks"][0]["result_revision"].is_null());
}

#[test]
fn strict_needs_a_project() {
    let args = ["--strict", "run.sls"].map(String::from);
    let error = parse_cli_script_request(&args).unwrap_err();
    assert!(error.to_string().contains("--project"), "{error}");
    let args = ["--project", "proj", "--strict", "run.sls"].map(String::from);
    let request = parse_cli_script_request(&args).unwrap().unwrap();
    assert!(request.strict);
}

struct StrictProject {
    root: PathBuf,
    project: PathBuf,
}

impl StrictProject {
    fn new(name: &str) -> Self {
        let root = PathBuf::from(format!("target/{name}"));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let project = create_project(&root, "demo").unwrap().root;
        fs::write(root.join("h2.xyz"), "2\nh2\nH 0 0 0\nH 0 0 0.9\n").unwrap();
        let this = Self { root, project };
        this.run(
            "open.sls",
            &format!("open {}", this.root.join("h2.xyz").display()),
            false,
        )
        .unwrap();
        this
    }

    fn run(&self, name: &str, body: &str, strict: bool) -> anyhow::Result<Vec<String>> {
        let script = self.root.join(name);
        fs::write(&script, format!("{body}\n")).unwrap();
        run_cli_script(&CliScriptRequest {
            project: Some(self.project.clone()),
            strict,
            script_path: script,
            script_args: BTreeMap::new(),
        })
        .map(|result| result.stdout_lines)
    }

    fn strict(&self, body: &str) -> anyhow::Result<Vec<String>> {
        self.run("strict.sls", body, true)
    }

    fn status(&self) -> serde_json::Value {
        let args = ["--project".to_string(), self.project.display().to_string()];
        serde_json::from_str(&super::run_cli_status(&args).unwrap()).unwrap()
    }

    fn tasks(&self, controller: &str) -> Vec<serde_json::Value> {
        self.status()["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|task| task["task"] == controller)
            .cloned()
            .collect()
    }
}

const ENERGY: &str = "qm energy --method rhf --basis sto-3g";

#[test]
fn strict_runs_need_pinned_inputs_at_the_expected_revision() {
    let p = StrictProject::new("test-cli-strict-pins");
    let entry = p.status()["entries"][0].clone();
    let (id, revision) = (&entry["id"], &entry["revision"]);

    let error = p.strict(ENERGY).unwrap_err();
    assert!(format!("{error:#}").contains("was not pinned"), "{error:#}");
    let error = p
        .strict(&format!(
            "--entry #{id}@{} {ENERGY}",
            revision.as_u64().unwrap() + 1
        ))
        .unwrap_err();
    assert!(format!("{error:#}").contains("is at revision"), "{error:#}");

    p.strict(&format!("--entry #{id}@{revision} {ENERGY}"))
        .unwrap();
    let energies = p.tasks("qm-energy");
    assert_eq!(energies.last().unwrap()["status"], "completed");
    // Without --strict the same unpinned line still runs on the active entry.
    p.run("loose.sls", ENERGY, false).unwrap();
}

#[test]
fn a_retried_request_id_does_not_run_again() {
    let p = StrictProject::new("test-cli-strict-request");
    let line = format!("--entry #1 --request-id e1 {ENERGY}");
    p.strict(&line).unwrap();
    let again = p.strict(&line).unwrap();
    assert!(again[0].contains("already completed"), "{again:?}");
    let energies = p.tasks("qm-energy");
    assert_eq!(energies.len(), 1);
    assert_eq!(energies[0]["request_id"], "e1");
}

#[test]
fn stale_upstream_results_are_refused_unless_allowed() {
    let p = StrictProject::new("test-cli-strict-stale");
    p.strict("--entry #1 qm optimize --method rhf --basis sto-3g")
        .unwrap();
    let optimized = p.tasks("qm-optimize")[0]["result_entry"].as_u64().unwrap();
    p.strict(&format!("--entry #{optimized} {ENERGY}")).unwrap();
    p.run("edit.sls", "activate #1\nhydrogen add", false)
        .unwrap();

    let error = p
        .strict(&format!("--entry #{optimized} {ENERGY}"))
        .unwrap_err();
    assert!(format!("{error:#}").contains("--allow-stale"), "{error:#}");
    p.strict(&format!("--entry #{optimized} --allow-stale {ENERGY}"))
        .unwrap();
    let energies = p.tasks("qm-energy");
    assert_eq!(energies.last().unwrap()["status"], "completed");
    assert_eq!(energies.last().unwrap()["stale_inputs_accepted"], true);
    assert_eq!(energies[0]["stale_inputs_accepted"], false);
}

#[test]
fn strict_md_needs_a_build_of_the_current_revision() {
    let p = StrictProject::new("test-cli-strict-md");
    // The native MD build only parameterizes noble gases.
    let argon = p.root.join("ar.xyz");
    fs::write(&argon, "2\nar\nAr 0 0 0\nAr 0 0 4\n").unwrap();
    p.run("ar.sls", &format!("open {}", argon.display()), false)
        .unwrap();
    let error = p.strict("--entry #2 md simulate").unwrap_err();
    assert!(format!("{error:#}").contains("no MD build"), "{error:#}");

    p.strict("--entry #2 md build").unwrap();
    let built = p.tasks("build-md-system")[0]["result_entry"]
        .as_u64()
        .unwrap();
    p.run(
        "edit.sls",
        &format!("activate #{built}\nhydrogen add"),
        false,
    )
    .unwrap();
    let error = p
        .strict(&format!("--entry #{built} md simulate"))
        .unwrap_err();
    assert!(
        format!("{error:#}").contains("changed after its MD build"),
        "{error:#}"
    );
}

#[test]
fn strict_scripts_write_only_inside_the_project() {
    let p = StrictProject::new("test-cli-strict-write");
    let outside = p.root.join("out.xyz");
    let error = p
        .strict(&format!("export {}", outside.display()))
        .unwrap_err();
    assert!(
        format!("{error:#}").contains("outside the project"),
        "{error:#}"
    );
    assert!(!outside.exists());

    let inside = p.project.join("out.xyz");
    p.strict(&format!("export {}", inside.display())).unwrap();
    assert!(inside.exists());
}
