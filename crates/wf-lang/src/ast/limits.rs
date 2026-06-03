// ---------------------------------------------------------------------------
// Limits block
// ---------------------------------------------------------------------------

/// `limits { max_memory = "256MB" max_instances = 10000 ... }`
#[non_exhaustive]
#[derive(::moju_derive::MoJu, Debug, Clone, PartialEq)]
#[moju(kind = "struct", domain = "Lang", module = "Lang.LangClauses")]
pub struct LimitsBlock {
    pub items: Vec<LimitItem>,
}

/// A single `key = value` entry in a limits block.
#[non_exhaustive]
#[derive(::moju_derive::MoJu, Debug, Clone, PartialEq)]
#[moju(kind = "struct", domain = "Lang", module = "Lang.LangClauses")]
pub struct LimitItem {
    pub key: String,
    pub value: String,
}
