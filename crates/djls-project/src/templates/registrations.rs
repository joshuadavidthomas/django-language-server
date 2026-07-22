use std::collections::BTreeMap;
use std::ops::ControlFlow;

use ruff_python_ast::Expr;
use ruff_python_ast::ExprAttribute;
use ruff_python_ast::ExprCall;
use ruff_python_ast::Keyword;
use ruff_python_ast::Stmt;
use ruff_python_ast::StmtAssign;
use ruff_python_ast::StmtExpr;
use ruff_python_ast::StmtFunctionDef;

use super::filters::FilterArityMap;
use super::libraries::TemplateLibraryId;
use super::names::TemplateSymbolName;
use super::symbols::SymbolDefinition;
use super::symbols::SymbolKey;
use super::symbols::TemplateSymbol;
use super::symbols::TemplateSymbolKind;
use super::tags::BlockSpec;
use super::tags::BlockSpecs;
use super::tags::TagRuleMap;
use super::tags::blocks::EndTagEvidence;
use crate::ast::ExprExt;
use crate::ast::Recurse;
use crate::ast::walk_stmts;
use crate::db::Db as ProjectDb;
use crate::python::RecoveredPythonModule;

/// Decorator helper names on `django.template.Library` that register filters.
const FILTER_DECORATORS: &[&str] = &["filter"];

/// Information about a single tag or filter registration found in source code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RegistrationInfo {
    name: String,
    kind: RegistrationKind,
    func_name: Option<String>,
}

/// The style of registration, distinguishing decorator helpers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RegistrationKind {
    Tag,
    SimpleTag,
    InclusionTag,
    SimpleBlockTag,
    Filter,
}

/// Collect registrations from a pre-parsed module body.
///
/// This avoids re-parsing the source when the caller already has the AST.
#[must_use]
pub(crate) fn collect_registrations_from_body(body: &[Stmt]) -> Vec<RegistrationInfo> {
    let mut registrations = Vec::new();
    walk_stmts(body, Recurse::IntoClasses, |stmt| {
        match stmt {
            Stmt::FunctionDef(func_def) => {
                collect_from_decorated_function(func_def, &mut registrations);
            }
            Stmt::Expr(StmtExpr { value, .. }) => {
                if let Expr::Call(call) = value.as_ref() {
                    collect_from_call_statement(call, &mut registrations);
                }
            }
            Stmt::ClassDef(_)
            | Stmt::Return(_)
            | Stmt::Delete(_)
            | Stmt::TypeAlias(_)
            | Stmt::Assign(_)
            | Stmt::AugAssign(_)
            | Stmt::AnnAssign(_)
            | Stmt::For(_)
            | Stmt::While(_)
            | Stmt::If(_)
            | Stmt::With(_)
            | Stmt::Match(_)
            | Stmt::Raise(_)
            | Stmt::Try(_)
            | Stmt::Assert(_)
            | Stmt::Import(_)
            | Stmt::ImportFrom(_)
            | Stmt::Global(_)
            | Stmt::Nonlocal(_)
            | Stmt::Pass(_)
            | Stmt::Break(_)
            | Stmt::Continue(_)
            | Stmt::IpyEscapeCommand(_) => {}
        }
        ControlFlow::Continue(())
    });
    registrations
}

fn for_each_registration(
    body: &[Stmt],
    module_name: &str,
    mut f: impl FnMut(&RegistrationInfo, Option<&StmtFunctionDef>, SymbolKey),
) {
    let registrations = collect_registrations_from_body(body);
    let func_defs = collect_func_defs(body);

    for reg in &registrations {
        let func = reg.func_name.as_deref().and_then(|name| {
            func_defs
                .iter()
                .find(|func| func.name.as_str() == name)
                .copied()
        });

        let kind = reg.kind;
        let key = SymbolKey {
            registration_module: module_name.to_string(),
            name: reg.name.clone(),
            kind: kind.symbol_kind(),
        };

        f(reg, func, key);
    }
}

/// Recursively collect all function definitions from a module body.
fn collect_func_defs(body: &[Stmt]) -> Vec<&StmtFunctionDef> {
    let mut defs = Vec::new();
    walk_stmts(body, Recurse::IntoClasses, |stmt| {
        if let Stmt::FunctionDef(func) = stmt {
            defs.push(func);
        }
        ControlFlow::Continue(())
    });
    defs
}

