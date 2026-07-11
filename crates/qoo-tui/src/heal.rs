//! Daemon self-heal: pure decision + async orchestration.
//! Port of packages/tui/src/heal.ts + packages/daemon/src/build-id.ts (parity oracle:
//! packages/tui/src/__tests__/heal.test.ts).

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use crate::ipc::client::RpcClient;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealDecision {
    None,
    Defer,
    RestartNow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownOutcome {
    Busy,
    Fallback,
}

/// True when the daemon's reported build differs from disk. A pre-feature daemon
/// sends `None`, which is always stale — it definitionally predates the field.
pub fn is_stale(snapshot_build_id: Option<&str>, disk_build_id: &str) -> bool {
    snapshot_build_id != Some(disk_build_id)
}

/// Pure self-heal decision (mirrors decideHeal). The loop guard keys on the
/// *target* disk build: once we attempt to reach build X we won't retry X, but a
/// fresh build (disk moves to Y) is a new mismatch worth healing.
pub fn decide_heal(
    snapshot_build_id: Option<&str>,
    disk_build_id: &str,
    running: usize,
    last_healed: Option<&str>,
) -> HealDecision {
    if !is_stale(snapshot_build_id, disk_build_id) {
        return HealDecision::None;
    }
    if last_healed == Some(disk_build_id) {
        return HealDecision::None; // already tried this build
    }
    if running > 0 {
        return HealDecision::Defer;
    }
    HealDecision::RestartNow
}

/// Build fingerprint: newest `.js` mtime (ms) in `dist_dir` as a string, `"0"`
/// when the dir holds no `.js` files or can't be read. Mirrors currentBuildId.
pub fn disk_build_id(dist_dir: &Path) -> String {
    let entries = match std::fs::read_dir(dist_dir) {
        Ok(e) => e,
        Err(_) => return "0".to_string(),
    };
    let mut newest: f64 = 0.0;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("js") {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        let Ok(modified) = meta.modified() else { continue };
        let Ok(dur) = modified.duration_since(std::time::UNIX_EPOCH) else {
            continue;
        };
        // Same decomposition as Node's mtimeMs: sec*1000 + nsec/1e6, as f64.
        let ms = dur.as_secs() as f64 * 1000.0 + dur.subsec_nanos() as f64 / 1_000_000.0;
        if ms > newest {
            newest = ms;
        }
    }
    // Rust f64 Display == V8 Number.toString (unique shortest round-trip).
    newest.to_string()
}

/// Classify a failed `shutdown` RPC. "busy" → a task raced in after our idle
/// check; respect the guard and abort. Anything else (unknown-method on an old
/// daemon, or unreachable) → fall back to the pidfile SIGTERM path.
pub fn classify_shutdown_error(err: &str) -> ShutdownOutcome {
    if err.contains("busy") {
        ShutdownOutcome::Busy
    } else {
        ShutdownOutcome::Fallback
    }
}

async fn socket_answers(sock: &Path) -> bool {
    match RpcClient::connect(sock).await {
        Ok(mut c) => matches!(
            c.call("ping", serde_json::Value::Null, Duration::from_millis(500)).await,
            Ok(v) if v == serde_json::json!("pong")
        ),
        Err(_) => false,
    }
}

/// Ask the daemon to shut down (SIGTERM fallback for an old daemon that lacks the
/// RPC), wait for the socket to go quiet, then spawn a fresh detached daemon. The
/// reconnect loop picks the new one up. Returns `Err("daemon busy — restart
/// deferred")` without spawning when the daemon reports it is busy — we never
/// force-kill a daemon with a task in flight.
pub async fn perform_heal(sock: &Path, pid_file: &Path, daemon_cli: &Path) -> Result<(), String> {
    // 1. Ask the daemon to shut down. Busy → abort (never force-kill live work).
    let mut shutdown_accepted = false;
    if let Ok(mut client) = RpcClient::connect(sock).await {
        match client
            .call("shutdown", serde_json::Value::Null, Duration::from_secs(5))
            .await
        {
            Ok(_) => shutdown_accepted = true,
            Err(e) => match classify_shutdown_error(&e) {
                ShutdownOutcome::Busy => {
                    return Err("daemon busy — restart deferred".to_string());
                }
                // unknown-method (old daemon) or unreachable → pidfile fallback below.
                ShutdownOutcome::Fallback => {}
            },
        }
    }
    // (connect failure also falls through to the pidfile fallback)

    // 2. SIGTERM fallback for an old daemon that lacks the shutdown RPC. No new
    //    runtime dependency: POSIX `kill` sends SIGTERM. (nix::sys::signal::kill
    //    is an equivalent if the crate is already present.)
    if !shutdown_accepted
        && let Ok(text) = std::fs::read_to_string(pid_file)
            && let Ok(pid) = text.trim().parse::<i32>()
                && pid > 0 {
                    let _ = tokio::process::Command::new("kill")
                        .arg("-TERM")
                        .arg(pid.to_string())
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status()
                        .await;
                }

    // 3. Poll ping until the old socket stops answering (bounded ~5s).
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        if !socket_answers(sock).await {
            break;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }

    // 4. Spawn a fresh detached daemon: `node <daemon_cli> daemon`, own process
    //    group, all stdio to /dev/null. Drop the handle (kill_on_drop is false).
    let mut cmd = tokio::process::Command::new("node");
    cmd.arg(daemon_cli)
        .arg("daemon")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        cmd.process_group(0); // detach from the TUI's group (tokio 1.21+)
    }
    cmd.spawn().map_err(|e| format!("failed to spawn daemon: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- is_stale (mirrors heal.test.ts `describe("isStale")`) ----
    #[test]
    fn is_stale_false_when_matching() {
        assert!(!is_stale(Some("100"), "100"));
    }
    #[test]
    fn is_stale_true_when_differing() {
        assert!(is_stale(Some("100"), "200"));
    }
    #[test]
    fn is_stale_true_for_pre_feature_daemon() {
        assert!(is_stale(None, "200"));
    }

    // ---- decide_heal (mirrors heal.test.ts `describe("decideHeal")`) ----
    #[test]
    fn decide_none_when_up_to_date() {
        assert_eq!(decide_heal(Some("100"), "100", 0, None), HealDecision::None);
    }
    #[test]
    fn decide_restart_now_when_stale_and_idle() {
        assert_eq!(decide_heal(Some("100"), "200", 0, None), HealDecision::RestartNow);
    }
    #[test]
    fn decide_defer_when_stale_but_running() {
        assert_eq!(decide_heal(Some("100"), "200", 1, None), HealDecision::Defer);
    }
    #[test]
    fn decide_restart_now_for_pre_feature_daemon_when_idle() {
        assert_eq!(decide_heal(None, "200", 0, None), HealDecision::RestartNow);
    }
    #[test]
    fn decide_none_loop_guard_already_attempted() {
        assert_eq!(decide_heal(Some("100"), "200", 0, Some("200")), HealDecision::None);
    }
    #[test]
    fn decide_restart_now_when_fresh_build_lands() {
        assert_eq!(decide_heal(Some("100"), "300", 0, Some("200")), HealDecision::RestartNow);
    }
    #[test]
    fn decide_none_loop_guard_wins_over_running() {
        assert_eq!(decide_heal(Some("100"), "200", 2, Some("200")), HealDecision::None);
    }

    // ---- disk_build_id ----
    #[test]
    fn disk_build_id_zero_when_no_js() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("readme.txt"), "x").unwrap();
        assert_eq!(disk_build_id(d.path()), "0");
    }
    #[test]
    fn disk_build_id_zero_when_dir_missing() {
        assert_eq!(disk_build_id(Path::new("/no/such/dir/qoo-xyz")), "0");
    }
    #[test]
    fn disk_build_id_picks_newest_js_ignoring_non_js() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.js"), "a").unwrap();
        std::thread::sleep(Duration::from_millis(25));
        std::fs::write(d.path().join("b.js"), "b").unwrap();
        // A newer *non*-.js sibling must be ignored.
        std::thread::sleep(Duration::from_millis(25));
        std::fs::write(d.path().join("c.ts"), "c").unwrap();

        let m = std::fs::metadata(d.path().join("b.js"))
            .unwrap()
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap();
        let expect =
            (m.as_secs() as f64 * 1000.0 + m.subsec_nanos() as f64 / 1_000_000.0).to_string();
        assert_eq!(disk_build_id(d.path()), expect);
    }

    // ---- classify_shutdown_error ----
    #[test]
    fn classify_busy() {
        assert_eq!(classify_shutdown_error("busy: task running"), ShutdownOutcome::Busy);
    }
    #[test]
    fn classify_unknown_method_is_fallback() {
        assert_eq!(classify_shutdown_error("unknown method: shutdown"), ShutdownOutcome::Fallback);
    }
    #[test]
    fn classify_unreachable_is_fallback() {
        assert_eq!(classify_shutdown_error("connection refused"), ShutdownOutcome::Fallback);
    }
}
