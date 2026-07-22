use super::ExprExt;
use super::Origin;
use super::PythonBinding;
use super::PythonBindingState;
use super::PythonModuleEvaluator;
use super::PythonUnknownCause;
use super::PythonValue;
use super::PythonValueKind;
use super::ast;
use crate::python::PythonPath;
use crate::python::PythonPathIntrinsic;

impl PythonModuleEvaluator<'_> {
    pub(super) fn evaluate_binding(&self, expression: &ast::Expr) -> PythonBinding {
        let origin = self.origin(expression);
        if let Some(value) = expression.string_literal() {
            return PythonBinding::bound(PythonValue::string(value.to_string(), origin), origin);
        }
        if let Some(value) = expression.bool_literal() {
            return PythonBinding::bound(PythonValue::bool(value, origin), origin);
        }
        if let Some(name) = expression.name_target() {
            if name == "__file__" {
                return self.state.binding_with_implicit_value(
                    name,
                    PythonValue::string(self.module.path().to_string(), origin),
                    origin,
                );
            }
            if let Some(intrinsic) = PythonPathIntrinsic::unbound_intrinsic(name) {
                return self.state.binding_with_intrinsic(name, intrinsic, origin);
            }
            return self.state.binding(name).cloned().unwrap_or_else(|| {
                PythonBinding::unknown(&PythonUnknownCause::UnsupportedExpression, origin)
            });
        }
        if is_unsupported_literal(expression) {
            return PythonBinding::bound(PythonValue::unsupported_literal(origin), origin);
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
            ast::Expr::BinOp(binary) if binary.op == ast::Operator::Div => combine_bindings(
                &self.evaluate_binding(&binary.left),
                &self.evaluate_binding(&binary.right),
                origin,
                |left, right| join_path_value(left, right, origin),
            ),
            ast::Expr::Call(call) => self.evaluate_call_binding(call, origin),
            ast::Expr::Dict(dict) => self.evaluate_dict_binding(dict, origin),
            ast::Expr::Attribute(attribute) => self.evaluate_attribute_binding(attribute, origin),
            ast::Expr::BoolOp(_)
            | ast::Expr::Named(_)
            | ast::Expr::BinOp(_)
            | ast::Expr::UnaryOp(_)
            | ast::Expr::Lambda(_)
            | ast::Expr::If(_)
            | ast::Expr::Set(_)
            | ast::Expr::ListComp(_)
            | ast::Expr::SetComp(_)
            | ast::Expr::DictComp(_)
            | ast::Expr::Generator(_)
            | ast::Expr::Await(_)
            | ast::Expr::Yield(_)
            | ast::Expr::YieldFrom(_)
            | ast::Expr::Compare(_)
            | ast::Expr::FString(_)
            | ast::Expr::TString(_)
            | ast::Expr::StringLiteral(_)
            | ast::Expr::BytesLiteral(_)
            | ast::Expr::NumberLiteral(_)
            | ast::Expr::BooleanLiteral(_)
            | ast::Expr::NoneLiteral(_)
            | ast::Expr::EllipsisLiteral(_)
            | ast::Expr::Subscript(_)
            | ast::Expr::Starred(_)
            | ast::Expr::Name(_)
            | ast::Expr::Slice(_)
            | ast::Expr::IpyEscapeCommand(_) => {
                PythonBinding::unknown(&PythonUnknownCause::UnsupportedExpression, origin)
            }
        }
    }

    /// Read `receiver.member` through the receiver's nominal abstract value.
    /// Module values use the import member projection; exact path values and
    /// path-library intrinsics expose only their supported static members.
    fn evaluate_attribute_binding(
        &self,
        attribute: &ast::ExprAttribute,
        origin: Origin,
    ) -> PythonBinding {
        let receiver = self.evaluate_binding(&attribute.value);
        project_bound_alternatives(&receiver, origin, |value| match &value.kind {
            PythonValueKind::Module(id) => {
                let member = self.project_module_member(id, attribute.attr.as_str(), origin);
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
            }
            PythonValueKind::Path(path) if attribute.attr.as_str() == "parent" => {
                path.parent().map_or_else(
                    || PythonBinding::unknown(&PythonUnknownCause::UnsupportedExpression, origin),
                    |parent| PythonBinding::bound(PythonValue::python_path(parent, origin), origin),
                )
            }
            PythonValueKind::Path(PythonPath::Intrinsic(intrinsic)) => {
                if self
                    .state
                    .module_effects
                    .path_intrinsic_is_contaminated(*intrinsic)
                {
                    PythonBinding::unknown(&PythonUnknownCause::UnsupportedMutation, origin)
                } else {
                    intrinsic.member(attribute.attr.as_str()).map_or_else(
                        || {
                            PythonBinding::unknown(
                                &PythonUnknownCause::UnsupportedExpression,
                                origin,
                            )
                        },
                        |member| {
                            PythonBinding::bound(
                                PythonValue::path_intrinsic(member, origin),
                                origin,
                            )
                        },
                    )
                }
            }
            PythonValueKind::Str(_)
            | PythonValueKind::Bool(_)
            | PythonValueKind::Path(_)
            | PythonValueKind::UnsupportedLiteral
            | PythonValueKind::List(_)
            | PythonValueKind::Tuple(_)
            | PythonValueKind::Dict(_)
            | PythonValueKind::Unknown(_) => {
                PythonBinding::unknown(&PythonUnknownCause::UnsupportedExpression, origin)
            }
        })
    }

    fn evaluate_call_binding(&self, call: &ast::ExprCall, origin: Origin) -> PythonBinding {
        if let ast::Expr::Attribute(attribute) = call.func.as_ref()
            && matches!(attribute.attr.as_str(), "resolve" | "joinpath")
        {
            if !call.arguments.keywords.is_empty() {
                return PythonBinding::unknown(&PythonUnknownCause::UnsupportedExpression, origin);
            }
            let receiver = self.evaluate_binding(&attribute.value);
            return if attribute.attr.as_str() == "joinpath" {
                let paths = project_bound_alternatives(&receiver, origin, |value| {
                    rebase_exact_path(value, origin)
                });
                self.join_path_arguments(paths, &call.arguments.args, origin)
            } else if call.arguments.args.is_empty() {
                project_bound_alternatives(&receiver, origin, |value| resolve_path(value, origin))
            } else {
                PythonBinding::unknown(&PythonUnknownCause::UnsupportedExpression, origin)
            };
        }

        let callable = self.evaluate_binding(&call.func);
        project_bound_alternatives(&callable, origin, |value| {
            self.evaluate_path_intrinsic_call(value, call, origin)
        })
    }

    fn evaluate_path_intrinsic_call(
        &self,
        value: &PythonValue,
        call: &ast::ExprCall,
        origin: Origin,
    ) -> PythonBinding {
        let PythonValueKind::Path(path) = &value.kind else {
            return PythonBinding::unknown(&PythonUnknownCause::UnsupportedExpression, origin);
        };
        let PythonPath::Intrinsic(intrinsic) = path else {
            return PythonBinding::unknown(&PythonUnknownCause::UnsupportedExpression, origin);
        };
        let intrinsic = *intrinsic;
        if self
            .state
            .module_effects
            .path_intrinsic_is_contaminated(intrinsic)
        {
            return PythonBinding::unknown(&PythonUnknownCause::UnsupportedMutation, origin);
        }
        match intrinsic {
            PythonPathIntrinsic::PathlibPathType => {
                let Some(argument) = single_positional_argument(&call.arguments) else {
                    return PythonBinding::unknown(
                        &PythonUnknownCause::UnsupportedExpression,
                        origin,
                    );
                };
                let argument = self.evaluate_binding(argument);
                project_bound_alternatives(&argument, origin, |argument| {
                    path_from_value(argument, origin)
                })
            }
            PythonPathIntrinsic::BuiltinStrType => {
                let Some(argument) = single_positional_argument(&call.arguments) else {
                    return PythonBinding::unknown(
                        &PythonUnknownCause::UnsupportedExpression,
                        origin,
                    );
                };
                let argument = self.evaluate_binding(argument);
                project_bound_alternatives(&argument, origin, |argument| {
                    string_path_from_value(argument, origin)
                })
            }
            PythonPathIntrinsic::OsPathJoinFunction
            | PythonPathIntrinsic::OsPathDirnameFunction
                if cfg!(windows) =>
            {
                PythonBinding::unknown(&PythonUnknownCause::UnsupportedExpression, origin)
            }
            PythonPathIntrinsic::OsPathJoinFunction => {
                if call.arguments.keywords.is_empty() && !call.arguments.args.is_empty() {
                    let first = self.evaluate_binding(&call.arguments.args[0]);
                    let base = project_bound_alternatives(&first, origin, |value| {
                        string_path_from_value(value, origin)
                    });
                    self.join_string_path_arguments(base, &call.arguments.args[1..], origin)
                } else {
                    PythonBinding::unknown(&PythonUnknownCause::UnsupportedExpression, origin)
                }
            }
            PythonPathIntrinsic::OsPathDirnameFunction => {
                let Some(argument) = single_positional_argument(&call.arguments) else {
                    return PythonBinding::unknown(
                        &PythonUnknownCause::UnsupportedExpression,
                        origin,
                    );
                };
                let argument = self.evaluate_binding(argument);
                project_bound_alternatives(&argument, origin, |value| {
                    let path = string_path_from_value(value, origin);
                    project_bound_alternatives(&path, origin, |path| {
                        parent_string_path(path, origin)
                    })
                })
            }
            PythonPathIntrinsic::BuiltinsModule
            | PythonPathIntrinsic::PathlibModule
            | PythonPathIntrinsic::OsModule
            | PythonPathIntrinsic::OsPathModule => {
                PythonBinding::unknown(&PythonUnknownCause::UnsupportedExpression, origin)
            }
        }
    }

    fn join_path_arguments(
        &self,
        mut paths: PythonBinding,
        arguments: &[ast::Expr],
        origin: Origin,
    ) -> PythonBinding {
        for argument in arguments {
            let segment = self.evaluate_binding(argument);
            paths = combine_bindings(&paths, &segment, origin, |path, segment| {
                join_path_value(path, segment, origin)
            });
        }
        paths
    }

    fn join_string_path_arguments(
        &self,
        mut paths: PythonBinding,
        arguments: &[ast::Expr],
        origin: Origin,
    ) -> PythonBinding {
        for argument in arguments {
            let segment = self.evaluate_binding(argument);
            paths = combine_bindings(&paths, &segment, origin, |path, segment| {
                join_string_path_value(path, segment, origin)
            });
        }
        paths
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
                ast::Expr::BoolOp(_)
                | ast::Expr::Named(_)
                | ast::Expr::BinOp(_)
                | ast::Expr::UnaryOp(_)
                | ast::Expr::Lambda(_)
                | ast::Expr::If(_)
                | ast::Expr::Dict(_)
                | ast::Expr::Set(_)
                | ast::Expr::ListComp(_)
                | ast::Expr::SetComp(_)
                | ast::Expr::DictComp(_)
                | ast::Expr::Generator(_)
                | ast::Expr::Await(_)
                | ast::Expr::Yield(_)
                | ast::Expr::YieldFrom(_)
                | ast::Expr::Compare(_)
                | ast::Expr::Call(_)
                | ast::Expr::FString(_)
                | ast::Expr::TString(_)
                | ast::Expr::StringLiteral(_)
                | ast::Expr::BytesLiteral(_)
                | ast::Expr::NumberLiteral(_)
                | ast::Expr::BooleanLiteral(_)
                | ast::Expr::NoneLiteral(_)
                | ast::Expr::EllipsisLiteral(_)
                | ast::Expr::Attribute(_)
                | ast::Expr::Subscript(_)
                | ast::Expr::Name(_)
                | ast::Expr::List(_)
                | ast::Expr::Tuple(_)
                | ast::Expr::Slice(_)
                | ast::Expr::IpyEscapeCommand(_) => (element, false),
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
                } else if !result.push_constructed_element(value) {
                    return PythonValue::unknown(
                        PythonUnknownCause::UnsupportedExpression,
                        Some(origin),
                    );
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
                        if let PythonValueKind::Dict(dictionary) = &mut result.kind {
                            dictionary.extend_from_unpack(unpacked, item_origin);
                            result
                        } else {
                            PythonValue::unknown(
                                PythonUnknownCause::UnsupportedExpression,
                                Some(origin),
                            )
                        }
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
                    if let (PythonValueKind::Dict(dictionary), PythonValueKind::Dict(entry)) =
                        (&mut result.kind, entry.kind)
                    {
                        dictionary.append_entries_from(entry);
                        result
                    } else {
                        PythonValue::unknown(
                            PythonUnknownCause::UnsupportedExpression,
                            Some(origin),
                        )
                    }
                });
        }
        dictionaries
    }
}

