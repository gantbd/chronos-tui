#![cfg(unix)]

use std::process::Command;

#[test]
fn capabilities_are_peer_owned_and_connection_free() {
    let output = Command::new(env!("CARGO_BIN_EXE_chronos-tui"))
        .arg("capabilities")
        .output()
        .expect("run capability discovery");
    assert!(output.status.success());
    let manifest: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(manifest["schema"], 1);
    assert_eq!(manifest["peer"], "chronos");
    assert_eq!(
        manifest["interactive"],
        serde_json::json!(["tasks.workflow.observe.v9", "product-alignment.observe.v1"])
    );
    assert_eq!(
        manifest["headless"],
        serde_json::json!(["tasks.workflow.observe.v9", "product-alignment.observe.v1"])
    );
}

#[test]
fn headless_reports_every_workstream_and_refuses_partial_success() {
    let scratch = std::env::temp_dir().join(format!("chronos-tui-headless-{}", std::process::id()));
    let primary = scratch.join("primary");
    let secondary = scratch.join("secondary");
    let output = Command::new(env!("CARGO_BIN_EXE_chronos-tui"))
        .arg("headless")
        .arg(&primary)
        .args(["--name", "Primary", "--workstream", "Secondary"])
        .arg(&secondary)
        .output()
        .expect("run headless CLI");

    assert!(!output.status.success());
    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("versioned JSON report");
    assert_eq!(report["version"], 1);
    assert_eq!(report["mode"], "headless");
    assert_eq!(report["peer"], "chronos");
    assert_eq!(report["capability"], "tasks.workflow.observe.v9");
    assert_eq!(report["complete"], false);
    assert_eq!(report["workstreams"][0]["name"], "Primary");
    assert_eq!(report["workstreams"][0]["status"], "not_sent");
    assert_eq!(report["workstreams"][1]["name"], "Secondary");
    assert_eq!(report["workstreams"][1]["status"], "not_sent");
}

#[test]
fn headless_configuration_failure_is_versioned_json() {
    let output = Command::new(env!("CARGO_BIN_EXE_chronos-tui"))
        .args(["headless", "--profile"])
        .output()
        .expect("run malformed headless CLI");
    assert!(!output.status.success());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["version"], 1);
    assert_eq!(report["mode"], "headless");
    assert_eq!(report["complete"], false);
    assert_eq!(report["peer"], "chronos");
    assert_eq!(report["capability"], "tasks.workflow.observe.v9");
    assert_eq!(report["status"], "configuration_refused");
    assert_eq!(report["workstreams"], serde_json::json!([]));
}
