use std::path::{Path, PathBuf};

pub fn state_path() -> PathBuf {
    if let Ok(dir) = std::env::var("QUEOHOH_STATE_DIR") {
        return PathBuf::from(dir);
    }
    let home = std::env::var("HOME").unwrap_or_default();
    Path::new(&home).join(".local/state/queohoh")
}

pub fn socket_path(state: &Path) -> PathBuf {
    state.join("daemon/daemon.sock")
}

pub fn pid_path(state: &Path) -> PathBuf {
    state.join("daemon/daemon.pid")
}

pub fn runs_path(state: &Path) -> PathBuf {
    state.join("runs")
}

/// Per-project TUI pane-layout persistence file (collapsed flags + divider
/// overrides). Sits directly under the state dir so it survives daemon restarts.
pub fn layout_path(state: &Path) -> PathBuf {
    state.join("tui-layout.json")
}

/// Directory holding the daemon's built `cli.js` (and siblings). Resolution
/// order — each step only wins when it can point at a real directory:
///
/// 1. `QUEOHOH_DAEMON_DIST` — explicit override (`mise run tui` sets this to
///    `$PWD/packages/daemon/dist` so a shared `CARGO_TARGET_DIR` binary still
///    heals against the worktree the operator launched from).
/// 2. `$CWD/packages/daemon/dist` — same idea without the env: launching the
///    binary from a checkout root pins heal to that checkout.
/// 3. Compile-time `CARGO_MANIFEST_DIR/../../packages/daemon/dist` — last
///    resort for `cargo run` from `crates/qoo-tui` and tests. Wrong under a
///    shared cargo target dir when the binary was last compiled in another
///    worktree (which is why (1)/(2) exist).
pub fn daemon_dist_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("QUEOHOH_DAEMON_DIST") {
        return PathBuf::from(dir);
    }
    if let Ok(cwd) = std::env::current_dir() {
        let from_cwd = cwd.join("packages/daemon/dist");
        if from_cwd.is_dir() {
            return from_cwd;
        }
    }
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packages/daemon/dist")
}

pub fn daemon_cli_path() -> PathBuf {
    daemon_dist_dir().join("cli.js")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    // The three env-mutating tests below touch process-global env vars
    // (`QUEOHOH_STATE_DIR`, `HOME`, `QUEOHOH_DAEMON_DIST`), so they race under
    // the default multi-threaded runner. `#[serial]` forces them to run one at a
    // time — deterministic under bare `cargo test` with no `--test-threads=1`.

    #[test]
    #[serial]
    fn state_path_honors_env_override() {
        // set_var is unsafe in edition 2024; env is process-global so we set and
        // restore within this single test.
        unsafe { std::env::set_var("QUEOHOH_STATE_DIR", "/tmp/qoo-state-xyz") };
        assert_eq!(state_path(), PathBuf::from("/tmp/qoo-state-xyz"));
        unsafe { std::env::remove_var("QUEOHOH_STATE_DIR") };
    }

    #[test]
    #[serial]
    fn state_path_defaults_under_home_local_state() {
        unsafe { std::env::remove_var("QUEOHOH_STATE_DIR") };
        unsafe { std::env::set_var("HOME", "/home/tester") };
        assert_eq!(
            state_path(),
            PathBuf::from("/home/tester/.local/state/queohoh")
        );
    }

    #[test]
    fn derived_paths_hang_off_state() {
        let state = Path::new("/s");
        assert_eq!(socket_path(state), PathBuf::from("/s/daemon/daemon.sock"));
        assert_eq!(pid_path(state), PathBuf::from("/s/daemon/daemon.pid"));
        assert_eq!(runs_path(state), PathBuf::from("/s/runs"));
        assert_eq!(layout_path(state), PathBuf::from("/s/tui-layout.json"));
    }

    #[test]
    #[serial]
    fn daemon_dist_honors_env_override() {
        unsafe { std::env::set_var("QUEOHOH_DAEMON_DIST", "/opt/dist") };
        assert_eq!(daemon_dist_dir(), PathBuf::from("/opt/dist"));
        assert_eq!(daemon_cli_path(), PathBuf::from("/opt/dist/cli.js"));
        unsafe { std::env::remove_var("QUEOHOH_DAEMON_DIST") };
    }

    #[test]
    #[serial]
    fn daemon_dist_prefers_cwd_checkout_over_compile_time() {
        // Shared CARGO_TARGET_DIR means the binary may have been compiled in
        // another worktree; a real `packages/daemon/dist` under cwd must win
        // so self-heal restarts THIS checkout's daemon.
        unsafe { std::env::remove_var("QUEOHOH_DAEMON_DIST") };
        let tmp = tempfile::tempdir().expect("tempdir");
        let dist = tmp.path().join("packages/daemon/dist");
        std::fs::create_dir_all(&dist).expect("mkdir dist");
        let prev = std::env::current_dir().expect("cwd");
        std::env::set_current_dir(tmp.path()).expect("chdir");
        let got = daemon_dist_dir();
        std::env::set_current_dir(prev).expect("restore cwd");
        // macOS temp dirs often live under /var → /private/var; compare after
        // canonicalize so the assertion is path-identity, not string identity.
        assert_eq!(
            got.canonicalize().expect("got"),
            dist.canonicalize().expect("dist"),
        );
    }
}
