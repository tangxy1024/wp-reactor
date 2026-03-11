mod alert;
mod close_exec;
mod context;
mod eval;
mod match_exec;

use std::collections::HashMap;

use wf_lang::FieldType;
use wf_lang::plan::RulePlan;

/// Evaluates score/entity expressions from a [`RulePlan`] and produces
/// [`OutputRecord`]s from CEP match/close outputs.
///
/// L1 rules use `execute_match` / `execute_close` (no joins).
/// L2 rules with joins use `execute_match_with_joins` / `execute_close_with_joins`
/// which accept a [`WindowLookup`] for resolving join data.
pub struct RuleExecutor {
    plan: RulePlan,
    yield_field_types: HashMap<String, FieldType>,
}

impl RuleExecutor {
    pub fn new(plan: RulePlan) -> Self {
        Self {
            plan,
            yield_field_types: HashMap::new(),
        }
    }

    pub fn new_with_yield_field_types(
        plan: RulePlan,
        yield_field_types: HashMap<String, FieldType>,
    ) -> Self {
        Self {
            plan,
            yield_field_types,
        }
    }

    pub fn plan(&self) -> &RulePlan {
        &self.plan
    }

    pub(crate) fn yield_field_type(&self, name: &str) -> Option<&FieldType> {
        self.yield_field_types.get(name)
    }
}
