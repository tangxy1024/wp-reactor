mod contracts;
mod intermediate;
pub mod lint;
mod rules;
mod scope;
mod types;

use crate::ast::{RuleDecl, WflFile};
use crate::schema::WindowSchema;

/// Severity level for semantic check diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

/// Semantic check error with optional rule/test context.
#[derive(Debug, Clone)]
pub struct CheckError {
    pub severity: Severity,
    pub rule: Option<String>,
    pub test: Option<String>,
    pub message: String,
}

impl std::fmt::Display for CheckError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let prefix = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        match (&self.rule, &self.test) {
            (Some(r), _) => write!(f, "{}: rule `{}`: {}", prefix, r, self.message),
            (_, Some(t)) => write!(f, "{}: test `{}`: {}", prefix, t, self.message),
            _ => write!(f, "{}: {}", prefix, self.message),
        }
    }
}

/// Perform L1 semantic checks on a parsed WflFile against the given window schemas.
/// Returns an empty Vec when all checks pass.
pub fn check_wfl(file: &WflFile, schemas: &[WindowSchema]) -> Vec<CheckError> {
    let mut errors = Vec::new();
    let effective_schemas = intermediate::effective_schemas_for_rules(&file.rules, schemas);

    for rule in &file.rules {
        rules::check_rule(rule, &effective_schemas, &mut errors);
    }

    intermediate::check_intermediate_target_graph(&file.rules, &mut errors, None);
    contracts::check_tests(file, &mut errors);

    rules::yield_version::check_yield_versions(file, &mut errors);

    errors
}

pub fn effective_schemas_for_rules(
    rules: &[RuleDecl],
    schemas: &[WindowSchema],
) -> Vec<WindowSchema> {
    intermediate::effective_schemas_for_rules(rules, schemas)
}

pub fn check_intermediate_target_graph(rules: &[RuleDecl], errors: &mut Vec<CheckError>) {
    intermediate::check_intermediate_target_graph(rules, errors, None);
}

#[cfg(test)]
mod tests;
