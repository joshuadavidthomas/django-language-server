use ruff_python_ast as ast;
use rustc_hash::FxHashSet;

use crate::ast::ExprExt;

pub(super) fn target_write_names(target: &ast::Expr) -> Vec<&str> {
    let mut names = Vec::new();
    collect_target_write_names(target, &mut names);
    names
}

fn collect_target_write_names<'a>(target: &'a ast::Expr, names: &mut Vec<&'a str>) {
    if let Some(name) = target.name_target() {
        names.push(name);
        return;
    }

    match target {
        ast::Expr::Attribute(attribute) => collect_target_write_names(&attribute.value, names),
        ast::Expr::Subscript(subscript) => collect_target_write_names(&subscript.value, names),
        ast::Expr::Tuple(tuple) => {
            for expression in &tuple.elts {
                collect_target_write_names(expression, names);
            }
        }
        ast::Expr::List(list) => {
            for expression in &list.elts {
                collect_target_write_names(expression, names);
            }
        }
        ast::Expr::Starred(starred) => collect_target_write_names(&starred.value, names),
        ast::Expr::If(_)
        | ast::Expr::Named(_)
        | ast::Expr::BinOp(_)
        | ast::Expr::UnaryOp(_)
        | ast::Expr::Lambda(_)
        | ast::Expr::BoolOp(_)
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
        | ast::Expr::ListComp(_)
        | ast::Expr::Set(_)
        | ast::Expr::SetComp(_)
        | ast::Expr::Dict(_)
        | ast::Expr::DictComp(_)
        | ast::Expr::Generator(_)
        | ast::Expr::Slice(_)
        | ast::Expr::IpyEscapeCommand(_)
        | ast::Expr::Name(_) => {}
    }
}

/// Collects names whose current bindings are read while evaluating an expression.
pub(super) fn expr_read_names(expression: &ast::Expr) -> FxHashSet<String> {
    ReadNameCollector::collect(expression, false).names
}

pub(super) fn expr_calls(expression: &ast::Expr) -> Vec<ast::ExprCall> {
    ReadNameCollector::collect(expression, true)
        .calls
        .unwrap_or_default()
}

#[derive(Default)]
struct ReadNameCollector {
    names: FxHashSet<String>,
    calls: Option<Vec<ast::ExprCall>>,
}

impl ReadNameCollector {
    fn collect(expression: &ast::Expr, collect_calls: bool) -> Self {
        let mut collector = Self {
            calls: collect_calls.then(Vec::new),
            ..Self::default()
        };
        collector.visit_expr(expression);
        collector
    }

    fn visit_expr(&mut self, expression: &ast::Expr) {
        if let Some(name) = expression.name_target() {
            self.names.insert(name.to_string());
        }

        match expression {
            ast::Expr::Attribute(attribute) => self.visit_expr(&attribute.value),
            ast::Expr::Subscript(subscript) => {
                self.visit_expr(&subscript.value);
                self.visit_expr(&subscript.slice);
            }
            ast::Expr::Call(call) => {
                if let Some(calls) = &mut self.calls {
                    calls.push(call.clone());
                }
                self.visit_expr(&call.func);
                self.visit_elements(&call.arguments.args);
                for keyword in &call.arguments.keywords {
                    self.visit_expr(&keyword.value);
                }
            }
            ast::Expr::BinOp(binary) => {
                self.visit_expr(&binary.left);
                self.visit_expr(&binary.right);
            }
            ast::Expr::UnaryOp(unary) => self.visit_expr(&unary.operand),
            ast::Expr::BoolOp(boolean) => self.visit_elements(&boolean.values),
            ast::Expr::Compare(compare) => {
                self.visit_expr(&compare.left);
                self.visit_elements(&compare.comparators);
            }
            ast::Expr::Tuple(tuple) => self.visit_elements(&tuple.elts),
            ast::Expr::List(list) => self.visit_elements(&list.elts),
            ast::Expr::Set(set) => self.visit_elements(&set.elts),
            ast::Expr::Dict(dictionary) => self.visit_dict(dictionary),
            ast::Expr::Starred(starred) => self.visit_expr(&starred.value),
            ast::Expr::If(if_expression) => {
                self.visit_expr(&if_expression.test);
                self.visit_expr(&if_expression.body);
                self.visit_expr(&if_expression.orelse);
            }
            ast::Expr::Lambda(lambda) => {
                if let Some(parameters) = &lambda.parameters {
                    self.visit_parameters(parameters);
                }
                self.visit_expr(&lambda.body);
            }
            ast::Expr::ListComp(comprehension) => {
                self.visit_comprehensions(&comprehension.generators);
                self.visit_expr(&comprehension.elt);
            }
            ast::Expr::SetComp(comprehension) => {
                self.visit_comprehensions(&comprehension.generators);
                self.visit_expr(&comprehension.elt);
            }
            ast::Expr::DictComp(comprehension) => {
                self.visit_comprehensions(&comprehension.generators);
                self.visit_expr(&comprehension.key);
                self.visit_expr(&comprehension.value);
            }
            ast::Expr::Generator(generator) => {
                self.visit_comprehensions(&generator.generators);
                self.visit_expr(&generator.elt);
            }
            ast::Expr::Await(await_expression) => self.visit_expr(&await_expression.value),
            ast::Expr::Yield(yield_expression) => {
                if let Some(value) = &yield_expression.value {
                    self.visit_expr(value);
                }
            }
            ast::Expr::YieldFrom(yield_from) => self.visit_expr(&yield_from.value),
            ast::Expr::Named(named) => self.visit_expr(&named.value),
            ast::Expr::Slice(slice) => self.visit_slice(slice),
            ast::Expr::FString(f_string) => {
                for part in &f_string.value {
                    if let ast::FStringPart::FString(f_string) = part {
                        self.visit_interpolated_string(&f_string.elements);
                    }
                }
            }
            ast::Expr::TString(t_string) => {
                for t_string in &t_string.value {
                    self.visit_interpolated_string(&t_string.elements);
                }
            }
            ast::Expr::Name(_)
            | ast::Expr::StringLiteral(_)
            | ast::Expr::BytesLiteral(_)
            | ast::Expr::NumberLiteral(_)
            | ast::Expr::BooleanLiteral(_)
            | ast::Expr::NoneLiteral(_)
            | ast::Expr::EllipsisLiteral(_)
            | ast::Expr::IpyEscapeCommand(_) => {}
        }
    }

