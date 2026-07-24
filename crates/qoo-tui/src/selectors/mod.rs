//! Pure derivation layer: snapshot → rows, column layouts, labels.
//!
//! Split across files via `include!` so they share one module namespace
//! (private helpers stay private; no `pub(super)` churn). No rendering, no I/O.

include!("rows.rs");
include!("labels.rs");
include!("cols.rs");

#[cfg(test)]
mod tests {
    include!("tests_a.rs");
    include!("tests_b.rs");
}
