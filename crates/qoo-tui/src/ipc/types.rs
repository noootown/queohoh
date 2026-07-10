use std::collections::HashMap;

use serde::{Deserialize, Deserializer};

/// `null` (or a missing field, via container `default`) → `T::default()`. Mirrors
/// `normalizeSnapshot`'s coercion of an old daemon's absent/nullish collections.
/// A *wrong-typed* value (e.g. a string where an array is expected) still errors;
/// the subscription's `unwrap_or_default()` is the crash-safety net for that
/// (the real daemon never sends wrong types — only missing fields on old builds).
fn nullable_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct StateSnapshot {
    #[serde(deserialize_with = "nullable_default")]
    pub tasks: Vec<TaskInstance>,
    #[serde(deserialize_with = "nullable_default")]
    pub archived_recent: Vec<TaskInstance>,
    #[serde(deserialize_with = "nullable_default")]
    pub sessions: Vec<SessionEntry>,
    #[serde(deserialize_with = "nullable_default")]
    pub running: Vec<String>,
    pub max_concurrent: Option<u32>,
    #[serde(deserialize_with = "nullable_default")]
    pub projects: Vec<Project>,
    #[serde(deserialize_with = "nullable_default")]
    pub worktrees: HashMap<String, Vec<WorktreeInfo>>,
    #[serde(deserialize_with = "nullable_default")]
    pub main_sessions: HashMap<String, String>,
    pub build_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Project {
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum TaskStatus {
    Queued,
    NeedsInput,
    Running,
    Done,
    Failed,
    #[default]
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct TaskInstance {
    pub id: String,
    pub status: TaskStatus,
    pub definition: Option<String>,
    pub item: Option<HashMap<String, String>>,
    pub item_key: Option<String>,
    pub target: TaskTarget,
    pub priority: String,
    pub created: String,
    /// Completion timestamp (ISO UTC), present once the task reaches a terminal
    /// status. `None` on an old daemon that omits it (via the container `default`)
    /// or on a task that hasn't finished — drives the FINISHED-section ordering.
    pub finished_at: Option<String>,
    pub source: String,
    pub ephemeral_worktree: bool,
    pub error: Option<String>,
    pub session: String,
    pub resume_session_id: Option<String>,
    pub model: Option<String>,
    pub prompt: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct TaskTarget {
    pub repo: String,
    #[serde(rename = "ref")]
    pub git_ref: String,
    pub worktree: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct SessionEntry {
    pub kind: String,
    pub key: String,
    pub lane: Option<String>,
    pub cwd: Option<String>,
    pub pid: Option<u32>,
    pub started_at: String,
    pub heartbeat_at: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct WorktreeInfo {
    pub name: String,
    pub path: String,
    pub branch: String,
    /// Daemon git enrichment (all `None` on an old daemon that omits them, via
    /// the container `default`): working tree has uncommitted changes,
    pub dirty: Option<bool>,
    /// unix epoch SECONDS of the last commit,
    pub last_commit_epoch: Option<u64>,
    /// and the author name of the last commit. A stale daemon may still send the
    /// retired `ahead`/`behind` fields — serde ignores them (no deny_unknown).
    pub last_commit_author: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ArgSpec {
    pub name: String,
    pub default: Option<String>,
    pub options: Option<Vec<String>>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct DefinitionSummary {
    pub repo: String,
    pub name: String,
    pub scope: String,
    pub args: Vec<ArgSpec>,
    pub has_discovery: bool,
    /// Human-editable cron expression, or `None` when the def has no schedule.
    /// `default` on the container covers old daemons that omit the field.
    pub cron: Option<String>,
    /// One-line human description of the def, or `None` when unset. `default` on
    /// the container covers old daemons that omit the field.
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct TaskDefinition {
    pub name: String,
    pub repo: String,
    pub discovery: Option<Discovery>,
    pub cron: Option<String>,
    pub args: Vec<ArgSpec>,
    pub dedup: String,
    pub worktree: String,
    pub pre_run: Option<String>,
    pub post_run: Option<String>,
    pub model: String,
    pub timeout_ms: u64,
    pub priority: String,
    pub prompt: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Discovery {
    pub command: String,
    pub item_key: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A full modern snapshot: every field present, one task with every field.
    fn modern_json() -> &'static str {
        r#"{
          "tasks": [{
            "id": "01TASKAAA000000000000000000",
            "status": "running",
            "definition": "pr-ready",
            "item": {"pr": "257"},
            "itemKey": "pr:257",
            "target": {"repo": "platform", "ref": "worktree:platform.feat-a", "worktree": "platform.feat-a"},
            "priority": "normal",
            "created": "2026-07-08T10:00:00.000Z",
            "finishedAt": "2026-07-08T10:05:00.000Z",
            "source": "tui",
            "ephemeralWorktree": false,
            "error": null,
            "session": "main",
            "resumeSessionId": "sess-1",
            "model": "opus",
            "prompt": "do the thing\n"
          }],
          "archivedRecent": [],
          "sessions": [{
            "kind": "interactive", "key": "s1", "lane": "platform:platform.feat-a",
            "cwd": "/wt/platform.feat-a", "pid": 4242,
            "startedAt": "2026-07-08T09:00:00.000Z", "heartbeatAt": "2026-07-08T10:00:00.000Z"
          }],
          "running": ["01TASKAAA000000000000000000"],
          "maxConcurrent": 3,
          "projects": [{"name": "platform"}, {"name": "web"}],
          "worktrees": {"platform": [{"name": "platform.feat-a", "path": "/wt/platform.feat-a", "branch": "feat-a",
            "dirty": true, "lastCommitEpoch": 1751970000, "lastCommitAuthor": "Kevin O'Shea"}]},
          "mainSessions": {"platform:platform.feat-a": "sess-main"},
          "buildId": "1751970000000"
        }"#
    }

    #[test]
    fn deserializes_a_full_modern_snapshot() {
        let s: StateSnapshot = serde_json::from_str(modern_json()).unwrap();
        assert_eq!(s.tasks.len(), 1);
        let t = &s.tasks[0];
        assert_eq!(t.id, "01TASKAAA000000000000000000");
        assert_eq!(t.status, TaskStatus::Running);
        assert_eq!(t.definition.as_deref(), Some("pr-ready"));
        assert_eq!(t.item.as_ref().unwrap().get("pr").map(String::as_str), Some("257"));
        assert_eq!(t.item_key.as_deref(), Some("pr:257"));
        assert_eq!(t.target.repo, "platform");
        assert_eq!(t.target.git_ref, "worktree:platform.feat-a");
        assert_eq!(t.target.worktree.as_deref(), Some("platform.feat-a"));
        assert!(!t.ephemeral_worktree);
        assert_eq!(t.session, "main");
        assert_eq!(t.resume_session_id.as_deref(), Some("sess-1"));
        assert_eq!(t.model.as_deref(), Some("opus"));
        assert_eq!(t.prompt, "do the thing\n");
        assert_eq!(t.finished_at.as_deref(), Some("2026-07-08T10:05:00.000Z"));
        assert_eq!(s.sessions[0].kind, "interactive");
        assert_eq!(s.sessions[0].pid, Some(4242));
        assert_eq!(s.max_concurrent, Some(3));
        assert_eq!(s.projects, vec![Project { name: "platform".into() }, Project { name: "web".into() }]);
        let wt = &s.worktrees["platform"][0];
        assert_eq!(wt.branch, "feat-a");
        assert_eq!(wt.dirty, Some(true));
        assert_eq!(wt.last_commit_epoch, Some(1_751_970_000));
        assert_eq!(wt.last_commit_author.as_deref(), Some("Kevin O'Shea"));
        assert_eq!(s.main_sessions["platform:platform.feat-a"], "sess-main");
        assert_eq!(s.build_id.as_deref(), Some("1751970000000"));
    }

    #[test]
    fn old_daemon_snapshot_missing_new_fields_defaults_without_error() {
        // Predates projects/worktrees/maxConcurrent/buildId (mirrors
        // use-daemon.test's OLD-daemon case): only the original four fields.
        let old = r#"{"tasks": [{"id": "t1", "target": {"repo": "platform", "ref": "temp"}}],
                      "archivedRecent": [], "sessions": [], "running": []}"#;
        let s: StateSnapshot = serde_json::from_str(old).unwrap();
        assert_eq!(s.tasks.len(), 1);
        assert_eq!(s.tasks[0].id, "t1");
        // status absent → Unknown (default); target.worktree absent → None.
        assert_eq!(s.tasks[0].status, TaskStatus::Unknown);
        assert_eq!(s.tasks[0].target.worktree, None);
        // finishedAt absent on an old daemon → None (additive field).
        assert_eq!(s.tasks[0].finished_at, None);
        assert_eq!(s.projects, vec![]);
        assert!(s.worktrees.is_empty());
        assert!(s.main_sessions.is_empty());
        assert_eq!(s.max_concurrent, None);
        // buildId absent → None means "stale" for self-heal — must NOT default to "".
        assert_eq!(s.build_id, None);
    }

    #[test]
    fn null_valued_collections_coerce_to_empty() {
        // The nullable_default shim: `null` where an array/object is expected → default.
        let s: StateSnapshot = serde_json::from_str(
            r#"{"tasks": null, "running": null, "worktrees": null, "projects": null}"#,
        )
        .unwrap();
        assert_eq!(s.tasks, vec![]);
        assert_eq!(s.running, vec![] as Vec<String>);
        assert!(s.worktrees.is_empty());
        assert_eq!(s.projects, vec![]);
    }

    #[test]
    fn worktree_git_enrichment_absent_defaults_to_none() {
        // An old daemon emits only name/path/branch; the git-enrichment fields
        // default to None (container `default`) rather than erroring.
        let s: StateSnapshot = serde_json::from_str(
            r#"{"worktrees": {"platform": [{"name": "wt-a", "path": "/wt/wt-a", "branch": "a"}]}}"#,
        )
        .unwrap();
        let wt = &s.worktrees["platform"][0];
        assert_eq!(wt.dirty, None);
        assert_eq!(wt.last_commit_epoch, None);
        assert_eq!(wt.last_commit_author, None);
    }

    #[test]
    fn stale_daemon_ahead_behind_fields_are_ignored() {
        // A daemon predating the author rewrite still emits ahead/behind; without
        // deny_unknown_fields serde silently drops them (new field stays None).
        let s: StateSnapshot = serde_json::from_str(
            r#"{"worktrees": {"platform": [{"name": "wt-a", "path": "/wt/wt-a", "branch": "a",
                "ahead": 3, "behind": 12, "lastCommitEpoch": 1751970000}]}}"#,
        )
        .unwrap();
        let wt = &s.worktrees["platform"][0];
        assert_eq!(wt.last_commit_epoch, Some(1_751_970_000));
        assert_eq!(wt.last_commit_author, None);
    }

    #[test]
    fn unknown_status_maps_to_unknown_variant() {
        let t: TaskInstance =
            serde_json::from_str(r#"{"id": "x", "status": "paused-by-alien"}"#).unwrap();
        assert_eq!(t.status, TaskStatus::Unknown);
    }

    #[test]
    fn definition_summary_cron_and_description_present_and_absent() {
        // camelCase `cron`/`description` deserialize into their fields...
        let with: DefinitionSummary = serde_json::from_str(
            r#"{"repo": "platform", "name": "pr-review", "scope": "project",
                "args": [], "hasDiscovery": true, "cron": "30 13 * * *",
                "description": "Review an open PR."}"#,
        )
        .unwrap();
        assert_eq!(with.cron.as_deref(), Some("30 13 * * *"));
        assert_eq!(with.description.as_deref(), Some("Review an open PR."));
        // ...and an old daemon that omits them defaults to None (container `default`).
        let without: DefinitionSummary = serde_json::from_str(
            r#"{"repo": "platform", "name": "lint", "scope": "global",
                "args": [], "hasDiscovery": false}"#,
        )
        .unwrap();
        assert_eq!(without.cron, None);
        assert_eq!(without.description, None);
    }

    #[test]
    fn kebab_status_needs_input_round_trips() {
        let t: TaskInstance =
            serde_json::from_str(r#"{"id": "x", "status": "needs-input"}"#).unwrap();
        assert_eq!(t.status, TaskStatus::NeedsInput);
    }
}