    fn visit_parameters(&mut self, parameters: &ast::Parameters) {
        for parameter in parameters.iter_non_variadic_params() {
            if let Some(default) = &parameter.default {
                self.visit_expr(default);
            }
        }
        for parameter in parameters {
            if let Some(annotation) = &parameter.as_parameter().annotation {
                self.visit_expr(annotation);
            }
        }
    }

    fn visit_comprehensions(&mut self, generators: &[ast::Comprehension]) {
        for generator in generators {
            self.visit_expr(&generator.iter);
            for condition in &generator.ifs {
                self.visit_expr(condition);
            }
        }
    }

    fn visit_interpolated_string(&mut self, elements: &ast::InterpolatedStringElements) {
        for element in elements {
            let ast::InterpolatedStringElement::Interpolation(interpolation) = element else {
                continue;
            };
            self.visit_expr(&interpolation.expression);
            if let Some(format_spec) = &interpolation.format_spec {
                self.visit_interpolated_string(&format_spec.elements);
            }
        }
    }

    fn visit_elements(&mut self, elements: &[ast::Expr]) {
        for expression in elements {
            self.visit_expr(expression);
        }
    }

    fn visit_dict(&mut self, dictionary: &ast::ExprDict) {
        for item in &dictionary.items {
            if let Some(key) = &item.key {
                self.visit_expr(key);
            }
            self.visit_expr(&item.value);
        }
    }

    fn visit_slice(&mut self, slice: &ast::ExprSlice) {
        if let Some(lower) = &slice.lower {
            self.visit_expr(lower);
        }
        if let Some(upper) = &slice.upper {
            self.visit_expr(upper);
        }
        if let Some(step) = &slice.step {
            self.visit_expr(step);
        }
    }
}

pub(super) fn pattern_bound_names(pattern: &ast::Pattern) -> Vec<&str> {
    let mut names = Vec::new();
    collect_pattern_bound_names(pattern, &mut names);
    names
}

