mod primitives;
mod scenario;
mod syntax;
#[cfg(test)]
mod tests;

use winnow::combinator::{cut_err, opt};
use winnow::error::{StrContext, StrContextValue};
use winnow::prelude::*;

use crate::parse_utils::quoted_string;

use crate::error::{self, LangReason, LangResult};
use crate::wfg_ast::*;

use self::primitives::ws_skip;
use self::scenario::scenario_decl;

// ---------------------------------------------------------------------------
// Top-level
// ---------------------------------------------------------------------------

/// Parse a `.wfg` scenario file from a string.
pub fn parse_wfg(input: &str) -> LangResult<WfgFile> {
    let mut rest = input;
    let result = wfg_file(&mut rest)
        .map_err(|e| error::error(LangReason::Parse, format!("parse error: {e}")))?;

    ws_skip(&mut rest)
        .map_err(|e| error::error(LangReason::Parse, format!("parse error: {e}")))?;
    if !rest.is_empty() {
        return error::fail(
            LangReason::Parse,
            format!(
                "unexpected trailing content: {:?}",
                &rest[..rest.len().min(60)]
            ),
        );
    }
    Ok(result)
}

fn wfg_file(input: &mut &str) -> ModalResult<WfgFile> {
    let mut uses = Vec::new();
    loop {
        ws_skip(input)?;
        if opt(crate::parse_utils::kw("use"))
            .parse_next(input)?
            .is_some()
        {
            ws_skip(input)?;
            let path = cut_err(quoted_string)
                .context(StrContext::Expected(StrContextValue::Description(
                    "quoted path after 'use'",
                )))
                .parse_next(input)?;
            uses.push(UseDecl { path });
        } else {
            break;
        }
    }

    ws_skip(input)?;
    let parsed = scenario_decl(input)?;

    Ok(WfgFile {
        uses,
        scenario: parsed.scenario,
        syntax: parsed.syntax,
    })
}