fn is_unsupported_literal(expression: &ast::Expr) -> bool {
    match expression {
        ast::Expr::NoneLiteral(_)
        | ast::Expr::NumberLiteral(_)
        | ast::Expr::BytesLiteral(_)
        | ast::Expr::EllipsisLiteral(_)
        | ast::Expr::Set(_) => true,
        ast::Expr::UnaryOp(unary)
            if matches!(unary.op, ast::UnaryOp::UAdd | ast::UnaryOp::USub)
                && matches!(unary.operand.as_ref(), ast::Expr::NumberLiteral(_)) =>
        {
            true
        }
        ast::Expr::BoolOp(_)
        | ast::Expr::Named(_)
        | ast::Expr::BooleanLiteral(_)
        | ast::Expr::BinOp(_)
        | ast::Expr::UnaryOp(_)
        | ast::Expr::Lambda(_)
        | ast::Expr::If(_)
        | ast::Expr::Dict(_)
        | ast::Expr::SetComp(_)
        | ast::Expr::DictComp(_)
        | ast::Expr::Generator(_)
        | ast::Expr::Await(_)
        | ast::Expr::Yield(_)
        | ast::Expr::YieldFrom(_)
        | ast::Expr::Compare(_)
        | ast::Expr::Call(_)
        | ast::Expr::FString(_)
        | ast::Expr::TString(_)
        | ast::Expr::StringLiteral(_)
        | ast::Expr::Attribute(_)
        | ast::Expr::Subscript(_)
        | ast::Expr::Starred(_)
        | ast::Expr::Name(_)
        | ast::Expr::List(_)
        | ast::Expr::ListComp(_)
        | ast::Expr::Slice(_)
        | ast::Expr::IpyEscapeCommand(_)
        | ast::Expr::Tuple(_) => false,
    }
}