fn collect_pattern_bound_names<'a>(pattern: &'a ast::Pattern, names: &mut Vec<&'a str>) {
    match pattern {
        ast::Pattern::MatchValue(_) | ast::Pattern::MatchSingleton(_) => {}
        ast::Pattern::MatchSequence(sequence) => {
            for pattern in &sequence.patterns {
                collect_pattern_bound_names(pattern, names);
            }
        }
        ast::Pattern::MatchMapping(mapping) => {
            for pattern in &mapping.patterns {
                collect_pattern_bound_names(pattern, names);
            }
            if let Some(rest) = &mapping.rest {
                names.push(rest.as_str());
            }
        }
        ast::Pattern::MatchClass(class) => {
            for pattern in &class.arguments.patterns {
                collect_pattern_bound_names(pattern, names);
            }
            for keyword in &class.arguments.keywords {
                collect_pattern_bound_names(&keyword.pattern, names);
            }
        }
        ast::Pattern::MatchStar(star) => {
            if let Some(name) = &star.name {
                names.push(name.as_str());
            }
        }
        ast::Pattern::MatchAs(match_as) => {
            if let Some(pattern) = &match_as.pattern {
                collect_pattern_bound_names(pattern, names);
            }
            if let Some(name) = &match_as.name {
                names.push(name.as_str());
            }
        }
        ast::Pattern::MatchOr(match_or) => {
            for pattern in &match_or.patterns {
                collect_pattern_bound_names(pattern, names);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use ruff_python_ast as ast;
    use ruff_python_parser::parse_module;

    use super::expr_read_names;
    use super::pattern_bound_names;
    use super::target_write_names;

    fn read_names(expression: &str) -> BTreeSet<String> {
        let source = format!("VALUE = {expression}\n");
        let module = parse_module(&source)
            .expect("expression should parse")
            .into_syntax();
        let assignment = match module.body.as_slice() {
            [ast::Stmt::Assign(assignment)] => Some(assignment),
            _ => None,
        }
        .expect("expected one assignment");
        expr_read_names(&assignment.value).into_iter().collect()
    }

    #[test]
    fn expression_reads_include_every_comprehension_input() {
        for (expression, expected) in [
            (
                "[result for target in iterable if condition]",
                &["condition", "iterable", "result"][..],
            ),
            (
                "{result for target in iterable if first if second}",
                &["first", "iterable", "result", "second"],
            ),
            (
                "{key: value for target in iterable if condition}",
                &["condition", "iterable", "key", "value"],
            ),
            (
                "(result for target in first_iterable if first_condition for nested in second_iterable if second_condition if third_condition)",
                &[
                    "first_condition",
                    "first_iterable",
                    "result",
                    "second_condition",
                    "second_iterable",
                    "third_condition",
                ],
            ),
        ] {
            assert_eq!(
                read_names(expression),
                expected.iter().map(|name| (*name).to_string()).collect(),
                "{expression}"
            );
        }
    }

    #[test]
    fn named_expression_reads_value_but_not_target() {
        assert_eq!(
            read_names("(target := source)"),
            ["source"].into_iter().map(str::to_string).collect()
        );
    }

    #[test]
    fn expression_reads_include_nested_formatted_string_interpolations() {
        assert_eq!(
            read_names("f'{value:{width}.{precision}}' f'{other}'"),
            ["other", "precision", "value", "width"]
                .into_iter()
                .map(str::to_string)
                .collect()
        );
        assert_eq!(
            read_names("t'{value:{width}}' t'{other}'"),
            ["other", "value", "width"]
                .into_iter()
                .map(str::to_string)
                .collect()
        );
    }

    #[test]
    fn expression_reads_include_lambda_defaults_and_body() {
        assert_eq!(
            read_names("lambda parameter=default: parameter + result"),
            ["default", "parameter", "result"]
                .into_iter()
                .map(str::to_string)
                .collect()
        );
    }

    #[test]
    fn expression_reads_include_call_attribute_and_subscript_inputs() {
        assert_eq!(
            read_names("receiver.method(argument, keyword=value)[index:start:stop]"),
            ["argument", "index", "receiver", "start", "stop", "value",]
                .into_iter()
                .map(str::to_string)
                .collect()
        );
    }

    #[test]
    fn target_writes_follow_attributes_and_subscripts_but_definitions_remain_explicit() {
        let module = parse_module("root.attr[index], [name, *rest] = value\n")
            .expect("assignment should parse")
            .into_syntax();
        let assignment = match module.body.as_slice() {
            [ast::Stmt::Assign(assignment)] => Some(assignment),
            _ => None,
        }
        .expect("expected one assignment");
        assert_eq!(
            target_write_names(&assignment.targets[0]),
            ["root", "name", "rest"]
        );
    }

    #[test]
    fn pattern_bindings_include_mapping_rest_star_and_as_names() {
        let module = parse_module(
            "match subject:\n    case {\"key\": [first, *rest], **mapping} as whole:\n        pass\n",
        )
        .expect("match should parse")
        .into_syntax();
        let statement = match module.body.as_slice() {
            [ast::Stmt::Match(statement)] => Some(statement),
            _ => None,
        }
        .expect("expected one match statement");
        assert_eq!(
            pattern_bound_names(&statement.cases[0].pattern),
            ["first", "rest", "mapping", "whole"]
        );
    }
}
