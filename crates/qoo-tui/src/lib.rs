// Test fixtures build structs as `let mut x = T::default(); x.field = …` — the
// readable idiom for wide structs where only a few fields matter to the test.
// Allowed in test builds only; production code keeps the strict lint.
#![cfg_attr(test, allow(clippy::field_reassign_with_default))]

pub mod paths;
pub mod ipc;
pub mod event;
pub mod heal;
pub mod action_menu;
pub mod app;
pub mod detail;
pub mod keymap;
pub mod layout;
pub mod runfiles;
pub mod selectors;
pub mod markup;
pub mod view;
pub mod hit;
pub mod worktree_context;

#[cfg(test)]
pub mod test_fixtures;