fn single_positional_argument(arguments: &ast::Arguments) -> Option<&ast::Expr> {
    if arguments.keywords.is_empty() && arguments.args.len() == 1 {
        arguments.args.first()
    } else {
        None
    }
}

fn project_bound_alternatives(
    binding: &PythonBinding,
    origin: Origin,
    project: impl Fn(&PythonValue) -> PythonBinding,
) -> PythonBinding {
    let mut result: Option<PythonBinding> = None;
    for (alternative, constraints) in binding.alternatives_with_constraints() {
        let projected = match alternative {
            PythonBindingState::Bound(bound) => project(&bound.value),
            PythonBindingState::Unbound => {
                PythonBinding::unknown(&PythonUnknownCause::UnsupportedExpression, origin)
            }
        };
        let Some(projected) = projected.intersect_constraints(constraints) else {
            continue;
        };
        result = Some(match result {
            Some(current) => current.join(projected, origin),
            None => projected,
        });
    }
    result.unwrap_or_else(|| {
        PythonBinding::unknown(&PythonUnknownCause::UnsupportedExpression, origin)
    })
}

fn path_from_value(value: &PythonValue, origin: Origin) -> PythonBinding {
    let path = match &value.kind {
        PythonValueKind::Path(PythonPath::Object(path)) => PythonPath::object(path.clone()),
        PythonValueKind::Str(text) => {
            let Some(path) = PythonPath::from_absolute_string(text) else {
                return PythonBinding::unknown(&PythonUnknownCause::UnsupportedExpression, origin);
            };
            path
        }
        PythonValueKind::Bool(_)
        | PythonValueKind::Path(PythonPath::Intrinsic(_))
        | PythonValueKind::UnsupportedLiteral
        | PythonValueKind::List(_)
        | PythonValueKind::Tuple(_)
        | PythonValueKind::Dict(_)
        | PythonValueKind::Module(_)
        | PythonValueKind::Unknown(_) => {
            return PythonBinding::unknown(&PythonUnknownCause::UnsupportedExpression, origin);
        }
    };
    PythonBinding::bound(PythonValue::python_path(path, origin), origin)
}

