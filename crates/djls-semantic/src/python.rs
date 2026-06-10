mod analysis;
mod blocks;
mod ext;
mod filters;
mod models;
mod registry;
mod signature;
mod types;

#[cfg(test)]
mod testing;

use djls_source::File;
use djls_source::FileKind;
use ruff_python_ast::Stmt;
use ruff_python_ast::StmtFunctionDef;
use ruff_python_ast::statement_visitor::StatementVisitor;
use ruff_python_ast::statement_visitor::walk_stmt;

use crate::python::analysis::CallContext;
use crate::python::analysis::calls::AbstractValueKey;
use crate::python::analysis::calls::extract_return_value;
use crate::python::analysis::state::AbstractValue;
use crate::python::analysis::state::Env;
use crate::python::analysis::statements::process_statements;
pub use crate::python::models::compute_model_graph;
pub use crate::python::models::extract::extract_model_graph;
pub use crate::python::models::graph::ModelGraph;
pub use crate::python::models::graph::ModulePath;
use crate::python::registry::ExtractionOutput;
pub(crate) use crate::python::registry::RegistrationInfo;
pub(crate) use crate::python::registry::RegistrationKind;
pub(crate) use crate::python::registry::collect_registrations_from_body;
pub(crate) use crate::python::types::ArgumentCountConstraint;
#[cfg(test)]
pub(crate) use crate::python::types::AsVar;
pub(crate) use crate::python::types::BlockSpec;
pub use crate::python::types::BlockSpecs;
pub(crate) use crate::python::types::ChoiceAt;
pub(crate) use crate::python::types::ExtractedArg;
pub(crate) use crate::python::types::ExtractedArgKind;
pub(crate) use crate::python::types::ExtractedDiagnosticConstraint;
pub(crate) use crate::python::types::ExtractedDiagnosticMessage;
pub(crate) use crate::python::types::ExtractedMessageArg;
pub(crate) use crate::python::types::ExtractedMessageTemplate;
pub use crate::python::types::ExtractionResult;
pub use crate::python::types::FilterArity;
pub use crate::python::types::FilterArityMap;
pub(crate) use crate::python::types::KnownOptions;
pub(crate) use crate::python::types::RequiredKeyword;
pub(crate) use crate::python::types::SplitPosition;
pub use crate::python::types::SymbolKey;
pub use crate::python::types::SymbolKind;
pub use crate::python::types::TagRule;
pub use crate::python::types::TagRuleMap;

/// Parsed Python module AST, cached by Salsa.
///
/// Wraps Ruff's statement list in a tracked struct. The parsed AST is
/// invalidated when the source file changes.
#[salsa::tracked]
pub(crate) struct ParsedPythonModule<'db> {
    #[tracked]
    #[returns(ref)]
    pub body: Vec<Stmt>,
}

/// Interned key for a helper function call.
///
/// Salsa uses interning to deduplicate identical helper calls: same file,
/// same callee name, same abstract argument values produce the same
/// `HelperCall` identity, enabling Salsa's built-in memoization.
#[salsa::interned]
pub(crate) struct HelperCall<'db> {
    pub file: File,
    #[returns(ref)]
    pub callee_name: String,
    #[returns(ref)]
    pub args: Vec<AbstractValueKey>,
}

/// Parse a Python source file into a cached AST.
///
/// Returns `None` for non-Python files or files that fail to parse.
/// The parsed AST is cached by Salsa and invalidated when
/// `file.source(db)` changes.
#[salsa::tracked]
pub(crate) fn parse_python_module(
    db: &dyn djls_source::Db,
    file: File,
) -> Option<ParsedPythonModule<'_>> {
    let source = file.source(db);
    if *source.kind() != FileKind::Python {
        return None;
    }

    let parsed = ruff_python_parser::parse_module(source.as_ref());
    let module = match parsed {
        Ok(parsed) => parsed.into_syntax(),
        Err(_) => return None,
    };

    Some(ParsedPythonModule::new(db, module.body))
}

/// Analyze a helper function call and return its abstract return value.
///
/// This is a Salsa tracked function with cycle recovery: if A calls B
/// which calls A (directly or transitively), the cycle resolves to
/// `AbstractValue::Unknown` instead of panicking.
///
/// Looks up the callee by name in the parsed module's AST, binds
/// parameters to the abstract argument values from `HelperCall`, runs
/// the analyzer on the callee body, and extracts the return
/// value.
#[salsa::tracked(
    cycle_initial=analyze_helper_cycle_initial,
    cycle_fn=analyze_helper_cycle_recover,
)]
pub(crate) fn analyze_helper(db: &dyn djls_source::Db, call: HelperCall<'_>) -> AbstractValue {
    let Some(parsed) = parse_python_module(db, call.file(db)) else {
        return AbstractValue::Unknown;
    };

    let callee_name = call.callee_name(db);
    let args = call.args(db);

    let Some(callee) = find_function_def(parsed.body(db), callee_name) else {
        return AbstractValue::Unknown;
    };

    let mut callee_env = Env::default();
    for (i, param) in callee.parameters.args.iter().enumerate() {
        let value = args
            .get(i)
            .map_or(AbstractValue::Unknown, AbstractValue::from);
        callee_env.set(param.parameter.name.to_string(), value);
    }

    let mut ctx = CallContext {
        db: Some(db),
        file: Some(call.file(db)),
    };

    let _result = process_statements(&callee.body, &mut callee_env, &mut ctx);

    extract_return_value(&callee.body, &mut callee_env)
}

