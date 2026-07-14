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
    pub build_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Project {
    pub name: String,
    /// The project's optional author identity (its `vars.yaml` `github_id:` key,
    /// e.g. `noootown`). `None` on an old daemon that omits it (via the container
    /// `default`), or when the project has no `github_id` configured. The
    /// WORKTREES pane matches it against each worktree's last-commit author
    /// email/name to sort "my" worktrees first; absent → that tier is a no-op.
    pub github_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum TaskStatus {
    Queued,
    NeedsInput,
    Running,
    Done,
    Failed,
    /// User-cancelled via the queue `x` action (skip on a queued/needs-input task,
    /// stop on a running one) — the daemon lands `cancelled`, distinct from a
    /// `failed` run. Rendered with its own glyph, never the red ✗.
    Cancelled,
    /// Skipped by a chain (an earlier step failed, so this one never ran). Wire
    /// value `skipped`; rendered dim, distinct from `cancelled`.
    Skipped,
    /// The worker claimed success (clean tree) but the task's `verify`
    /// (done-condition) command disagreed — non-zero exit or timeout. Wire value
    /// `verify-failed` (kebab, like `needs-input`). Rendered with its own glyph in
    /// error red, distinct from a worker `failed`.
    VerifyFailed,
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
    /// Start timestamp (ISO UTC) of the CURRENT run, re-stamped by the daemon each
    /// time the worker flips the task to `running`. `None` on an old daemon that
    /// omits it (via the container `default`) or on a task that has never run —
    /// the live `⏱` timer falls back to `created` in that case. Re-stamping on a
    /// re-run is what restarts the timer from the re-run, not the original create.
    pub started_at: Option<String>,
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
    /// Done-condition (`verify`) fields (additive; all `None` on an old daemon
    /// that omits them, via the container `default`). `verify` is the configured
    /// command; `verified` is the last verdict (true/false, `None` = never run);
    /// `verify_exit_code` is the command's exit (`None` on timeout / never run);
    /// `verify_output` is a bounded (~4 KB) tail of its combined output. The
    /// distinct `verify-failed` status carries the headline; these detail it.
    pub verify: Option<String>,
    pub verified: Option<bool>,
    pub verify_exit_code: Option<i64>,
    pub verify_output: Option<String>,
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
    /// the author name of the last commit. A stale daemon may still send the
    /// retired `ahead`/`behind` fields — serde ignores them (no deny_unknown).
    pub last_commit_author: Option<String>,
    /// and the author EMAIL of the last commit (git `%ae`; `None`/null when
    /// unknown or on an old daemon). Paired with `last_commit_author` for the
    /// WORKTREES "mine-first" sort — the project's `github_id` is matched as a
    /// case-insensitive substring of either.
    pub last_commit_author_email: Option<String>,
    /// Short hash of the last commit (git `%h`; `None`/null when unknown or on an
    /// old daemon). Shown in the worktree detail info tab.
    pub last_commit_hash: Option<String>,
    /// Open PR number for this worktree's branch (`None`/null when there is no
    /// open PR, `gh` is unavailable, or on an old daemon). Shown as `#<n>` in the
    /// worktree detail info tab.
    pub pr_number: Option<u64>,
    /// Web URL of that open PR (`None`/null when there is no open PR, `gh` is
    /// unavailable, or on an old daemon that predates the field). Paired with
    /// `pr_number` so the `#<n>` chip in the detail info tab and the WORKTREES
    /// PR column open the PR in a browser on a click.
    pub pr_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ArgSpec {
    pub name: String,
    #[serde(default, rename = "type")]
    pub r#type: Option<String>,
    pub default: Option<String>,
    pub options: Option<Vec<String>>,
    pub description: Option<String>,
}

impl ArgSpec {
    /// True when this arg is the worktree/target selector (rendered as a
    /// combobox; resolves to a ref on submit).
    pub fn is_worktree(&self) -> bool {
        self.r#type.as_deref() == Some("worktree")
    }

    /// `type: branch` — rendered as a dropdown seeded with the repo's worktree
    /// branches (incl. main/master).
    pub fn is_branch(&self) -> bool {
        self.r#type.as_deref() == Some("branch")
    }

    /// `type: text` — the multiline, auto-growing textarea (the opt-in for
    /// free-text args that need more than one line, e.g. a problem scenario).
    /// Plain free-text args with no `type` render as a single-line input.
    pub fn is_text(&self) -> bool {
        self.r#type.as_deref() == Some("text")
    }
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
    /// Model the def runs with (e.g. `claude-fable-5`), shown in the TASKS def
    /// rows (prefix-stripped). `None` on an old daemon that omits the field (via
    /// the container `default`) — a modern daemon always sends it (config default
    /// `"sonnet"`), so this is `Option` purely for backward compatibility.
    pub model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct TaskDefinition {
    pub name: String,
    pub repo: String,
    /// One-line human description of the def, or `None` when unset (via the
    /// container `default` — also covers an old daemon that omits it). Shown in
    /// the run detail's `info` sub-tab Config section.
    pub description: Option<String>,
    pub discovery: Option<Discovery>,
    pub cron: Option<String>,
    pub args: Vec<ArgSpec>,
    pub dedup: String,
    pub worktree: String,
    pub pre_run: Option<String>,
    pub post_run: Option<String>,
    pub model: String,
    /// The authored `model` alias resolved to a concrete id against the effective
    /// per-project table (e.g. `opus` → `claude-opus-4-8`). `None` on an old
    /// daemon that omits it (via the container `default`) — the detail pane then
    /// falls back to showing the authored `model` alone. A modern daemon always
    /// sends it (unknown/full ids resolve to themselves).
    pub model_resolved: Option<String>,
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

/// Read-only mirror of the daemon's `settings` RPC (Task 4). Every field is
/// `#[serde(default)]` so an OLD daemon that omits the whole block — or any
/// subtree of it — deserializes to empties rather than erroring; the app stores
/// a *failed* fetch as `Some(None)` and never reaches `from_value`, but a
/// partial/forward-compatible payload from a mixed-version daemon still lands
/// cleanly. Keys are single lowercase words on the wire (`models`/`defaults`/
/// `global`/`entries`/`source`/`projects`/`repo`), so field names match 1:1 and
/// no `rename_all` is needed.
#[derive(Debug, Clone, PartialEq, Default, serde::Deserialize)]
pub struct SettingsPayload {
    #[serde(default)]
    pub models: SettingsModels,
}

#[derive(Debug, Clone, PartialEq, Default, serde::Deserialize)]
pub struct SettingsModels {
    /// Built-in alias → model-id defaults the daemon ships with.
    #[serde(default)]
    pub defaults: std::collections::BTreeMap<String, String>,
    /// Built-in default model an ad-hoc / enqueue run uses when nothing sets one
    /// (the launcher form preselects this, unless a project overrides it). Empty
    /// on an old daemon that omits the field.
    #[serde(default)]
    pub default_model: String,
    /// The global override layer (config-file `models:` block). Overlays
    /// `defaults` to form the effective global table.
    #[serde(default)]
    pub global: SettingsLayer,
    /// Only projects that actually override models; each carries its deltas.
    #[serde(default)]
    pub projects: Vec<SettingsProjectLayer>,
}

#[derive(Debug, Clone, PartialEq, Default, serde::Deserialize)]
pub struct SettingsLayer {
    #[serde(default)]
    pub entries: std::collections::BTreeMap<String, String>,
    /// Path the layer was loaded from, shown as the section's provenance line.
    #[serde(default)]
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Default, serde::Deserialize)]
pub struct SettingsProjectLayer {
    #[serde(default)]
    pub repo: String,
    #[serde(default)]
    pub entries: std::collections::BTreeMap<String, String>,
    /// This project's `default_model` override from vars.yaml, when set (empty
    /// otherwise — the caller falls back to [`SettingsModels::default_model`]).
    #[serde(default)]
    pub default_model: String,
    #[serde(default)]
    pub source: String,
}

impl SettingsModels {
    /// Effective default model for `repo`: the project's `default_model` override
    /// when set, else the built-in `default_model`, falling back to `"opus"` when
    /// an old daemon omitted the field entirely.
    pub fn default_model_for(&self, repo: &str) -> String {
        let project = self
            .projects
            .iter()
            .find(|p| p.repo == repo)
            .map(|p| p.default_model.as_str())
            .filter(|s| !s.is_empty());
        project
            .or(Some(self.default_model.as_str()).filter(|s| !s.is_empty()))
            .unwrap_or("opus")
            .to_string()
    }
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
            "startedAt": "2026-07-08T10:00:30.000Z",
            "finishedAt": "2026-07-08T10:05:00.000Z",
            "source": "tui",
            "ephemeralWorktree": false,
            "error": null,
            "session": "main",
            "resumeSessionId": "sess-1",
            "model": "opus",
            "prompt": "do the thing\n",
            "verify": "gh pr view --json labels -q '.labels[].name' | grep -qx ready-for-review",
            "verified": false,
            "verifyExitCode": 1,
            "verifyOutput": "checking labels...\nno match\n"
          }],
          "archivedRecent": [],
          "sessions": [{
            "kind": "interactive", "key": "s1", "lane": "platform:platform.feat-a",
            "cwd": "/wt/platform.feat-a", "pid": 4242,
            "startedAt": "2026-07-08T09:00:00.000Z", "heartbeatAt": "2026-07-08T10:00:00.000Z"
          }],
          "running": ["01TASKAAA000000000000000000"],
          "maxConcurrent": 3,
          "projects": [{"name": "platform", "githubId": "noootown"}, {"name": "web"}],
          "worktrees": {"platform": [{"name": "platform.feat-a", "path": "/wt/platform.feat-a", "branch": "feat-a",
            "dirty": true, "lastCommitEpoch": 1751970000, "lastCommitAuthor": "Kevin O'Shea",
            "lastCommitAuthorEmail": "kevin@justicebid.com"}]},
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
        assert_eq!(t.started_at.as_deref(), Some("2026-07-08T10:00:30.000Z"));
        assert_eq!(t.finished_at.as_deref(), Some("2026-07-08T10:05:00.000Z"));
        assert_eq!(t.verify.as_deref(), Some(
            "gh pr view --json labels -q '.labels[].name' | grep -qx ready-for-review",
        ));
        assert_eq!(t.verified, Some(false));
        assert_eq!(t.verify_exit_code, Some(1));
        assert_eq!(t.verify_output.as_deref(), Some("checking labels...\nno match\n"));
        assert_eq!(s.sessions[0].kind, "interactive");
        assert_eq!(s.sessions[0].pid, Some(4242));
        assert_eq!(s.max_concurrent, Some(3));
        assert_eq!(
            s.projects,
            vec![
                Project { name: "platform".into(), github_id: Some("noootown".into()) },
                Project { name: "web".into(), github_id: None },
            ]
        );
        let wt = &s.worktrees["platform"][0];
        assert_eq!(wt.branch, "feat-a");
        assert_eq!(wt.dirty, Some(true));
        assert_eq!(wt.last_commit_epoch, Some(1_751_970_000));
        assert_eq!(wt.last_commit_author_email.as_deref(), Some("kevin@justicebid.com"));
        assert_eq!(wt.last_commit_author.as_deref(), Some("Kevin O'Shea"));
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
        // startedAt/finishedAt absent on an old daemon → None (additive fields).
        assert_eq!(s.tasks[0].started_at, None);
        assert_eq!(s.tasks[0].finished_at, None);
        // verify fields absent on an old daemon → None (additive fields).
        assert_eq!(s.tasks[0].verify, None);
        assert_eq!(s.tasks[0].verified, None);
        assert_eq!(s.tasks[0].verify_exit_code, None);
        assert_eq!(s.tasks[0].verify_output, None);
        assert_eq!(s.projects, vec![]);
        assert!(s.worktrees.is_empty());
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
                "description": "Review an open PR.", "model": "claude-fable-5"}"#,
        )
        .unwrap();
        assert_eq!(with.cron.as_deref(), Some("30 13 * * *"));
        assert_eq!(with.description.as_deref(), Some("Review an open PR."));
        assert_eq!(with.model.as_deref(), Some("claude-fable-5"));
        // ...and an old daemon that omits them defaults to None (container `default`).
        let without: DefinitionSummary = serde_json::from_str(
            r#"{"repo": "platform", "name": "lint", "scope": "global",
                "args": [], "hasDiscovery": false}"#,
        )
        .unwrap();
        assert_eq!(without.cron, None);
        assert_eq!(without.description, None);
        assert_eq!(without.model, None);
    }

    #[test]
    fn task_definition_model_resolved_present_and_absent() {
        // A modern daemon sends camelCase `modelResolved`...
        let with: TaskDefinition = serde_json::from_str(
            r#"{"name": "pr-ready", "repo": "acme", "model": "opus",
                "modelResolved": "claude-opus-4-8", "timeoutMs": 1800000}"#,
        )
        .unwrap();
        assert_eq!(with.model, "opus");
        assert_eq!(with.model_resolved.as_deref(), Some("claude-opus-4-8"));
        // ...and an old daemon that omits it defaults to None (container `default`).
        let without: TaskDefinition =
            serde_json::from_str(r#"{"name": "lint", "repo": "acme", "model": "sonnet"}"#).unwrap();
        assert_eq!(without.model, "sonnet");
        assert_eq!(without.model_resolved, None);
    }

    #[test]
    fn kebab_status_needs_input_round_trips() {
        let t: TaskInstance =
            serde_json::from_str(r#"{"id": "x", "status": "needs-input"}"#).unwrap();
        assert_eq!(t.status, TaskStatus::NeedsInput);
    }

    #[test]
    fn kebab_status_verify_failed_round_trips() {
        // The wire value is `verify-failed` (kebab, via rename_all), NOT the
        // Unknown fallback — a modern TUI renders it with its own glyph.
        let t: TaskInstance =
            serde_json::from_str(r#"{"id": "x", "status": "verify-failed"}"#).unwrap();
        assert_eq!(t.status, TaskStatus::VerifyFailed);
    }

    #[test]
    fn settings_payload_full_deserializes() {
        // The exact shape the daemon's `settings` RPC returns (Task 4).
        let s: SettingsPayload = serde_json::from_str(
            r#"{"models": {
                "defaults": {"opus": "claude-opus-4-8", "sonnet": "claude-sonnet-4-5"},
                "default_model": "opus",
                "global": {"entries": {"sonnet": "claude-sonnet-4-6"},
                           "source": "/home/ian/.config/qoo/config.yaml"},
                "projects": [{"repo": "acme", "entries": {"opus": "claude-opus-4-9"},
                              "default_model": "sonnet", "source": "/repos/acme/vars.yaml"}]
            }}"#,
        )
        .unwrap();
        assert_eq!(s.models.defaults.get("opus").map(String::as_str), Some("claude-opus-4-8"));
        assert_eq!(s.models.default_model, "opus");
        assert_eq!(s.models.projects[0].default_model, "sonnet");
        // A project override wins; a repo with no layer falls back to the built-in.
        assert_eq!(s.models.default_model_for("acme"), "sonnet");
        assert_eq!(s.models.default_model_for("other"), "opus");
        // BTreeMap: iteration order is sorted, so snapshots stay deterministic.
        assert_eq!(
            s.models.defaults.keys().cloned().collect::<Vec<_>>(),
            vec!["opus", "sonnet"]
        );
        assert_eq!(s.models.global.entries.get("sonnet").map(String::as_str), Some("claude-sonnet-4-6"));
        assert_eq!(s.models.global.source, "/home/ian/.config/qoo/config.yaml");
        assert_eq!(s.models.projects.len(), 1);
        assert_eq!(s.models.projects[0].repo, "acme");
        assert_eq!(s.models.projects[0].entries.get("opus").map(String::as_str), Some("claude-opus-4-9"));
        assert_eq!(s.models.projects[0].source, "/repos/acme/vars.yaml");
    }

    #[test]
    fn settings_payload_empty_object_defaults_without_error() {
        // An old daemon (predating the settings RPC) that somehow returns `{}` —
        // or any partial subtree — must default rather than panic. Every field is
        // `#[serde(default)]`, so the empty object is a fully-defaulted payload.
        let s: SettingsPayload = serde_json::from_str("{}").unwrap();
        assert_eq!(s, SettingsPayload::default());
        assert!(s.models.defaults.is_empty());
        assert!(s.models.global.entries.is_empty());
        assert_eq!(s.models.global.source, "");
        assert!(s.models.projects.is_empty());
        // Partial: `models` present but `projects`/`global` omitted.
        let partial: SettingsPayload =
            serde_json::from_str(r#"{"models": {"defaults": {"haiku": "claude-haiku-4-5"}}}"#).unwrap();
        assert_eq!(partial.models.defaults.get("haiku").map(String::as_str), Some("claude-haiku-4-5"));
        assert!(partial.models.projects.is_empty());
        assert_eq!(partial.models.global.source, "");
        // Old daemon omits default_model entirely → resolver falls back to "opus".
        assert_eq!(partial.models.default_model, "");
        assert_eq!(partial.models.default_model_for("anything"), "opus");
    }
}
