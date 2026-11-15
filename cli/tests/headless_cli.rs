#![cfg(unix)]

use std::process::Command;

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
    assert_eq!(report["capability"], "tasks.observe.v1");
    assert_eq!(report["complete"], false);
    assert_eq!(report["workstreams"][0]["name"], "Primary");
    assert_eq!(report["workstreams"][0]["status"], "not_sent");
    assert_eq!(report["workstreams"][1]["name"], "Secondary");
    assert_eq!(report["workstreams"][1]["status"], "not_sent");
}
