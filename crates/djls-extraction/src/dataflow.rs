mod calls;
mod constraints;
pub(crate) mod domain;
mod eval;

use ruff_python_ast::StmtFunctionDef;

use crate::types::TagRule;

/// Analyze a compile function using dataflow analysis to extract argument constraints.
///
/// This is the main entry point for the dataflow analyzer. It tracks `token`
/// and `parser` parameters through the function body, extracting constraints
/// from `raise TemplateSyntaxError(...)` guards.
///
/// `module_funcs` provides all function definitions in the same module, used
/// for bounded-depth inlining of helper function calls.
#[must_use]
pub fn analyze_compile_function(
    _func: &StmtFunctionDef,
    _module_funcs: &[&StmtFunctionDef],
) -> TagRule {
    TagRule {
        arg_constraints: Vec::new(),
        required_keywords: Vec::new(),
        known_options: None,
        extracted_args: Vec::new(),
    }
}
