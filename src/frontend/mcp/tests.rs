use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use super::handle;
use crate::backend::project::create_project;

fn project(name: &str) -> PathBuf {
    let root = PathBuf::from(format!("target/{name}"));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("h2.xyz"), "2\nh2\nH 0 0 0\nH 0 0 0.9\n").unwrap();
    create_project(&root, "demo").unwrap().root
}

fn request(id: u64, method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

fn call(project: &Path, name: &str, arguments: Value) -> (String, bool) {
    let params = json!({ "name": name, "arguments": arguments });
    let reply = handle(project, &request(9, "tools/call", params)).unwrap();
    let result = &reply["result"];
    (
        result["content"][0]["text"].as_str().unwrap().to_string(),
        result["isError"].as_bool().unwrap(),
    )
}

#[test]
fn initialize_negotiates_and_lists_the_tools() {
    let project = project("test-mcp-init");
    let reply = handle(
        &project,
        &request(1, "initialize", json!({ "protocolVersion": "2025-03-26" })),
    )
    .unwrap();
    assert_eq!(reply["id"], 1);
    assert_eq!(reply["result"]["protocolVersion"], "2025-03-26");
    assert!(reply["result"]["capabilities"]["tools"].is_object());

    let unknown = handle(
        &project,
        &request(2, "initialize", json!({ "protocolVersion": "1999-01-01" })),
    )
    .unwrap();
    assert_eq!(
        unknown["result"]["protocolVersion"],
        super::PROTOCOL_VERSIONS[0]
    );

    let notification = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
    assert!(handle(&project, &notification).is_none());

    let listed = handle(&project, &request(3, "tools/list", json!({}))).unwrap();
    let names: Vec<&str> = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["project_status", "run_commands", "command_catalog"]);

    let missing = handle(&project, &request(4, "resources/list", json!({}))).unwrap();
    assert_eq!(missing["error"]["code"], -32601);
}

#[test]
fn run_commands_applies_the_strict_rules_and_status_sees_the_result() {
    let project = project("test-mcp-run");
    let open = format!(
        "open {}",
        project.parent().unwrap().join("h2.xyz").display()
    );
    let (text, is_error) = call(&project, "run_commands", json!({ "commands": open }));
    assert!(!is_error, "{text}");

    let energy = "qm energy --method rhf --basis sto-3g";
    let (text, is_error) = call(&project, "run_commands", json!({ "commands": energy }));
    assert!(is_error);
    assert!(text.contains("was not pinned"), "{text}");

    let pinned = format!("# single point\n--entry #1@0 --request-id e1 {energy}");
    let (text, is_error) = call(&project, "run_commands", json!({ "commands": pinned }));
    assert!(!is_error, "{text}");

    let (text, is_error) = call(&project, "project_status", json!({}));
    assert!(!is_error, "{text}");
    let report: Value = serde_json::from_str(&text).unwrap();
    let energies: Vec<&Value> = report["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|task| task["task"] == "qm-energy" && task["status"] == "completed")
        .collect();
    assert_eq!(energies.len(), 1, "{report:#}");
    assert_eq!(energies[0]["request_id"], "e1");
    assert_eq!(report["project"]["open_elsewhere"], false);
}

#[test]
fn a_failing_line_is_named_after_the_output_of_earlier_lines() {
    let project = project("test-mcp-fail");
    let commands = "qm recommend general\nfrobnicate";
    let (text, is_error) = call(&project, "run_commands", json!({ "commands": commands }));
    assert!(is_error);
    assert!(text.contains("recommended level of theory"), "{text}");
    assert!(text.contains("`frobnicate` failed"), "{text}");

    let reply = handle(
        &project,
        &request(5, "tools/call", json!({ "name": "nope", "arguments": {} })),
    )
    .unwrap();
    assert_eq!(reply["error"]["code"], -32602);
}
