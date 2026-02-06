//! Argument structure extraction for template tags.
//!
//! This module extracts argument specifications from Python function signatures
//! and AST patterns. For `simple_tag`/`inclusion_tag`/`simple_block_tag`,
//! arguments come directly from the function signature. For manual `@register.tag`,
//! we reconstruct from `ExtractedRule` conditions and AST analysis.

use ruff_python_ast::Expr;
use ruff_python_ast::ModModule;
use ruff_python_ast::Number;
use ruff_python_ast::Stmt;

use crate::context::FunctionContext;
use crate::parser::ParsedModule;
use crate::registry::RegistrationInfo;
use crate::types::DecoratorKind;
use crate::types::ExtractedArg;
use crate::types::ExtractedArgKind;
use crate::types::ExtractedRule;
use crate::types::RuleCondition;
use crate::ExtractionError;

/// Extract argument structure for a tag registration.
///
/// Dispatches based on decorator kind:
/// - `SimpleTag` / `InclusionTag` / `SimpleBlockTag` → `extract_args_from_signature`
/// - `Tag` / `HelperWrapper` / `Custom` → `reconstruct_args_from_rules_and_ast`
pub fn extract_args(
    parsed: &ParsedModule,
    reg: &RegistrationInfo,
    rules: &[ExtractedRule],
    ctx: &FunctionContext,
) -> Vec<ExtractedArg> {
    match reg.decorator_kind {
        DecoratorKind::SimpleTag
        | DecoratorKind::InclusionTag
        | DecoratorKind::SimpleBlockTag => {
            extract_args_from_signature(parsed, reg).unwrap_or_default()
        }
        DecoratorKind::Tag | DecoratorKind::HelperWrapper(_) | DecoratorKind::Custom(_) => {
            reconstruct_args_from_rules_and_ast(parsed, reg, rules, ctx)
        }
    }
}

/// Extract argument structure from a `simple_tag`/`inclusion_tag`/`simple_block_tag` function.
///
/// These decorators use `parse_bits()` internally, which parses arguments
/// based on the function signature. The function parameters directly map
/// to template arguments.
///
/// Handles:
/// - Regular params → positional args (required if no default, optional if default)
/// - `*args` → VarArgs
/// - `**kwargs` → KeywordArgs
/// - `takes_context=True` → skip first param ("context")
fn extract_args_from_signature(
    parsed: &ParsedModule,
    reg: &RegistrationInfo,
) -> Result<Vec<ExtractedArg>, ExtractionError> {
    let ModModule { body, .. } = parsed.ast();

    let func_def = body.iter().find_map(|stmt| {
        if let Stmt::FunctionDef(fd) = stmt {
            if fd.name.as_str() == reg.function_name {
                return Some(fd);
            }
        }
        None
    });

    let Some(func_def) = func_def else {
        return Ok(Vec::new());
    };

    let mut extracted_args = Vec::new();
    let params = &func_def.parameters;

    // Determine if this function takes_context (check decorator kwargs)
    let mut takes_context = check_takes_context_decorator(parsed, reg);

    // For simple_block_tag, Django always passes context and nodelist
    // If takes_context isn't explicitly set, the first param is still context
    if matches!(reg.decorator_kind, DecoratorKind::SimpleBlockTag) && !takes_context {
        takes_context = true;
    }

    // Collect all positional parameters (posonlyargs + args)
    let mut positional_params: Vec<_> = params
        .posonlyargs
        .iter()
        .chain(params.args.iter())
        .collect();

    // Skip 'context' parameter if takes_context is true
    if takes_context && !positional_params.is_empty() {
        positional_params.remove(0);
    }

    // For simple_block_tag, skip the last parameter (nodelist)
    let is_simple_block = matches!(reg.decorator_kind, DecoratorKind::SimpleBlockTag);
    let end_index = if is_simple_block && !positional_params.is_empty() {
        positional_params.len() - 1
    } else {
        positional_params.len()
    };

    // Convert parameters to ExtractedArg
    for (_i, param) in positional_params.iter().enumerate().take(end_index) {
        let name = param.parameter.name.to_string();
        let has_default = param.default.is_some();

        let arg = ExtractedArg {
            name,
            kind: ExtractedArgKind::Variable,
            required: !has_default,
        };
        extracted_args.push(arg);
    }

    // Handle *args
    if params.vararg.is_some() {
        extracted_args.push(ExtractedArg {
            name: "args".to_string(),
            kind: ExtractedArgKind::VarArgs,
            required: false,
        });
    }

    // Handle **kwargs
    if params.kwarg.is_some() {
        extracted_args.push(ExtractedArg {
            name: "kwargs".to_string(),
            kind: ExtractedArgKind::KeywordArgs,
            required: false,
        });
    }

    // For simple_tag and inclusion_tag, Django allows `... as varname` syntax
    // Add optional `as varname` arguments
    if matches!(
        reg.decorator_kind,
        DecoratorKind::SimpleTag | DecoratorKind::InclusionTag
    ) {
        extracted_args.push(ExtractedArg {
            name: "as".to_string(),
            kind: ExtractedArgKind::Literal {
                value: "as".to_string(),
            },
            required: false,
        });
        extracted_args.push(ExtractedArg {
            name: "varname".to_string(),
            kind: ExtractedArgKind::Variable,
            required: false,
        });
    }

    Ok(extracted_args)
}