fn string_path_from_value(value: &PythonValue, origin: Origin) -> PythonBinding {
    let text = match &value.kind {
        PythonValueKind::Str(text) => text.clone(),
        PythonValueKind::Path(PythonPath::Object(path)) => path.to_string(),
        PythonValueKind::Bool(_)
        | PythonValueKind::Path(PythonPath::Intrinsic(_))
        | PythonValueKind::UnsupportedLiteral
        | PythonValueKind::List(_)
        | PythonValueKind::Tuple(_)
        | PythonValueKind::Dict(_)
        | PythonValueKind::Module(_)
        | PythonValueKind::Unknown(_) => {
            return PythonBinding::unknown(&PythonUnknownCause::UnsupportedExpression, origin);
        }
    };
    PythonBinding::bound(PythonValue::string(text, origin), origin)
}

fn rebase_exact_path(value: &PythonValue, origin: Origin) -> PythonBinding {
    let PythonValueKind::Path(PythonPath::Object(path)) = &value.kind else {
        return PythonBinding::unknown(&PythonUnknownCause::UnsupportedExpression, origin);
    };
    PythonBinding::bound(PythonValue::path(path.clone(), origin), origin)
}

fn resolve_path(value: &PythonValue, origin: Origin) -> PythonBinding {
    let PythonValueKind::Path(path) = &value.kind else {
        return PythonBinding::unknown(&PythonUnknownCause::UnsupportedExpression, origin);
    };
    let Some(resolved) = path.resolve() else {
        return PythonBinding::unknown(&PythonUnknownCause::UnsupportedExpression, origin);
    };
    PythonBinding::bound(PythonValue::python_path(resolved, origin), origin)
}

