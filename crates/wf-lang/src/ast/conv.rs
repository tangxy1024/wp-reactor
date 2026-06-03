use super::Expr;

/// `conv { chain; chain; ... }`
#[non_exhaustive]
#[derive(::moju_derive::MoJu, Debug, Clone, PartialEq)]
#[moju(kind = "struct", domain = "Lang", module = "Lang.LangConv")]
pub struct ConvClause {
    pub chains: Vec<ConvChain>,
}

/// `step | step | step ;`
#[non_exhaustive]
#[derive(::moju_derive::MoJu, Debug, Clone, PartialEq)]
#[moju(kind = "struct", domain = "Lang", module = "Lang.LangConv")]
pub struct ConvChain {
    pub steps: Vec<ConvStep>,
}

/// A single conv operation: `sort(expr)`, `top(N)`, `dedup(expr)`, `where(expr)`
#[non_exhaustive]
#[derive(::moju_derive::MoJu, Debug, Clone, PartialEq)]
#[moju(kind = "state", domain = "Lang", module = "Lang.LangConv")]
pub enum ConvStep {
    Sort(Vec<SortKey>),
    Top(u64),
    Dedup(Expr),
    Where(Expr),
}

/// Sort key with direction: `expr` (ascending) or `-expr` (descending).
#[non_exhaustive]
#[derive(::moju_derive::MoJu, Debug, Clone, PartialEq)]
#[moju(kind = "struct", domain = "Lang", module = "Lang.LangConv")]
pub struct SortKey {
    pub expr: Expr,
    pub descending: bool,
}