fn analyze_helper_cycle_initial(
    _db: &dyn djls_source::Db,
    _id: salsa::Id,
    _call: HelperCall<'_>,
) -> AbstractValue {
    AbstractValue::Unknown
}

fn analyze_helper_cycle_recover(
    _db: &dyn djls_source::Db,
    _cycle: &salsa::Cycle,
    _last_provisional: &AbstractValue,
    _value: AbstractValue,
    _call: HelperCall<'_>,
) -> AbstractValue {
    AbstractValue::Unknown
}

fn find_function_def<'a>(body: &'a [Stmt], name: &str) -> Option<&'a StmtFunctionDef> {
    for stmt in body {
        match stmt {
            Stmt::FunctionDef(func) if func.name.as_str() == name => return Some(func),
            Stmt::ClassDef(class) => {
                if let Some(found) = find_function_def(&class.body, name) {
                    return Some(found);
                }
            }
            _ => {}
        }
    }
    None
}

/// Extract tag validation rules from a Python file, cached by Salsa.
///
/// This domain-specific query lets tag argument validation depend only on tag
/// rule extraction. Filter-only changes can backdate here and avoid invalidating
/// tag specs.
#[salsa::tracked(returns(ref))]
pub fn extract_tag_rules(
    db: &dyn djls_source::Db,
    file: File,
    registration_module: ModulePath,
) -> TagRuleMap {
    let Some(parsed) = parse_python_module(db, file) else {
        return TagRuleMap::default();
    };

    extract_tag_rules_from_body(parsed.body(db), registration_module)
}

fn extract_tag_rules_from_body(body: &[Stmt], registration_module: ModulePath) -> TagRuleMap {
    let registration_module = registration_module.into_string();
    let registrations = collect_registrations_from_body(body);
    let func_defs = collect_func_defs(body);
    let mut tag_rules = TagRuleMap::default();

    for (reg, func, key) in registered_functions(&registrations, &func_defs, &registration_module) {
        if let Some(rule) = reg.kind.extract_tag_rule(func) {
            tag_rules.insert(key, rule.into());
        }
    }

    tag_rules
}

/// Extract filter arities from a Python file, cached by Salsa.
///
/// This domain-specific query lets filter argument validation depend only on
/// filter signature extraction.
#[salsa::tracked(returns(ref))]
pub fn extract_filter_arities(
    db: &dyn djls_source::Db,
    file: File,
    registration_module: ModulePath,
) -> FilterArityMap {
    let Some(parsed) = parse_python_module(db, file) else {
        return FilterArityMap::default();
    };

    extract_filter_arities_from_body(parsed.body(db), registration_module)
}

fn extract_filter_arities_from_body(
    body: &[Stmt],
    registration_module: ModulePath,
) -> FilterArityMap {
    let registration_module = registration_module.into_string();
    let registrations = collect_registrations_from_body(body);
    let func_defs = collect_func_defs(body);
    let mut filter_arities = FilterArityMap::default();

    for (reg, func, key) in registered_functions(&registrations, &func_defs, &registration_module) {
        if let Some(arity) = reg.kind.extract_filter_arity(func) {
            filter_arities.insert(key, arity);
        }
    }

    filter_arities
}

/// Extract block specs from a Python file, cached by Salsa.
///
/// This domain-specific query lets structural tag validation depend on block
/// extraction without also depending on filter arities.
#[salsa::tracked(returns(ref))]
pub fn extract_block_specs(
    db: &dyn djls_source::Db,
    file: File,
    registration_module: ModulePath,
) -> BlockSpecs {
    let Some(parsed) = parse_python_module(db, file) else {
        return BlockSpecs::default();
    };

    extract_block_specs_from_body(parsed.body(db), registration_module)
}

fn extract_block_specs_from_body(body: &[Stmt], registration_module: ModulePath) -> BlockSpecs {
    let registration_module = registration_module.into_string();
    let registrations = collect_registrations_from_body(body);
    let func_defs = collect_func_defs(body);
    let mut block_specs = BlockSpecs::default();

    for (reg, func, key) in registered_functions(&registrations, &func_defs, &registration_module) {
        if let Some(block_spec) = normalize_block_spec(reg.kind.extract_block_spec(func), &key.name)
        {
            block_specs.insert(key, block_spec);
        }
    }

    block_specs
}