/// Extract registrations from a decorated function definition.
///
/// Handles patterns like:
/// - `@register.tag` (bare decorator)
/// - `@register.simple_tag(name="alias")`
/// - `@register.tag("name")`
/// - `@register.filter`
fn collect_from_decorated_function(
    func_def: &StmtFunctionDef,
    registrations: &mut Vec<RegistrationInfo>,
) {
    let func_name = func_def.name.as_str();

    for decorator in &func_def.decorator_list {
        // Try tag decorator
        if let Some((name, kind)) = tag_name_from_decorator(&decorator.expression, func_name) {
            registrations.push(RegistrationInfo {
                name,
                kind,
                func_name: Some(func_name.to_string()),
            });
            continue;
        }

        // Try filter decorator
        if let Some(name) = filter_name_from_decorator(&decorator.expression, func_name) {
            registrations.push(RegistrationInfo {
                name,
                kind: RegistrationKind::Filter,
                func_name: Some(func_name.to_string()),
            });
        }
    }
}

/// Extract a tag name from a decorator expression.
///
/// Returns `Some((name, kind))` if the decorator is a tag registration.
fn tag_name_from_decorator(expr: &Expr, func_name: &str) -> Option<(String, RegistrationKind)> {
    // Bare decorator: `@register.tag`
    if let Expr::Attribute(ExprAttribute { attr, .. }) = expr
        && let Some(kind) = tag_decorator_kind(attr.as_str())
    {
        return Some((func_name.to_string(), kind));
    }

    // Call decorator: `@register.tag(...)` or `@register.simple_tag(name="alias")`
    if let Expr::Call(ExprCall {
        func, arguments, ..
    }) = expr
        && let Expr::Attribute(ExprAttribute { attr, .. }) = func.as_ref()
        && let Some(kind) = tag_decorator_kind(attr.as_str())
    {
        // Priority: name= kwarg > first positional string (for @register.tag only) > func_name
        let name_override = kw_name_from(&arguments.keywords);

        let positional_name = if attr.as_str() == "tag" {
            first_string_arg(&arguments.args)
        } else {
            None
        };

        let name = name_override
            .or(positional_name)
            .unwrap_or_else(|| func_name.to_string());

        return Some((name, kind));
    }

    None
}

/// Extract a filter name from a decorator expression.
///
/// Returns `Some(name)` if the decorator is a filter registration.
fn filter_name_from_decorator(expr: &Expr, func_name: &str) -> Option<String> {
    // Bare decorator: `@register.filter`
    if let Expr::Attribute(ExprAttribute { attr, .. }) = expr
        && FILTER_DECORATORS.contains(&attr.as_str())
    {
        return Some(func_name.to_string());
    }

    // Call decorator: `@register.filter(name="alias")` or `@register.filter("alias")`
    if let Expr::Call(ExprCall {
        func, arguments, ..
    }) = expr
        && let Expr::Attribute(ExprAttribute { attr, .. }) = func.as_ref()
        && FILTER_DECORATORS.contains(&attr.as_str())
    {
        let name_override = kw_name_from(&arguments.keywords);
        let positional_name = first_string_arg(&arguments.args);
        let name = name_override
            .or(positional_name)
            .unwrap_or_else(|| func_name.to_string());
        return Some(name);
    }

    None
}

/// Extract registrations from a call expression statement.
///
/// Handles patterns like:
/// - `register.tag("name", compile_func)`
/// - `register.tag("name", SomeNode.handle)`
/// - `register.filter("name", filter_func)`
/// - `register.simple_tag(func, name="alias")`
fn collect_from_call_statement(call: &ExprCall, registrations: &mut Vec<RegistrationInfo>) {
    // Try tag call-style registration
    if let Some((name, kind, func_name)) = tag_registration_from_call(call) {
        registrations.push(RegistrationInfo {
            name,
            kind,
            func_name,
        });
        return;
    }

    // Try filter call-style registration
    if let Some((name, func_name)) = filter_registration_from_call(call) {
        registrations.push(RegistrationInfo {
            name,
            kind: RegistrationKind::Filter,
            func_name,
        });
    }
}

