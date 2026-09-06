use ruff_python_ast::CmpOp;
use ruff_python_ast::Expr;
use ruff_python_ast::ExprCompare;
use ruff_python_ast::Stmt;
use ruff_python_ast::StmtIf;

use crate::templates::tags::analysis::CallContext;
use crate::templates::tags::analysis::constraints::ExtractedTagConstraints;
use crate::templates::tags::analysis::expressions::eval_expr;
use crate::templates::tags::analysis::extract_arg_names;
use crate::templates::tags::analysis::guards::extract_complete_rejecting_guard;
use crate::templates::tags::analysis::process_statements;
use crate::templates::tags::analysis::state::AbstractValue;
use crate::templates::tags::analysis::state::Env;
use crate::templates::tags::types::ArgumentCountConstraint;
use crate::templates::tags::types::ArgumentFormCoverage;
use crate::templates::tags::types::RequiredKeyword;
use crate::templates::tags::types::SplitPosition;
use crate::templates::tags::types::TagArgumentForm;
use crate::templates::tags::types::TagArgumentKind;
use crate::templates::tags::types::TagArgumentSyntax;

const MAX_FORM_STATES: usize = 64;

enum DirectFlow {
    Return,
    Raise,
}

fn direct_flow(body: &[Stmt]) -> Option<DirectFlow> {
    body.iter().find_map(|stmt| {
        if matches!(stmt, Stmt::Return(_)) {
            Some(DirectFlow::Return)
        } else if matches!(stmt, Stmt::Raise(_)) {
            Some(DirectFlow::Raise)
        } else {
            None
        }
    })
}

/// Recognize a fixed-length `if`/`elif` dispatch without turning statement
/// analysis into a general path-sensitive Python evaluator.
pub(super) fn extract_if_argument_syntax(
    stmt_if: &StmtIf,
    env: &Env,
    ctx: &mut CallContext<'_>,
) -> Option<TagArgumentSyntax> {
    let mut branches = Vec::new();
    branches.push((Some(stmt_if.test.as_ref()), stmt_if.body.as_slice()));
    branches.extend(
        stmt_if
            .elif_else_clauses
            .iter()
            .map(|clause| (clause.test.as_ref(), clause.body.as_slice())),
    );

    let mut forms = Vec::new();
    let mut complete = true;
    let mut handled_lengths = Vec::new();
    let mut saw_rejecting_else = false;

    for (test, body) in branches {
        let Some(test) = test else {
            saw_rejecting_else = matches!(direct_flow(body), Some(DirectFlow::Raise));
            if !saw_rejecting_else {
                complete = false;
            }
            continue;
        };

        let split_length = exact_split_length(test, env)?;
        if handled_lengths.contains(&split_length) {
            continue;
        }
        handled_lengths.push(split_length);

        if matches!(direct_flow(body), Some(DirectFlow::Raise)) {
            continue;
        }

        let branch = extract_forms_from_body(body, env.clone(), ctx, split_length);
        forms.extend(branch.forms);
        complete &= branch.complete;
        if forms.len() > MAX_FORM_STATES {
            forms.truncate(MAX_FORM_STATES);
            complete = false;
        }
    }

    // A lone equality check is commonly an early special case inside a
    // larger parser. Requiring alternatives keeps this recognizer scoped to
    // actual syntax dispatches.
    if handled_lengths.len() < 2 && !saw_rejecting_else {
        return None;
    }
    complete &= saw_rejecting_else;

    if forms.is_empty() {
        Some(TagArgumentSyntax::Unknown)
    } else {
        Some(TagArgumentSyntax::Forms {
            forms,
            coverage: if complete {
                ArgumentFormCoverage::Complete
            } else {
                ArgumentFormCoverage::Partial
            },
        })
    }
}

struct BranchForms {
    forms: Vec<TagArgumentForm>,
    complete: bool,
}

#[derive(Clone)]
struct FormState {
    env: Env,
    constraints: ExtractedTagConstraints,
}

struct FormStates {
    continuing: Vec<FormState>,
    returned: Vec<FormState>,
    complete: bool,
}

fn extract_forms_from_body(
    body: &[Stmt],
    env: Env,
    ctx: &mut CallContext<'_>,
    split_length: usize,
) -> BranchForms {
    let outcome = process_form_statements(
        body,
        vec![FormState {
            env,
            constraints: ExtractedTagConstraints::default(),
        }],
        ctx,
        split_length,
    );

    BranchForms {
        forms: outcome
            .returned
            .into_iter()
            .chain(outcome.continuing)
            .map(|state| build_form(split_length, &state.env, &state.constraints))
            .collect(),
        complete: outcome.complete,
    }
}

