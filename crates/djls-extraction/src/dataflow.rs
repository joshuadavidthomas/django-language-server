pub(crate) mod calls;
mod constraints;
pub(crate) mod domain;
pub(crate) mod eval;

use ruff_python_ast::StmtFunctionDef;

use crate::types::TagRule;

pub use calls::HelperCache;

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
    func: &StmtFunctionDef,
    module_funcs: &[&StmtFunctionDef],
) -> TagRule {
    let mut cache = HelperCache::new();
    analyze_compile_function_with_cache(func, module_funcs, &mut cache)
}

/// Analyze a compile function with an existing helper cache.
///
/// When analyzing multiple compile functions in the same module, passing
/// a shared cache avoids re-analyzing helpers called by multiple functions.
#[must_use]
pub fn analyze_compile_function_with_cache(
    func: &StmtFunctionDef,
    module_funcs: &[&StmtFunctionDef],
    cache: &mut HelperCache,
) -> TagRule {
    let params = &func.parameters;
    let parser_param = params
        .args
        .first()
        .map_or("parser", |p| p.parameter.name.as_str());
    let token_param = params
        .args
        .get(1)
        .map_or("token", |p| p.parameter.name.as_str());

    let mut env = domain::Env::for_compile_function(parser_param, token_param);
    let mut ctx = eval::AnalysisContext {
        module_funcs,
        caller_name: func.name.as_str(),
        call_depth: 0,
        cache,
    };

    eval::process_statements(&func.body, &mut env, &mut ctx);

    let c = constraints::extract_constraints(&func.body, &env);

    TagRule {
        arg_constraints: c.arg_constraints,
        required_keywords: c.required_keywords,
        known_options: None,
        extracted_args: Vec::new(),
    }
}
