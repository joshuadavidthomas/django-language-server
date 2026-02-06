use ruff_python_ast::Expr;
use ruff_python_ast::Stmt;

use crate::context::FunctionContext;
use crate::parser::ParsedModule;
use crate::registry::RegistrationInfo;
use crate::types::DecoratorKind;
use crate::types::ExtractedArg;
use crate::types::ExtractedArgKind;
use crate::types::ExtractedRule;
use crate::types::RuleCondition;

/// Dispatch argument extraction based on decorator kind.
pub fn extract_args(
    parsed: &ParsedModule,
    registration: &RegistrationInfo,
    rules: &[ExtractedRule],
    ctx: &FunctionContext,
) -> Vec<ExtractedArg> {
    match &registration.decorator_kind {
        DecoratorKind::SimpleTag | DecoratorKind::InclusionTag | DecoratorKind::SimpleBlockTag => {
            extract_args_from_signature(parsed, registration)
        }
        DecoratorKind::Tag | DecoratorKind::HelperWrapper(_) | DecoratorKind::Custom(_) => {
            reconstruct_args_from_rules_and_ast(parsed, registration, rules, ctx)
        }
    }
}

/// Extract argument structure from a `simple_tag`/`inclusion_tag`/`simple_block_tag`
/// function signature.
///
/// These decorators use Django's `parse_bits()` internally, which maps function
/// parameters directly to template arguments. Handles:
/// - `takes_context=True` → skip first "context" param
/// - `simple_block_tag` → skip last "nodelist" param
/// - Regular params → positional args (required if no default)
/// - `*args` → `VarArgs`
/// - `**kwargs` → `KeywordArgs`
fn extract_args_from_signature(
    parsed: &ParsedModule,
    registration: &RegistrationInfo,
) -> Vec<ExtractedArg> {
    let module = parsed.ast();

    let func_def = module.body.iter().find_map(|stmt| {
        if let Stmt::FunctionDef(fd) = stmt {
            if fd.name.as_str() == registration.function_name {
                return Some(fd);
            }
        }
        None
    });

    let Some(func_def) = func_def else {
        return Vec::new();
    };

    let takes_context = has_takes_context(parsed, registration);

    let params = &func_def.parameters;
    let mut args: Vec<ExtractedArg> = Vec::new();

    let is_simple_block = matches!(registration.decorator_kind, DecoratorKind::SimpleBlockTag);

    let positional_params: Vec<_> = params
        .posonlyargs
        .iter()
        .chain(params.args.iter())
        .collect();

    let skip_first = takes_context && !positional_params.is_empty();
    let skip_last = is_simple_block && positional_params.len() > usize::from(skip_first);

    let start = usize::from(skip_first);
    let end = if skip_last {
        positional_params.len().saturating_sub(1)
    } else {
        positional_params.len()
    };

    for param in &positional_params[start..end] {
        let name = param.parameter.name.to_string();
        let required = param.default.is_none();
        args.push(ExtractedArg {
            name,
            kind: ExtractedArgKind::Variable,
            required,
        });
    }

    if params.vararg.is_some() {
        let name = params
            .vararg
            .as_ref()
            .map_or_else(|| "args".to_string(), |v| v.name.to_string());
        args.push(ExtractedArg {
            name,
            kind: ExtractedArgKind::VarArgs,
            required: false,
        });
    }

    if params.kwarg.is_some() {
        let name = params
            .kwarg
            .as_ref()
            .map_or_else(|| "kwargs".to_string(), |v| v.name.to_string());
        args.push(ExtractedArg {
            name,
            kind: ExtractedArgKind::KeywordArgs,
            required: false,
        });
    }

    args
}