fn process_form_statements(
    body: &[Stmt],
    mut continuing: Vec<FormState>,
    ctx: &mut CallContext<'_>,
    split_length: usize,
) -> FormStates {
    let mut returned = Vec::new();
    let mut complete = true;

    for stmt in body {
        let mut next = Vec::new();
        for mut state in continuing {
            if let Stmt::If(nested) = stmt
                && let Some(mut alternatives) =
                    extract_literal_dispatch(nested, &state, ctx, split_length)
            {
                next.append(&mut alternatives.continuing);
                returned.append(&mut alternatives.returned);
                complete &= alternatives.complete;
                continue;
            }

            if let Stmt::If(nested) = stmt {
                let Some(constraints) = extract_complete_rejecting_guard(nested, &state.env) else {
                    complete = false;
                    continue;
                };
                state.constraints.extend(constraints);
                if constraints_are_compatible(&state.constraints, split_length) {
                    next.push(state);
                }
                continue;
            }

            if matches!(stmt, Stmt::Return(_)) {
                returned.push(state);
                continue;
            }
            if matches!(stmt, Stmt::Raise(_)) {
                continue;
            }

            let supported = matches!(
                stmt,
                Stmt::Assign(_) | Stmt::AnnAssign(_) | Stmt::Expr(_) | Stmt::Pass(_)
            );
            if !supported {
                complete = false;
                continue;
            }

            let (result, _) = process_statements(std::slice::from_ref(stmt), &mut state.env, ctx);
            state.constraints.extend(result.constraints);
            if constraints_are_compatible(&state.constraints, split_length) {
                next.push(state);
            }
        }

        continuing = next;
        if continuing.len() + returned.len() > MAX_FORM_STATES {
            returned.truncate(MAX_FORM_STATES);
            continuing.truncate(MAX_FORM_STATES.saturating_sub(returned.len()));
            complete = false;
        }
        if continuing.is_empty() {
            break;
        }
    }

    FormStates {
        continuing,
        returned,
        complete,
    }
}

fn extract_literal_dispatch(
    stmt_if: &StmtIf,
    state: &FormState,
    ctx: &mut CallContext<'_>,
    split_length: usize,
) -> Option<FormStates> {
    let mut branches = Vec::new();
    branches.push((Some(stmt_if.test.as_ref()), stmt_if.body.as_slice()));
    branches.extend(
        stmt_if
            .elif_else_clauses
            .iter()
            .map(|clause| (clause.test.as_ref(), clause.body.as_slice())),
    );

    let mut continuing = Vec::new();
    let mut returned = Vec::new();
    let mut complete = true;
    let mut handled_literals = Vec::new();
    let mut saw_rejecting_else = false;

    for (test, body) in branches {
        let Some(test) = test else {
            saw_rejecting_else = matches!(direct_flow(body), Some(DirectFlow::Raise));
            if !saw_rejecting_else {
                complete = false;
            }
            continue;
        };
        let (position, literal) = split_literal_equality(test, &state.env)?;
        if handled_literals
            .iter()
            .any(|(handled_position, handled_literal)| {
                *handled_position == position && handled_literal == &literal
            })
        {
            continue;
        }
        if handled_literals
            .iter()
            .any(|(handled_position, _)| *handled_position != position)
        {
            // Reaching this elif also requires every earlier condition at a
            // different position to be false. Forms cannot express that
            // negative cross-position correlation.
            complete = false;
        }
        handled_literals.push((position, literal.clone()));

        if matches!(direct_flow(body), Some(DirectFlow::Raise)) {
            continue;
        }

        let mut branch_state = state.clone();
        branch_state
            .constraints
            .required_keywords
            .push(RequiredKeyword {
                position,
                value: literal,
            });
        if !constraints_are_compatible(&branch_state.constraints, split_length) {
            continue;
        }

        let mut branch = process_form_statements(body, vec![branch_state], ctx, split_length);
        continuing.append(&mut branch.continuing);
        returned.append(&mut branch.returned);
        complete &= branch.complete;
        if continuing.len() + returned.len() > MAX_FORM_STATES {
            returned.truncate(MAX_FORM_STATES);
            continuing.truncate(MAX_FORM_STATES.saturating_sub(returned.len()));
            complete = false;
        }
    }

    complete &= saw_rejecting_else;
    Some(FormStates {
        continuing,
        returned,
        complete,
    })
}

