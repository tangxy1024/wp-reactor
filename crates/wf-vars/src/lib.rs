mod context;
mod expand;
mod trace;

pub use context::ConfigVarContext;
pub use expand::{
    collect_active_external_sources, expand_toml_with_sources, expand_value_with_sources,
    external_value_with_source, preprocess_toml, resolve_toml_vars_with_sources,
    resolve_value_vars_with_sources,
};
pub use trace::{ExpandedToml, SourceAtom, TracedValue, render_source_label};
