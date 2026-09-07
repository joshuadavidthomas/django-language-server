//! Bounded syntax census: candidates are registration sites, not proven registrations.

use std::collections::BTreeSet;

use anyhow::Context as _;
use djls_project::TemplateSymbolKind;
use ruff_python_ast::Expr;
use ruff_python_ast::ExprAttribute;
use ruff_python_ast::ExprCall;
use ruff_python_ast::Stmt;
use ruff_python_ast::visitor;
use ruff_python_ast::visitor::Visitor;
use ruff_text_size::Ranged;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActualCandidate {
    pub kind: TemplateSymbolKind,
    pub name_status: NameStatus,
    pub name: String,
    pub helper: RegistrationHelper,
    pub form: RegistrationForm,
    pub span: SourceSpan,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum NameStatus {
    Known,
    Unresolved,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegistrationHelper {
    Tag,
    Filter,
    SimpleTag,
    InclusionTag,
    SimpleBlockTag,
}

impl RegistrationHelper {
    fn from_name(name: &str) -> Option<Self> {
        match name {
            "tag" => Some(Self::Tag),
            "filter" => Some(Self::Filter),
            "simple_tag" => Some(Self::SimpleTag),
            "inclusion_tag" => Some(Self::InclusionTag),
            "simple_block_tag" => Some(Self::SimpleBlockTag),
            _ => None,
        }
    }

    fn kind(self) -> TemplateSymbolKind {
        match self {
            Self::Tag | Self::SimpleTag | Self::InclusionTag | Self::SimpleBlockTag => {
                TemplateSymbolKind::Tag
            }
            Self::Filter => TemplateSymbolKind::Filter,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegistrationForm {
    Decorator,
    Direct,
    Curried,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceSpan {
    pub start: u32,
    pub end: u32,
}

pub fn census_source(source: &str) -> anyhow::Result<Vec<ActualCandidate>> {
    let parsed = ruff_python_parser::parse_module(source).context("invalid Python source")?;
    let module = parsed.into_syntax();
    let mut census = CensusVisitor {
        source,
        candidates: Vec::new(),
        seen: BTreeSet::new(),
        suppressed_factories: BTreeSet::new(),
    };
    census.visit_body(&module.body);
    Ok(census.finish())
}

struct CensusVisitor<'a> {
    source: &'a str,
    candidates: Vec<ActualCandidate>,
    seen: BTreeSet<(u32, u32)>,
    suppressed_factories: BTreeSet<(u32, u32)>,
}

impl CensusVisitor<'_> {
    fn finish(mut self) -> Vec<ActualCandidate> {
        self.candidates
            .sort_by_key(|candidate| candidate.span.start);
        self.candidates
    }

    fn inspect_decorator(&mut self, expression: &Expr, function_name: &str) {
        if let Expr::Attribute(attribute) = expression {
            self.record_attribute(
                attribute,
                RegistrationForm::Decorator,
                Some(function_name),
                true,
                expression,
                None,
            );
        } else if let Expr::Call(call) = expression {
            self.inspect_call(call, RegistrationForm::Decorator, Some(function_name), true);
        }
    }

    fn inspect_call(
        &mut self,
        call: &ExprCall,
        form: RegistrationForm,
        applied_name: Option<&str>,
        applied_name_proven: bool,
    ) {
        let range = range_key(call);
        if self.seen.contains(&range) || self.suppressed_factories.contains(&range) {
            return;
        }

        if let Expr::Attribute(attribute) = call.func.as_ref() {
            self.record_attribute(
                attribute,
                form,
                applied_name,
                applied_name_proven,
                call,
                Some(call),
            );
            return;
        }

        let Expr::Call(factory) = call.func.as_ref() else {
            return;
        };
        let Expr::Attribute(attribute) = factory.func.as_ref() else {
            return;
        };
        if RegistrationHelper::from_name(attribute.attr.as_str()).is_none() {
            return;
        }
        self.suppressed_factories.insert(range_key(factory));
        let callable_name = call
            .arguments
            .args
            .first()
            .and_then(callable_name)
            .or(applied_name);
        let callable_name_proven =
            callable_name.is_some_and(|name| applied_name == Some(name) && applied_name_proven);
        self.record_attribute(
            attribute,
            RegistrationForm::Curried,
            callable_name,
            callable_name_proven,
            call,
            Some(factory),
        );
    }

    fn record_attribute(
        &mut self,
        attribute: &ExprAttribute,
        form: RegistrationForm,
        applied_name: Option<&str>,
        applied_name_proven: bool,
        evidence: &impl Ranged,
        arguments: Option<&ExprCall>,
    ) {
        let Some(helper) = RegistrationHelper::from_name(attribute.attr.as_str()) else {
            return;
        };
        let evidence_range = range_key(evidence);
        if !self.seen.insert(evidence_range) {
            return;
        }
        let Expr::Name(receiver_name) = attribute.value.as_ref() else {
            return;
        };
        if receiver_name.id.as_str() != "register" {
            return;
        }
        // This bare `register` receiver is only spelling evidence. The census deliberately does
        // not resolve the binding.
        let (name_status, name) = registration_name(
            self.source,
            helper,
            form,
            arguments,
            applied_name,
            applied_name_proven,
        );
        self.candidates.push(ActualCandidate {
            kind: helper.kind(),
            helper,
            form,
            name_status,
            name,
            span: SourceSpan {
                start: evidence.range().start().to_u32(),
                end: evidence.range().end().to_u32(),
            },
        });
    }
}

impl<'a> Visitor<'a> for CensusVisitor<'a> {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        if let Stmt::FunctionDef(function) = stmt {
            for decorator in &function.decorator_list {
                self.inspect_decorator(&decorator.expression, function.name.as_str());
            }
        }
        visitor::walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        if let Expr::Call(call) = expr {
            self.inspect_call(call, RegistrationForm::Direct, None, false);
        }
        visitor::walk_expr(self, expr);
    }
}

fn registration_name(
    source: &str,
    helper: RegistrationHelper,
    form: RegistrationForm,
    call: Option<&ExprCall>,
    applied_name: Option<&str>,
    applied_name_proven: bool,
) -> (NameStatus, String) {
    if let Some(call) = call {
        if let Some(name) = call.arguments.keywords.iter().find_map(|keyword| {
            keyword
                .arg
                .as_ref()
                .filter(|arg| arg.as_str() == "name")
                .map(|_| &keyword.value)
        }) {
            return expression_name(source, name);
        }

        if matches!(helper, RegistrationHelper::Tag | RegistrationHelper::Filter) {
            let first = call.arguments.args.first();
            let positional_is_explicit_name = call.arguments.args.len() >= 2
                || matches!(
                    form,
                    RegistrationForm::Decorator | RegistrationForm::Curried
                )
                || first.is_some_and(|expression| string_literal(expression).is_some());
            if positional_is_explicit_name && let Some(name) = first {
                return expression_name(source, name);
            }
        }

        let callable = match helper {
            RegistrationHelper::Tag | RegistrationHelper::Filter => {
                let keyword_name = if helper == RegistrationHelper::Tag {
                    "compile_function"
                } else {
                    "filter_func"
                };
                call.arguments
                    .keywords
                    .iter()
                    .find_map(|keyword| {
                        keyword
                            .arg
                            .as_ref()
                            .filter(|arg| arg.as_str() == keyword_name)
                            .map(|_| &keyword.value)
                    })
                    .or_else(|| call.arguments.args.first())
            }
            RegistrationHelper::SimpleTag | RegistrationHelper::SimpleBlockTag => {
                call.arguments.args.first()
            }
            RegistrationHelper::InclusionTag => call.arguments.args.get(1),
        };
        if let Some(expression) = callable {
            return (
                NameStatus::Unresolved,
                format!("callable-name:{}", expression_text(source, expression)),
            );
        }
    }

    if let Some(name) = applied_name {
        if applied_name_proven {
            return (NameStatus::Known, name.to_string());
        }
        return (NameStatus::Unresolved, format!("callable-name:{name}"));
    }
    (NameStatus::Unresolved, "<implicit-name>".to_string())
}

fn expression_name(source: &str, expression: &Expr) -> (NameStatus, String) {
    if let Some(value) = string_literal(expression) {
        (NameStatus::Known, value.to_string())
    } else {
        (NameStatus::Unresolved, expression_text(source, expression))
    }
}

fn string_literal(expression: &Expr) -> Option<&str> {
    let Expr::StringLiteral(literal) = expression else {
        return None;
    };
    Some(literal.value.to_str())
}

fn callable_name(expression: &Expr) -> Option<&str> {
    let Expr::Name(name) = expression else {
        return None;
    };
    Some(name.id.as_str())
}

fn expression_text(source: &str, expression: &impl Ranged) -> String {
    let range = expression.range();
    source
        .get(range.start().to_usize()..range.end().to_usize())
        .unwrap_or("<unavailable-expression>")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn range_key(value: &impl Ranged) -> (u32, u32) {
    (value.range().start().to_u32(), value.range().end().to_u32())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn census_covers_direct_decorator_and_curried_forms() {
        let source = r#"
@register.tag
def alpha(parser, token): pass

@register.filter(name="renamed")
def beta(value): pass

register.simple_tag(gamma)
register.inclusion_tag("card.html")(delta)
register.simple_block_tag(name="panel")(epsilon)
"#;
        let candidates = census_source(source).expect("test source should parse");
        assert_eq!(candidates.len(), 5);
        assert_eq!(candidates[0].name, "alpha");
        assert_eq!(candidates[1].name, "renamed");
        assert_eq!(candidates[2].form, RegistrationForm::Direct);
        assert_eq!(candidates[3].form, RegistrationForm::Curried);
        assert_eq!(candidates[4].helper, RegistrationHelper::SimpleBlockTag);
    }

    #[test]
    fn census_reports_unknown_names_and_does_not_treat_other_receivers_as_register() {
        let candidates = census_source(
            "register.tag(name=dynamic)(factory())\nother.tag(name='not-a-register')\n",
        )
        .expect("test source should parse");
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].name_status, NameStatus::Unresolved);
        assert_eq!(candidates[0].name, "dynamic");
    }
}