/// Extract validation rules from a Python registration module source.
///
/// Parses the source with Ruff's Python parser, walks the AST to find
/// `@register.tag` / `@register.filter` decorators, and extracts validation
/// semantics (argument counts, block structure, option constraints) from the
/// associated compile functions.
///
/// The `module_path` parameter is the dotted Python module path (e.g.,
/// `"django.template.defaulttags"`) used as the `registration_module` field
/// in `SymbolKey`s. Pass an empty string if unknown.
///
/// Returns an `ExtractionResult` mapping each discovered `SymbolKey` to its
/// extracted rules.
#[must_use]
pub fn extract_rules(source: &str, module_path: &str) -> ExtractionResult {
    let Ok(parsed) = ruff_python_parser::parse_module(source) else {
        return ExtractionResult::default();
    };
    let module = parsed.into_syntax();
    let registrations = collect_registrations_from_body(&module.body);
    let func_defs = collect_func_defs(&module.body);

    let mut result = ExtractionResult::default();

    for (reg, func, key) in registered_functions(&registrations, &func_defs, module_path) {
        match reg.kind.extract(func) {
            ExtractionOutput::Filter(arity) => {
                result.filter_arities.insert(key, arity);
            }
            ExtractionOutput::Tag { rule, block_spec } => {
                if let Some(rule) = rule {
                    result.tag_rules.insert(key.clone(), rule.into());
                }
                if let Some(block_spec) = normalize_block_spec(block_spec, &key.name) {
                    result.block_specs.insert(key, block_spec);
                }
            }
        }
    }

    result
}

fn registered_functions<'a>(
    registrations: &'a [RegistrationInfo],
    func_defs: &'a [&'a ruff_python_ast::StmtFunctionDef],
    module_path: &'a str,
) -> impl Iterator<
    Item = (
        &'a RegistrationInfo,
        &'a ruff_python_ast::StmtFunctionDef,
        SymbolKey,
    ),
> + 'a {
    registrations.iter().filter_map(move |reg| {
        let func = reg
            .func_name
            .as_deref()
            .and_then(|name| func_defs.iter().find(|f| f.name.as_str() == name).copied())?;
        let kind: RegistrationKind = reg.kind;
        let key = SymbolKey {
            registration_module: module_path.to_string(),
            name: reg.name.clone(),
            kind: kind.symbol_kind(),
        };
        Some((reg, func, key))
    })
}

fn normalize_block_spec(block_spec: Option<BlockSpec>, tag_name: &str) -> Option<BlockSpec> {
    block_spec.map(|mut block_spec| {
        if block_spec.end_tag.is_none() {
            block_spec.end_tag = Some(format!("end{tag_name}"));
        }
        block_spec
    })
}

/// Recursively collect all function definitions from a module body.
fn collect_func_defs(body: &[Stmt]) -> Vec<&StmtFunctionDef> {
    let mut visitor = FunctionDefCollector::default();
    visitor.visit_body(body);
    visitor.defs
}

#[derive(Default)]
struct FunctionDefCollector<'a> {
    defs: Vec<&'a StmtFunctionDef>,
}

