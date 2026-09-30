//! End-to-end CLI tests: the thin `lumos` binary plus the library's
//! `run` / `run_app` dispatch and `write_tree` idempotency.

use std::fs;
use std::path::PathBuf;
use std::process::Command as Proc;

use lumos_cli::{render_make, run, run_app, write_tree, AppContext, MakeKind};

fn lumos() -> Proc {
    Proc::new(env!("CARGO_BIN_EXE_lumos"))
}

fn scratch(case: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lumos-cli-test-{}-{}", std::process::id(), case));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn binary_help_and_version_exit_zero() {
    for argv in [["--help"].as_slice(), &["version"], &["help", "serve"]] {
        let output = lumos().args(argv).output().unwrap();
        assert!(output.status.success(), "{argv:?}");
        assert!(!output.stdout.is_empty(), "{argv:?}");
    }
}

#[test]
fn binary_new_scaffolds_golden_tree() {
    let root = scratch("new");
    let output = lumos()
        .args(["new", "blog", "--path"])
        .arg(root.join("blog"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let app = root.join("blog");
    let cargo = fs::read_to_string(app.join("Cargo.toml")).unwrap();
    assert!(cargo.contains("name = \"blog\""), "{cargo}");
    assert!(cargo.contains("lumos-cli"), "{cargo}");
    let lib = fs::read_to_string(app.join("src/lib.rs")).unwrap();
    assert!(lib.contains("RouteRegistry"), "{lib}");
    assert!(lib.contains("route_entries"), "{lib}");
    let cli = fs::read_to_string(app.join("src/bin/cli.rs")).unwrap();
    assert!(cli.contains("run_app"), "{cli}");
    assert!(app.join("src/main.rs").exists());
    assert!(app.join("config/app.toml").exists());
    assert!(app.join("src/controllers/users.rs").exists());
}

#[test]
fn binary_rejects_app_commands_with_direction() {
    let output = lumos().args(["migrate"]).output().unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("cli"), "{stderr}");
}

#[test]
fn lib_run_reports_help_version_and_unknown() {
    assert_eq!(run(&[]), 0);
    assert_eq!(run(&["--version".to_string()]), 0);
    assert_eq!(run(&["frobnicate".to_string()]), 1);
    // App-linked commands fail with direction, not success.
    assert_eq!(run(&["migrate".to_string()]), 1);
}

#[test]
fn write_tree_is_idempotent_without_force() {
    let root = scratch("idempotent");
    let generated = render_make(MakeKind::Controller, "Widget", "20240101000000").unwrap();
    let first = write_tree(&root, &generated.files, false).unwrap();
    assert_eq!(first.len(), 1);
    assert!(root.join(&first[0]).exists());

    let again = write_tree(&root, &generated.files, false);
    assert!(again.is_err(), "second write without --force must fail");
    let forced = write_tree(&root, &generated.files, true).unwrap();
    assert_eq!(forced.len(), 1);
}

#[tokio::test]
async fn run_app_serves_route_list_and_missing_db() {
    use lumos_cli::RouteRegistry;

    let mut registry = RouteRegistry::new();
    registry.route("GET", "/users", "UsersController::index");
    let context = AppContext::new().registry(registry);
    let code = run_app(&["route:list".to_string()], &context).await;
    assert_eq!(code, 0);

    let code = run_app(&["migrate".to_string()], &context).await;
    assert_eq!(code, 1, "migrate without a db must fail loudly");
}
