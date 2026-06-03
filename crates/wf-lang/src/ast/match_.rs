use std::time::Duration;

use super::*;

// ---------------------------------------------------------------------------
// Match clause
// ---------------------------------------------------------------------------

/// Window mode: sliding (default), fixed (L3), or session (L3).
#[derive(::moju_derive::MoJu, Debug, Clone, Copy, PartialEq, Eq)]
#[moju(kind = "state", domain = "Lang", module = "Lang.LangMatch")]
pub enum WindowMode {
    Sliding,
    Fixed,
    Session(std::time::Duration), // gap duration
}

/// Close block mode: OR (independent paths) or AND (both required).
#[derive(::moju_derive::MoJu, Debug, Clone, Copy, PartialEq, Eq)]
#[moju(kind = "state", domain = "Lang", module = "Lang.LangMatch")]
pub enum CloseMode {
    /// `on close { ... }` — event path and close path fire independently.
    Or,
    /// `and close { ... }` — both event and close paths must satisfy.
    And,
}

/// A parsed close block with its mode and steps.
#[derive(::moju_derive::MoJu, Debug, Clone, PartialEq)]
#[moju(kind = "struct", domain = "Lang", module = "Lang.LangMatch")]
pub struct CloseBlock {
    pub mode: CloseMode,
    pub steps: Vec<MatchStep>,
}

/// `match<keys:dur[:fixed]> { [key {...}] on event { ... } [on close|and close { ... }] }`
#[non_exhaustive]
#[derive(::moju_derive::MoJu, Debug, Clone, PartialEq)]
#[moju(kind = "struct", domain = "Lang", module = "Lang.LangMatch")]
pub struct MatchClause {
    pub keys: Vec<FieldRef>,
    pub key_mapping: Option<Vec<KeyMapItem>>,
    pub duration: Duration,
    pub window_mode: WindowMode,
    pub on_event: Vec<MatchStep>,
    pub on_close: Option<CloseBlock>,
}

impl MatchClause {
    pub fn placeholder() -> Self {
        Self {
            keys: Vec::new(),
            key_mapping: None,
            duration: Duration::from_secs(1),
            window_mode: WindowMode::Sliding,
            on_event: Vec::new(),
            on_close: None,
        }
    }
}

/// `on each alias [where expr]`
#[non_exhaustive]
#[derive(::moju_derive::MoJu, Debug, Clone, PartialEq)]
#[moju(kind = "struct", domain = "Lang", module = "Lang.LangMatch")]
pub struct EachClause {
    pub alias: String,
    pub filter: Option<Expr>,
}

/// Explicit key mapping: `logical = alias.field`
#[non_exhaustive]
#[derive(::moju_derive::MoJu, Debug, Clone, PartialEq)]
#[moju(kind = "struct", domain = "Lang", module = "Lang.LangMatch")]
pub struct KeyMapItem {
    pub logical_name: String,
    pub source_field: FieldRef,
}

/// One semicolon-terminated match step, potentially with `||` OR branches.
#[non_exhaustive]
#[derive(::moju_derive::MoJu, Debug, Clone, PartialEq)]
#[moju(kind = "struct", domain = "Lang", module = "Lang.LangMatch")]
pub struct MatchStep {
    pub branches: Vec<StepBranch>,
}

/// `[label:] source[.field]["field"] [&& guard] pipe_chain`
#[non_exhaustive]
#[derive(::moju_derive::MoJu, Debug, Clone, PartialEq)]
#[moju(kind = "struct", domain = "Lang", module = "Lang.LangMatch")]
pub struct StepBranch {
    pub label: Option<String>,
    pub source: String,
    pub field: Option<FieldSelector>,
    pub guard: Option<Expr>,
    pub pipe: PipeChain,
}

/// `{ | transform } | measure cmp threshold`
#[non_exhaustive]
#[derive(::moju_derive::MoJu, Debug, Clone, PartialEq)]
#[moju(kind = "struct", domain = "Lang", module = "Lang.LangMatch")]
pub struct PipeChain {
    pub transforms: Vec<Transform>,
    pub measure: Measure,
    pub cmp: CmpOp,
    pub threshold: Expr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Transform {
    Distinct,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Measure {
    Count,
    Sum,
    Avg,
    Min,
    Max,
}