fn parent_string_path(value: &PythonValue, origin: Origin) -> PythonBinding {
    let PythonValueKind::Str(path) = &value.kind else {
        return PythonBinding::unknown(&PythonUnknownCause::UnsupportedExpression, origin);
    };
    PythonBinding::bound(
        PythonValue::string(os_path_dirname(path, cfg!(windows)), origin),
        origin,
    )
}

fn os_path_dirname(path: &str, windows: bool) -> String {
    let is_separator = |character| character == '/' || (windows && character == '\\');
    if windows {
        let trimmed = path.trim_end_matches(is_separator);
        if is_windows_path_root(trimmed, is_separator) {
            return path.to_string();
        }
    }
    let Some((separator_index, _)) = path
        .char_indices()
        .rev()
        .find(|(_, character)| is_separator(*character))
    else {
        return if windows && path.as_bytes().get(1) == Some(&b':') {
            path[..2].to_string()
        } else {
            String::new()
        };
    };
    let head = &path[..=separator_index];
    if head.chars().all(is_separator) {
        return head.to_string();
    }
    let trimmed = head.trim_end_matches(is_separator);
    if windows && is_windows_path_root(trimmed, is_separator) {
        head.to_string()
    } else {
        trimmed.to_string()
    }
}

fn is_windows_path_root(trimmed: &str, is_separator: impl Fn(char) -> bool) -> bool {
    let bytes = trimmed.as_bytes();
    if bytes.len() == 2 && bytes[1] == b':' {
        return true;
    }
    let leading_separators = trimmed
        .chars()
        .take_while(|character| is_separator(*character))
        .count();
    leading_separators >= 2
        && trimmed
            .split(is_separator)
            .filter(|component| !component.is_empty())
            .count()
            == 2
}

