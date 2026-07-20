use super::Evaluator;
use super::ExprExt;
use super::Origin;
use super::PythonBinding;
use super::PythonBindingState;
use super::PythonUnknownCause;
use super::PythonValue;
use super::PythonValueKind;
use super::ast;

impl Evaluator<'_> {
    pub(super) fn evaluate_binding(&self, expression: &ast::Expr) -> PythonBinding {
        let origin = self.origin(expression);
        if let Some(value) = expression.string_literal() {
            return PythonBinding::bound(PythonValue::string(value.to_string(), origin), origin);
        }
        if let Some(value) = expression.bool_literal() {
            return PythonBinding::bound(PythonValue::bool(value, origin), origin);
        }
        if let Some(path) = self
            .state
            .path_bindings()
            .evaluate(expression, self.module.path())
        {
            return PythonBinding::bound(PythonValue::path(path, origin), origin);
        }
        if let Some(name) = expression.name_target()
            && let Some(binding) = self.state.binding(name)
        {
            return binding.clone();
        }
        match expression {
            ast::Expr::List(list) => self.evaluate_sequence_binding(
                &list.elts,
                PythonValue::list(Vec::new(), origin),
                origin,
            ),
            ast::Expr::Tuple(tuple) => self.evaluate_sequence_binding(
                &tuple.elts,
                PythonValue::tuple(Vec::new(), origin),
                origin,
            ),
            ast::Expr::BinOp(binary) if binary.op == ast::Operator::Add => combine_bindings(
                &self.evaluate_binding(&binary.left),
                &self.evaluate_binding(&binary.right),
                origin,
                |left, right| left.add(&right, origin),
            ),
            ast::Expr::Dict(dict) => self.evaluate_dict_binding(dict, origin),
            ast::Expr::Attribute(attribute) => self.evaluate_attribute_binding(attribute, origin),
            _ => PythonBinding::unknown(&PythonUnknownCause::UnsupportedExpression, origin),
        }
    }

    /// Read `receiver.member`. Only a module receiver resolves through the
    /// policy-neutral member projection; every other receiver keeps the existing
    /// unsupported-expression behavior. Module writes remain unsupported and
    /// never create object state here.
    fn evaluate_attribute_binding(
        &self,
        attribute: &ast::ExprAttribute,
        origin: Origin,
    ) -> PythonBinding {
        let receiver = self.evaluate_binding(&attribute.value);
        receiver.project_module_alternatives(
            origin,
            |id, _constraints| {
                let member = self.project_module_member(id, attribute.attr.as_str(), origin);
                // An expression read translates residual absence to a typed
                // module-attribute unknown (distinct from the import caller's
                // `MissingImportMember`).
                member.replace_unbound_with(
                    Some(PythonBinding::unknown(
                        &PythonUnknownCause::ModuleAttribute {
                            module: id.name().clone(),
                            member: attribute.attr.to_string(),
                        },
                        origin,
                    )),
                    origin,
                )
            },
            &PythonUnknownCause::UnsupportedExpression,
        )
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

    fn evaluate_sequence_binding(
        &self,
        elements: &[ast::Expr],
        sequence: PythonValue,
        origin: Origin,
    ) -> PythonBinding {
        let mut lists = PythonBinding::bound(sequence, origin);
        for element in elements {
            let element_origin = self.origin(element);
            let (expression, starred) = match element {
                ast::Expr::Starred(starred) => (starred.value.as_ref(), true),
                _ => (element, false),
            };
            let values = self.evaluate_binding(expression);
            lists = combine_bindings(&lists, &values, element_origin, |mut result, value| {
                // A prior starred element may have collapsed this alternative to
                // an unsupported-expression unknown; do not resurrect it.
                if matches!(result.kind, PythonValueKind::Unknown(_)) {
                    return result;
                }
                if starred {
                    // A definitely non-iterable starred source (bool) makes the
                    // whole constructed expression unknown: no prefix that could
                    // never exist at runtime survives.
                    if result
                        .star_extend_construction(&value, element_origin)
                        .is_none()
                    {
                        return PythonValue::unknown(
                            PythonUnknownCause::UnsupportedExpression,
                            Some(origin),
                        );
                    }
                } else {
                    result.push_constructed_element(value);
                }
                result
            });
        }
        lists
    }

    fn evaluate_dict_binding(&self, dictionary: &ast::ExprDict, origin: Origin) -> PythonBinding {
        let mut dictionaries = PythonBinding::bound(PythonValue::empty_dict(origin), origin);
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
                        dictionary.extend_from_unpack(unpacked, item_origin);
                        result
                    },
                );
                continue;
            };

            // Evaluate the key and value alternatives first, then combine them
            // into complete single-entry dictionaries so no placeholder value
            // is ever stored in the log.
            let keys = self.evaluate_binding(key);
            let values = self.evaluate_binding(&item.value);
            let entries = combine_bindings(&keys, &values, item_origin, |key, value| {
                PythonValue::dict_entry(key, value, item_origin)
            });
            dictionaries =
                combine_bindings(&dictionaries, &entries, item_origin, |mut result, entry| {
                    let (PythonValueKind::Dict(dictionary), PythonValueKind::Dict(entry)) =
                        (&mut result.kind, entry.kind)
                    else {
                        unreachable!("dictionary construction combines dictionaries")
                    };
                    dictionary.append_entries_from(entry);
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
