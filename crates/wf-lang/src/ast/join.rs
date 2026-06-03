use std::time::Duration;

use super::*;

// ---------------------------------------------------------------------------
// Join clause
// ---------------------------------------------------------------------------

/// `join window snapshot/asof on cond`
#[non_exhaustive]
#[derive(::moju_derive::MoJu, Debug, Clone, PartialEq)]
#[moju(kind = "struct", domain = "Lang", module = "Lang.LangJoin")]
pub struct JoinClause {
    pub target_window: String,
    pub mode: JoinMode,
    pub conditions: Vec<JoinCondition>,
}

/// Join time-point semantics.
#[non_exhaustive]
#[derive(::moju_derive::MoJu, Debug, Clone, PartialEq)]
#[moju(kind = "state", domain = "Lang", module = "Lang.LangJoin")]
pub enum JoinMode {
    Snapshot,
    Asof { within: Option<Duration> },
}

/// `left == right` in a join on-clause.
#[non_exhaustive]
#[derive(::moju_derive::MoJu, Debug, Clone, PartialEq)]
#[moju(kind = "struct", domain = "Lang", module = "Lang.LangJoin")]
pub struct JoinCondition {
    pub left: FieldRef,
    pub right: FieldRef,
}