fn build_form(
    split_length: usize,
    env: &Env,
    constraints: &ExtractedTagConstraints,
) -> TagArgumentForm {
    let count = [ArgumentCountConstraint::Exact(split_length)];
    let mut form = TagArgumentForm {
        arguments: extract_arg_names(
            env,
            &constraints.required_keywords,
            &constraints.choice_at_constraints,
            &count,
        ),
    };
    apply_constraints(&mut form, constraints);
    form
}

fn constraints_are_compatible(constraints: &ExtractedTagConstraints, split_length: usize) -> bool {
    if constraints
        .arg_constraints
        .iter()
        .any(|constraint| match constraint {
            ArgumentCountConstraint::Exact(length) => split_length != *length,
            ArgumentCountConstraint::Min(length) => split_length < *length,
            ArgumentCountConstraint::Max(length) => split_length > *length,
            ArgumentCountConstraint::OneOf(lengths) => !lengths.contains(&split_length),
        })
    {
        return false;
    }

    let arguments_len = split_length.saturating_sub(1);

    for index in 0..arguments_len {
        let required = constraints
            .required_keywords
            .iter()
            .filter(|keyword| keyword.position.to_bits_index(arguments_len) == Some(index))
            .map(|keyword| keyword.value.as_str())
            .collect::<Vec<_>>();
        if required
            .first()
            .is_some_and(|first| required.iter().any(|value| value != first))
        {
            return false;
        }

        let choices = constraints
            .choice_at_constraints
            .iter()
            .filter(|choice| choice.position.to_bits_index(arguments_len) == Some(index))
            .collect::<Vec<_>>();
        if let Some(required) = required.first()
            && choices
                .iter()
                .any(|choice| !choice.values.iter().any(|value| value == required))
        {
            return false;
        }
        if let Some(first) = choices.first()
            && !first.values.iter().any(|candidate| {
                choices
                    .iter()
                    .skip(1)
                    .all(|choice| choice.values.contains(candidate))
            })
        {
            return false;
        }
    }

    true
}

fn apply_constraints(form: &mut TagArgumentForm, constraints: &ExtractedTagConstraints) {
    let arguments_len = form.arguments.len();
    for (index, argument) in form.arguments.iter_mut().enumerate() {
        let choices = constraints
            .choice_at_constraints
            .iter()
            .filter(|choice| choice.position.to_bits_index(arguments_len) == Some(index))
            .collect::<Vec<_>>();
        if let Some(first) = choices.first() {
            let intersection = first
                .values
                .iter()
                .filter(|candidate| {
                    choices
                        .iter()
                        .skip(1)
                        .all(|choice| choice.values.contains(candidate))
                })
                .cloned()
                .collect::<Vec<_>>();
            argument.kind = TagArgumentKind::Choice(intersection);
        }

        if let Some(keyword) = constraints
            .required_keywords
            .iter()
            .find(|keyword| keyword.position.to_bits_index(arguments_len) == Some(index))
        {
            argument.name.clone_from(&keyword.value);
            argument.kind = TagArgumentKind::Literal(keyword.value.clone());
        }
    }
}

fn exact_split_length(expr: &Expr, env: &Env) -> Option<usize> {
    let Expr::Compare(ExprCompare {
        left,
        ops,
        comparators,
        ..
    }) = expr
    else {
        return None;
    };
    let [CmpOp::Eq] = &**ops else {
        return None;
    };
    let [right] = &**comparators else {
        return None;
    };

    let mut local_env = env.clone();
    match (
        eval_expr(left, &mut local_env),
        eval_expr(right, &mut local_env),
    ) {
        (AbstractValue::SplitLength(split), AbstractValue::Int(value))
        | (AbstractValue::Int(value), AbstractValue::SplitLength(split)) => usize::try_from(value)
            .ok()
            .map(|value| split.resolve_length(value)),
        _ => None,
    }
}

fn split_literal_equality(expr: &Expr, env: &Env) -> Option<(SplitPosition, String)> {
    let Expr::Compare(ExprCompare {
        left,
        ops,
        comparators,
        ..
    }) = expr
    else {
        return None;
    };
    let [CmpOp::Eq] = &**ops else {
        return None;
    };
    let [right] = &**comparators else {
        return None;
    };

    let mut local_env = env.clone();
    match (
        eval_expr(left, &mut local_env),
        eval_expr(right, &mut local_env),
    ) {
        (AbstractValue::SplitElement { index }, AbstractValue::Str(value))
        | (AbstractValue::Str(value), AbstractValue::SplitElement { index }) => {
            Some((index, value))
        }
        _ => None,
    }
}
