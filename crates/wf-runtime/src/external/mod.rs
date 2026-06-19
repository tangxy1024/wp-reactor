//! External function runtime — bridges `external()` WFL calls to remote services.
//!
//! P0 implements Redis backend via `wp_knowledge::facade`.  Architecture:
//!
//! ```text
//! eval_builtin_func_with_l3("external", [service, args...])
//!   → ExternalRuntime::call(service, args)
//!     → LRU cache lookup
//!       hit  → return cached Value
//!       miss → Redis command (via wp_knowledge facade)
//!              → cache result → return Value
//!              error/timeout → on_error fallback
//! ```

mod redis_backend;
mod runtime;

pub use runtime::ExternalRuntime;
