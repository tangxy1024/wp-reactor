use super::*;

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

/// `events { alias: window [&& filter] ... }`
#[non_exhaustive]
#[derive(::moju_derive::MoJu, Debug, Clone, PartialEq)]
#[moju(kind = "struct", domain = "Lang", module = "Lang.LangRule")]
pub struct EventsBlock {
    pub decls: Vec<EventDecl>,
}

/// `alias : window [&& filter]`
#[non_exhaustive]
#[derive(::moju_derive::MoJu, Debug, Clone, PartialEq)]
#[moju(kind = "struct", domain = "Lang", module = "Lang.LangRule")]
pub struct EventDecl {
    pub alias: String,
    pub window: String,
    pub filter: Option<Expr>,
}
