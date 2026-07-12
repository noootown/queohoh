use super::*;
use serde_json::json;

fn app() -> App {
    App::new(std::path::PathBuf::from("/tmp/runs"), std::path::PathBuf::from("/tmp/daemon.sock"))
}

#[test]
fn dispatch_rpc_applies_per_method_defaults() {
    let mut a = app();

    // Plain mutation: 5s timeout, not timeout-ok, no invalidation.
    let cmd = a.dispatch_rpc("rerun task", "retry", json!({ "id": "t1" }), RpcOpts::default());
    match cmd {
        Cmd::Rpc { call, timeout_ms, timeout_is_ok, invalidate_defs_for, .. } => {
            assert_eq!(call.method, "retry");
            assert_eq!(call.params, json!({ "id": "t1" }));
            assert_eq!(timeout_ms, 5_000);
            assert!(!timeout_is_ok);
            assert_eq!(invalidate_defs_for, None);
        }
        other => panic!("expected Cmd::Rpc, got {other:?}"),
    }

    // (createWorktree no longer routes through dispatch_rpc — it has a
    // dedicated Cmd whose 10-minute budget lives in the event handler.)

    // runDefinition: client timeout is success; invalidation passed by caller.
    let cmd = a.dispatch_rpc(
        "run definition",
        "runDefinition",
        json!({ "repo": "p", "name": "d", "args": [], "source": "tui" }),
        RpcOpts { invalidate_defs_for: Some("p".into()), ..RpcOpts::default() },
    );
    match cmd {
        Cmd::Rpc { timeout_ms, timeout_is_ok, invalidate_defs_for, .. } => {
            assert_eq!(timeout_ms, 5_000);
            assert!(timeout_is_ok);
            assert_eq!(invalidate_defs_for.as_deref(), Some("p"));
        }
        other => panic!("expected Cmd::Rpc, got {other:?}"),
    }
}

#[test]
fn action_result_sets_status_then_next_list_key_clears_it() {
    let mut a = app();
    let u = a.update(Event::ActionResult { status: Some("boom".into()), invalidate_defs_for: None });
    assert!(u.dirty);
    assert_eq!(a.status_line.as_deref(), Some("boom"));
    assert!(u.cmds.is_empty());

    // Any list-mode keypress clears the status line (Task 11 behavior; a
    // no-op key like Char('z') still passes through the clear-at-top path).
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    a.update(Event::Key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE)));
    assert_eq!(a.status_line, None);
}

#[test]
fn action_result_invalidation_drops_cache_and_refetches() {
    let mut a = app();
    a.defs_by_project.insert("platform".into(), vec![]);
    let u = a.update(Event::ActionResult { status: None, invalidate_defs_for: Some("platform".into()) });
    assert!(u.dirty);
    assert!(!a.defs_by_project.contains_key("platform"));
    assert!(
        u.cmds.iter().any(|c| matches!(c, Cmd::FetchDefinitions { repo } if repo == "platform")),
        "expected a FetchDefinitions for the invalidated repo, got {:?}", u.cmds,
    );
    // A None status leaves the status line untouched.
    assert_eq!(a.status_line, None);
}