/// Extract tag registration info from a call expression.
///
/// Returns `Some((name, kind, func_name))` for patterns like:
/// - `register.tag("name", func)`
/// - `register.simple_tag(func, name="alias")`
fn tag_registration_from_call(
    call: &ExprCall,
) -> Option<(String, RegistrationKind, Option<String>)> {
    let Expr::Attribute(ExprAttribute { attr, .. }) = call.func.as_ref() else {
        return None;
    };
    let kind = tag_decorator_kind(attr.as_str())?;

    let name_override = kw_name_from(&call.arguments.keywords);
    let func_name = kw_callable_name(&call.arguments.keywords, &["compile_function", "func"]);

    let args = &call.arguments.args;

    if args.len() >= 2 {
        // `register.tag("name", func)` — first arg is string name, second is callable
        if let Some(name) = args[0].string_literal() {
            let fn_name = callable_name(&args[1]).or(func_name);
            return Some((
                name_override.unwrap_or_else(|| name.to_string()),
                kind,
                fn_name,
            ));
        }
    }

    if args.len() == 1 {
        // `register.simple_tag(func, name="alias")` or `register.tag(func)`
        let fn_name = callable_name(&args[0]).or(func_name.clone());
        if let Some(name) = name_override {
            return Some((name, kind, fn_name));
        }
        // Fallback: use the callable name as the registration name.
        // Handles both simple names (`do_for`) and attribute callables (`ForNode.handle`).
        if let Some(name) = callable_name(&args[0]) {
            return Some((name.clone(), kind, Some(name)));
        }
    }

    // No positional args but has name= kwarg
    if let Some(name) = name_override {
        return Some((name, kind, func_name));
    }

    None
}

/// Extract filter registration info from a call expression.
///
/// Returns `Some((name, func_name))` for patterns like:
/// - `register.filter("name", func)`
/// - `register.filter(func, name="alias")`
fn filter_registration_from_call(call: &ExprCall) -> Option<(String, Option<String>)> {
    let Expr::Attribute(ExprAttribute { attr, .. }) = call.func.as_ref() else {
        return None;
    };
    if !FILTER_DECORATORS.contains(&attr.as_str()) {
        return None;
    }

    let name_override = kw_name_from(&call.arguments.keywords);
    let func_name = kw_callable_name(&call.arguments.keywords, &["filter_func", "func"]);

    let args = &call.arguments.args;

    if args.len() >= 2 {
        // `register.filter("name", func)`
        if let Some(name) = args[0].string_literal() {
            let fn_name = callable_name(&args[1]).or(func_name);
            return Some((name_override.unwrap_or_else(|| name.to_string()), fn_name));
        }
    }

    if args.len() == 1 {
        let fn_name = callable_name(&args[0]).or(func_name.clone());
        if let Some(name) = name_override {
            return Some((name, fn_name));
        }
        // Fallback: use the callable name as the registration name.
        // Handles both simple names (`my_func`) and attribute callables (`MyClass.method`).
        if let Some(name) = callable_name(&args[0]) {
            return Some((name.clone(), Some(name)));
        }
    }

    if let Some(name) = name_override {
        return Some((name, func_name));
    }

    None
}

/// Map decorator attr name to `RegistrationKind`.
fn tag_decorator_kind(attr: &str) -> Option<RegistrationKind> {
    match attr {
        "tag" => Some(RegistrationKind::Tag),
        "simple_tag" => Some(RegistrationKind::SimpleTag),
        "inclusion_tag" => Some(RegistrationKind::InclusionTag),
        "simple_block_tag" => Some(RegistrationKind::SimpleBlockTag),
        _ => None,
    }
}

/// Extract the `name=` keyword argument value as a string.
fn kw_name_from(keywords: &[Keyword]) -> Option<String> {
    kw_constant_str(keywords, "name")
}

/// Extract a keyword argument's string constant value by argument name.
fn kw_constant_str(keywords: &[Keyword], name: &str) -> Option<String> {
    for kw in keywords {
        let Some(arg) = &kw.arg else { continue };
        if arg.as_str() != name {
            continue;
        }
        if let Some(s) = kw.value.string_literal() {
            return Some(s.to_string());
        }
    }
    None
}

/// Extract a callable name from keyword arguments by checking the given keyword names.
fn kw_callable_name(keywords: &[Keyword], kwarg_names: &[&str]) -> Option<String> {
    for kw in keywords {
        let Some(arg) = &kw.arg else { continue };
        if kwarg_names.contains(&arg.as_str())
            && let Some(name) = kw.value.name_target()
        {
            return Some(name.to_string());
        }
    }
    None
}

/// Extract the first positional argument's string value.
fn first_string_arg(args: &[Expr]) -> Option<String> {
    args.first()
        .and_then(ExprExt::string_literal)
        .map(str::to_string)
}