impl<'a> StatementVisitor<'a> for FunctionDefCollector<'a> {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        match stmt {
            Stmt::FunctionDef(func) => {
                self.defs.push(func);
            }
            Stmt::ClassDef(_) => {
                walk_stmt(self, stmt);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use ruff_python_parser::parse_module;
    use serde::Serialize;

    use super::*;
    use crate::python::testing::django_source;

    /// A deterministically-ordered version of `ExtractionResult` for snapshot testing.
    ///
    /// `FxHashMap` iteration order is non-deterministic, so we convert to `BTreeMap`
    /// (sorted by `SymbolKey` string representation) before serializing.
    #[derive(Debug, Serialize)]
    struct SortedExtractionResult {
        tag_rules: BTreeMap<String, TagRule>,
        filter_arities: BTreeMap<String, FilterArity>,
        block_specs: BTreeMap<String, BlockSpec>,
    }

    impl From<ExtractionResult> for SortedExtractionResult {
        fn from(result: ExtractionResult) -> Self {
            let key_str = |k: &SymbolKey| {
                let kind = match k.kind {
                    SymbolKind::Tag => "tag",
                    SymbolKind::Filter => "filter",
                };
                format!("{}::{kind}::{}", k.registration_module, k.name)
            };
            Self {
                tag_rules: result
                    .tag_rules
                    .iter()
                    .map(|(k, v)| (key_str(k), v.as_ref().clone()))
                    .collect(),
                filter_arities: result
                    .filter_arities
                    .iter()
                    .map(|(k, v)| (key_str(k), v.clone()))
                    .collect(),
                block_specs: result
                    .block_specs
                    .as_map()
                    .iter()
                    .map(|(k, v)| (key_str(k), v.clone()))
                    .collect(),
            }
        }
    }

    fn snapshot(result: ExtractionResult) -> SortedExtractionResult {
        result.into()
    }

    #[test]
    fn registry_collection_is_reachable_from_python_module() {
        let registrations: Vec<RegistrationInfo> = collect_registrations_from_body(&[]);
        assert!(registrations.is_empty());
        let _ = RegistrationKind::Tag;
    }

    // (d) Pure Rust — tests parser infrastructure works
    #[test]
    fn smoke_test_ruff_parser() {
        let source = r#"
from django import template

register = template.Library()

@register.simple_tag
def hello():
    return "Hello, world!"
"#;

        let result = parse_module(source);
        assert!(result.is_ok());

        let parsed = result.unwrap();
        let module = parsed.into_syntax();
        assert!(!module.body.is_empty());
    }

    // Corpus: `no_params` in tests/template_tests/templatetags/custom.py —
    // `@register.simple_tag` with no user args, exercises simple_tag pipeline
    #[test]
    fn extract_rules_simple_tag() {
        let source = django_source("tests/template_tests/templatetags/custom.py").unwrap();
        let result = extract_rules(&source, "tests.template_tests.templatetags.custom");
        let key = SymbolKey::tag("tests.template_tests.templatetags.custom", "no_params");
        assert!(
            result.tag_rules.contains_key(&key),
            "should extract simple_tag no_params"
        );
    }

    // Corpus: `cut` in django/template/defaultfilters.py — `@register.filter`
    // with required arg (value, arg), exercises filter pipeline
    #[test]
    fn extract_rules_filter() {
        let source = django_source("django/template/defaultfilters.py").unwrap();
        let result = extract_rules(&source, "django.template.defaultfilters");
        let key = SymbolKey::filter("django.template.defaultfilters", "lower");
        assert!(result.filter_arities.contains_key(&key));
        let arity = &result.filter_arities[&key];
        assert!(!arity.expects_arg);
    }

    // Corpus: `default` in django/template/defaultfilters.py — filter with
    // required arg (value, arg)
    #[test]
    fn extract_rules_filter_with_arg() {
        let source = django_source("django/template/defaultfilters.py").unwrap();
        let result = extract_rules(&source, "django.template.defaultfilters");
        let key = SymbolKey::filter("django.template.defaultfilters", "default");
        assert!(result.filter_arities.contains_key(&key));
        let arity = &result.filter_arities[&key];
        assert!(arity.expects_arg);
        assert!(!arity.arg_optional);
    }

    // Corpus: `block` in django/template/loader_tags.py — `@register.tag("block")`
    // with parser.parse(("endblock",)) block spec
    #[test]
    fn extract_rules_block_tag() {
        let source = django_source("django/template/loader_tags.py").unwrap();
        let result = extract_rules(&source, "django.template.loader_tags");
        let key = SymbolKey::tag("django.template.loader_tags", "block");
        assert!(
            result.block_specs.as_map().contains_key(&key),
            "should extract block spec for block tag"
        );
        let spec = &result.block_specs.as_map()[&key];
        assert_eq!(spec.end_tag.as_deref(), Some("endblock"));
    }

    // (b) Edge case — empty source has no registrations
    #[test]
    fn extract_rules_empty_source() {
        let result = extract_rules("", "test.module");
        assert!(result.is_empty());
    }

    // (b) Edge case — invalid Python returns empty result
    #[test]
    fn extract_rules_invalid_python() {
        let result = extract_rules("def {invalid python", "test.module");
        assert!(result.is_empty());
    }

    // (b) Edge case — valid Python with no registrations
    #[test]
    fn extract_rules_no_registrations() {
        let source = r"
def regular_function():
    pass

class MyClass:
    pass
";
        let result = extract_rules(source, "test.module");
        assert!(result.is_empty());
    }

    // Corpus: defaulttags.py has both tags and filters (via `cycle` tag +
    // querystring simple_tag). Validates multiple registration kinds extracted.
    #[test]
    fn extract_rules_multiple_registrations() {
        let source = django_source("django/template/defaulttags.py").unwrap();
        let result = extract_rules(&source, "django.template.defaulttags");
        let tag_key = SymbolKey::tag("django.template.defaulttags", "for");
        let simple_key = SymbolKey::tag("django.template.defaulttags", "querystring");
        assert!(
            result.tag_rules.contains_key(&tag_key),
            "should extract tag rule for 'for'"
        );
        assert!(
            result.tag_rules.contains_key(&simple_key),
            "should extract tag rule for 'querystring'"
        );
    }

    // (b) Edge case — call-style registration where the function def isn't
    // in the same file. Registration found but no matching func def → no rules.
    #[test]
    fn extract_rules_call_style_registration_no_func_def() {
        let source = r#"
from django import template
from somewhere import do_for
register = template.Library()

register.tag("for", do_for)
"#;
        let result = extract_rules(source, "test.module");
        assert!(result.tag_rules.is_empty());
        assert!(result.block_specs.is_empty());
    }

    // Corpus golden tests — full pipeline extraction on real Django modules.
    // These snapshot the complete extraction output for each module.

    // Corpus: django/template/defaulttags.py — the largest built-in templatetag
    // module. Exercises bare @register.tag, @register.tag("name"),
    // @register.tag(name="name"), @register.simple_tag, len checks (exact, min,
    // max, not-in), keyword position checks, option loops, block specs with
    // intermediates, opaque blocks, dynamic end tags, and multiple raise statements.
    #[test]
    fn golden_defaulttags() {
        let source = django_source("django/template/defaulttags.py").unwrap();
        insta::assert_yaml_snapshot!(snapshot(extract_rules(
            &source,
            "django.template.defaulttags"
        )));
    }

    // Corpus: django/template/loader_tags.py — block, extends, include tags.
    // Exercises simple block (endblock), option loop (include with/only),
    // and non-block tags (extends).
    #[test]
    fn golden_loader_tags() {
        let source = django_source("django/template/loader_tags.py").unwrap();
        insta::assert_yaml_snapshot!(snapshot(extract_rules(
            &source,
            "django.template.loader_tags"
        )));
    }

    // Corpus: django/template/defaultfilters.py — all built-in filters.
    // Exercises @register.filter (bare), @register.filter("name"),
    // @register.filter(is_safe=True), filters with no arg, required arg,
    // and optional arg (default parameter).
    #[test]
    fn golden_defaultfilters() {
        let source = django_source("django/template/defaultfilters.py").unwrap();
        insta::assert_yaml_snapshot!(snapshot(extract_rules(
            &source,
            "django.template.defaultfilters"
        )));
    }

    // Corpus: django/templatetags/i18n.py — i18n tags.
    // Exercises @register.tag("name"), @register.filter, and the
    // blocktranslate next_token loop pattern.
    #[test]
    fn golden_i18n() {
        let source = django_source("django/templatetags/i18n.py").unwrap();
        insta::assert_yaml_snapshot!(snapshot(extract_rules(&source, "django.templatetags.i18n")));
    }

    // Corpus: tests/template_tests/templatetags/inclusion.py — inclusion tags.
    // Exercises @register.inclusion_tag with and without takes_context,
    // various arg counts, and keyword-only defaults.
    #[test]
    fn golden_inclusion_tags() {
        let source = django_source("tests/template_tests/templatetags/inclusion.py").unwrap();
        insta::assert_yaml_snapshot!(snapshot(extract_rules(
            &source,
            "tests.template_tests.templatetags.inclusion"
        )));
    }

    // Corpus: tests/template_tests/templatetags/custom.py — simple tags.
    // Exercises @register.simple_tag with and without takes_context,
    // @register.simple_tag(name="..."), @register.simple_block_tag,
    // @register.filter, and various arg patterns.
    #[test]
    fn golden_custom_tags() {
        let source = django_source("tests/template_tests/templatetags/custom.py").unwrap();
        insta::assert_yaml_snapshot!(snapshot(extract_rules(
            &source,
            "tests.template_tests.templatetags.custom"
        )));
    }

    // Corpus: tests/template_tests/templatetags/testtags.py — call-style
    // registrations. Exercises register.tag("name", func) and
    // register.filter("name", func) call-style patterns.
    #[test]
    fn golden_testtags() {
        let source = django_source("tests/template_tests/templatetags/testtags.py").unwrap();
        insta::assert_yaml_snapshot!(snapshot(extract_rules(
            &source,
            "tests.template_tests.templatetags.testtags"
        )));
    }

    // Pattern-specific corpus assertions — validate specific extraction
    // behaviors using real Django code, complementing the full-module snapshots.

    // Corpus: `autoescape` in defaulttags.py — bare @register.tag decorator.
    // Registration name defaults to function name.
    #[test]
    fn corpus_decorator_bare_tag() {
        let source = django_source("django/template/defaulttags.py").unwrap();
        let result = extract_rules(&source, "django.template.defaulttags");
        let key = SymbolKey::tag("django.template.defaulttags", "autoescape");
        assert!(
            result.tag_rules.contains_key(&key) || result.block_specs.as_map().contains_key(&key),
            "autoescape should be extracted"
        );
    }

    // Corpus: `for` in defaulttags.py — @register.tag("for") with explicit
    // positional string name overriding function name `do_for`.
    #[test]
    fn corpus_decorator_tag_with_explicit_name() {
        let source = django_source("django/template/defaulttags.py").unwrap();
        let result = extract_rules(&source, "django.template.defaulttags");
        let key = SymbolKey::tag("django.template.defaulttags", "for");
        assert!(
            result.tag_rules.contains_key(&key),
            "'for' tag should be extracted (name from decorator string arg)"
        );
    }

    // Corpus: `partialdef` in defaulttags.py — @register.tag(name="partialdef")
    // with name kwarg overriding function name `partialdef_func`.
    #[test]
    fn corpus_decorator_tag_with_name_kwarg() {
        let source = django_source("django/template/defaulttags.py").unwrap();
        let result = extract_rules(&source, "django.template.defaulttags");
        let key = SymbolKey::tag("django.template.defaulttags", "partialdef");
        assert!(
            result.tag_rules.contains_key(&key) || result.block_specs.as_map().contains_key(&key),
            "partialdef should be extracted (name from kwarg)"
        );
    }

    // Corpus: `no_params` in custom.py — @register.simple_tag with zero user args.
    #[test]
    fn corpus_simple_tag_no_args() {
        let source = django_source("tests/template_tests/templatetags/custom.py").unwrap();
        let result = extract_rules(&source, "tests.template_tests.templatetags.custom");
        let key = SymbolKey::tag("tests.template_tests.templatetags.custom", "no_params");
        assert!(result.tag_rules.contains_key(&key));
        let rule = &result.tag_rules[&key];
        assert!(rule.extracted_args.is_empty());
    }

    // Corpus: `one_param` in custom.py — @register.simple_tag with one required arg.
    #[test]
    fn corpus_simple_tag_with_args() {
        let source = django_source("tests/template_tests/templatetags/custom.py").unwrap();
        let result = extract_rules(&source, "tests.template_tests.templatetags.custom");
        let key = SymbolKey::tag("tests.template_tests.templatetags.custom", "one_param");
        assert!(result.tag_rules.contains_key(&key));
        let rule = &result.tag_rules[&key];
        assert_eq!(rule.extracted_args.len(), 1);
        assert!(rule.extracted_args[0].required);
    }

    // Corpus: `no_params_with_context` in custom.py —
    // @register.simple_tag(takes_context=True), context param excluded from args.
    #[test]
    fn corpus_simple_tag_takes_context() {
        let source = django_source("tests/template_tests/templatetags/custom.py").unwrap();
        let result = extract_rules(&source, "tests.template_tests.templatetags.custom");
        let key = SymbolKey::tag(
            "tests.template_tests.templatetags.custom",
            "no_params_with_context",
        );
        assert!(result.tag_rules.contains_key(&key));
        let rule = &result.tag_rules[&key];
        assert!(
            rule.extracted_args.is_empty(),
            "context param should not appear as extracted arg"
        );
    }

    // Corpus: `inclusion_one_param` in inclusion.py — @register.inclusion_tag
    // with one required arg.
    #[test]
    fn corpus_inclusion_tag() {
        let source = django_source("tests/template_tests/templatetags/inclusion.py").unwrap();
        let result = extract_rules(&source, "tests.template_tests.templatetags.inclusion");
        let key = SymbolKey::tag(
            "tests.template_tests.templatetags.inclusion",
            "inclusion_one_param",
        );
        assert!(result.tag_rules.contains_key(&key));
        let rule = &result.tag_rules[&key];
        assert_eq!(rule.extracted_args.len(), 1);
        assert!(rule.extracted_args[0].required);
    }

    // Corpus: `inclusion_no_params_with_context` in inclusion.py —
    // @register.inclusion_tag with takes_context=True.
    #[test]
    fn corpus_inclusion_tag_takes_context() {
        let source = django_source("tests/template_tests/templatetags/inclusion.py").unwrap();
        let result = extract_rules(&source, "tests.template_tests.templatetags.inclusion");
        let key = SymbolKey::tag(
            "tests.template_tests.templatetags.inclusion",
            "inclusion_no_params_with_context",
        );
        assert!(result.tag_rules.contains_key(&key));
        let rule = &result.tag_rules[&key];
        assert!(
            rule.extracted_args.is_empty(),
            "context param should not appear as extracted arg"
        );
    }

    // Corpus: `inclusion_one_default` in inclusion.py — inclusion_tag with
    // one required + one optional arg.
    #[test]
    fn corpus_inclusion_tag_with_args() {
        let source = django_source("tests/template_tests/templatetags/inclusion.py").unwrap();
        let result = extract_rules(&source, "tests.template_tests.templatetags.inclusion");
        let key = SymbolKey::tag(
            "tests.template_tests.templatetags.inclusion",
            "inclusion_one_default",
        );
        assert!(result.tag_rules.contains_key(&key));
        let rule = &result.tag_rules[&key];
        assert_eq!(rule.extracted_args.len(), 2);
        assert!(rule.extracted_args[0].required);
        assert!(!rule.extracted_args[1].required);
    }

    // Corpus: `querystring` in defaulttags.py — @register.simple_tag(name="querystring",
    // takes_context=True) with name kwarg on simple_tag.
    #[test]
    fn corpus_simple_tag_with_name_kwarg() {
        let source = django_source("django/template/defaulttags.py").unwrap();
        let result = extract_rules(&source, "django.template.defaulttags");
        let key = SymbolKey::tag("django.template.defaulttags", "querystring");
        assert!(
            result.tag_rules.contains_key(&key),
            "querystring should be extracted via name kwarg"
        );
    }

    // Corpus: `widthratio` in defaulttags.py — real Django uses
    // `if len(bits) == 4 / elif len(bits) == 6 / else` pattern, which
    // extracts as required keyword "as" at position 4 (for the 6-arg form).
    #[test]
    fn corpus_len_exact_check() {
        let source = django_source("django/template/defaulttags.py").unwrap();
        let result = extract_rules(&source, "django.template.defaulttags");
        let key = SymbolKey::tag("django.template.defaulttags", "widthratio");
        assert!(
            result.tag_rules.contains_key(&key),
            "widthratio should be extracted"
        );
        let rule = &result.tag_rules[&key];
        assert!(
            !rule.required_keywords.is_empty(),
            "widthratio should have required keyword (as)"
        );
    }

    // Corpus: `cycle` in defaulttags.py — `len(args) < 2` → Min(2).
    #[test]
    fn corpus_len_min_check() {
        let source = django_source("django/template/defaulttags.py").unwrap();
        let result = extract_rules(&source, "django.template.defaulttags");
        let key = SymbolKey::tag("django.template.defaulttags", "cycle");
        assert!(result.tag_rules.contains_key(&key));
        let rule = &result.tag_rules[&key];
        assert!(
            rule.arg_constraints
                .contains(&ArgumentCountConstraint::Min(2)),
            "cycle should have Min(2) constraint"
        );
    }

    // Corpus: `templatetag` in defaulttags.py — `len(bits) != 2` → Exact(2).
    // Real `debug` tag has no split_contents, so we use `templatetag` which
    // has a clean `len(bits) != 2` check for the exact constraint pattern.
    #[test]
    fn corpus_len_exact_check_templatetag() {
        let source = django_source("django/template/defaulttags.py").unwrap();
        let result = extract_rules(&source, "django.template.defaulttags");
        let key = SymbolKey::tag("django.template.defaulttags", "templatetag");
        assert!(result.tag_rules.contains_key(&key));
        let rule = &result.tag_rules[&key];
        assert!(
            rule.arg_constraints
                .contains(&ArgumentCountConstraint::Exact(2)),
            "templatetag should have Exact(2) constraint"
        );
    }

    // Corpus: `url` in defaulttags.py — multiple raise statements:
    // `len(bits) < 2` and additional constraints.
    #[test]
    fn corpus_multiple_raise_statements() {
        let source = django_source("django/template/defaulttags.py").unwrap();
        let result = extract_rules(&source, "django.template.defaulttags");
        let key = SymbolKey::tag("django.template.defaulttags", "url");
        assert!(result.tag_rules.contains_key(&key));
        let rule = &result.tag_rules[&key];
        assert!(
            rule.arg_constraints
                .contains(&ArgumentCountConstraint::Min(2)),
            "url should have Min(2) constraint"
        );
    }

    // Corpus: `include` in loader_tags.py — while-loop option parsing
    // (with, only options).
    #[test]
    fn corpus_option_loop() {
        let source = django_source("django/template/loader_tags.py").unwrap();
        let result = extract_rules(&source, "django.template.loader_tags");
        let key = SymbolKey::tag("django.template.loader_tags", "include");
        assert!(result.tag_rules.contains_key(&key));
        let rule = &result.tag_rules[&key];
        assert!(
            rule.known_options.is_some(),
            "include should have known_options from while-loop"
        );
    }

    // Corpus: `do_for` in defaulttags.py — block with "empty" intermediate
    // and "endfor" end tag.
    #[test]
    fn corpus_for_tag_with_empty() {
        let source = django_source("django/template/defaulttags.py").unwrap();
        let result = extract_rules(&source, "django.template.defaulttags");
        let key = SymbolKey::tag("django.template.defaulttags", "for");
        assert!(result.block_specs.as_map().contains_key(&key));
        let spec = &result.block_specs.as_map()[&key];
        assert_eq!(spec.end_tag.as_deref(), Some("endfor"));
        assert!(spec.intermediates.contains(&"empty".to_string()));
    }

    // Corpus: `do_if` in defaulttags.py — block with elif/else intermediates.
    #[test]
    fn corpus_block_with_intermediates() {
        let source = django_source("django/template/defaulttags.py").unwrap();
        let result = extract_rules(&source, "django.template.defaulttags");
        let key = SymbolKey::tag("django.template.defaulttags", "if");
        assert!(result.block_specs.as_map().contains_key(&key));
        let spec = &result.block_specs.as_map()[&key];
        assert_eq!(spec.end_tag.as_deref(), Some("endif"));
        assert!(spec.intermediates.contains(&"elif".to_string()));
        assert!(spec.intermediates.contains(&"else".to_string()));
    }

    // Corpus: `comment` in defaulttags.py — opaque block (skip_past).
    // Real `verbatim` actually uses parser.parse(), not skip_past — only
    // `comment` is truly opaque in defaulttags.py.
    #[test]
    fn corpus_opaque_block() {
        let source = django_source("django/template/defaulttags.py").unwrap();
        let result = extract_rules(&source, "django.template.defaulttags");
        let key = SymbolKey::tag("django.template.defaulttags", "comment");
        assert!(result.block_specs.as_map().contains_key(&key));
        let spec = &result.block_specs.as_map()[&key];
        assert!(spec.opaque);
        assert_eq!(spec.end_tag.as_deref(), Some("endcomment"));
    }

    // Corpus: `verbatim` in defaulttags.py — uses parser.parse(), not
    // skip_past. No split_contents call (no argument validation).
    #[test]
    fn corpus_non_opaque_no_split_contents() {
        let source = django_source("django/template/defaulttags.py").unwrap();
        let result = extract_rules(&source, "django.template.defaulttags");
        let key = SymbolKey::tag("django.template.defaulttags", "verbatim");
        assert!(result.block_specs.as_map().contains_key(&key));
        let spec = &result.block_specs.as_map()[&key];
        assert!(
            !spec.opaque,
            "real verbatim uses parser.parse(), not skip_past"
        );
        assert_eq!(spec.end_tag.as_deref(), Some("endverbatim"));
    }

    // Corpus: `spaceless` in defaulttags.py — uses `token.split_contents()[0]`
    // in f-string for dynamic end tag name.
    #[test]
    fn corpus_dynamic_end_tag() {
        let source = django_source("django/template/defaulttags.py").unwrap();
        let result = extract_rules(&source, "django.template.defaulttags");
        let key = SymbolKey::tag("django.template.defaulttags", "spaceless");
        assert!(result.block_specs.as_map().contains_key(&key));
        let spec = &result.block_specs.as_map()[&key];
        // Dynamic end-tag detected as None (computed at runtime), but
        // extract_rules() fills it with "end{name}" as fallback
        assert!(spec.end_tag.is_some());
    }

    // Corpus: `do_block` in loader_tags.py — simple block tag with endblock.
    #[test]
    fn corpus_simple_block() {
        let source = django_source("django/template/loader_tags.py").unwrap();
        let result = extract_rules(&source, "django.template.loader_tags");
        let key = SymbolKey::tag("django.template.loader_tags", "block");
        assert!(result.block_specs.as_map().contains_key(&key));
        let spec = &result.block_specs.as_map()[&key];
        assert_eq!(spec.end_tag.as_deref(), Some("endblock"));
        assert!(spec.intermediates.is_empty());
        assert!(!spec.opaque);
    }

    // Corpus: `title` in defaultfilters.py — filter with no arg (value only).
    #[test]
    fn corpus_filter_no_arg() {
        let source = django_source("django/template/defaultfilters.py").unwrap();
        let result = extract_rules(&source, "django.template.defaultfilters");
        let key = SymbolKey::filter("django.template.defaultfilters", "title");
        assert!(result.filter_arities.contains_key(&key));
        let arity = &result.filter_arities[&key];
        assert!(!arity.expects_arg);
    }

    // Corpus: `default` in defaultfilters.py — filter with required arg.
    #[test]
    fn corpus_filter_required_arg() {
        let source = django_source("django/template/defaultfilters.py").unwrap();
        let result = extract_rules(&source, "django.template.defaultfilters");
        let key = SymbolKey::filter("django.template.defaultfilters", "default");
        assert!(result.filter_arities.contains_key(&key));
        let arity = &result.filter_arities[&key];
        assert!(arity.expects_arg);
        assert!(!arity.arg_optional);
    }

    // Corpus: `date` in defaultfilters.py — filter with optional arg (arg=None).
    #[test]
    fn corpus_filter_optional_arg() {
        let source = django_source("django/template/defaultfilters.py").unwrap();
        let result = extract_rules(&source, "django.template.defaultfilters");
        let key = SymbolKey::filter("django.template.defaultfilters", "date");
        assert!(result.filter_arities.contains_key(&key));
        let arity = &result.filter_arities[&key];
        assert!(arity.expects_arg);
        assert!(arity.arg_optional);
    }

    // Corpus: `escapejs` in defaultfilters.py — @register.filter("escapejs")
    // with positional string name, bare filter decorator with no user arg.
    #[test]
    fn corpus_filter_bare_decorator() {
        let source = django_source("django/template/defaultfilters.py").unwrap();
        let result = extract_rules(&source, "django.template.defaultfilters");
        let key = SymbolKey::filter("django.template.defaultfilters", "lower");
        assert!(result.filter_arities.contains_key(&key));
    }

    // Corpus: `escapejs` in defaultfilters.py — @register.filter("escapejs")
    // demonstrates named filter via positional string arg.
    #[test]
    fn corpus_filter_with_name() {
        let source = django_source("django/template/defaultfilters.py").unwrap();
        let result = extract_rules(&source, "django.template.defaultfilters");
        let key = SymbolKey::filter("django.template.defaultfilters", "escapejs");
        assert!(
            result.filter_arities.contains_key(&key),
            "escapejs should be extracted (name from positional string)"
        );
    }

    // Corpus: `addslashes` in defaultfilters.py — @register.filter(is_safe=True)
    // with kwarg but no name override.
    #[test]
    fn corpus_filter_is_safe() {
        let source = django_source("django/template/defaultfilters.py").unwrap();
        let result = extract_rules(&source, "django.template.defaultfilters");
        let key = SymbolKey::filter("django.template.defaultfilters", "addslashes");
        assert!(
            result.filter_arities.contains_key(&key),
            "addslashes should be extracted with is_safe kwarg"
        );
    }

    // (b) Edge case — method-style registration (self parameter).
    // Not standard Django — tests that class method registrations handle
    // the extra `self` parameter.
    #[test]
    fn golden_filter_method_style() {
        let source = r"
from django import template
register = template.Library()

class StringFilter:
    def upper(self, value):
        return value.upper()

register.filter('upper', StringFilter().upper)
";
        insta::assert_yaml_snapshot!(snapshot(extract_rules(source, "app.templatetags.filters")));
    }

    // (b) Edge case — non-bits variable name in split_contents.
    // Tests that the extraction uses the dynamically detected split variable,
    // NOT a hardcoded "bits" name.
    #[test]
    fn golden_non_bits_variable() {
        let source = r#"
from django import template
register = template.Library()

@register.tag
def custom_tag(parser, token):
    parts = token.split_contents()
    if len(parts) != 3:
        raise template.TemplateSyntaxError("'custom_tag' requires exactly two arguments")
    return CustomNode(parts[1], parts[2])
"#;
        insta::assert_yaml_snapshot!(snapshot(extract_rules(source, "app.templatetags.custom")));
    }

    // (b) Edge case — empty source
    #[test]
    fn golden_empty_source() {
        insta::assert_yaml_snapshot!(snapshot(extract_rules("", "test.module")));
    }

    // (b) Edge case — invalid Python
    #[test]
    fn golden_invalid_python() {
        insta::assert_yaml_snapshot!(snapshot(extract_rules("def {invalid", "test.module")));
    }

    // (b) Edge case — no registrations in valid Python
    #[test]
    fn golden_no_registrations() {
        let source = r"
def helper():
    pass

class Config:
    DEBUG = True
";
        insta::assert_yaml_snapshot!(snapshot(extract_rules(source, "test.module")));
    }

    // (b) Edge case — call-style registration with missing function definition
    #[test]
    fn golden_call_style_no_func_def() {
        let source = r#"
from django import template
from somewhere import do_for
register = template.Library()

register.tag("for", do_for)
"#;
        insta::assert_yaml_snapshot!(snapshot(extract_rules(source, "test.module")));
    }
}
