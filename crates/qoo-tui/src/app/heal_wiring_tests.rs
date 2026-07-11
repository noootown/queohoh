use super::*;
use crate::event::{Cmd, Event};
use crate::ipc::types::StateSnapshot;
use serial_test::serial;
use std::path::PathBuf;

// The env-mutating tests below touch process-global `QUEOHOH_DAEMON_DIST`.
// `#[serial]` (serial_test's global lock, default group) excludes them not
// only against each other but against `paths.rs`'s `#[serial]` env tests,
// which mutate the same var — a local Mutex would not cross that boundary.

fn dist_with_js() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("cli.js"), "x").unwrap();
    d
}

fn app() -> App {
    App::new(std::env::temp_dir(), PathBuf::from("/tmp/qoo-nope.sock"))
}

fn snap(build_id: Option<&str>, running: Vec<String>) -> StateSnapshot {
    StateSnapshot {
        build_id: build_id.map(str::to_string),
        running,
        ..Default::default()
    }
}

#[test]
#[serial]
fn restart_now_sets_status_records_guard_and_emits_heal() {
    let dist = dist_with_js();
    unsafe { std::env::set_var("QUEOHOH_DAEMON_DIST", dist.path()) };
    let disk = crate::heal::disk_build_id(dist.path());

    let mut app = app();
    let upd = app.update(Event::Snapshot(snap(Some("stale-build"), vec![])));

    assert_eq!(app.status_line.as_deref(), Some("daemon outdated — restarting…"));
    assert_eq!(app.last_healed_build_id.as_deref(), Some(disk.as_str()));
    assert!(upd.cmds.iter().any(|c| matches!(c, Cmd::Heal)));
    unsafe { std::env::remove_var("QUEOHOH_DAEMON_DIST") };
}

#[test]
#[serial]
fn defer_when_task_running_no_heal_cmd() {
    let dist = dist_with_js();
    unsafe { std::env::set_var("QUEOHOH_DAEMON_DIST", dist.path()) };

    let mut app = app();
    let upd = app.update(Event::Snapshot(snap(Some("stale-build"), vec!["t1".into()])));

    assert_eq!(app.status_line.as_deref(), Some("daemon outdated — will restart when idle"));
    assert!(app.last_healed_build_id.is_none());
    assert!(!upd.cmds.iter().any(|c| matches!(c, Cmd::Heal)));
    unsafe { std::env::remove_var("QUEOHOH_DAEMON_DIST") };
}

#[test]
#[serial]
fn declined_loop_guard_says_restart_manually() {
    let dist = dist_with_js();
    unsafe { std::env::set_var("QUEOHOH_DAEMON_DIST", dist.path()) };
    let disk = crate::heal::disk_build_id(dist.path());

    let mut app = app();
    app.last_healed_build_id = Some(disk); // already attempted this build
    let upd = app.update(Event::Snapshot(snap(Some("stale-build"), vec![])));

    assert_eq!(app.status_line.as_deref(), Some("daemon still outdated — restart it manually"));
    assert!(!upd.cmds.iter().any(|c| matches!(c, Cmd::Heal)));
    unsafe { std::env::remove_var("QUEOHOH_DAEMON_DIST") };
}

#[test]
#[serial]
fn healthy_snapshot_resets_guard_and_clears_heal_status() {
    let dist = dist_with_js();
    unsafe { std::env::set_var("QUEOHOH_DAEMON_DIST", dist.path()) };
    let disk = crate::heal::disk_build_id(dist.path());

    let mut app = app();
    // Simulate mid-flight state left by a prior restart-now.
    app.last_healed_build_id = Some("old-attempt".into());
    app.healing = true;
    app.heal_status_shown = true;
    app.status_line = Some("daemon outdated — restarting…".into());

    app.update(Event::Snapshot(snap(Some(&disk), vec![])));

    assert!(app.last_healed_build_id.is_none());
    assert!(!app.healing);
    assert!(!app.heal_status_shown);
    assert!(app.status_line.is_none());
    unsafe { std::env::remove_var("QUEOHOH_DAEMON_DIST") };
}

#[test]
fn action_result_during_heal_resets_healing_and_owns_status() {
    let mut app = app();
    app.healing = true; // a Cmd::Heal is in flight
    app.update(Event::ActionResult {
        status: Some("daemon busy — restart deferred".into()),
        invalidate_defs_for: None,
    });
    assert_eq!(app.status_line.as_deref(), Some("daemon busy — restart deferred"));
    assert!(!app.healing);
    assert!(app.heal_status_shown); // heal-owned → cleared by next healthy snapshot
}
