use super::Evaluator;
use super::ExprExt;
use super::Origin;
use super::PythonBinding;
use super::PythonBindingState;
use super::PythonDict;
use super::PythonDictItem;
use super::PythonList;
use super::PythonListItem;
use super::PythonUnknown;
use super::PythonUnknownCause;
use super::PythonValue;
use super::PythonValueKind;
use super::ast;
use super::evaluate_path;

impl Evaluator<'_> {
    pub(super) fn evaluate_binding(&self, expression: &ast::Expr) -> PythonBinding {
        let origin = self.origin(expression);
        if let Some(value) = expression.string_literal() {
            return PythonBinding::bound(
                PythonValue::known(PythonValueKind::Str(value.to_string()), origin),
                origin,
            );
        }
        if let Some(value) = expression.bool_literal() {
            return PythonBinding::bound(
                PythonValue::known(PythonValueKind::Bool(value), origin),
                origin,
            );
        }
        if let Some(path) =
            evaluate_path(expression, self.module.path(), &self.state.path_bindings())
        {
            return PythonBinding::bound(
                PythonValue::known(PythonValueKind::Path(path), origin),
                origin,
            );
        }
        if let Some(name) = expression.name_target()
            && let Some(binding) = self.state.binding(name)
        {
            return binding.clone();
        }
        match expression {
            ast::Expr::List(list) => self.evaluate_list_binding(&list.elts, origin),
            ast::Expr::Tuple(tuple) => self.evaluate_list_binding(&tuple.elts, origin),
            ast::Expr::BinOp(binary) if binary.op == ast::Operator::Add => combine_bindings(
                &self.evaluate_binding(&binary.left),
                &self.evaluate_binding(&binary.right),
                origin,
                |left, right| add_values(left, right, origin),
            ),
            ast::Expr::Dict(dict) => self.evaluate_dict_binding(dict, origin),
            _ => PythonBinding::unknown(&PythonUnknownCause::UnsupportedExpression, origin),
        }
    }

    pub(in crate::python::evaluation) fn evaluate_value(
        &self,
        expression: &ast::Expr,
    ) -> PythonValue {
        let origin = self.origin(expression);
        self.evaluate_binding(expression)
            .single_bound()
            .map_or_else(
                || PythonValue::unknown(PythonUnknownCause::UnsupportedExpression, Some(origin)),
                |bound| bound.value.clone(),
            )
    }

    fn evaluate_list_binding(&self, elements: &[ast::Expr], origin: Origin) -> PythonBinding {
        let mut lists = PythonBinding::bound(
            PythonValue::known(PythonValueKind::List(PythonList::new(Vec::new())), origin),
            origin,
        );
        for element in elements {
            let element_origin = self.origin(element);
            let (expression, starred) = match element {
                ast::Expr::Starred(starred) => (starred.value.as_ref(), true),
                _ => (element, false),
            };
            let values = self.evaluate_binding(expression);
            lists = combine_bindings(&lists, &values, element_origin, |mut result, value| {
                let PythonValueKind::List(list) = &mut result.kind else {
                    unreachable!("list construction starts with a list")
                };
                if starred {
                    match value.kind {
                        PythonValueKind::List(unpacked) => list.extend(&unpacked, element_origin),
                        PythonValueKind::Unknown(unknown) => {
                            list.append(&PythonListItem::UnknownUnpack(unknown));
                        }
                        PythonValueKind::Str(_)
                        | PythonValueKind::Bool(_)
                        | PythonValueKind::Path(_)
                        | PythonValueKind::Dict(_) => {
                            list.append(&PythonListItem::UnknownUnpack(PythonUnknown {
                                cause: PythonUnknownCause::UnsupportedExpression,
                                origin: Some(element_origin),
                            }));
                        }
                    }
                } else {
                    let item = match value.kind {
                        PythonValueKind::Unknown(unknown) => {
                            PythonListItem::UnknownElement(unknown)
                        }
                        PythonValueKind::Str(_)
                        | PythonValueKind::Bool(_)
                        | PythonValueKind::Path(_)
                        | PythonValueKind::List(_)
                        | PythonValueKind::Dict(_) => PythonListItem::Value(value),
                    };
                    list.append(&item);
                }
                result
            });
        }
        lists
    }

    fn evaluate_dict_binding(&self, dictionary: &ast::ExprDict, origin: Origin) -> PythonBinding {
        let mut dictionaries = PythonBinding::bound(
            PythonValue::known(
                PythonValueKind::Dict(PythonDict { items: Vec::new() }),
                origin,
            ),
            origin,
        );
        for item in &dictionary.items {
            let item_origin = self.origin(&item.value);
            let Some(key) = &item.key else {
                let unpacked = self.evaluate_binding(&item.value);
                dictionaries = combine_bindings(
                    &dictionaries,
                    &unpacked,
                    item_origin,
                    |mut result, unpacked| {
                        let PythonValueKind::Dict(dictionary) = &mut result.kind else {
                            unreachable!("dictionary construction starts with a dictionary")
                        };
                        match unpacked.kind {
                            PythonValueKind::Dict(unpacked) => {
                                dictionary.items.extend(unpacked.items);
                            }
                            PythonValueKind::Unknown(unknown) => {
                                dictionary
                                    .items
                                    .push(PythonDictItem::UnknownUnpack(unknown));
                            }
                            PythonValueKind::Str(_)
                            | PythonValueKind::Bool(_)
                            | PythonValueKind::Path(_)
                            | PythonValueKind::List(_) => {
                                dictionary.items.push(PythonDictItem::UnknownUnpack(
                                    PythonUnknown {
                                        cause: PythonUnknownCause::UnsupportedExpression,
                                        origin: Some(item_origin),
                                    },
                                ));
                            }
                        }
                        result
                    },
                );
                continue;
            };

            let keys = self.evaluate_binding(key);
            dictionaries =
                combine_bindings(&dictionaries, &keys, item_origin, |mut result, key| {
                    let PythonValueKind::Dict(dictionary) = &mut result.kind else {
                        unreachable!("dictionary construction starts with a dictionary")
                    };
                    dictionary.items.push(PythonDictItem::Entry {
                        key,
                        value: PythonValue::unknown(
                            PythonUnknownCause::UnsupportedExpression,
                            Some(item_origin),
                        ),
                    });
                    result
                });
            let values = self.evaluate_binding(&item.value);
            dictionaries =
                combine_bindings(&dictionaries, &values, item_origin, |mut result, value| {
                    let PythonValueKind::Dict(dictionary) = &mut result.kind else {
                        unreachable!("dictionary construction starts with a dictionary")
                    };
                    let Some(PythonDictItem::Entry { value: slot, .. }) =
                        dictionary.items.last_mut()
                    else {
                        unreachable!("a dictionary entry was just appended")
                    };
                    *slot = value;
                    result
                });
        }
        dictionaries
    }
}

