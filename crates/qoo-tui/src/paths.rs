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

pub fn daemon_dist_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("QUEOHOH_DAEMON_DIST") {
        return PathBuf::from(dir);
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
    }

    #[test]
    #[serial]
    fn daemon_dist_honors_env_override() {
        unsafe { std::env::set_var("QUEOHOH_DAEMON_DIST", "/opt/dist") };
        assert_eq!(daemon_dist_dir(), PathBuf::from("/opt/dist"));
        assert_eq!(daemon_cli_path(), PathBuf::from("/opt/dist/cli.js"));
        unsafe { std::env::remove_var("QUEOHOH_DAEMON_DIST") };
    }
}
