pub mod context;
pub mod error;
pub mod expand;
pub mod scoped;
pub mod trace;

pub use context::ConfigVarContext;
pub use error::{VarsError, VarsReason, VarsResult};
pub use expand::{
    collect_active_external_sources, expand_toml, expand_toml_with_sources, expand_value,
    expand_value_with_sources, external_value_with_source, preprocess_toml, resolve_toml_vars,
    resolve_toml_vars_with_sources, resolve_value_vars, resolve_value_vars_with_sources,
};
pub(crate) use scoped::{
    inject_loader_scoped_vars, materialize_loader_scoped_vars, render_scoped_var_source_label,
};
pub use trace::{ExpandedToml, SourceAtom, TracedValue, render_source_label};