/// Check whether a registration decorator has `takes_context=True`.
fn has_takes_context(parsed: &ParsedModule, registration: &RegistrationInfo) -> bool {
    let module = parsed.ast();

    let func_def = module.body.iter().find_map(|stmt| {
        if let Stmt::FunctionDef(fd) = stmt {
            if fd.name.as_str() == registration.function_name {
                return Some(fd);
            }
        }
        None
    });

    let Some(func_def) = func_def else {
        return false;
    };

    for decorator in &func_def.decorator_list {
        if let Expr::Call(call) = &decorator.expression {
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

    false
}

/// Reconstruct argument structure for a manual `@register.tag` function.
///
/// Uses extracted rules and AST patterns to infer argument structure:
/// 1. Determine arg count bounds from rules
/// 2. Fill known positions from `LiteralAt`/`ChoiceAt` rules
/// 3. Try to fill variable names from AST (tuple unpacking, indexed access)
/// 4. Fill remaining with generic names
fn reconstruct_args_from_rules_and_ast(
    parsed: &ParsedModule,
    registration: &RegistrationInfo,
    rules: &[ExtractedRule],
    ctx: &FunctionContext,
) -> Vec<ExtractedArg> {
    let (min_args, max_args) = infer_arg_bounds(rules);

    let Some(arg_count) = max_args.or(min_args) else {
        return Vec::new();
    };

    // arg_count is in split_contents terms (includes tag name at index 0)
    // Template args exclude tag name, so subtract 1
    let template_arg_count = arg_count.saturating_sub(1);
    if template_arg_count == 0 {
        return Vec::new();
    }

    // Create slots for each position
    let mut slots: Vec<Option<ExtractedArg>> = vec![None; template_arg_count];

    // Fill from LiteralAt/ChoiceAt rules (index offset: extraction N → slot N-1)
    fill_slots_from_rules(&mut slots, rules);

    // Try to fill variable names from AST
    fill_slots_from_ast(&mut slots, parsed, registration, ctx);

    // Fill remaining slots with generic names
    let mut result = Vec::with_capacity(template_arg_count);
    for (i, slot) in slots.into_iter().enumerate() {
        result.push(slot.unwrap_or_else(|| ExtractedArg {
            name: format!("arg{}", i + 1),
            kind: ExtractedArgKind::Variable,
            required: min_args.is_none_or(|min| i + 1 < min),
        }));
    }

    result
}

/// Infer argument count bounds from extracted rules.
///
/// Returns `(min_args, max_args)` in `split_contents` terms (includes tag name).
fn infer_arg_bounds(rules: &[ExtractedRule]) -> (Option<usize>, Option<usize>) {
    let mut min_args: Option<usize> = None;
    let mut max_args: Option<usize> = None;

    for rule in rules {
        match &rule.condition {
            RuleCondition::ExactArgCount { count, negated } => {
                if *negated {
                    // Error when != count, so exact count is required
                    min_args = Some(*count);
                    max_args = Some(*count);
                }
            }
            RuleCondition::MinArgCount { min } => {
                min_args = Some(min_args.map_or(*min, |m: usize| m.max(*min)));
            }
            RuleCondition::MaxArgCount { max } => {
                // MaxArgCount{max:3} means error when len <= 3
                // So minimum is max+1
                let implied_min = max + 1;
                min_args = Some(min_args.map_or(implied_min, |m: usize| m.max(implied_min)));
            }
            RuleCondition::ArgCountComparison { count, op } => {
                use crate::types::ComparisonOp;
                match op {
                    ComparisonOp::Lt => {
                        // Error when len < count → min is count
                        min_args = Some(min_args.map_or(*count, |m: usize| m.max(*count)));
                    }
                    ComparisonOp::LtEq => {
                        let implied = count + 1;
                        min_args = Some(min_args.map_or(implied, |m: usize| m.max(implied)));
                    }
                    ComparisonOp::Gt => {
                        // Error when len > count → max is count
                        max_args = Some(max_args.map_or(*count, |m: usize| m.min(*count)));
                    }
                    ComparisonOp::GtEq => {
                        let implied = count.saturating_sub(1);
                        max_args = Some(max_args.map_or(implied, |m: usize| m.min(implied)));
                    }
                }
            }
            RuleCondition::LiteralAt { index, .. } | RuleCondition::ChoiceAt { index, .. } => {
                // These imply at least index+1 args
                let implied = index + 1;
                min_args = Some(min_args.map_or(implied, |m: usize| m.max(implied)));
            }
            _ => {}
        }
    }

    (min_args, max_args)
}

/// Fill slots from `LiteralAt` and `ChoiceAt` rules.
fn fill_slots_from_rules(slots: &mut [Option<ExtractedArg>], rules: &[ExtractedRule]) {
    for rule in rules {
        match &rule.condition {
            RuleCondition::LiteralAt {
                index,
                value,
                negated,
            } => {
                if *negated {
                    // negated: error when bits[N-1] != value → position N-1 SHOULD be value
                    if let Some(slot_idx) = index.checked_sub(1) {
                        if slot_idx < slots.len() && slots[slot_idx].is_none() {
                            slots[slot_idx] = Some(ExtractedArg {
                                name: value.clone(),
                                kind: ExtractedArgKind::Literal {
                                    value: value.clone(),
                                },
                                required: true,
                            });
                        }
                    }
                }
            }
            RuleCondition::ChoiceAt {
                index,
                choices,
                negated,
            } => {
                if *negated {
                    // negated: error when bits[N-1] not in choices → position SHOULD be one of choices
                    if let Some(slot_idx) = index.checked_sub(1) {
                        if slot_idx < slots.len() && slots[slot_idx].is_none() {
                            slots[slot_idx] = Some(ExtractedArg {
                                name: choices.join("_or_"),
                                kind: ExtractedArgKind::Choice {
                                    values: choices.clone(),
                                },
                                required: true,
                            });
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// Fill slots from AST patterns (tuple unpacking and indexed access).
fn fill_slots_from_ast(
    slots: &mut [Option<ExtractedArg>],
    parsed: &ParsedModule,
    registration: &RegistrationInfo,
    ctx: &FunctionContext,
) {
    let module = parsed.ast();

    let func_def = module.body.iter().find_map(|stmt| {
        if let Stmt::FunctionDef(fd) = stmt {
            if fd.name.as_str() == registration.function_name {
                return Some(fd);
            }
        }
        None
    });

    let Some(func_def) = func_def else {
        return;
    };

    let split_var = ctx.split.as_deref();
    let Some(split_var) = split_var else {
        return;
    };

    // Look for tuple unpacking: `tag_name, arg1, arg2 = bits`
    for stmt in &func_def.body {
        if let Stmt::Assign(assign) = stmt {
            if is_name_expr(&assign.value, split_var) {
                if let Some(Expr::Tuple(tuple)) = assign.targets.first() {
                    // Skip index 0 (tag name), fill remaining
                    for (i, elt) in tuple.elts.iter().enumerate().skip(1) {
                        let slot_idx = i - 1;
                        if slot_idx < slots.len() && slots[slot_idx].is_none() {
                            if let Expr::Name(name) = elt {
                                let var_name = name.id.to_string();
                                // Skip names starting with _ (conventionally unused)
                                if !var_name.starts_with('_') {
                                    slots[slot_idx] = Some(ExtractedArg {
                                        name: var_name,
                                        kind: ExtractedArgKind::Variable,
                                        required: true,
                                    });
                                }
                            }
                        }
                    }
                    return;
                }
            }
        }
    }

    // Look for indexed access: `format_string = bits[1]`
    for stmt in &func_def.body {
        find_indexed_access_in_stmt(stmt, split_var, slots);
    }
}

/// Recursively search for `name = split_var[N]` patterns in statements.
fn find_indexed_access_in_stmt(stmt: &Stmt, split_var: &str, slots: &mut [Option<ExtractedArg>]) {
    match stmt {
        Stmt::Assign(assign) => {
            if let Some(Expr::Name(target_name)) = assign.targets.first() {
                if let Expr::Subscript(sub) = assign.value.as_ref() {
                    if is_name_expr(&sub.value, split_var) {
                        if let Some(idx) = extract_int_from_expr(&sub.slice) {
                            // Extraction index N → slot N-1
                            if let Some(slot_idx) =
                                idx.checked_sub(1).and_then(|v| usize::try_from(v).ok())
                            {
                                if slot_idx < slots.len() && slots[slot_idx].is_none() {
                                    slots[slot_idx] = Some(ExtractedArg {
                                        name: target_name.id.to_string(),
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
        Stmt::If(if_stmt) => {
            for s in &if_stmt.body {
                find_indexed_access_in_stmt(s, split_var, slots);
            }
            for clause in &if_stmt.elif_else_clauses {
                for s in &clause.body {
                    find_indexed_access_in_stmt(s, split_var, slots);
                }
            }
        }
        Stmt::For(for_stmt) => {
            for s in &for_stmt.body {
                find_indexed_access_in_stmt(s, split_var, slots);
            }
        }
        Stmt::Try(try_stmt) => {
            for s in &try_stmt.body {
                find_indexed_access_in_stmt(s, split_var, slots);
            }
            for handler in &try_stmt.handlers {
                let ruff_python_ast::ExceptHandler::ExceptHandler(h) = handler;
                for s in &h.body {
                    find_indexed_access_in_stmt(s, split_var, slots);
                }
            }
        }
        _ => {}
    }
}

/// Check if an expression is a name reference to the given variable.
fn is_name_expr(expr: &Expr, name: &str) -> bool {
    matches!(expr, Expr::Name(n) if n.id.as_str() == name)
}

/// Extract an integer value from an expression (for subscript indices).
fn extract_int_from_expr(expr: &Expr) -> Option<i64> {
    if let Expr::NumberLiteral(num) = expr {
        if let ruff_python_ast::Number::Int(int_val) = &num.value {
            return int_val.as_i64();
        }
    }
    // Handle negative indices like -1
    if let Expr::UnaryOp(unary) = expr {
        if matches!(unary.op, ruff_python_ast::UnaryOp::USub) {
            if let Some(val) = extract_int_from_expr(&unary.operand) {
                return Some(-val);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use crate::extract_rules;
    use crate::types::ExtractedArgKind;

    #[test]
    fn simple_tag_signature() {
        let source = r"
@register.simple_tag
def now(format_string):
    return datetime.now().strftime(format_string)
";
        let result = extract_rules(source).unwrap();
        assert_eq!(result.tags.len(), 1);
        let args = &result.tags[0].extracted_args;
        assert_eq!(args.len(), 1);
        assert_eq!(args[0].name, "format_string");
        assert!(matches!(args[0].kind, ExtractedArgKind::Variable));
        assert!(args[0].required);
    }

    #[test]
    fn simple_tag_with_optional() {
        let source = r#"
@register.simple_tag
def greeting(name, style="formal"):
    return f"Hello {name}"
"#;
        let result = extract_rules(source).unwrap();
        let args = &result.tags[0].extracted_args;
        assert_eq!(args.len(), 2);
        assert_eq!(args[0].name, "name");
        assert!(args[0].required);
        assert_eq!(args[1].name, "style");
        assert!(!args[1].required);
    }

    #[test]
    fn simple_tag_takes_context() {
        let source = r"
@register.simple_tag(takes_context=True)
def current_user(context):
    return context['user']
";
        let result = extract_rules(source).unwrap();
        let args = &result.tags[0].extracted_args;
        // context param should be skipped
        assert!(args.is_empty());
    }

    #[test]
    fn simple_tag_takes_context_with_args() {
        let source = r"
@register.simple_tag(takes_context=True)
def user_info(context, field):
    return context['user'].get(field)
";
        let result = extract_rules(source).unwrap();
        let args = &result.tags[0].extracted_args;
        assert_eq!(args.len(), 1);
        assert_eq!(args[0].name, "field");
        assert!(args[0].required);
    }

    #[test]
    fn inclusion_tag_signature() {
        let source = r#"
@register.inclusion_tag("results.html")
def show_results(poll):
    return {"choices": poll.choice_set.all()}
"#;
        let result = extract_rules(source).unwrap();
        let args = &result.tags[0].extracted_args;
        assert_eq!(args.len(), 1);
        assert_eq!(args[0].name, "poll");
        assert!(args[0].required);
    }

    #[test]
    fn inclusion_tag_takes_context() {
        let source = r#"
@register.inclusion_tag("nav.html", takes_context=True)
def navigation(context, section):
    return {"section": section}
"#;
        let result = extract_rules(source).unwrap();
        let args = &result.tags[0].extracted_args;
        // context skipped
        assert_eq!(args.len(), 1);
        assert_eq!(args[0].name, "section");
    }

    #[test]
    fn simple_block_tag_skips_nodelist() {
        let source = r"
@register.simple_block_tag
def myblock(content, nodelist):
    return nodelist.render(content)
";
        let result = extract_rules(source).unwrap();
        let args = &result.tags[0].extracted_args;
        // nodelist (last param) should be skipped
        assert_eq!(args.len(), 1);
        assert_eq!(args[0].name, "content");
    }

    #[test]
    fn simple_block_tag_takes_context_and_nodelist() {
        let source = r"
@register.simple_block_tag(takes_context=True)
def ctx_block(context, extra, nodelist):
    pass
";
        let result = extract_rules(source).unwrap();
        let args = &result.tags[0].extracted_args;
        // context skipped, nodelist skipped
        assert_eq!(args.len(), 1);
        assert_eq!(args[0].name, "extra");
    }

    #[test]
    fn simple_tag_varargs() {
        let source = r"
@register.simple_tag
def multi(first, *rest):
    return first
";
        let result = extract_rules(source).unwrap();
        let args = &result.tags[0].extracted_args;
        assert_eq!(args.len(), 2);
        assert_eq!(args[0].name, "first");
        assert!(args[0].required);
        assert_eq!(args[1].name, "rest");
        assert!(matches!(args[1].kind, ExtractedArgKind::VarArgs));
        assert!(!args[1].required);
    }

    #[test]
    fn simple_tag_kwargs() {
        let source = r"
@register.simple_tag
def options(name, **attrs):
    return name
";
        let result = extract_rules(source).unwrap();
        let args = &result.tags[0].extracted_args;
        assert_eq!(args.len(), 2);
        assert_eq!(args[0].name, "name");
        assert_eq!(args[1].name, "attrs");
        assert!(matches!(args[1].kind, ExtractedArgKind::KeywordArgs));
    }

    #[test]
    fn manual_tag_with_literal_from_rules() {
        let source = r#"
@register.tag("for")
def do_for(parser, token):
    parts = token.split_contents()
    if len(parts) < 4:
        raise TemplateSyntaxError("'for' needs at least four words")
    if parts[2] != "in":
        raise TemplateSyntaxError("'for' should use 'for x in y'")
    nodelist = parser.parse(("endfor",))
    parser.delete_first_token()
    return ForNode(nodelist)
"#;
        let result = extract_rules(source).unwrap();
        let tag = &result.tags[0];
        assert_eq!(tag.name, "for");

        // Should have reconstructed args with "in" literal at position 1
        // and variable names from other positions
        let args = &tag.extracted_args;
        assert!(!args.is_empty());

        // Position 1 (extraction index 2) should be literal "in"
        let in_arg = args
            .iter()
            .find(|a| matches!(&a.kind, ExtractedArgKind::Literal { value } if value == "in"));
        assert!(in_arg.is_some(), "Expected 'in' literal arg, got: {args:?}");
    }

    #[test]
    fn manual_tag_tuple_unpacking() {
        let source = r#"
@register.tag
def cycle(parser, token):
    bits = token.split_contents()
    if len(bits) < 2:
        raise TemplateSyntaxError("needs args")
    tag_name, *values = bits
    return CycleNode(values)
"#;
        let result = extract_rules(source).unwrap();
        let args = &result.tags[0].extracted_args;
        // With starred unpacking, we can't easily infer positional args
        // The fallback should still produce something
        assert!(!args.is_empty());
    }

    #[test]
    fn manual_tag_indexed_access() {
        let source = r#"
@register.tag
def mytag(parser, token):
    bits = token.split_contents()
    if len(bits) != 3:
        raise TemplateSyntaxError("needs 2 args")
    format_string = bits[1]
    output_var = bits[2]
    return MyNode(format_string, output_var)
"#;
        let result = extract_rules(source).unwrap();
        let args = &result.tags[0].extracted_args;
        assert_eq!(args.len(), 2);
        assert_eq!(args[0].name, "format_string");
        assert_eq!(args[1].name, "output_var");
    }

    #[test]
    fn manual_tag_no_rules_no_args() {
        let source = r"
@register.tag
def csrf_token(parser, token):
    return CsrfTokenNode()
";
        let result = extract_rules(source).unwrap();
        let args = &result.tags[0].extracted_args;
        assert!(args.is_empty());
    }
}
