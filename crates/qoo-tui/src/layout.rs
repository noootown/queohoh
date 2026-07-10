//! Per-project pane-layout persistence: the session divider overrides and the
//! three collapsed flags, keyed by project name and round-tripped to
//! `<state_dir>/tui-layout.json`. A missing or corrupt file degrades to
//! defaults — parsing never errors and never crashes the UI.

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// Current on-disk schema version.
pub const VERSION: u32 = 1;

/// One project's saved pane geometry. Every field is optional/defaulted so a
/// partial or older file still parses; `collapsed` is `[queue, tasks, worktrees]`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ProjectLayout {
    pub left_cols: Option<u16>,
    pub queue_h: Option<u16>,
    pub tasks_h: Option<u16>,
    pub collapsed: [bool; 3],
}

/// On-disk shape: `{ "version": 1, "projects": { "<name>": {...} } }`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LayoutFile {
    pub version: u32,
    pub projects: HashMap<String, ProjectLayout>,
}

/// Parse raw file bytes into a project→layout map. Any error (malformed JSON,
/// wrong shape) yields an empty map so a corrupt file degrades to defaults.
pub fn parse(bytes: &[u8]) -> HashMap<String, ProjectLayout> {
    serde_json::from_slice::<LayoutFile>(bytes)
        .map(|f| f.projects)
        .unwrap_or_default()
}

/// Serialize a project→layout map to the versioned on-disk JSON string.
pub fn serialize(projects: &HashMap<String, ProjectLayout>) -> String {
    let file = LayoutFile { version: VERSION, projects: projects.clone() };
    serde_json::to_string_pretty(&file).unwrap_or_else(|_| "{}".to_string())
}

/// Load the layout map from disk. Missing or corrupt file → empty map.
pub fn load(path: &Path) -> HashMap<String, ProjectLayout> {
    match std::fs::read(path) {
        Ok(bytes) => parse(&bytes),
        Err(_) => HashMap::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pl(left: Option<u16>, q: Option<u16>, t: Option<u16>, c: [bool; 3]) -> ProjectLayout {
        ProjectLayout { left_cols: left, queue_h: q, tasks_h: t, collapsed: c }
    }

    #[test]
    fn round_trips_projects_and_version() {
        let mut map = HashMap::new();
        map.insert("acme".to_string(), pl(Some(87), Some(20), Some(9), [false, true, false]));
        map.insert("web".to_string(), pl(None, None, None, [true, true, true]));
        let json = serialize(&map);
        // The versioned envelope and camelCase field names are on the wire.
        assert!(json.contains("\"version\": 1"));
        assert!(json.contains("\"leftCols\""));
        assert!(json.contains("\"queueH\""));
        assert!(json.contains("\"tasksH\""));
        let parsed = parse(json.as_bytes());
        assert_eq!(parsed, map);
    }

    #[test]
    fn corrupt_or_empty_input_yields_defaults() {
        assert!(parse(b"not json at all").is_empty());
        assert!(parse(b"").is_empty());
        assert!(parse(b"{\"version\": 1}").is_empty()); // no projects key → empty map
        // Garbage nested value still degrades to empty, never panics.
        assert!(parse(b"{\"projects\": 5}").is_empty());
    }

    #[test]
    fn per_project_isolation_survives_a_round_trip() {
        let mut map = HashMap::new();
        map.insert("a".to_string(), pl(Some(50), None, None, [true, false, false]));
        map.insert("b".to_string(), pl(Some(90), Some(30), None, [false, false, true]));
        let parsed = parse(serialize(&map).as_bytes());
        assert_eq!(parsed.get("a").unwrap().collapsed, [true, false, false]);
        assert_eq!(parsed.get("b").unwrap().collapsed, [false, false, true]);
        assert_eq!(parsed.get("a").unwrap().left_cols, Some(50));
        assert_eq!(parsed.get("b").unwrap().queue_h, Some(30));
    }

    #[test]
    fn partial_project_object_fills_defaults() {
        // A project with only `collapsed` set — the divider overrides default to
        // None (older files, or a project that only ever toggled collapse).
        let parsed = parse(b"{\"version\":1,\"projects\":{\"acme\":{\"collapsed\":[false,true,false]}}}");
        let acme = parsed.get("acme").unwrap();
        assert_eq!(acme.collapsed, [false, true, false]);
        assert_eq!(acme.left_cols, None);
        assert_eq!(acme.queue_h, None);
    }
}