fn join_path_value(left: PythonValue, right: PythonValue, origin: Origin) -> PythonValue {
    let (PythonValueKind::Path(path), PythonValueKind::Str(segment)) = (left.kind, right.kind)
    else {
        return PythonValue::unknown(PythonUnknownCause::UnsupportedExpression, Some(origin));
    };
    path.join(&segment).map_or_else(
        || PythonValue::unknown(PythonUnknownCause::UnsupportedExpression, Some(origin)),
        |joined| PythonValue::python_path(joined, origin),
    )
}

fn join_string_path_value(left: PythonValue, right: PythonValue, origin: Origin) -> PythonValue {
    let PythonValueKind::Str(path) = left.kind else {
        return PythonValue::unknown(PythonUnknownCause::UnsupportedExpression, Some(origin));
    };
    let segment = match right.kind {
        PythonValueKind::Str(segment) => segment,
        PythonValueKind::Path(PythonPath::Object(segment)) => segment.to_string(),
        PythonValueKind::Bool(_)
        | PythonValueKind::Path(PythonPath::Intrinsic(_))
        | PythonValueKind::UnsupportedLiteral
        | PythonValueKind::List(_)
        | PythonValueKind::Tuple(_)
        | PythonValueKind::Dict(_)
        | PythonValueKind::Module(_)
        | PythonValueKind::Unknown(_) => {
            return PythonValue::unknown(PythonUnknownCause::UnsupportedExpression, Some(origin));
        }
    };
    let joined = if segment.starts_with('/') {
        segment
    } else if path.is_empty() || path.ends_with('/') {
        format!("{path}{segment}")
    } else {
        format!("{path}/{segment}")
    };
    PythonValue::string(joined, origin)
}

fn combine_bindings(
    left: &PythonBinding,
    right: &PythonBinding,
    origin: Origin,
    combine: impl Fn(PythonValue, PythonValue) -> PythonValue,
) -> PythonBinding {
    let mut result: Option<PythonBinding> = None;
    for (left, left_constraints) in left.alternatives_with_constraints() {
        let left = match left {
            PythonBindingState::Bound(left) => left.value.clone(),
            PythonBindingState::Unbound => {
                PythonValue::unknown(PythonUnknownCause::UnsupportedExpression, Some(origin))
            }
        };
        for (right, right_constraints) in right.alternatives_with_constraints() {
            let right = match right {
                PythonBindingState::Bound(right) => right.value.clone(),
                PythonBindingState::Unbound => {
                    PythonValue::unknown(PythonUnknownCause::UnsupportedExpression, Some(origin))
                }
            };
            let constraints = left_constraints.intersection(right_constraints);
            if constraints.is_impossible() {
                continue;
            }
            if let Some(alternative) =
                PythonBinding::constrained_bound(combine(left.clone(), right), origin, &constraints)
            {
                result = Some(match result {
                    Some(current) => current.join(alternative, origin),
                    None => alternative,
                });
            }
        }
    }
    result.unwrap_or_else(|| {
        PythonBinding::unknown(&PythonUnknownCause::UnsupportedExpression, origin)
    })
}

#[cfg(test)]
mod tests {
    use super::os_path_dirname;

    #[test]
    fn os_path_dirname_preserves_posix_string_semantics() {
        assert_eq!(os_path_dirname("", false), "");
        assert_eq!(os_path_dirname("name", false), "");
        assert_eq!(os_path_dirname("relative/name", false), "relative");
        assert_eq!(os_path_dirname("/project/templates", false), "/project");
        assert_eq!(os_path_dirname("/project/", false), "/project");
        assert_eq!(os_path_dirname("/", false), "/");
        assert_eq!(os_path_dirname("///name", false), "///");
    }

    #[test]
    fn os_path_dirname_preserves_windows_drive_and_unc_roots() {
        assert_eq!(os_path_dirname("C:\\", true), "C:\\");
        assert_eq!(os_path_dirname("C:\\project\\file", true), "C:\\project");
        assert_eq!(
            os_path_dirname("\\\\server\\share\\file", true),
            "\\\\server\\share\\"
        );
    }
}
