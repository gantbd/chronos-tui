//! Verify the independently distributed default client profile, not live UI.
use std::process::Command;

#[test]
fn presentation_is_unpublished_engine_free_and_has_no_peer_ui_dependency() {
    let root = env!("CARGO_PKG_NAME");
    let output = Command::new(env!("CARGO"))
        .args([
            "tree",
            "--manifest-path",
            concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"),
            "--edges",
            "normal",
            "--prefix",
            "none",
            "--locked",
            "--offline",
        ])
        .output()
        .expect("inspect default presentation profile");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let tree = String::from_utf8(output.stdout).unwrap();
    for package in tree
        .lines()
        .filter_map(|line| line.split_whitespace().next())
    {
        let peer_ui = package.ends_with("-tui") || package.contains("-gui-");
        assert!(
            !peer_ui || package == root,
            "presentation depends on peer {package}"
        );
        assert!(
            !matches!(
                package,
                "chronos-engine" | "chronos-host" | "chronos-service" | "chronos-standalone"
            ) && !package.starts_with("chronos-adapter-"),
            "default presentation imports authority side {package}"
        );
    }
    assert!(include_str!("../Cargo.toml").contains("publish = false"));
    assert!(
        !std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/main.rs")
            .exists(),
        "the reusable presentation package must not own the CLI entry point"
    );
    let cli = include_str!("../cli/Cargo.toml");
    assert!(cli.contains("name = \"chronos-tui-cli\""));
    assert!(cli.contains("chronos-tui = { path = \"..\" }"));
    let headless = include_str!("live/headless.sh");
    assert!(headless.contains("--package chronos-tui-cli"));
    assert!(headless.contains("headless --profile"));
    assert!(headless.contains("tasks.workflow.observe.v9"));
}