fn combine_bindings(
    left: &PythonBinding,
    right: &PythonBinding,
    origin: Origin,
    combine: impl Fn(PythonValue, PythonValue) -> PythonValue,
) -> PythonBinding {
    let mut result: Option<PythonBinding> = None;
    for (left, left_constraints) in left.alternatives_with_constraints() {
        let PythonBindingState::Bound(left) = left else {
            continue;
        };
        for (right, right_constraints) in right.alternatives_with_constraints() {
            let PythonBindingState::Bound(right) = right else {
                continue;
            };
            let constraints = left_constraints.intersection(right_constraints);
            if constraints.is_impossible() {
                continue;
            }
            let alternative = PythonBinding::constrained_bound(
                combine(left.value.clone(), right.value.clone()),
                origin,
                &constraints,
            )
            .expect("combined binding constraints are feasible");
            result = Some(match result {
                Some(current) => current.join(alternative, origin),
                None => alternative,
            });
        }
    }
    result.unwrap_or_else(|| {
        PythonBinding::unknown(&PythonUnknownCause::UnsupportedExpression, origin)
    })
}

pub(super) fn add_values(left: PythonValue, right: PythonValue, origin: Origin) -> PythonValue {
    match (left.kind, right.kind) {
        (PythonValueKind::List(mut left), PythonValueKind::List(right)) => {
            left.extend(&right, origin);
            PythonValue::known(PythonValueKind::List(left), origin)
        }
        (
            PythonValueKind::Str(_)
            | PythonValueKind::Bool(_)
            | PythonValueKind::Path(_)
            | PythonValueKind::List(_)
            | PythonValueKind::Dict(_)
            | PythonValueKind::Unknown(_),
            _,
        ) => PythonValue::unknown(PythonUnknownCause::UnsupportedExpression, Some(origin)),
    }
}