/// Best-effort callable name extraction for debugging / registration mapping.
fn callable_name(expr: &Expr) -> Option<String> {
    if let Some(name) = expr.name_target() {
        return Some(name.to_string());
    }

    match expr {
        Expr::Attribute(ExprAttribute { value, attr, .. }) => {
            let base = callable_name(value)?;
            Some(format!("{base}.{}", attr.as_str()))
        }
        Expr::BoolOp(_)
        | Expr::Named(_)
        | Expr::BinOp(_)
        | Expr::UnaryOp(_)
        | Expr::Lambda(_)
        | Expr::If(_)
        | Expr::Dict(_)
        | Expr::Set(_)
        | Expr::ListComp(_)
        | Expr::SetComp(_)
        | Expr::DictComp(_)
        | Expr::Generator(_)
        | Expr::Await(_)
        | Expr::Yield(_)
        | Expr::YieldFrom(_)
        | Expr::Compare(_)
        | Expr::Call(_)
        | Expr::FString(_)
        | Expr::TString(_)
        | Expr::StringLiteral(_)
        | Expr::BytesLiteral(_)
        | Expr::NumberLiteral(_)
        | Expr::BooleanLiteral(_)
        | Expr::NoneLiteral(_)
        | Expr::EllipsisLiteral(_)
        | Expr::Subscript(_)
        | Expr::Starred(_)
        | Expr::Name(_)
        | Expr::List(_)
        | Expr::Tuple(_)
        | Expr::Slice(_)
        | Expr::IpyEscapeCommand(_) => None,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum TemplateLibraryDefinitionState {
    Failed,
    ParsedNotLibrary {
        parse_quality: TemplateLibraryParseQuality,
    },
    Library {
        parse_quality: TemplateLibraryParseQuality,
    },
}

/// Equality-bearing registration facts for one Template Library source module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateLibraryDefinitionFacts {
    state: TemplateLibraryDefinitionState,
    tags: BTreeMap<String, TemplateSymbol>,
    filters: BTreeMap<String, TemplateSymbol>,
}

impl TemplateLibraryDefinitionFacts {
    #[must_use]
    pub fn is_library(&self) -> bool {
        matches!(self.state, TemplateLibraryDefinitionState::Library { .. })
    }

    #[must_use]
    pub(crate) fn is_recovered(&self) -> bool {
        matches!(
            self.state,
            TemplateLibraryDefinitionState::ParsedNotLibrary {
                parse_quality: TemplateLibraryParseQuality::Recovered,
            } | TemplateLibraryDefinitionState::Library {
                parse_quality: TemplateLibraryParseQuality::Recovered,
            }
        )
    }

    #[must_use]
    pub(crate) fn source_failed(&self) -> bool {
        matches!(self.state, TemplateLibraryDefinitionState::Failed)
    }

    pub(crate) fn symbols(&self) -> impl Iterator<Item = &TemplateSymbol> {
        self.tags.values().chain(self.filters.values())
    }

    #[must_use]
    pub fn symbol(&self, kind: TemplateSymbolKind, name: &str) -> Option<&TemplateSymbol> {
        match kind {
            TemplateSymbolKind::Tag => self.tags.get(name),
            TemplateSymbolKind::Filter => self.filters.get(name),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TemplateLibraryParseQuality {
    Exact,
    Recovered,
}

/// Canonical indexed analysis of one Template Library source module.
///
/// Registration discovery happens here once. Equality-bearing projections below keep changes in
/// Tag Definitions, Filter Definitions, Tag Rules, Block Specs, and Filter Arity independent.
#[derive(Clone, Debug, PartialEq)]
struct TemplateLibrarySourceAnalysis {
    definitions: TemplateLibraryDefinitionFacts,
    tag_rules: TagRuleMap,
    block_specs: BlockSpecs,
    filter_arities: FilterArityMap,
}

impl TemplateLibrarySourceAnalysis {
    fn failed() -> Self {
        Self {
            definitions: TemplateLibraryDefinitionFacts {
                state: TemplateLibraryDefinitionState::Failed,
                tags: BTreeMap::new(),
                filters: BTreeMap::new(),
            },
            tag_rules: TagRuleMap::default(),
            block_specs: BlockSpecs::default(),
            filter_arities: FilterArityMap::default(),
        }
    }
}

#[salsa::tracked(returns(ref))]
fn template_library_source_analysis(
    db: &dyn ProjectDb,
    key: TemplateLibraryId,
) -> TemplateLibrarySourceAnalysis {
    let Some(file) = key.file(db) else {
        return TemplateLibrarySourceAnalysis::failed();
    };
    let Ok(Some(module)) = RecoveredPythonModule::from_file(db, file) else {
        return TemplateLibrarySourceAnalysis::failed();
    };
    let parse_quality = if module.has_ordinary_syntax_errors(db) {
        TemplateLibraryParseQuality::Recovered
    } else {
        TemplateLibraryParseQuality::Exact
    };

    let mut tags = BTreeMap::new();
    let mut filters = BTreeMap::new();
    let mut tag_rules = TagRuleMap::default();
    let mut block_specs = BlockSpecs::default();
    let mut filter_arities = FilterArityMap::default();
    let registration_module = key.module(db).as_str();

    for_each_registration(
        module.body(db),
        registration_module,
        |registration, func, symbol_key| {
            if let Ok(name) = TemplateSymbolName::parse(&registration.name) {
                let kind = registration.kind.symbol_kind();
                let symbol = TemplateSymbol {
                    kind,
                    name,
                    definition: SymbolDefinition::Exact {
                        file: file.path(db).to_path_buf(),
                    },
                    doc: None,
                };
                match kind {
                    TemplateSymbolKind::Tag => {
                        tags.insert(symbol.name().to_string(), symbol);
                    }
                    TemplateSymbolKind::Filter => {
                        filters.insert(symbol.name().to_string(), symbol);
                    }
                }
            }

            let Some(func) = func else {
                return;
            };
            if let Some(rule) = registration.kind.extract_tag_rule(func) {
                tag_rules.insert(symbol_key.clone(), rule.into());
            }
            if let Some(block_spec) = registration.kind.extract_block_spec(func) {
                let end_tag = match block_spec.end_tag {
                    EndTagEvidence::Literal(end_tag) => Some(end_tag),
                    EndTagEvidence::SelfNamed => Some(format!("end{}", symbol_key.name)),
                    EndTagEvidence::Unknown => None,
                };
                block_specs.insert(
                    symbol_key.clone(),
                    BlockSpec {
                        end_tag,
                        intermediates: block_spec.intermediates,
                        opaque: block_spec.opaque,
                    },
                );
            }
            if let Some(arity) = registration.kind.extract_filter_arity(func) {
                filter_arities.insert(symbol_key, arity);
            }
        },
    );

    let defines_library = module
        .body(db)
        .iter()
        .any(TemplateLibraryDefinitionFacts::stmt_defines_library);
    let state = if defines_library || !tags.is_empty() || !filters.is_empty() {
        TemplateLibraryDefinitionState::Library { parse_quality }
    } else {
        TemplateLibraryDefinitionState::ParsedNotLibrary { parse_quality }
    };
    TemplateLibrarySourceAnalysis {
        definitions: TemplateLibraryDefinitionFacts {
            state,
            tags,
            filters,
        },
        tag_rules,
        block_specs,
        filter_arities,
    }
}

/// Independently backdatable Tag analysis for one Template Library.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TemplateLibraryTagFacts {
    tag_rules: TagRuleMap,
    block_specs: BlockSpecs,
}

impl TemplateLibraryTagFacts {
    #[must_use]
    pub fn tag_rules(&self) -> &TagRuleMap {
        &self.tag_rules
    }

    #[must_use]
    pub fn block_specs(&self) -> &BlockSpecs {
        &self.block_specs
    }
}

/// Independently backdatable Filter analysis for one Template Library.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TemplateLibraryFilterFacts {
    filter_arities: FilterArityMap,
}

impl TemplateLibraryFilterFacts {
    #[must_use]
    pub fn filter_arities(&self) -> &FilterArityMap {
        &self.filter_arities
    }
}

#[salsa::tracked(returns(ref))]
pub fn template_library_definition_facts(
    db: &dyn ProjectDb,
    key: TemplateLibraryId,
) -> TemplateLibraryDefinitionFacts {
    template_library_source_analysis(db, key)
        .definitions
        .clone()
}

#[salsa::tracked(returns(ref))]
pub fn template_library_tag_facts(
    db: &dyn ProjectDb,
    key: TemplateLibraryId,
) -> TemplateLibraryTagFacts {
    let analysis = template_library_source_analysis(db, key);
    TemplateLibraryTagFacts {
        tag_rules: analysis.tag_rules.clone(),
        block_specs: analysis.block_specs.clone(),
    }
}

#[salsa::tracked(returns(ref))]
pub fn template_library_filter_facts(
    db: &dyn ProjectDb,
    key: TemplateLibraryId,
) -> TemplateLibraryFilterFacts {
    TemplateLibraryFilterFacts {
        filter_arities: template_library_source_analysis(db, key)
            .filter_arities
            .clone(),
    }
}

impl TemplateLibraryDefinitionFacts {
    fn stmt_defines_library(stmt: &Stmt) -> bool {
        let Stmt::Assign(StmtAssign { targets, value, .. }) = stmt else {
            return false;
        };
        if !targets
            .iter()
            .any(|target| target.name_target() == Some("register"))
        {
            return false;
        }

        let Expr::Call(ExprCall { func, .. }) = value.as_ref() else {
            return false;
        };
        match func.as_ref() {
            Expr::Attribute(ExprAttribute { value, attr, .. }) => {
                attr.as_str() == "Library" && value.name_target() == Some("template")
            }
            expr @ (Expr::BoolOp(_)
            | Expr::Named(_)
            | Expr::BinOp(_)
            | Expr::UnaryOp(_)
            | Expr::Lambda(_)
            | Expr::If(_)
            | Expr::Dict(_)
            | Expr::Set(_)
            | Expr::ListComp(_)
            | Expr::SetComp(_)
            | Expr::DictComp(_)
            | Expr::Generator(_)
            | Expr::Await(_)
            | Expr::Yield(_)
            | Expr::YieldFrom(_)
            | Expr::Compare(_)
            | Expr::Call(_)
            | Expr::FString(_)
            | Expr::TString(_)
            | Expr::StringLiteral(_)
            | Expr::BytesLiteral(_)
            | Expr::NumberLiteral(_)
            | Expr::BooleanLiteral(_)
            | Expr::NoneLiteral(_)
            | Expr::EllipsisLiteral(_)
            | Expr::Subscript(_)
            | Expr::Starred(_)
            | Expr::Name(_)
            | Expr::List(_)
            | Expr::Tuple(_)
            | Expr::Slice(_)
            | Expr::IpyEscapeCommand(_)) => expr.name_target() == Some("Library"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::templates::tags::testing::fixture_source;

    fn fixture(path: &str) -> &'static str {
        fixture_source(path).expect("requested corpus fixture should exist")
    }

    fn collect_registrations(source: &str) -> Vec<RegistrationInfo> {
        let parsed = ruff_python_parser::parse_module(source).expect("valid Python");
        let module = parsed.into_syntax();
        collect_registrations_from_body(&module.body)
    }

    fn find_reg<'a>(regs: &'a [RegistrationInfo], name: &str) -> &'a RegistrationInfo {
        regs.iter()
            .find(|r| r.name == name)
            .expect("requested registration should have been collected")
    }

    // Corpus: `autoescape` in django/template/defaulttags.py uses `@register.tag` (bare)
    #[test]
    fn decorator_bare_tag() {
        let source = fixture("django/template/defaulttags.py");
        let regs = collect_registrations(source);
        let reg = find_reg(&regs, "autoescape");
        assert_eq!(reg.kind, RegistrationKind::Tag);
        assert_eq!(reg.func_name.as_deref(), Some("autoescape"));
    }

    // Corpus: `querystring` in django/template/defaulttags.py uses
    // `@register.simple_tag(name="querystring", takes_context=True)`
    #[test]
    fn decorator_simple_tag_with_name_kwarg() {
        let source = fixture("django/template/defaulttags.py");
        let regs = collect_registrations(source);
        let reg = find_reg(&regs, "querystring");
        assert_eq!(reg.kind, RegistrationKind::SimpleTag);
        assert_eq!(reg.func_name.as_deref(), Some("querystring"));
    }

    // Corpus: `inclusion_no_params` in tests/template_tests/templatetags/inclusion.py uses
    // `@register.inclusion_tag("inclusion.html")`
    #[test]
    fn decorator_inclusion_tag() {
        let source = fixture("tests/template_tests/templatetags/inclusion.py");
        let regs = collect_registrations(source);
        let reg = find_reg(&regs, "inclusion_no_params");
        assert_eq!(reg.kind, RegistrationKind::InclusionTag);
    }

    // Corpus: `cut` in django/template/defaultfilters.py uses `@register.filter` (bare)
    #[test]
    fn decorator_filter_bare() {
        let source = fixture("django/template/defaultfilters.py");
        let regs = collect_registrations(source);
        let reg = find_reg(&regs, "cut");
        assert_eq!(reg.kind, RegistrationKind::Filter);
    }

    // Corpus: `escapejs` in django/template/defaultfilters.py uses
    // `@register.filter("escapejs")` — positional string name, func is `escapejs_filter`
    #[test]
    fn decorator_filter_with_positional_string_name() {
        let source = fixture("django/template/defaultfilters.py");
        let regs = collect_registrations(source);
        let reg = find_reg(&regs, "escapejs");
        assert_eq!(reg.kind, RegistrationKind::Filter);
        assert_eq!(reg.func_name.as_deref(), Some("escapejs_filter"));
    }

    // Corpus: `other_echo` in tests/template_tests/templatetags/testtags.py uses
    // `register.tag("other_echo", echo)` — call-style registration
    #[test]
    fn call_style_tag_registration() {
        let source = fixture("tests/template_tests/templatetags/testtags.py");
        let regs = collect_registrations(source);
        let reg = find_reg(&regs, "other_echo");
        assert_eq!(reg.kind, RegistrationKind::Tag);
        assert_eq!(reg.func_name.as_deref(), Some("echo"));
    }

    // Corpus: `intcomma` in wagtail/admin/templatetags/wagtailadmin_tags.py uses
    // `register.filter("intcomma", intcomma)` — call-style filter registration
    #[test]
    fn call_style_filter_registration() {
        let source = fixture("wagtail/admin/templatetags/wagtailadmin_tags.py");
        let regs = collect_registrations(source);
        let reg = find_reg(&regs, "intcomma");
        assert_eq!(reg.kind, RegistrationKind::Filter);
        assert_eq!(reg.func_name.as_deref(), Some("intcomma"));
    }

    // Corpus: `for` in django/template/defaulttags.py uses `@register.tag("for")`
    // — positional string name overrides function name `do_for`
    #[test]
    fn tag_with_positional_string_name() {
        let source = fixture("django/template/defaulttags.py");
        let regs = collect_registrations(source);
        let reg = find_reg(&regs, "for");
        assert_eq!(reg.kind, RegistrationKind::Tag);
        assert_eq!(reg.func_name.as_deref(), Some("do_for"));
    }

    // Corpus: `addslashes` in django/template/defaultfilters.py uses
    // `@register.filter(is_safe=True)` — name defaults to function name
    #[test]
    fn filter_with_is_safe_kwarg() {
        let source = fixture("django/template/defaultfilters.py");
        let regs = collect_registrations(source);
        let reg = find_reg(&regs, "addslashes");
        assert_eq!(reg.kind, RegistrationKind::Filter);
        assert_eq!(reg.func_name.as_deref(), Some("addslashes"));
    }

    // Corpus: `partialdef` in django/template/defaulttags.py uses
    // `@register.tag(name="partialdef")` — name kwarg overrides func name `partialdef_func`
    #[test]
    fn tag_with_name_kwarg() {
        let source = fixture("django/template/defaulttags.py");
        let regs = collect_registrations(source);
        let reg = find_reg(&regs, "partialdef");
        assert_eq!(reg.kind, RegistrationKind::Tag);
        assert_eq!(reg.func_name.as_deref(), Some("partialdef_func"));
    }

    // Corpus: `dialog` in wagtail/admin/templatetags/wagtailadmin_tags.py uses
    // `register.tag("dialog", DialogNode.handle)` — call-style with method callable
    #[test]
    fn call_style_tag_with_method_callable() {
        let source = fixture("wagtail/admin/templatetags/wagtailadmin_tags.py");
        let regs = collect_registrations(source);
        let reg = find_reg(&regs, "dialog");
        assert_eq!(reg.kind, RegistrationKind::Tag);
        assert_eq!(reg.func_name.as_deref(), Some("DialogNode.handle"));
    }

    // Corpus: `div` in tests/template_tests/templatetags/custom.py uses
    // `@register.simple_block_tag` (bare decorator)
    #[test]
    fn simple_block_tag_decorator() {
        let source = fixture("tests/template_tests/templatetags/custom.py");
        let regs = collect_registrations(source);
        let reg = find_reg(&regs, "div");
        assert_eq!(reg.kind, RegistrationKind::SimpleBlockTag);
    }

    // Corpus: defaulttags.py has many registrations (tags + simple_tags)
    #[test]
    fn multiple_registrations() {
        let source = fixture("django/template/defaulttags.py");
        let regs = collect_registrations(source);
        assert!(
            regs.len() > 10,
            "expected many registrations in defaulttags.py, got {}",
            regs.len()
        );
        let tags: Vec<_> = regs
            .iter()
            .filter(|r| r.kind == RegistrationKind::Tag)
            .collect();
        assert!(
            tags.len() > 5,
            "expected multiple Tag registrations, got {}",
            tags.len()
        );
        assert!(regs.iter().any(|r| r.name == "for"));
        assert!(regs.iter().any(|r| r.name == "if"));
        assert!(regs.iter().any(|r| r.name == "autoescape"));
    }

    // Corpus: testtags.py has decorator @register.tag + call-style register.tag
    // Tests that both decorator and call-style registrations are discovered
    #[test]
    fn mixed_decorator_and_call_style() {
        let source = fixture("tests/template_tests/templatetags/testtags.py");
        let regs = collect_registrations(source);
        let tag_regs: Vec<_> = regs
            .iter()
            .filter(|r| r.kind == RegistrationKind::Tag)
            .collect();
        assert_eq!(tag_regs.len(), 2);
        assert!(tag_regs.iter().any(|r| r.name == "echo"));
        assert!(tag_regs.iter().any(|r| r.name == "other_echo"));
    }

    // Edge case: @register.tag() with empty parens — function name used as tag name.
    // Corpus: no clean isolatable example of empty parens (all corpus uses bare or with args).
    #[test]
    fn function_name_fallback() {
        let source = r"
from django import template
register = template.Library()

@register.tag()
def current_time(parser, token):
    pass
";
        let regs = collect_registrations(source);
        assert_eq!(regs.len(), 1);
        assert_eq!(regs[0].name, "current_time");
        assert_eq!(regs[0].kind, RegistrationKind::Tag);
    }

    // Edge case: register.simple_tag(my_func, name="alias") — call-style with func positional
    // and name kwarg. Rare pattern, not found cleanly in corpus.
    #[test]
    fn simple_tag_func_positional() {
        let source = r#"
from django import template
register = template.Library()

register.simple_tag(my_func, name="alias")
"#;
        let regs = collect_registrations(source);
        assert_eq!(regs.len(), 1);
        assert_eq!(regs[0].name, "alias");
        assert_eq!(regs[0].kind, RegistrationKind::SimpleTag);
        assert_eq!(regs[0].func_name.as_deref(), Some("my_func"));
    }

    #[test]
    fn empty_source() {
        let regs = collect_registrations("");
        assert!(regs.is_empty());
    }

    // Edge case: source with no registration patterns
    #[test]
    fn no_registrations() {
        let source = r"
def regular_function():
    pass

class MyClass:
    pass
";
        let regs = collect_registrations(source);
        assert!(regs.is_empty());
    }

    // Edge case: register.tag(do_something) — single func arg, no name string.
    // Valid Django API but rare. Not found cleanly in corpus.
    #[test]
    fn call_style_single_func_no_name() {
        let source = r"
from django import template
register = template.Library()

register.tag(do_something)
";
        let regs = collect_registrations(source);
        assert_eq!(regs.len(), 1);
        assert_eq!(regs[0].name, "do_something");
        assert_eq!(regs[0].kind, RegistrationKind::Tag);
    }

    // Edge case: register.filter(my_filter_func) — single func arg, no name string.
    // Valid Django API but rare. Not found cleanly in corpus.
    #[test]
    fn call_style_filter_single_func_no_name() {
        let source = r"
from django import template
register = template.Library()

register.filter(my_filter_func)
";
        let regs = collect_registrations(source);
        assert_eq!(regs.len(), 1);
        assert_eq!(regs[0].name, "my_filter_func");
        assert_eq!(regs[0].kind, RegistrationKind::Filter);
    }

    // Edge case: name kwarg overrides positional string arg.
    // Tests priority: name= kwarg wins over positional string.
    #[test]
    fn name_kwarg_overrides_positional_for_tag() {
        let source = r#"
from django import template
register = template.Library()

@register.tag("positional_name", name="kwarg_name")
def my_tag(parser, token):
    pass
"#;
        let regs = collect_registrations(source);
        assert_eq!(regs.len(), 1);
        assert_eq!(regs[0].name, "kwarg_name");
    }
}
