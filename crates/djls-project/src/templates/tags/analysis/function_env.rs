use std::cell::OnceCell;

use ruff_python_ast::StmtFunctionDef;

use crate::templates::tags::analysis::CallContext;
use crate::templates::tags::analysis::Env;
use crate::templates::tags::analysis::constants::StaticBindings;
use crate::templates::tags::analysis::constants::seed_static_bindings;
use crate::templates::tags::analysis::process_statements;

/// The compile function body interpreted like the Tag Rule interpreter does, once, on first use.
///
/// Block structure and unique-name extraction both read it, and interpreting a body is the
/// expensive part of either.
pub(crate) struct FunctionEnv<'a> {
    func: &'a StmtFunctionDef,
    static_bindings: Option<&'a StaticBindings>,
    parser_var: &'a str,
    token_var: &'a str,
    env: OnceCell<Env>,
}

impl<'a> FunctionEnv<'a> {
    pub(crate) fn new(
        func: &'a StmtFunctionDef,
        static_bindings: Option<&'a StaticBindings>,
        parser_var: &'a str,
        token_var: &'a str,
    ) -> Self {
        Self {
            func,
            static_bindings,
            parser_var,
            token_var,
            env: OnceCell::new(),
        }
    }

    /// Bind the parser and token to the function's first two parameters, as Django calls it.
    pub(crate) fn for_compile_function(
        func: &'a StmtFunctionDef,
        static_bindings: Option<&'a StaticBindings>,
    ) -> Option<Self> {
        let parser_var = func.parameters.args.first()?.parameter.name.as_str();
        let token_var = func.parameters.args.get(1)?.parameter.name.as_str();
        Some(Self::new(func, static_bindings, parser_var, token_var))
    }

    pub(crate) fn func(&self) -> &'a StmtFunctionDef {
        self.func
    }

    pub(crate) fn static_bindings(&self) -> Option<&'a StaticBindings> {
        self.static_bindings
    }

    pub(crate) fn parser_var(&self) -> &'a str {
        self.parser_var
    }

    pub(crate) fn token_var(&self) -> &'a str {
        self.token_var
    }

    pub(crate) fn get(&self) -> &Env {
        self.env.get_or_init(|| {
            let mut env = Env::for_compile_function(self.parser_var, self.token_var);
            if let Some(static_bindings) = self.static_bindings {
                seed_static_bindings(static_bindings, self.func, &mut env);
            }
            let mut ctx = CallContext {
                source: None,
                manual_as_var_strips: &[],
            };
            let _result = process_statements(&self.func.body, &mut env, &mut ctx);
            env
        })
    }
}
