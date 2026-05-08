use orion_error::{OrionError, StructError, UnifiedReason};

#[derive(Debug, Clone, PartialEq, OrionError)]
pub enum LangReason {
    #[orion_error(message = "parse error", identity = "logic.wf_lang.parse")]
    Parse,
    #[orion_error(message = "validation error", identity = "logic.wf_lang.validation")]
    Validation,
    #[orion_error(message = "compile error", identity = "logic.wf_lang.compile")]
    Compile,
    #[orion_error(transparent)]
    General(UnifiedReason),
}

pub type LangError = StructError<LangReason>;
pub type LangResult<T> = Result<T, LangError>;