/// Check if the decorator has `takes_context=True`.
fn check_takes_context_decorator(parsed: &ParsedModule, reg: &RegistrationInfo) -> bool {
    let ModModule { body, .. } = parsed.ast();

    for stmt in body {
        if let Stmt::FunctionDef(func_def) = stmt {
            if func_def.name.as_str() == reg.function_name {
                for decorator in &func_def.decorator_list {
                    if let Expr::Call(call) = &decorator.expression {
                        // Check for takes_context keyword argument
                        for kw in &call.arguments.keywords {
                            if let Some(arg) = &kw.arg {
                                if arg.as_str() == "takes_context" {
                                    if let Expr::BooleanLiteral(b) = &kw.value {
                                        return b.value;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    false
}

/// Reconstruct argument structure for a manual `@register.tag` function.
///
/// Uses a combination of:
/// 1. ExtractedRule conditions (literal positions, choices, arg count bounds)
/// 2. AST analysis (tuple unpacking, indexed access for variable names)
/// 3. Generic fallback names when AST analysis can't determine names
///
/// The index offset is already accounted for: extraction indices include the
/// tag name (index 0), but extracted args exclude it. `LiteralAt{index:2}`
/// becomes arg position 1 (0-indexed in the result).
fn reconstruct_args_from_rules_and_ast(
    parsed: &ParsedModule,
    reg: &RegistrationInfo,
    rules: &[ExtractedRule],
    ctx: &FunctionContext,
) -> Vec<ExtractedArg> {
    // Determine total arg count from rules
    let max_index = rules
        .iter()
        .filter_map(|rule| match &rule.condition {
            RuleCondition::ExactArgCount { count, .. } => Some(*count),
            RuleCondition::MinArgCount { min } => Some(*min + 1),
            RuleCondition::MaxArgCount { max } => Some(*max + 1),
            RuleCondition::ArgCountComparison { count, .. } => Some(*count),
            RuleCondition::LiteralAt { index, .. } => Some(*index + 1),
            RuleCondition::ChoiceAt { index, .. } => Some(*index + 1),
            _ => None,
        })
        .max();

    let arg_count = max_index.unwrap_or(1).saturating_sub(1);

    if arg_count == 0 {
        return Vec::new();
    }

    let mut args: Vec<Option<ExtractedArg>> = vec![None; arg_count];

    // Fill known positions from LiteralAt/ChoiceAt rules (adjusting index by -1)
    for rule in rules {
        match &rule.condition {
            RuleCondition::LiteralAt { index, value, .. } if *index > 0 => {
                let bits_index = index - 1;
                if bits_index < arg_count {
                    args[bits_index] = Some(ExtractedArg {
                        name: value.clone(),
                        kind: ExtractedArgKind::Literal {
                            value: value.clone(),
                        },
                        required: true,
                    });
                }
            }
            RuleCondition::ChoiceAt {
                index, choices, ..
            } if *index > 0 => {
                let bits_index = index - 1;
                if bits_index < arg_count {
                    let name = choices.first().cloned().unwrap_or_else(|| "choice".to_string());
                    args[bits_index] = Some(ExtractedArg {
                        name,
                        kind: ExtractedArgKind::Choice {
                            values: choices.clone(),
                        },
                        required: true,
                    });
                }
            }
            _ => {}
        }
    }

    // Try to fill variable names from AST
    fill_arg_names_from_ast(parsed, reg, ctx, &mut args);

    // Fill remaining with generic names
    for (i, arg_slot) in args.iter_mut().enumerate() {
        if arg_slot.is_none() {
            *arg_slot = Some(ExtractedArg {
                name: format!("arg{i}"),
                kind: ExtractedArgKind::Variable,
                required: true,
            });
        }
    }

    // Determine required/optional from MinArgCount rules
    let min_required = rules
        .iter()
        .filter_map(|rule| match &rule.condition {
            RuleCondition::MinArgCount { min } => Some(*min),
            RuleCondition::ExactArgCount { count, .. } => Some(*count - 1),
            _ => None,
        })
        .min();

    if let Some(min) = min_required {
        let min_bits = min.saturating_sub(1);
        for (i, arg) in args.iter_mut().enumerate() {
            if let Some(ref mut a) = arg {
                a.required = i < min_bits;
            }
        }
    }

    args.into_iter().flatten().collect()
}

/// Fill argument names from AST patterns (tuple unpacking, indexed access).
fn fill_arg_names_from_ast(
    parsed: &ParsedModule,
    reg: &RegistrationInfo,
    ctx: &FunctionContext,
    args: &mut [Option<ExtractedArg>],
) {
    let ModModule { body, .. } = parsed.ast();

    let Some(func_def) = body.iter().find_map(|stmt| {
        if let Stmt::FunctionDef(fd) = stmt {
            if fd.name.as_str() == reg.function_name {
                return Some(fd);
            }
        }
        None
    }) else {
        return;
    };

    let split_var = ctx.split_var.as_deref();

    // Look for tuple unpacking: tag_name, arg1, arg2 = token.split_contents()
    for stmt in &func_def.body {
        if let Stmt::Assign(assign) = stmt {
            // Check if this is unpacking from split_contents
            if is_split_contents_call(&assign.value, split_var) {
                // Extract names from targets
                for (i, target) in assign.targets.iter().enumerate() {
                    if let Expr::Name(name) = target {
                        if i < args.len() && args[i].is_none() {
                            args[i] = Some(ExtractedArg {
                                name: name.id.to_string(),
                                kind: ExtractedArgKind::Variable,
                                required: true,
                            });
                        }
                    }
                }
            }
        }
    }

    // Look for indexed access: format_string = bits[1]
    for stmt in &func_def.body {
        if let Stmt::Assign(assign) = stmt {
            if let Some(Expr::Subscript(sub)) = assign.targets.first() {
                if let Expr::Name(target_name) = sub.value.as_ref() {
                    if Some(target_name.id.as_str()) == split_var {
                        if let Expr::NumberLiteral(num) = sub.slice.as_ref() {
                            if let Number::Int(int_val) = &num.value {
                                if let Some(index) = int_val.as_u64() {
                                    let bits_index = index as usize;
                                    if bits_index > 0 && bits_index - 1 < args.len() && args[bits_index - 1].is_none() {
                                        if let Expr::Name(var_name) = assign.value.as_ref() {
                                            args[bits_index - 1] = Some(ExtractedArg {
                                                name: var_name.id.to_string(),
                                                kind: ExtractedArgKind::Variable,
                                                required: true,
                                            });
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Check if an expression is `<split_var>[N].split_contents()`.
fn is_split_contents_call(expr: &Expr, split_var: Option<&str>) -> bool {
    let Expr::Call(call) = expr else { return false };
    let Expr::Attribute(attr) = call.func.as_ref() else { return false };

    if attr.attr.as_str() != "split_contents" {
        return false;
    }

    if let Some(expected) = split_var {
        if let Expr::Name(name) = attr.value.as_ref() {
            return name.id.as_str() == expected;
        }
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract_rules;

    #[test]
    fn test_simple_tag_extracts_signature() {
        let source = r#"
@register.simple_tag
def now(format_string):
    return datetime.now().strftime(format_string)
"#;
        let result = extract_rules(source).unwrap();
        assert_eq!(result.tags.len(), 1);
        assert_eq!(result.tags[0].extracted_args.len(), 3); // format_string + as + varname

        // First arg should be format_string
        assert_eq!(result.tags[0].extracted_args[0].name, "format_string");
        assert!(matches!(
            result.tags[0].extracted_args[0].kind,
            ExtractedArgKind::Variable
        ));
        assert!(result.tags[0].extracted_args[0].required);

        // Last two should be optional "as" and "varname"
        assert_eq!(result.tags[0].extracted_args[1].name, "as");
        assert!(!result.tags[0].extracted_args[1].required);
        assert_eq!(result.tags[0].extracted_args[2].name, "varname");
        assert!(!result.tags[0].extracted_args[2].required);
    }

    #[test]
    fn test_simple_tag_with_optional_param() {
        let source = r#"
@register.simple_tag
def greet(name="World"):
    return f"Hello, {name}!"
"#;
        let result = extract_rules(source).unwrap();
        assert_eq!(result.tags[0].extracted_args.len(), 3);

        // name should be optional
        assert_eq!(result.tags[0].extracted_args[0].name, "name");
        assert!(!result.tags[0].extracted_args[0].required);
    }

    #[test]
    fn test_simple_tag_takes_context_skips_context() {
        let source = r#"
@register.simple_tag(takes_context=True)
def my_tag(context, name):
    return context.get(name)
"#;
        let result = extract_rules(source).unwrap();
        assert_eq!(result.tags[0].extracted_args.len(), 3);

        // First arg should be name (context is skipped)
        assert_eq!(result.tags[0].extracted_args[0].name, "name");
    }

    #[test]
    fn test_inclusion_tag_extracts_signature() {
        let source = r#"
@register.inclusion_tag("template.html")
def show_results(results):
    return {"results": results}
"#;
        let result = extract_rules(source).unwrap();
        assert_eq!(result.tags[0].extracted_args.len(), 3);
        assert_eq!(result.tags[0].extracted_args[0].name, "results");
    }

    #[test]
    fn test_simple_block_tag_skips_nodelist() {
        let source = r#"
@register.simple_block_tag
def myblock(context, nodelist):
    return nodelist.render(context)
"#;
        let result = extract_rules(source).unwrap();
        // Should have no args (nodelist is skipped)
        assert!(result.tags[0].extracted_args.is_empty());
    }

    #[test]
    fn test_simple_block_tag_with_param() {
        let source = r#"
@register.simple_block_tag
def myblock(context, count, nodelist):
    return nodelist.render(context) * count
"#;
        let result = extract_rules(source).unwrap();
        assert_eq!(result.tags[0].extracted_args.len(), 1);
        assert_eq!(result.tags[0].extracted_args[0].name, "count");
    }

    #[test]
    fn test_manual_tag_reconstructs_from_rules() {
        // This simulates what the "for" tag would extract
        let source = r#"
@register.tag
def for_tag(parser, token):
    bits = token.split_contents()
    if len(bits) < 4:
        raise TemplateSyntaxError("'for' statements should have at least four words")
    if bits[2] != "in":
        raise TemplateSyntaxError("'for' statements should use 'for x in y'")
    return ForNode(bits[1], bits[3])
"#;
        let result = extract_rules(source).unwrap();
        let args = &result.tags[0].extracted_args;

        // Should have 3 args: target, "in" literal, iterable
        assert_eq!(args.len(), 3, "Expected 3 args: {:?}", args);

        // First should be variable
        assert!(matches!(args[0].kind, ExtractedArgKind::Variable));

        // Second should be "in" literal
        assert_eq!(args[1].name, "in");
        assert!(matches!(
            args[1].kind,
            ExtractedArgKind::Literal { value: ref v } if v == "in"
        ));

        // Third should be variable
        assert!(matches!(args[2].kind, ExtractedArgKind::Variable));
    }

    #[test]
    fn test_manual_tag_choice_from_rules() {
        let source = r#"
@register.tag
def autoescape(parser, token):
    bits = token.split_contents()
    if len(bits) != 2:
        raise TemplateSyntaxError("'autoescape' tag requires exactly one argument")
    if bits[1] not in ("on", "off"):
        raise TemplateSyntaxError("'autoescape' argument should be 'on' or 'off'")
    return AutoescapeNode(bits[1])
"#;
        let result = extract_rules(source).unwrap();
        let args = &result.tags[0].extracted_args;

        // Should have 1 arg: choice of "on"/"off"
        assert_eq!(args.len(), 1);
        assert!(matches!(
            &args[0].kind,
            ExtractedArgKind::Choice { values } if values.contains(&"on".to_string()) && values.contains(&"off".to_string())
        ));
    }

    #[test]
    fn test_simple_tag_with_args_kwargs() {
        let source = r#"
@register.simple_tag
def my_tag(*args, **kwargs):
    return str(args)
"#;
        let result = extract_rules(source).unwrap();
        let args = &result.tags[0].extracted_args;

        // Should have args, kwargs, as, varname
        assert_eq!(args.len(), 4);

        // First should be VarArgs
        assert!(matches!(args[0].kind, ExtractedArgKind::VarArgs));

        // Second should be KeywordArgs
        assert!(matches!(args[1].kind, ExtractedArgKind::KeywordArgs));
    }
}
