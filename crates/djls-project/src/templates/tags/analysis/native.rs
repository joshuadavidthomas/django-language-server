use djls_source::File;
use djls_source::Span;
use ruff_python_ast::ExprCall;

use crate::ast::ExprExt;
use crate::ast::RangedExt;
use crate::python::PythonFunctionDefinition;
use crate::templates::tags::analysis::TagSourceContext;
use crate::templates::tags::analysis::state::AbstractValue;
use crate::templates::tags::analysis::state::AssignmentCall;
use crate::templates::tags::analysis::state::Env;
use crate::templates::tags::types::AssignmentMode;

pub(super) struct ResolvedNativeCall<'call> {
    call: &'call ExprCall,
    file: File,
    function: Span,
}

pub(super) fn classify_native_call<'call>(
    target: &PythonFunctionDefinition,
    call: &'call ExprCall,
    source: &mut TagSourceContext<'_>,
) -> Option<ResolvedNativeCall<'call>> {
    let canonical = source
        .lookup
        .exact_exported_function("django.template.base", "token_kwargs")?;
    let module = canonical.module()?;
    (!module.search_path().is_project_code()
        && module.name().as_str() == "django.template.base"
        && target == &canonical
        && source
            .lookup
            .is_canonical_export(&canonical, "token_kwargs"))
    .then_some(ResolvedNativeCall {
        call,
        file: source.function.file(),
        function: source.function.definition_span(),
    })
}

impl ResolvedNativeCall<'_> {
    pub(super) fn evaluate(
        self,
        args: &[AbstractValue],
        keywords: &[AbstractValue],
        env: &mut Env,
    ) -> AbstractValue {
        let assignment = self.assignment(args, env);
        for argument in args.iter().chain(keywords) {
            env.forget_aliases(argument);
        }
        if let Some((name, assignment)) = assignment {
            env.set(
                name.to_string(),
                AbstractValue::AssignmentRemainder(assignment),
            );
            AbstractValue::AssignmentMap(assignment)
        } else {
            AbstractValue::Unknown
        }
    }

    fn assignment(&self, args: &[AbstractValue], env: &Env) -> Option<(&str, AssignmentCall)> {
        let [bits, _] = self.call.arguments.args.as_ref() else {
            return None;
        };
        let [
            bits_value @ AbstractValue::SplitResult(split),
            AbstractValue::Parser,
        ] = args
        else {
            return None;
        };
        let name = bits.name_target()?;
        // Evaluating a later argument can mutate the list already read here.
        if env.get(name) != bits_value {
            return None;
        }
        let mode = match self.call.arguments.keywords.as_ref() {
            [] => AssignmentMode::Modern,
            [keyword]
                if keyword
                    .arg
                    .as_ref()
                    .is_some_and(|name| name == "support_legacy")
                    && keyword.value.bool_literal() == Some(false) =>
            {
                AssignmentMode::Modern
            }
            [keyword]
                if keyword
                    .arg
                    .as_ref()
                    .is_some_and(|name| name == "support_legacy")
                    && keyword.value.bool_literal() == Some(true) =>
            {
                AssignmentMode::ModernOrLegacy
            }
            [_] | [_, ..] => return None,
        };
        Some((
            name,
            AssignmentCall {
                file: self.file,
                function: self.function,
                call: self.call.span(),
                split: *split,
                mode,
            },
        ))
    }
}
