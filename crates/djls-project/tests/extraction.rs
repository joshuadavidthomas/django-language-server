use std::fs;

use camino::Utf8Path;
use djls_project::ArgumentCountConstraint;
use djls_project::ArgumentFormCoverage;
use djls_project::AssignmentMode;
use djls_project::BodyAnalysisEvidence;
use djls_project::ChoiceAt;
use djls_project::ExtractedMessageArg;
use djls_project::ExtractedMessageTemplate;
use djls_project::FilterArity;
use djls_project::ParameterRequirement;
use djls_project::PythonModuleName;
use djls_project::RemainderPolicy;
use djls_project::SymbolKey;
use djls_project::TagArgumentKind;
use djls_project::TagArgumentPatternKind;
use djls_project::TagArgumentSyntax;
use djls_project::TemplateLibraryId;
use djls_project::TemplateSymbolKind;
use djls_project::UniqueKeyCardinality;
use djls_project::template_library_definition_facts;
use djls_project::template_library_filter_facts;
use djls_project::template_library_registration_dependencies;
use djls_project::template_library_tag_facts;
use djls_project::template_symbol_source;
use djls_project::testing::PythonSyntaxErrorClass;
use djls_project::testing::python_syntax_errors;
use djls_project::testing::template_library_definition_facts_snapshot;
use djls_source::ChangeEvent;
use djls_source::File;
use djls_source::SourceChanges;
use djls_source::Span;
use djls_testing::Corpus;
use djls_testing::ExtractionBundle;
use djls_testing::ProjectFixture;
use djls_testing::SalsaEventLog;
use djls_testing::TestDatabase;
use djls_testing::extract_bundle;
use djls_testing::sorted_snapshot;
use salsa::Database as _;

const ALLAUTH_TAGS_SOURCE: &str = include_str!("../src/templates/tags/testdata/allauth_tags.py");
const CUSTOM_SOURCE: &str = include_str!("../src/templates/tags/testdata/django_custom.py");
const DEFAULTFILTERS_SOURCE: &str =
    include_str!("../src/templates/tags/testdata/django_defaultfilters.py");
const DEFAULTTAGS_SOURCE: &str =
    include_str!("../src/templates/tags/testdata/django_defaulttags.py");
const I18N_SOURCE: &str = include_str!("../src/templates/tags/testdata/django_i18n.py");
const INCLUSION_SOURCE: &str = include_str!("../src/templates/tags/testdata/django_inclusion.py");
const LOADER_TAGS_SOURCE: &str =
    include_str!("../src/templates/tags/testdata/django_loader_tags.py");
const TESTTAGS_SOURCE: &str = include_str!("../src/templates/tags/testdata/django_testtags.py");
const TZ_SOURCE: &str = include_str!("../src/templates/tags/testdata/django_tz.py");
const ADMIN_URLS_SOURCE: &str = include_str!("../src/templates/tags/testdata/django_admin_urls.py");
const WAGTAILADMIN_TAGS_SOURCE: &str =
    include_str!("../src/templates/tags/testdata/wagtailadmin_tags.py");

fn extract_source(
    source: &str,
    module_name: &str,
) -> Result<ExtractionBundle, Box<dyn std::error::Error>> {
    let db = TestDatabase::new();
    let path = Utf8Path::new("/test/extraction.py");
    db.add_file(path.as_str(), source)?;
    let file = db.file(path)?;
    let module_name = PythonModuleName::parse(module_name)?;
    Ok(extract_bundle(&db, file, module_name))
}

fn execution_count(db: &TestDatabase, events: &[salsa::Event], query_name: &str) -> usize {
    events
        .iter()
        .filter(|event| match &event.kind {
            salsa::EventKind::WillExecute { database_key } => db
                .ingredient_debug_name(database_key.ingredient_index())
                .ends_with(query_name),
            salsa::EventKind::DidValidateMemoizedValue { .. }
            | salsa::EventKind::WillBlockOn { .. }
            | salsa::EventKind::WillIterateCycle { .. }
            | salsa::EventKind::DidFinalizeCycle { .. }
            | salsa::EventKind::WillCheckCancellation
            | salsa::EventKind::DidSetCancellationFlag
            | salsa::EventKind::WillDiscardStaleOutput { .. }
            | salsa::EventKind::DidDiscard { .. }
            | salsa::EventKind::DidDiscardAccumulated { .. }
            | salsa::EventKind::DidInternValue { .. }
            | salsa::EventKind::DidReuseInternedValue { .. }
            | salsa::EventKind::DidValidateInternedValue { .. } => false,
        })
        .count()
}

// Corpus: `no_params` in tests/template_tests/templatetags/custom.py —
// `@register.simple_tag` with no user args, exercises simple_tag pipeline
#[test]
fn extract_bundle_simple_tag() {
    let result = extract_source(CUSTOM_SOURCE, "tests.template_tests.templatetags.custom")
        .expect("simple-tag extraction fixture should build");
    let key = SymbolKey::tag("tests.template_tests.templatetags.custom", "no_params");
    assert!(
        result.tag_rules.contains_key(&key),
        "should extract simple_tag no_params"
    );
}

// Corpus: `cut` in django/template/defaultfilters.py — `@register.filter`
// with required arg (value, arg), exercises filter pipeline
#[test]
fn extract_bundle_filter() {
    let result = extract_source(DEFAULTFILTERS_SOURCE, "django.template.defaultfilters")
        .expect("filter extraction fixture should build");
    let key = SymbolKey::filter("django.template.defaultfilters", "lower");
    assert_eq!(
        result.filter_arities.get(&key),
        Some(&FilterArity::NoArgument)
    );
}

// Corpus: `default` in django/template/defaultfilters.py — filter with
// required arg (value, arg)
#[test]
fn extract_bundle_filter_with_arg() {
    let result = extract_source(DEFAULTFILTERS_SOURCE, "django.template.defaultfilters")
        .expect("filter-with-argument extraction fixture should build");
    let key = SymbolKey::filter("django.template.defaultfilters", "default");
    assert_eq!(
        result.filter_arities.get(&key),
        Some(&FilterArity::RequiredArgument)
    );
}

// Corpus: `block` in django/template/loader_tags.py — `@register.tag("block")`
// with parser.parse(("endblock",)) block spec
#[test]
fn extract_bundle_block_tag() {
    let result = extract_source(LOADER_TAGS_SOURCE, "django.template.loader_tags")
        .expect("block-tag extraction fixture should build");
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
fn pipeline_tuple_unpack_extracts_exact_arity_for_local_registrations() {
    let source = r#"
from django import template
register = template.Library()

@register.tag
def stylesheet(parser, token):
    try:
        tag_name, name = token.split_contents()
    except ValueError:
        message = "%r requires exactly one argument"
        raise template.TemplateSyntaxError(message % token.split_contents()[0])
    return Node(name)

@register.tag
def javascript(parser, token):
    try:
        tag_name, name = token.split_contents()
    except ValueError:
        raise template.TemplateSyntaxError("requires exactly one argument")
    return Node(name)
"#;
    let result = extract_source(source, "pipeline_tags").expect("pipeline rules should extract");
    for tag in ["stylesheet", "javascript"] {
        let rule = &result.tag_rules[&SymbolKey::tag("pipeline_tags", tag)];
        assert_eq!(
            rule.arg_constraints,
            vec![ArgumentCountConstraint::Exact(2)],
            "tag: {tag}"
        );
        assert_eq!(rule.diagnostic_messages.as_ref().map(Vec::len), Some(1));
    }
}

#[test]
fn tuple_unpack_message_requires_reachable_builtin_value_error_handler() {
    let cases = [
        r#"
def compile_tag(parser, token):
    try:
        tag_name, value = token.split_contents()
    except Exception:
        raise SyntaxError("broad handler")
    except ValueError:
        raise SyntaxError("dead value handler")
register.tag("checked", compile_tag)
"#,
        r#"
ValueError = RuntimeError
def compile_tag(parser, token):
    try:
        tag_name, value = token.split_contents()
    except ValueError:
        raise SyntaxError("shadowed handler")
register.tag("checked", compile_tag)
"#,
        r#"
def compile_tag(parser, token):
    try:
        tag_name, value = token.split_contents()
    except ValueError:
        raise SyntaxError("possibly shadowed handler")
register.tag("checked", compile_tag)
from external import *
"#,
    ];
    for declarations in cases {
        let source =
            format!("from django import template\nregister = template.Library()\n{declarations}");
        let result = extract_source(&source, "handler_tags").expect("fixture should extract");
        let rule = &result.tag_rules[&SymbolKey::tag("handler_tags", "checked")];
        assert_eq!(
            rule.arg_constraints,
            vec![ArgumentCountConstraint::Exact(2)]
        );
        assert!(rule.diagnostic_messages.as_ref().is_none_or(Vec::is_empty));
    }
}

#[test]
fn negative_length_facts_survive_untracked_branches() {
    let source = r#"
from django import template
register = template.Library()
@register.tag
def checked(parser, token):
    bits = token.split_contents()
    if len(bits) != 2:
        if len(bits) == 2:
            return template.Node()
    assert runtime_condition()
    if len(bits) == 3:
        return template.Node()
    raise template.TemplateSyntaxError("bad count")
"#;
    let result = extract_source(source, "negative_length_tags").expect("fixture should extract");
    let key = SymbolKey::tag("negative_length_tags", "checked");
    let rule = result
        .tag_rules
        .get(&key)
        .unwrap_or_else(|| panic!("missing checked rule: {:?}", result.tag_rules));
    assert_eq!(
        rule.arg_constraints,
        vec![ArgumentCountConstraint::Exact(3)]
    );
}

#[test]
fn tuple_unpack_recovering_broad_handler_accepts_wrong_arity() {
    let source = r#"
from django import template
register = template.Library()
def compile_tag(parser, token):
    try:
        tag_name, value = token.split_contents()
    except:
        return template.Node()
    return template.Node()
register.tag("checked", compile_tag)
"#;
    let result = extract_source(source, "recovering_handler_tags").expect("fixture should extract");
    let key = SymbolKey::tag("recovering_handler_tags", "checked");
    assert!(result.tag_rules.get(&key).is_none_or(|rule| {
        !rule
            .arg_constraints
            .contains(&ArgumentCountConstraint::Exact(2))
    }));
}

#[test]
fn while_true_later_iteration_return_does_not_publish_first_iteration_width() {
    let source = r"
from django import template
register = template.Library()
@register.tag
def looping(parser, token):
    bits = token.split_contents()
    while True:
        bits.pop(0)
        if len(bits) == 2:
            return Node()
";
    let result = extract_source(source, "loop_paths").expect("fixture should extract");
    let key = SymbolKey::tag("loop_paths", "looping");
    assert!(
        result
            .tag_rules
            .get(&key)
            .is_none_or(|rule| rule.arg_constraints.is_empty())
    );
}

#[test]
fn accepting_wildcard_before_direct_raise_suppresses_match_aggregate() {
    let source = r#"
from django import template
register = template.Library()
@register.tag
def matched(parser, token):
    accept = True
    match token.split_contents():
        case ["matched", "special"]:
            return Node()
        case _:
            if accept:
                return Node()
            raise template.TemplateSyntaxError("bad")
"#;
    let result = extract_source(source, "match_paths").expect("fixture should extract");
    let key = SymbolKey::tag("match_paths", "matched");
    assert!(result.tag_rules.get(&key).is_none_or(|rule| {
        rule.arg_constraints.is_empty() && rule.required_keywords.is_empty()
    }));
}

#[test]
fn ordered_match_dispatch_ignores_unreachable_later_cases() {
    let source = r#"
from django import template
register = template.Library()
@register.tag
def matched(parser, token):
    match token.split_contents():
        case ["matched", value]:
            return Node(value)
        case ["matched", value]:
            raise template.TemplateSyntaxError("unreachable")
        case _:
            raise template.TemplateSyntaxError("bad count")
"#;
    let result = extract_source(source, "ordered_match_tags").expect("fixture should extract");
    let key = SymbolKey::tag("ordered_match_tags", "matched");
    assert!(
        result.tag_rules.contains_key(&key),
        "registration should be extracted"
    );
    let rule = &result.tag_rules[&key];
    let (forms, coverage) = rule.argument_syntax.forms().expect("known match form");
    assert_eq!(coverage, ArgumentFormCoverage::Partial);
    assert_eq!(forms.len(), 1);
    assert_eq!(forms[0].pattern().len(), 1);
}

#[test]
fn guarded_match_case_falls_through_to_later_cases() {
    let source = r#"
from django import template
register = template.Library()
@register.tag
def matched(parser, token):
    match token.split_contents():
        case ["matched", mode] if mode == "blocked":
            raise template.TemplateSyntaxError("blocked")
        case ["matched", "safe"]:
            return Node()
        case _:
            raise template.TemplateSyntaxError("bad mode")
"#;
    let result = extract_source(source, "guarded_match_tags").expect("fixture should extract");
    let key = SymbolKey::tag("guarded_match_tags", "matched");
    assert!(
        result.tag_rules.contains_key(&key),
        "registration should be extracted"
    );
    let rule = &result.tag_rules[&key];
    let (forms, _) = rule.argument_syntax.forms().expect("known guarded form");
    assert_eq!(forms.len(), 1);
    assert_eq!(
        forms[0].pattern()[0].kind,
        TagArgumentPatternKind::Literal("safe".to_string())
    );
}

#[test]
fn match_or_keeps_different_lengths_and_literals_correlated() {
    let source = r#"
from django import template
register = template.Library()
@register.tag
def matched(parser, token):
    match token.split_contents():
        case [_, "short"] | [_, _, "long"]:
            return Node()
        case _:
            raise template.TemplateSyntaxError("bad form")
"#;
    let result = extract_source(source, "alternative_match_tags").expect("fixture should extract");
    let key = SymbolKey::tag("alternative_match_tags", "matched");
    assert!(
        result.tag_rules.contains_key(&key),
        "registration should be extracted"
    );
    let rule = &result.tag_rules[&key];
    let (forms, _) = rule
        .argument_syntax
        .forms()
        .expect("correlated match forms");
    assert_eq!(forms.len(), 2);
    assert!(forms.iter().any(|form| form.match_full(&["short"]).is_ok()));
    assert!(
        forms
            .iter()
            .any(|form| form.match_full(&["anything", "long"]).is_ok())
    );
    assert!(rule.required_keywords.is_empty());
}

#[test]
fn unsupported_nested_match_captures_replace_previous_values() {
    let source = r#"
from django import template
register = template.Library()
@register.tag
def nested(parser, token):
    bits = token.split_contents()
    if len(bits) != 3:
        raise template.TemplateSyntaxError("count")
    value = bits[2]
    match bits:
        case [_, str(value), _]:
            if value != "safe":
                raise template.TemplateSyntaxError("value")
            return Node()
        case _:
            raise template.TemplateSyntaxError("shape")
"#;
    let result = extract_source(source, "nested_capture").expect("fixture should extract");
    let rule = &result.tag_rules[&SymbolKey::tag("nested_capture", "nested")];
    assert_eq!(
        rule.arg_constraints,
        vec![ArgumentCountConstraint::Exact(3)]
    );
    assert!(rule.required_keywords.is_empty());
}

#[test]
fn match_keyword_proof_survives_an_unrelated_conditional_assignment() {
    let source = r#"
from django import template
register = template.Library()
@register.tag
def matched(parser, token):
    match token.split_contents():
        case "matched", name, "inline":
            inline = True
        case "matched", name:
            inline = False
        case _:
            raise template.TemplateSyntaxError("bad arguments")
    metadata = runtime_value() if runtime_condition() else None
    return Node(name, inline, metadata)
"#;
    let result = extract_source(source, "match_metadata").expect("fixture should extract");
    let rule = &result.tag_rules[&SymbolKey::tag("match_metadata", "matched")];
    assert_eq!(
        rule.arg_constraints,
        vec![ArgumentCountConstraint::OneOf(vec![2, 3])]
    );
    assert_eq!(
        rule.required_keywords,
        vec![djls_project::RequiredKeyword {
            position: djls_project::SplitPosition::Forward(2),
            value: "inline".to_string()
        }]
    );
    let (forms, _) = rule
        .argument_syntax
        .forms()
        .expect("known argument alternatives");
    assert_eq!(forms.len(), 2);
}

#[test]
fn match_case_body_uses_shared_try_loop_and_finally_execution() {
    let source = r#"
from django import template
register = template.Library()
@register.tag
def matched(parser, token):
    match token.split_contents():
        case ["matched", value]:
            for _ in (1,):
                try:
                    raise ValueError("handled")
                except ValueError:
                    break
                finally:
                    continue
            return Node(value)
        case _:
            raise template.TemplateSyntaxError("bad count")
"#;
    let result = extract_source(source, "nested_match_tags").expect("fixture should extract");
    let key = SymbolKey::tag("nested_match_tags", "matched");
    assert!(
        result.tag_rules.contains_key(&key),
        "registration should be extracted"
    );
    let rule = &result.tag_rules[&key];
    let (forms, _) = rule
        .argument_syntax
        .forms()
        .expect("known nested match form");
    assert_eq!(forms.len(), 1);
    assert_eq!(forms[0].pattern().len(), 1);
}

#[test]
fn match_capture_replaces_an_earlier_environment_value() {
    let source = r#"
from django import template
register = template.Library()
@register.tag
def matched(parser, token):
    bits = token.split_contents()
    if len(bits) != 2:
        raise template.TemplateSyntaxError("bad count")
    value = bits[1]
    match runtime_value():
        case value:
            pass
    if value != "safe":
        raise template.TemplateSyntaxError("bad value")
    return Node()
"#;
    let result = extract_source(source, "capture_match_tags").expect("fixture should extract");
    let key = SymbolKey::tag("capture_match_tags", "matched");
    assert!(
        result.tag_rules.contains_key(&key),
        "registration should be extracted"
    );
    assert!(result.tag_rules[&key].required_keywords.is_empty());
}

#[test]
fn effectful_match_guard_invalidates_the_saved_split_subject() {
    let source = r#"
from django import template
register = template.Library()
@register.tag
def matched(parser, token):
    bits = token.split_contents()
    if len(bits) != 2:
        raise template.TemplateSyntaxError("bad count")
    match bits:
        case ["matched", value] if mutate(bits):
            raise template.TemplateSyntaxError("blocked")
        case ["matched", captured]:
            pass
    if captured != "safe":
        raise template.TemplateSyntaxError("bad value")
    return Node()
"#;
    let result = extract_source(source, "guard_effect_match_tags").expect("fixture should extract");
    let key = SymbolKey::tag("guard_effect_match_tags", "matched");
    assert!(
        result.tag_rules.contains_key(&key),
        "registration should be extracted"
    );
    assert!(result.tag_rules[&key].required_keywords.is_empty());
}

#[test]
fn mutable_holder_capture_invalidates_aliased_split_width() {
    let source = r#"
from django import template
register = template.Library()
@register.tag
def aliased(parser, token):
    bits = token.split_contents()
    holder = [bits]
    holder[0][:] = ["aliased", "forced"]
    if len(bits) != 2:
        raise template.TemplateSyntaxError("bad")
    return Node()
"#;
    let result = extract_source(source, "alias_paths").expect("fixture should extract");
    let key = SymbolKey::tag("alias_paths", "aliased");
    assert!(
        result
            .tag_rules
            .get(&key)
            .is_none_or(|rule| rule.arg_constraints.is_empty())
    );
}

#[test]
fn shadowed_value_error_handler_may_catch_explicit_type_error() {
    let source = r#"
from django import template
register = template.Library()
@register.tag
def shadowed(parser, token):
    bits = token.split_contents()
    is_three = len(bits) == 3
    ValueError = TypeError
    try:
        if is_three:
            raise TypeError
    except (ValueError,):
        return Node()
    if len(bits) != 2:
        raise template.TemplateSyntaxError("bad")
    return Node()
"#;
    let result = extract_source(source, "shadowed_handler").expect("fixture should extract");
    let key = SymbolKey::tag("shadowed_handler", "shadowed");
    assert_eq!(
        result.tag_rules[&key].arg_constraints,
        vec![ArgumentCountConstraint::OneOf(vec![2, 3])]
    );
}

#[test]
fn known_split_unpack_routes_builtin_value_error_by_proven_identity() {
    let source = r#"
from django import template
register = template.Library()

@register.tag("checked")
def checked(parser, token):
    bits = token.split_contents()
    try:
        tag, argument = bits
    except TypeError:
        return Node()
    if len(bits) != 2:
        raise template.TemplateSyntaxError("bad")
    return Node()
"#;
    let result = extract_source(source, "unpack_identity").expect("fixture should extract");
    let rule = &result.tag_rules[&SymbolKey::tag("unpack_identity", "checked")];
    assert_eq!(
        rule.arg_constraints,
        vec![ArgumentCountConstraint::Exact(2)]
    );
}

#[test]
fn builtin_value_error_handler_accepts_known_unpack_failures() {
    let source = r"
from django import template
register = template.Library()
@register.tag
def recovering(parser, token):
    bits = token.split_contents()
    try:
        tag, argument = bits
    except ValueError:
        return Node()
    return Node()
";
    let result = extract_source(source, "unpack_recovery").expect("fixture should extract");
    let key = SymbolKey::tag("unpack_recovery", "recovering");
    assert!(result.tag_rules.get(&key).is_none_or(|rule| {
        !rule
            .arg_constraints
            .contains(&ArgumentCountConstraint::Exact(2))
    }));
}

#[test]
fn split_unpack_executes_at_its_statement_and_through_nested_control_flow() {
    let source = r#"
from django import template
register = template.Library()

@register.tag
def nested(parser, token):
    try:
        if runtime_condition():
            tag, value = token.split_contents()
        else:
            tag, value = token.split_contents()
    except ValueError:
        raise template.TemplateSyntaxError("nested needs one argument")
    return Node(value)

@register.tag
def nested_target(parser, token):
    try:
        tag, (left, right) = token.split_contents()
    except ValueError:
        raise template.TemplateSyntaxError("nested target needs one argument")
    return Node(left, right)

@register.tag
def prefixed(parser, token):
    bits = token.split_contents()
    try:
        bits.pop(0)
        first, second = bits
    except ValueError:
        raise template.TemplateSyntaxError("prefixed needs two arguments")
    return Node(first, second)

@register.tag
def starred(parser, token):
    bits = token.split_contents()
    try:
        tag, *middle, last = token.split_contents()
    except ValueError:
        raise template.TemplateSyntaxError("starred needs an argument")
    return Node(middle, last)

@register.tag
def nonraising(parser, token):
    bits = token.split_contents()
    try:
        marker = 1
    except ValueError:
        return Node()
    if len(bits) != 2:
        raise template.TemplateSyntaxError("nonraising needs one argument")
    return Node(bits[1], marker)
"#;
    let result = extract_source(source, "statement_unpack").expect("fixture should extract");
    for (tag, constraint) in [
        ("prefixed", ArgumentCountConstraint::Exact(3)),
        ("nested_target", ArgumentCountConstraint::Exact(2)),
        ("nonraising", ArgumentCountConstraint::Exact(2)),
        ("nested", ArgumentCountConstraint::Exact(2)),
    ] {
        let key = SymbolKey::tag("statement_unpack", tag);
        let rule = result
            .tag_rules
            .get(&key)
            .unwrap_or_else(|| panic!("missing rule for tag: {tag}"));
        assert_eq!(rule.arg_constraints, vec![constraint], "tag: {tag}");
    }
    let starred = &result.tag_rules[&SymbolKey::tag("statement_unpack", "starred")];
    assert_eq!(
        starred.arg_constraints,
        vec![ArgumentCountConstraint::Min(2)]
    );
    assert_eq!(starred.diagnostic_messages.as_ref().map(Vec::len), Some(1));
}

#[test]
fn failed_known_tuple_unpack_does_not_write_before_value_error() {
    let source = r#"
from django import template
register = template.Library()
@register.tag
def checked(parser, token):
    bits = token.split_contents()
    bits.pop(0)
    try:
        bits, missing = (token.split_contents(),)
    except ValueError:
        first, second = bits
        return template.Node(first, second)
    raise template.TemplateSyntaxError("unreachable")
"#;
    let result = extract_source(source, "failed_unpack_tags").expect("fixture should extract");
    let key = SymbolKey::tag("failed_unpack_tags", "checked");
    let rule = result
        .tag_rules
        .get(&key)
        .unwrap_or_else(|| panic!("missing checked rule: {:?}", result.tag_rules));
    assert_eq!(
        rule.arg_constraints,
        vec![ArgumentCountConstraint::Exact(3)]
    );
}

#[test]
fn failed_known_for_target_skips_the_body_and_reaches_value_error() {
    let source = r#"
from django import template
register = template.Library()
@register.tag
def checked(parser, token):
    bits = token.split_contents()
    left = bits[1]
    right = bits[2]
    try:
        for left, right in ((1,),):
            return template.Node(left, right)
    except ValueError:
        if len(bits) != 3:
            raise template.TemplateSyntaxError("expected two arguments")
        return template.Node()
    raise template.TemplateSyntaxError("unreachable")
"#;
    let result = extract_source(source, "failed_for_target_tags").expect("fixture should extract");
    let rule = &result.tag_rules[&SymbolKey::tag("failed_for_target_tags", "checked")];
    assert_eq!(
        rule.arg_constraints,
        vec![ArgumentCountConstraint::Exact(3)]
    );
}

#[test]
fn maybe_failing_for_target_keeps_body_and_handler_paths() {
    let source = r#"
from django import template
register = template.Library()
@register.tag
def checked(parser, token):
    bits = token.split_contents()
    try:
        for left, right in (runtime_value(),):
            if len(bits) != 2:
                raise template.TemplateSyntaxError("body count")
            return template.Node(left, right)
    except ValueError:
        if len(bits) != 3:
            raise template.TemplateSyntaxError("handler count")
        return template.Node()
    raise template.TemplateSyntaxError("unreachable")
"#;
    let result = extract_source(source, "maybe_for_target_tags").expect("fixture should extract");
    let rule = &result.tag_rules[&SymbolKey::tag("maybe_for_target_tags", "checked")];
    assert_eq!(
        rule.arg_constraints,
        vec![ArgumentCountConstraint::OneOf(vec![2, 3])]
    );
}

#[test]
fn split_result_for_value_keeps_its_unpack_arity_provenance() {
    let source = r#"
from django import template
register = template.Library()
@register.tag
def checked(parser, token):
    try:
        for tag_name, argument in (token.split_contents(),):
            return template.Node(argument)
    except ValueError:
        raise template.TemplateSyntaxError("expected one argument")
    raise template.TemplateSyntaxError("unreachable")
"#;
    let result = extract_source(source, "split_for_target_tags").expect("fixture should extract");
    let rule = &result.tag_rules[&SymbolKey::tag("split_for_target_tags", "checked")];
    assert_eq!(
        rule.arg_constraints,
        vec![ArgumentCountConstraint::Exact(2)]
    );
}

#[test]
fn opaque_call_before_known_unpack_reaches_each_matching_handler() {
    let source = r#"
from django import template
register = template.Library()
@register.tag
def checked(parser, token):
    bits = token.split_contents()
    try:
        callback()
        left, right = (1,)
    except IndexError:
        if len(bits) != 4:
            raise template.TemplateSyntaxError("index count")
        return template.Node()
    except ValueError:
        if len(bits) != 3:
            raise template.TemplateSyntaxError("value count")
        return template.Node()
    raise template.TemplateSyntaxError("unreachable")
"#;
    let result = extract_source(source, "implicit_raise_tags").expect("fixture should extract");
    let rule = &result.tag_rules[&SymbolKey::tag("implicit_raise_tags", "checked")];
    assert_eq!(
        rule.arg_constraints,
        vec![ArgumentCountConstraint::OneOf(vec![3, 4])]
    );
}

#[test]
fn rebound_token_method_call_retains_unknown_exception_edge() {
    let source = r#"
from django import template
register = template.Library()
@register.tag
def checked(parser, token):
    bits = token.split_contents()
    token = callback
    try:
        token.split_contents()
        left, right = (1,)
    except IndexError:
        if len(bits) != 4:
            raise template.TemplateSyntaxError("index count")
        return template.Node()
    except ValueError:
        if len(bits) != 3:
            raise template.TemplateSyntaxError("value count")
        return template.Node()
    raise template.TemplateSyntaxError("unreachable")
"#;
    let result = extract_source(source, "rebound_token_tags").expect("fixture should extract");
    let rule = &result.tag_rules[&SymbolKey::tag("rebound_token_tags", "checked")];
    assert_eq!(
        rule.arg_constraints,
        vec![ArgumentCountConstraint::OneOf(vec![3, 4])]
    );
}

#[test]
fn opaque_token_receiver_keeps_its_evaluation_exception() {
    let source = r#"
from django import template
register = template.Library()
@register.tag
def checked(parser, token):
    bits = token.split_contents()
    try:
        (callback(), token)[1].split_contents()
        left, right = (1,)
    except IndexError:
        if len(bits) != 4:
            raise template.TemplateSyntaxError("index count")
        return template.Node()
    except ValueError:
        if len(bits) != 3:
            raise template.TemplateSyntaxError("value count")
        return template.Node()
    raise template.TemplateSyntaxError("unreachable")
"#;
    let result = extract_source(source, "receiver_tags").expect("fixture should extract");
    let rule = &result.tag_rules[&SymbolKey::tag("receiver_tags", "checked")];
    assert_eq!(
        rule.arg_constraints,
        vec![ArgumentCountConstraint::OneOf(vec![3, 4])]
    );
}

#[test]
fn unpack_outcomes_respect_target_order_and_finally_override() {
    let source = r#"
from django import template
register = template.Library()

@register.tag
def ordered(parser, token):
    bits = token.split_contents()
    try:
        saved = first, second = bits
    except ValueError:
        if len(saved) == 3:
            return Node()
        raise template.TemplateSyntaxError("bad count")
    return Node(first, second)

@register.tag
def finalized(parser, token):
    bits = token.split_contents()
    try:
        tag, value = bits
    except ValueError:
        raise template.TemplateSyntaxError("bad count")
    finally:
        return Node()
"#;
    let result = extract_source(source, "unpack_control").expect("fixture should extract");
    for tag in ["ordered", "finalized"] {
        assert!(
            result
                .tag_rules
                .get(&SymbolKey::tag("unpack_control", tag))
                .is_none_or(|rule| {
                    !rule
                        .arg_constraints
                        .contains(&ArgumentCountConstraint::Exact(2))
                })
        );
    }
}

#[test]
fn proven_builtin_handlers_route_explicit_exceptions_in_order() {
    let source = r#"
from django import template
register = template.Library()
@register.tag
def ordered(parser, token):
    bits = token.split_contents()
    try:
        raise TypeError("wrong type")
    except ValueError:
        return Node()
    except TypeError:
        pass
    if len(bits) != 2:
        raise template.TemplateSyntaxError("bad")
    return Node()
"#;
    let result = extract_source(source, "ordered_handlers").expect("fixture should extract");
    let rule = &result.tag_rules[&SymbolKey::tag("ordered_handlers", "ordered")];
    assert_eq!(
        rule.arg_constraints,
        vec![ArgumentCountConstraint::Exact(2)]
    );
}

#[test]
fn wagtail_include_block_keeps_its_required_argument_name() {
    let source = r#"
from django import template
from django.template.defaulttags import token_kwargs
register = template.Library()

@register.tag
def include_block(parser, token):
    tokens = token.split_contents()
    try:
        tag_name = tokens.pop(0)
        block_var_token = tokens.pop(0)
    except IndexError:
        raise template.TemplateSyntaxError("requires one argument")
    block_var = parser.compile_filter(block_var_token)
    if tokens and tokens[0] == "with":
        tokens.pop(0)
        extra_context = token_kwargs(tokens, parser)
    else:
        extra_context = None
    if tokens and tokens[0] == "only":
        tokens.pop(0)
    if tokens:
        raise template.TemplateSyntaxError("unexpected argument")
    return Node(block_var, extra_context)
"#;
    let result = extract_source(source, "wagtail_tags").expect("fixture should extract");
    let rule = &result.tag_rules[&SymbolKey::tag("wagtail_tags", "include_block")];
    let parameters = rule
        .argument_syntax
        .parameters()
        .expect("parameter evidence");
    assert_eq!(parameters.len(), 1);
    assert_eq!(parameters[0].name, "block_var_token");
}

#[test]
fn compressor_constants_and_negated_membership_extract_typed_constraints() {
    let source = r#"
from django import template
register = template.Library()
OUTPUT_FILE = "file"
OUTPUT_INLINE = "inline"
OUTPUT_PRELOAD = "preload"
OUTPUT_MODES = (OUTPUT_FILE, OUTPUT_INLINE, OUTPUT_PRELOAD)

@register.tag
def compress(parser, token):
    args = token.split_contents()
    if not len(args) in (2, 3, 4):
        raise template.TemplateSyntaxError("wrong count")
    mode = args[2]
    if mode not in OUTPUT_MODES:
        raise template.TemplateSyntaxError("wrong mode")
"#;
    let result = extract_source(source, "compress_tags").expect("compress rule should extract");
    let rule = &result.tag_rules[&SymbolKey::tag("compress_tags", "compress")];
    assert_eq!(
        rule.arg_constraints,
        vec![ArgumentCountConstraint::OneOf(vec![2, 3, 4])]
    );
    assert_eq!(
        rule.choice_at_constraints,
        vec![ChoiceAt {
            position: djls_project::SplitPosition::Forward(2),
            values: vec![
                "file".to_string(),
                "inline".to_string(),
                "preload".to_string()
            ],
        }]
    );
}

#[test]
fn class_literal_dictionary_keys_extract_as_choices() {
    let source = r#"
from django import template
register = template.Library()
OPEN = "{%"
CLOSE = "%}"
class TagNode:
    mapping = {"open": OPEN, "close": CLOSE}

@register.tag
def templatetag(parser, token):
    bits = token.contents.split()
    tag = bits[1]
    if tag not in TagNode.mapping:
        raise template.TemplateSyntaxError("bad tag")
"#;
    let result = extract_source(source, "class_tags").expect("class choices should extract");
    let rule = &result.tag_rules[&SymbolKey::tag("class_tags", "templatetag")];
    assert_eq!(
        rule.choice_at_constraints,
        vec![ChoiceAt {
            position: djls_project::SplitPosition::Forward(1),
            values: vec!["open".to_string(), "close".to_string()],
        }]
    );
}

#[test]
fn static_choices_reject_rebinding_local_shadowing_and_mutation() {
    let cases = [
        r#"
VALUES = ("old", "other")
def compile_tag(parser, token):
    bits = token.split_contents()
    if bits[1] not in VALUES:
        raise SyntaxError("bad")
register.tag("checked", compile_tag)
VALUES = ("new",)
"#,
        r#"
VALUES = ("old", "other")
def compile_tag(parser, token):
    bits = token.split_contents()
    if bits[1] not in VALUES:
        raise SyntaxError("bad")
    VALUES = runtime_value()
register.tag("checked", compile_tag)
"#,
        r#"
VALUES = ["old", "other"]
VALUES.append("new")
def compile_tag(parser, token):
    bits = token.split_contents()
    if bits[1] not in VALUES:
        raise SyntaxError("bad")
register.tag("checked", compile_tag)
"#,
        r#"
VALUES = ("old", "other")
def helper(value=(VALUES := ("new",))):
    pass
def compile_tag(parser, token):
    bits = token.split_contents()
    if bits[1] not in VALUES:
        raise SyntaxError("bad")
register.tag("checked", compile_tag)
"#,
        r#"
VALUES = ("old", "other")
from config import VALUES
def compile_tag(parser, token):
    bits = token.split_contents()
    if bits[1] not in VALUES:
        raise SyntaxError("bad")
register.tag("checked", compile_tag)
"#,
    ];
    for declarations in cases {
        let source =
            format!("from django import template\nregister = template.Library()\n{declarations}");
        let result = extract_source(&source, "shadow_tags").expect("fixture should extract");
        let key = SymbolKey::tag("shadow_tags", "checked");
        assert!(
            result
                .tag_rules
                .get(&key)
                .is_none_or(|rule| rule.choice_at_constraints.is_empty()),
            "source:\n{source}"
        );
    }
}

#[test]
fn mutable_local_collections_do_not_become_persistent_exact_choices() {
    for (assignment, mutation) in [
        ("choices = ['a']", "choices.append('b')"),
        ("choices: list[str] = ['a']", "choices.append('b')"),
        ("choices, = (['a'],)", "choices.append('b')"),
        ("choices: set[str] = {'a'}", "choices.add('b')"),
    ] {
        let source = format!(
            r#"
from django import template
register = template.Library()
def compile_tag(parser, token):
    bits = token.split_contents()
    {assignment}
    {mutation}
    if bits[1] not in choices:
        raise SyntaxError("bad")
register.tag("mutable", compile_tag)

def direct_tag(parser, token):
    bits = token.split_contents()
    if bits[1] not in {{"a", "b"}}:
        raise SyntaxError("bad")
register.tag("direct", direct_tag)
"#
        );
        let result = extract_source(&source, "mutable_tags").expect("fixture should extract");
        assert!(
            result
                .tag_rules
                .get(&SymbolKey::tag("mutable_tags", "mutable"))
                .is_none_or(|rule| rule.choice_at_constraints.is_empty()),
            "source:\n{source}"
        );
        assert_eq!(
            result.tag_rules[&SymbolKey::tag("mutable_tags", "direct")].choice_at_constraints,
            vec![ChoiceAt {
                position: djls_project::SplitPosition::Forward(1),
                values: vec!["a".to_string(), "b".to_string()],
            }],
            "source:\n{source}"
        );
    }
}

#[test]
fn module_tuple_constants_preserve_repeated_positions() {
    let source = r#"
from django import template
register = template.Library()
VALUES = ("a", "a", "b")
def compile_tag(parser, token):
    bits = token.split_contents()
    first, second, third = VALUES
    if bits[1] not in (second,):
        raise SyntaxError("bad")
register.tag("checked", compile_tag)
"#;
    let result = extract_source(source, "tuple_tags").expect("fixture should extract");
    assert_eq!(
        result.tag_rules[&SymbolKey::tag("tuple_tags", "checked")].choice_at_constraints,
        vec![ChoiceAt {
            position: djls_project::SplitPosition::Forward(1),
            values: vec!["a".to_string()],
        }]
    );
}

#[test]
fn unresolved_star_import_only_invalidates_earlier_module_constants() {
    for (declarations, expected_values) in [
        (
            r#"
VALUES = ("old", "other")
from external import *
"#,
            Vec::new(),
        ),
        (
            r#"
from external import *
VALUES = ("new", "other")
"#,
            vec!["new".to_string(), "other".to_string()],
        ),
    ] {
        let source = format!(
            r#"
from django import template
register = template.Library()
{declarations}
def compile_tag(parser, token):
    bits = token.split_contents()
    if bits[1] not in VALUES:
        raise SyntaxError("bad")
register.tag("checked", compile_tag)
"#
        );
        let result = extract_source(&source, "star_tags").expect("fixture should extract");
        let choices = result
            .tag_rules
            .get(&SymbolKey::tag("star_tags", "checked"))
            .map_or(&[][..], |rule| rule.choice_at_constraints.as_slice());
        let expected = if expected_values.is_empty() {
            Vec::new()
        } else {
            vec![ChoiceAt {
                position: djls_project::SplitPosition::Forward(1),
                values: expected_values,
            }]
        };
        assert_eq!(choices, expected, "source:\n{source}");
    }
}

#[test]
fn shadowed_len_does_not_produce_argument_counts() {
    let cases = [
        r#"
len = runtime_length
def compile_tag(parser, token):
    bits = token.split_contents()
    if len(bits) != 2:
        raise SyntaxError("bad")
register.tag("checked", compile_tag)
"#,
        r#"
def compile_tag(parser, token):
    bits = token.split_contents()
    if len(bits) != 2:
        raise SyntaxError("bad")
    len = runtime_length
register.tag("checked", compile_tag)
"#,
        r#"
def compile_tag(parser, token):
    bits = token.split_contents()
    if len(bits) != 2:
        raise SyntaxError("bad")
register.tag("checked", compile_tag)
len = runtime_length
"#,
    ];

    for declarations in cases {
        let source =
            format!("from django import template\nregister = template.Library()\n{declarations}");
        let result = extract_source(&source, "shadowed_len_tags").expect("fixture should extract");
        let key = SymbolKey::tag("shadowed_len_tags", "checked");
        assert!(
            result
                .tag_rules
                .get(&key)
                .is_none_or(|rule| rule.arg_constraints.is_empty()),
            "source:\n{source}"
        );
    }
}

#[test]
fn shadowed_list_call_may_mutate_its_split_argument() {
    let cases = [
        r#"
list = mutate_argument
def compile_tag(parser, token):
    bits = token.split_contents()
    copied = list(bits)
    if len(copied) != 2:
        raise SyntaxError("bad")
    if len(bits) != 2:
        raise SyntaxError("bad")
register.tag("checked", compile_tag)
"#,
        r#"
def compile_tag(parser, token):
    bits = token.split_contents()
    list = mutate_argument
    copied = list(bits)
    if len(copied) != 2:
        raise SyntaxError("bad")
    if len(bits) != 2:
        raise SyntaxError("bad")
register.tag("checked", compile_tag)
"#,
        r#"
def compile_tag(parser, token):
    bits = token.split_contents()
    copied = list(bits)
    if len(copied) != 2:
        raise SyntaxError("bad")
    if len(bits) != 2:
        raise SyntaxError("bad")
register.tag("checked", compile_tag)
list = mutate_argument
"#,
    ];

    for declarations in cases {
        let source =
            format!("from django import template\nregister = template.Library()\n{declarations}");
        let result = extract_source(&source, "shadowed_list_tags").expect("fixture should extract");
        let key = SymbolKey::tag("shadowed_list_tags", "checked");
        assert!(
            result
                .tag_rules
                .get(&key)
                .is_none_or(|rule| rule.arg_constraints.is_empty()),
            "source:\n{source}"
        );
    }
}

#[test]
fn proven_builtin_list_preserves_split_argument_evidence() {
    let source = r#"
from django import template
register = template.Library()
def copied_tag(parser, token):
    bits = token.split_contents()
    copied = list(bits)
    if len(copied) != 2:
        raise SyntaxError("bad")
register.tag("copied", copied_tag)

def preserved_tag(parser, token):
    bits = token.split_contents()
    list(bits)
    if len(bits) != 2:
        raise SyntaxError("bad")
register.tag("preserved", preserved_tag)
"#;

    let result = extract_source(source, "builtin_list_tags").expect("fixture should extract");
    for tag in ["copied", "preserved"] {
        assert_eq!(
            result.tag_rules[&SymbolKey::tag("builtin_list_tags", tag)].arg_constraints,
            vec![ArgumentCountConstraint::Exact(2)],
            "tag: {tag}"
        );
    }
}

#[test]
fn module_constants_do_not_replace_known_compile_parameters() {
    let source = r#"
from django import template
register = template.Library()
parser = "module parser"
token = "module token"
def compile_tag(parser, token):
    bits = token.split_contents()
    if len(bits) != 2:
        raise SyntaxError("bad")
register.tag("checked", compile_tag)
"#;
    let result = extract_source(source, "parameter_tags").expect("fixture should extract");
    assert_eq!(
        result.tag_rules[&SymbolKey::tag("parameter_tags", "checked")].arg_constraints,
        vec![ArgumentCountConstraint::Exact(2)]
    );
}

#[test]
fn static_choices_reject_nested_global_and_function_scope_bindings() {
    let cases = [
        r#"
VALUES = ("old", "other")
def replace():
    global VALUES
    VALUES = ("new",)
def compile_tag(parser, token):
    bits = token.split_contents()
    if bits[1] not in VALUES:
        raise SyntaxError("bad")
register.tag("checked", compile_tag)
"#,
        r#"
VALUES = ("old", "other")
def compile_tag(parser, token):
    bits = token.split_contents()
    if bits[1] not in VALUES:
        raise SyntaxError("bad")
    import config as VALUES
register.tag("checked", compile_tag)
"#,
        r#"
package = ("old", "other")
def replace():
    global package
    import package.child
def compile_tag(parser, token):
    bits = token.split_contents()
    if bits[1] not in package:
        raise SyntaxError("bad")
register.tag("checked", compile_tag)
"#,
        r#"
VALUES = ("old", "other")
def compile_tag(parser, token):
    bits = token.split_contents()
    if bits[1] not in VALUES:
        raise SyntaxError("bad")
    if (VALUES := runtime_value()):
        pass
register.tag("checked", compile_tag)
"#,
        r#"
VALUES = ("old", "other")
def compile_tag(parser, token):
    bits = token.split_contents()
    if bits[1] not in VALUES:
        raise SyntaxError("bad")
    try:
        runtime_call()
    except RuntimeError as VALUES:
        pass
register.tag("checked", compile_tag)
"#,
        r#"
VALUES = ("old", "other")
def compile_tag(parser, token):
    bits = token.split_contents()
    if bits[1] not in VALUES:
        raise SyntaxError("bad")
    match runtime_value():
        case [*VALUES]:
            pass
register.tag("checked", compile_tag)
"#,
    ];
    for declarations in cases {
        let source =
            format!("from django import template\nregister = template.Library()\n{declarations}");
        let result = extract_source(&source, "scope_tags").expect("fixture should extract");
        let key = SymbolKey::tag("scope_tags", "checked");
        assert!(
            result
                .tag_rules
                .get(&key)
                .is_none_or(|rule| rule.choice_at_constraints.is_empty()),
            "source:\n{source}"
        );
    }
}

#[test]
fn unrelated_nested_writes_and_read_only_global_declarations_keep_tuple_choices() {
    let source = r#"
from django import template
register = template.Library()
VALUES = ("old", "other")
UNRELATED = ("before",)
def helper():
    global VALUES, UNRELATED
    UNRELATED = ("after",)
    return VALUES

def compile_tag(parser, token):
    global VALUES
    bits = token.split_contents()
    if bits[1] not in VALUES:
        raise SyntaxError("bad")
register.tag("checked", compile_tag)
"#;
    let result = extract_source(source, "global_tags").expect("fixture should extract");
    let rule = &result.tag_rules[&SymbolKey::tag("global_tags", "checked")];
    assert_eq!(rule.choice_at_constraints[0].values, ["old", "other"]);
}

#[test]
fn class_choices_reject_mutations_calls_and_dynamic_keys() {
    let cases = [
        r#"
class TagNode:
    mapping = {"open": VALUE}
TagNode.mapping["close"] = VALUE
"#,
        r#"
class TagNode:
    mapping = {"open": make_value()}
"#,
        r"
class TagNode:
    mapping = {dynamic_key: VALUE}
",
        r#"
class TagNode:
    mapping = {"open": VALUE}
TagNode = replacement
"#,
        r#"
class TagNode:
    mapping = {"open": VALUE}
alias = TagNode.mapping
"#,
        r#"
class TagNode:
    mapping = {"open": VALUE}
consume(TagNode.mapping)
"#,
        r#"
class TagNode(Base, metaclass=Meta):
    mapping = {"open": VALUE}
"#,
        r#"
class TagNode:
    mapping = {"open": VALUE}
    alias = mapping
    alias["close"] = VALUE
"#,
        r#"
class TagNode:
    mapping = {"open": VALUE}
Alias = TagNode
Alias.mapping["close"] = VALUE
"#,
        r#"
class TagNode:
    mapping = {"open": VALUE}
consume([TagNode])
"#,
        r#"
class TagNode:
    mapping = {"open": VALUE}
def expose():
    return {"node": TagNode}
"#,
        r#"
@decorate
class TagNode:
    mapping = {"open": VALUE}
"#,
        r#"
class TagNode(Base, **OPTIONS):
    mapping = {"open": VALUE}
"#,
        r#"
class TagNode:
    mapping = {"open": VALUE}
    for mapping in values:
        pass
"#,
        r#"
class TagNode:
    mapping = {"open": VALUE}
    import package.mapping as mapping
"#,
    ];
    for declarations in cases {
        let source = format!(
            "from django import template\nregister = template.Library()\n{declarations}\n\ndef compile_tag(parser, token):\n    bits = token.split_contents()\n    if bits[1] not in TagNode.mapping:\n        raise SyntaxError('bad')\nregister.tag('checked', compile_tag)\n"
        );
        let result =
            extract_source(&source, "class_negative_tags").expect("fixture should extract");
        let key = SymbolKey::tag("class_negative_tags", "checked");
        assert!(
            result
                .tag_rules
                .get(&key)
                .is_none_or(|rule| rule.choice_at_constraints.is_empty()),
            "source:\n{source}"
        );
    }
}

#[test]
fn extract_bundle_empty_source() {
    let result = extract_source("", "test.module").expect("empty extraction fixture should build");
    assert!(result.is_empty());
}

// (b) Edge case — invalid Python returns empty result
#[test]
fn extract_bundle_invalid_python() {
    let result = extract_source("def {invalid python", "test.module")
        .expect("invalid-Python extraction fixture should build");
    assert!(result.is_empty());
}

#[test]
fn recovered_syntax_retains_tag_block_and_filter_facts_with_error_span() {
    let source = r#"from django import template
register = template.Library()

@register.filter
def known_filter(value, arg):
    return value

@register.tag("known_tag")
def do_known(parser, token):
    bits = token.split_contents()
    if len(bits) != 1:
        raise template.TemplateSyntaxError("expected no arguments")
    nodelist = parser.parse(("endknown_tag",))
    parser.delete_first_token()
    return nodelist

def broken("#;
    let db = TestDatabase::new();
    let path = Utf8Path::new("/test/templatetags/known.py");
    db.add_file(path.as_str(), source)
        .expect("recovered Python fixture should be added to the test database");
    let file = db
        .file(path)
        .expect("recovered Python fixture should exist in the test database");
    let module_name = PythonModuleName::parse("test.templatetags.known")
        .expect("test Python module name should be valid");

    let result = extract_bundle(&db, file, module_name);
    let filter = SymbolKey::filter("test.templatetags.known", "known_filter");
    let tag = SymbolKey::tag("test.templatetags.known", "known_tag");
    assert!(result.filter_arities.contains_key(&filter));
    assert!(result.tag_rules.contains_key(&tag));
    assert_eq!(
        result.block_specs.as_map()[&tag].end_tag.as_deref(),
        Some("endknown_tag")
    );

    let errors = python_syntax_errors(&db, file).expect("file should be Python");
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].class, PythonSyntaxErrorClass::Ordinary);
    assert_eq!(
        errors[0].span,
        Span::new(
            u32::try_from(source.len()).expect("test source length should fit in a span offset"),
            0
        )
    );
    assert!(!errors[0].message.is_empty());
}

#[test]
fn parser_distinguishes_empty_python_from_non_python() {
    let db = TestDatabase::new();
    db.add_file("/test/empty.py", "")
        .expect("empty Python fixture should be added to the test database");
    db.add_file("/test/notes.txt", "")
        .expect("text fixture should be added to the test database");

    assert_eq!(
        python_syntax_errors(
            &db,
            db.file(Utf8Path::new("/test/empty.py"))
                .expect("empty Python fixture should exist in the test database"),
        ),
        Some(Vec::new())
    );
    assert_eq!(
        python_syntax_errors(
            &db,
            db.file(Utf8Path::new("/test/notes.txt"))
                .expect("text fixture should exist in the test database"),
        ),
        None
    );
}

#[test]
fn template_symbol_source_separates_definition_identity_from_location() {
    let db = TestDatabase::new();
    let path = Utf8Path::new("/test/templatetags/navigation.py");
    let source = "from django import template\nregister = template.Library()\n@register.simple_tag(name='shown')\ndef implementation(value):\n    return value\n";
    db.add_file(path.as_str(), source)
        .expect("template-tag fixture should be added to the test database");
    let file = db
        .file(path)
        .expect("template-tag fixture should exist in the test database");
    let key = TemplateLibraryId::new(
        &db,
        Some(file),
        PythonModuleName::parse("test.templatetags.navigation")
            .expect("test Python module name should be valid"),
    );
    let symbol = template_library_definition_facts(&db, key)
        .symbol(TemplateSymbolKind::Tag, "shown")
        .expect("registered Tag should be extracted");
    let definition = symbol.definition.clone();
    let source_location =
        template_symbol_source(&db, symbol).expect("local declaration should be navigable");

    assert_eq!(source_location.file(), file);
    assert_eq!(
        source.get(
            source_location.definition_span().start_usize()
                ..source_location.definition_span().end_usize()
        ),
        Some("@register.simple_tag(name='shown')\ndef implementation(value):\n    return value")
    );
    assert_eq!(
        source.get(
            source_location.name_span().start_usize()..source_location.name_span().end_usize()
        ),
        Some("implementation")
    );
    assert!(source_location.definition_span().start() <= source_location.name_span().start());
    assert!(source_location.name_span().end() <= source_location.definition_span().end());
    assert_eq!(symbol.definition, definition);
}

#[test]
fn template_symbol_location_shift_backdates_semantic_products() {
    let event_log = SalsaEventLog::default();
    let mut db = TestDatabase::with_event_log(event_log.clone());
    let path = Utf8Path::new("/test/templatetags/navigation.py");
    let source = "from django import template\nregister = template.Library()\n@register.simple_tag(name='shown')\ndef implementation(value):\n    return value\n@register.filter(name='filtered')\ndef filtering(value):\n    return value\n";
    db.add_file(path.as_str(), source)
        .expect("template-tag fixture should be added to the test database");
    let file = db
        .file(path)
        .expect("template-tag fixture should exist in the test database");
    let module = PythonModuleName::parse("test.templatetags.navigation")
        .expect("test Python module name should be valid");
    let (definitions_before, tag_facts_before, filter_facts_before, source_before) = {
        let key = TemplateLibraryId::new(&db, Some(file), module.clone());
        let definition_snapshot = template_library_definition_facts_snapshot(&db, key);
        let definitions = template_library_definition_facts(&db, key);
        let tag_facts = template_library_tag_facts(&db, key).clone();
        let filter_facts = template_library_filter_facts(&db, key).clone();
        let symbol = definitions
            .symbol(TemplateSymbolKind::Tag, "shown")
            .expect("registered Tag should be extracted");
        let source =
            template_symbol_source(&db, symbol).expect("local declaration should be navigable");
        (definition_snapshot, tag_facts, filter_facts, source)
    };
    drop(
        event_log
            .take()
            .expect("Salsa event log should be readable before the fixture edit"),
    );

    db.add_file(path.as_str(), &format!("\n{source}"))
        .expect("shifted template-tag fixture should be added to the test database");
    SourceChanges::new([ChangeEvent::ContentChanged(path.to_path_buf())]).apply(&mut db);

    let key = TemplateLibraryId::new(&db, Some(file), module);
    let definition_snapshot_after = template_library_definition_facts_snapshot(&db, key);
    let definitions_after = template_library_definition_facts(&db, key);
    let tag_facts_after = template_library_tag_facts(&db, key);
    let filter_facts_after = template_library_filter_facts(&db, key);
    let symbol_after = definitions_after
        .symbol(TemplateSymbolKind::Tag, "shown")
        .expect("shifted registered Tag should be extracted");
    let source_after =
        template_symbol_source(&db, symbol_after).expect("shifted declaration should navigate");

    assert_eq!(definition_snapshot_after, definitions_before);
    assert_eq!(tag_facts_after, &tag_facts_before);
    assert_eq!(filter_facts_after, &filter_facts_before);
    assert_eq!(
        source_after.definition_span().start(),
        source_before.definition_span().start() + 1
    );
    assert_eq!(
        source_after.name_span().start(),
        source_before.name_span().start() + 1
    );

    let events = event_log
        .take()
        .expect("Salsa event log should be readable after the fixture edit");
    assert_eq!(
        execution_count(&db, &events, "template_library_source_analysis"),
        1
    );
    assert_eq!(
        execution_count(&db, &events, "template_library_definition_facts"),
        1
    );
    assert_eq!(
        execution_count(&db, &events, "template_library_tag_facts"),
        1
    );
    assert_eq!(
        execution_count(&db, &events, "template_library_filter_facts"),
        1
    );
    assert_eq!(
        execution_count(&db, &events, "template_library_symbol_sources"),
        1
    );
}

#[test]
fn template_symbol_source_rejects_open_registration_inventory() {
    let db = TestDatabase::new();
    let path = Utf8Path::new("/test/templatetags/open.py");
    let source = "from django import template\nregister = template.Library()\ndef first(parser, token):\n    pass\nregister.tag('shown', first)\nif FLAG:\n    register.tag('shown', replacement)\n";
    db.add_file(path.as_str(), source)
        .expect("template-tag fixture should be added to the test database");
    let file = db
        .file(path)
        .expect("template-tag fixture should exist in the test database");
    let key = TemplateLibraryId::new(
        &db,
        Some(file),
        PythonModuleName::parse("test.templatetags.open")
            .expect("test Python module name should be valid"),
    );
    let symbol = template_library_definition_facts(&db, key)
        .symbol(TemplateSymbolKind::Tag, "shown")
        .expect("known registration should survive the open inventory");

    assert_eq!(template_symbol_source(&db, symbol), None);
}

#[test]
fn template_symbol_source_resolves_a_preceding_plain_function() {
    let db = TestDatabase::new();
    let path = Utf8Path::new("/test/templatetags/direct.py");
    let source = "from django import template\nregister = template.Library()\ndef implementation(parser, token):\n    pass\nregister.tag('direct', implementation)\n";
    db.add_file(path.as_str(), source)
        .expect("direct-registration fixture should be added to the test database");
    let file = db
        .file(path)
        .expect("direct-registration fixture should exist in the test database");
    let key = TemplateLibraryId::new(
        &db,
        Some(file),
        PythonModuleName::parse("test.templatetags.direct")
            .expect("test Python module name should be valid"),
    );
    let symbol = template_library_definition_facts(&db, key)
        .symbol(TemplateSymbolKind::Tag, "direct")
        .expect("direct registration should be extracted");
    let location = template_symbol_source(&db, symbol)
        .expect("the preceding plain function should be navigable");

    assert_eq!(location.file(), file);
    assert_eq!(
        &source[location.name_span().start_usize()..location.name_span().end_usize()],
        "implementation"
    );
}

#[test]
fn named_expression_rebinding_invalidates_later_python_function_facts() {
    let db = TestDatabase::new();
    let path = Utf8Path::new("/test/templatetags/named.py");
    let source = "from django import template\nregister = template.Library()\ndef first(parser, token): pass\ndef second(parser, token): pass\n(first := second)\nregister.tag(first)\n";
    db.add_file(path.as_str(), source)
        .expect("named-expression fixture should be added to the test database");
    let file = db
        .file(path)
        .expect("named-expression fixture should exist in the test database");
    let key = TemplateLibraryId::new(
        &db,
        Some(file),
        PythonModuleName::parse("test.templatetags.named")
            .expect("test Python module name should be valid"),
    );
    assert!(
        template_library_definition_facts(&db, key)
            .symbol(TemplateSymbolKind::Tag, "first")
            .is_none(),
        "a callable-derived name must not fall back to source spelling"
    );
    assert!(template_library_tag_facts(&db, key).tag_rules().is_empty());
}

#[test]
fn template_symbol_source_rejects_member_callable() {
    let db = TestDatabase::new();
    let path = Utf8Path::new("/test/templatetags/member.py");
    let source = "from django import template\nregister = template.Library()\ndef first(parser, token):\n    pass\nregister.tag('member', first)\nclass Node:\n    def handle(self, parser, token):\n        pass\nregister.tag('member', Node.handle)\n";
    db.add_file(path.as_str(), source)
        .expect("template-tag fixture should be added to the test database");
    let file = db
        .file(path)
        .expect("template-tag fixture should exist in the test database");
    let key = TemplateLibraryId::new(
        &db,
        Some(file),
        PythonModuleName::parse("test.templatetags.member")
            .expect("test Python module name should be valid"),
    );
    let symbol = template_library_definition_facts(&db, key)
        .symbol(TemplateSymbolKind::Tag, "member")
        .expect("member registration should remain a known Tag Definition");

    assert_eq!(template_symbol_source(&db, symbol), None);
}

#[test]
fn later_unresolved_callable_clears_an_overwritten_tag_rule() {
    let db = TestDatabase::new();
    let path = Utf8Path::new("/test/templatetags/overwritten.py");
    let source = "from django import template\nregister = template.Library()\ndef first(parser, token):\n    bits = token.split_contents()\n    if len(bits) != 2: raise ValueError()\nclass Node:\n    def handle(self, parser, token): pass\nregister.tag('shown', first)\nregister.tag('shown', Node.handle)\n";
    db.add_file(path.as_str(), source)
        .expect("overwritten-registration fixture should be added");
    let file = db
        .file(path)
        .expect("overwritten-registration fixture should exist");
    let key = TemplateLibraryId::new(
        &db,
        Some(file),
        PythonModuleName::parse("test.templatetags.overwritten")
            .expect("test Python module name should be valid"),
    );

    assert!(
        template_library_definition_facts(&db, key)
            .symbol(TemplateSymbolKind::Tag, "shown")
            .is_some()
    );
    assert!(
        !template_library_tag_facts(&db, key)
            .tag_rules()
            .contains_key(&SymbolKey::tag("test.templatetags.overwritten", "shown"))
    );
}

#[test]
fn explicit_names_survive_keyword_member_callables() {
    let db = TestDatabase::new();
    let path = Utf8Path::new("/test/templatetags/keyword_member.py");
    let source = "from django import template\nregister = template.Library()\nclass Node:\n    def handle(self, parser, token): pass\nregister.tag('known_tag', compile_function=Node.handle)\nregister.filter('known_filter', filter_func=Node.handle)\n";
    db.add_file(path.as_str(), source)
        .expect("keyword-member fixture should be added");
    let file = db.file(path).expect("keyword-member fixture should exist");
    let key = TemplateLibraryId::new(
        &db,
        Some(file),
        PythonModuleName::parse("test.templatetags.keyword_member")
            .expect("test Python module name should be valid"),
    );
    let facts = template_library_definition_facts(&db, key);

    assert!(facts.symbol(TemplateSymbolKind::Tag, "known_tag").is_some());
    assert!(
        facts
            .symbol(TemplateSymbolKind::Filter, "known_filter")
            .is_some()
    );
}

#[test]
fn comment_only_edit_backdates_parsed_body_consumers() {
    let event_log = SalsaEventLog::default();
    let mut db = TestDatabase::with_event_log(event_log.clone());
    let path = Utf8Path::new("/test/templatetags/known.py");
    let source = "from django import template\nregister = template.Library()\n@register.simple_tag\ndef known():\n    return 'known'\n";
    db.add_file(path.as_str(), source)
        .expect("template-tag fixture should be added to the test database");
    let file = db
        .file(path)
        .expect("template-tag fixture should exist in the test database");
    let module_name = PythonModuleName::parse("test.templatetags.known")
        .expect("test Python module name should be valid");

    {
        let key = TemplateLibraryId::new(&db, Some(file), module_name.clone());
        assert!(!template_library_tag_facts(&db, key).tag_rules().is_empty());
    }
    drop(
        event_log
            .take()
            .expect("Salsa event log should be readable before the fixture edit"),
    );

    db.add_file(path.as_str(), &format!("{source}# comment only\n"))
        .expect("updated template-tag fixture should be added to the test database");
    SourceChanges::new([ChangeEvent::ContentChanged(path.to_path_buf())]).apply(&mut db);

    let key = TemplateLibraryId::new(&db, Some(file), module_name);
    assert!(!template_library_tag_facts(&db, key).tag_rules().is_empty());
    let events = event_log
        .take()
        .expect("Salsa event log should be readable after the fixture edit");
    assert_eq!(execution_count(&db, &events, "parse_python_file"), 1);
    assert_eq!(
        execution_count(&db, &events, "template_library_tag_facts"),
        0
    );
}

#[test]
fn template_library_extraction_products_execute_once_and_share_parsing() {
    let event_log = SalsaEventLog::default();
    let db = TestDatabase::with_event_log(event_log.clone());

    db.add_file("/test/defaulttags.py", DEFAULTTAGS_SOURCE)
        .expect("default-tags fixture should be added to the test database");
    let tags_file = db
        .file(Utf8Path::new("/test/defaulttags.py"))
        .expect("default-tags fixture should exist in the test database");
    let tags_module = PythonModuleName::parse("django.template.defaulttags")
        .expect("test Python module name should be valid");
    let tags_key = TemplateLibraryId::new(&db, Some(tags_file), tags_module);
    let facts = template_library_definition_facts(&db, tags_key);
    assert!(facts.is_library());
    assert!(facts.symbol(TemplateSymbolKind::Tag, "for").is_some());
    assert!(facts.symbol(TemplateSymbolKind::Filter, "for").is_none());
    let tag_facts = template_library_tag_facts(&db, tags_key);
    assert!(
        tag_facts.tag_rules().keys().any(
            |key| key.name == "for" && key.registration_module == "django.template.defaulttags"
        )
    );
    assert!(
        tag_facts
            .block_specs()
            .as_map()
            .keys()
            .any(|key| key.name == "for")
    );

    let events = event_log
        .take()
        .expect("Salsa event log should be readable after Tag facts are queried");
    assert_eq!(execution_count(&db, &events, "parse_python_file"), 1);
    assert_eq!(
        execution_count(&db, &events, "template_library_source_analysis"),
        1,
        "definitions, Tag Rules, and Block Specs must share one registration analysis",
    );
    assert_eq!(
        execution_count(&db, &events, "template_library_definition_facts"),
        1
    );
    assert_eq!(
        execution_count(&db, &events, "template_library_tag_facts"),
        1
    );

    db.add_file("/test/defaultfilters.py", DEFAULTFILTERS_SOURCE)
        .expect("default-filters fixture should be added to the test database");
    let filters_file = db
        .file(Utf8Path::new("/test/defaultfilters.py"))
        .expect("default-filters fixture should exist in the test database");
    let filters_key = TemplateLibraryId::new(
        &db,
        Some(filters_file),
        PythonModuleName::parse("django.template.defaultfilters")
            .expect("test Python module name should be valid"),
    );
    let filters = template_library_filter_facts(&db, filters_key);
    assert!(
        filters
            .filter_arities()
            .keys()
            .any(|key| key.name == "lower"
                && key.registration_module == "django.template.defaultfilters")
    );

    let events = event_log
        .take()
        .expect("Salsa event log should be readable after Filter facts are queried");
    assert_eq!(execution_count(&db, &events, "parse_python_file"), 1);
    assert_eq!(
        execution_count(&db, &events, "template_library_source_analysis"),
        1,
    );
    assert_eq!(
        execution_count(&db, &events, "template_library_filter_facts"),
        1
    );

    let _ = template_library_filter_facts(&db, filters_key);
    assert_eq!(
        execution_count(
            &db,
            &event_log
                .take()
                .expect("Salsa event log should be readable after repeated Filter queries"),
            "template_library_filter_facts",
        ),
        0,
        "same-revision extraction should be memoized",
    );
}

#[test]
fn locked_sentry_asset_helpers_resolve_through_project_backed_imports() {
    let corpus = Corpus::require().expect("synced corpus should be available for corpus tests");
    let sentry_root = corpus.root().join("repos/sentry/src/sentry");
    let registration_source = fs::read_to_string(
        sentry_root
            .join("templatetags/sentry_assets.py")
            .as_std_path(),
    )
    .expect("locked Sentry Template Library source should be readable");
    let helper_source = fs::read_to_string(sentry_root.join("utils/assets.py").as_std_path())
        .expect("locked Sentry asset helper source should be readable");

    let mut db = TestDatabase::new();
    ProjectFixture::new("/test/project")
        .django_settings_module("settings")
        .file("/test/project/settings.py", "INSTALLED_APPS = []\n")
        .file("/test/project/sentry/__init__.py", "")
        .file("/test/project/sentry/templatetags/__init__.py", "")
        .file(
            "/test/project/sentry/templatetags/sentry_assets.py",
            &registration_source,
        )
        .file("/test/project/sentry/utils/__init__.py", "")
        .file("/test/project/sentry/utils/assets.py", &helper_source)
        .install(&mut db)
        .expect("locked Sentry source fixture should install");

    let registration_file = db
        .file(Utf8Path::new(
            "/test/project/sentry/templatetags/sentry_assets.py",
        ))
        .expect("Sentry registration source should exist");
    let library = TemplateLibraryId::new(
        &db,
        Some(registration_file),
        PythonModuleName::parse("sentry.templatetags.sentry_assets")
            .expect("Sentry Template Library module should be valid"),
    );
    let definitions = template_library_definition_facts(&db, library);
    let tag_facts = template_library_tag_facts(&db, library);

    for (name, function_name) in [
        ("asset_url", "get_asset_url"),
        ("frontend_app_asset_url", "get_frontend_app_asset_url"),
    ] {
        let symbol = definitions
            .symbol(TemplateSymbolKind::Tag, name)
            .unwrap_or_else(|| panic!("imported Sentry Tag `{name}` should be registered"));
        let source = template_symbol_source(&db, symbol).unwrap_or_else(|| {
            panic!("imported Sentry Tag `{name}` should retain source identity")
        });
        assert_eq!(
            source.file().path(&db),
            Utf8Path::new("/test/project/sentry/utils/assets.py")
        );
        assert_eq!(
            &helper_source[source.name_span().start_usize()..source.name_span().end_usize()],
            function_name
        );
        assert!(matches!(
            tag_facts.tag_rules()[&SymbolKey::tag("sentry.templatetags.sentry_assets", name)]
                .argument_syntax,
            TagArgumentSyntax::Signature { ref parameters, .. }
                if parameters.len() == 2
                    && parameters.iter().all(|parameter| parameter.requirement.is_required())
        ));
    }
}

// This small in-memory fixture complements the locked Sentry case above. It isolates source
// identity and invalidation behavior without depending on unrelated imports in the real project.
fn imported_registration_fixture(
    package_init: &str,
    registration_source: &str,
    implementation_source: &str,
) -> Result<(TestDatabase, File, PythonModuleName), String> {
    let mut db = TestDatabase::new();
    ProjectFixture::new("/test/project")
        .django_settings_module("settings")
        .file("/test/project/settings.py", "INSTALLED_APPS = []\n")
        .file("/test/project/pkg/__init__.py", package_init)
        .file("/test/project/pkg/tags.py", registration_source)
        .file("/test/project/pkg/implementation.py", implementation_source)
        .install(&mut db)
        .map_err(|error| error.to_string())?;
    let file = db
        .file(Utf8Path::new("/test/project/pkg/tags.py"))
        .map_err(|error| error.to_string())?;
    let module = PythonModuleName::parse("pkg.tags").map_err(|error| error.to_string())?;
    Ok((db, file, module))
}

#[test]
fn from_import_prefers_an_exact_package_member_over_a_same_named_child() {
    let (db, file, module) = imported_registration_fixture(
        "implementation = 'not the child module'\n",
        "from django import template\nfrom . import implementation\nregister = template.Library()\nregister.tag(implementation.TAG, implementation.compile_tag)\n",
        "TAG = 'child_tag'\ndef compile_tag(parser, token): pass\n",
    )
    .expect("member-precedence fixture should install");
    let key = TemplateLibraryId::new(&db, Some(file), module);

    let facts = template_library_definition_facts(&db, key);
    assert!(facts.symbol(TemplateSymbolKind::Tag, "child_tag").is_none());
}

#[test]
fn from_import_does_not_bypass_package_getattr() {
    let (db, file, module) = imported_registration_fixture(
        "def __getattr__(name): return dynamic_member\n",
        "from django import template\nfrom . import implementation\nregister = template.Library()\nregister.tag(implementation.TAG, implementation.compile_tag)\n",
        "TAG = 'child_tag'\ndef compile_tag(parser, token): pass\n",
    )
    .expect("package-getattr fixture should install");
    let key = TemplateLibraryId::new(&db, Some(file), module);

    assert!(
        template_library_definition_facts(&db, key)
            .symbol(TemplateSymbolKind::Tag, "child_tag")
            .is_none()
    );
}

#[test]
fn from_import_resolves_a_namespace_package_sibling() {
    let mut db = TestDatabase::new();
    ProjectFixture::new("/test/project")
        .django_settings_module("settings")
        .file("/test/project/settings.py", "INSTALLED_APPS = []\n")
        .file(
            "/test/project/pkg/tags.py",
            "from django import template\nfrom . import implementation\nregister = template.Library()\nregister.tag(implementation.TAG, implementation.compile_tag)\n",
        )
        .file(
            "/test/project/pkg/implementation.py",
            "TAG = 'namespace_tag'\ndef compile_tag(parser, token): pass\n",
        )
        .install(&mut db)
        .expect("namespace-package fixture should install");
    let file = db
        .file(Utf8Path::new("/test/project/pkg/tags.py"))
        .expect("namespace registration source should exist");
    let key = TemplateLibraryId::new(
        &db,
        Some(file),
        PythonModuleName::parse("pkg.tags").expect("namespace module name should be valid"),
    );

    assert!(
        template_library_definition_facts(&db, key)
            .symbol(TemplateSymbolKind::Tag, "namespace_tag")
            .is_some()
    );
}

#[test]
fn reading_imported_module_members_through_an_unrelated_call_keeps_resolution_exact() {
    let (db, file, module) = imported_registration_fixture(
        "",
        "from django import template\nfrom . import implementation\nregister = template.Library()\nconsume([implementation.TAG, implementation.compile_tag])\nregister.tag(implementation.TAG, implementation.compile_tag)\n",
        "TAG = 'member_reads'\ndef compile_tag(parser, token): pass\n",
    )
    .expect("module-member-read fixture should install");
    let key = TemplateLibraryId::new(&db, Some(file), module);

    assert!(
        template_library_definition_facts(&db, key)
            .symbol(TemplateSymbolKind::Tag, "member_reads")
            .is_some()
    );
}

#[test]
fn unused_lazy_from_import_does_not_open_registration_evidence() {
    let mut db = TestDatabase::new();
    ProjectFixture::new("/test/project")
        .django_settings_module("settings")
        .file("/test/project/settings.py", "INSTALLED_APPS = []\n")
        .file("/test/project/pkg/__init__.py", "")
        .file(
            "/test/project/pkg/tags.py",
            "from django import template\nfrom . import implementation\nfrom .unrelated import UNUSED\nregister = template.Library()\nignored = UNUSED\nregister.tag(implementation.TAG, implementation.compile_tag)\n",
        )
        .file(
            "/test/project/pkg/implementation.py",
            "TAG = 'focused'\ndef compile_tag(parser, token): pass\n",
        )
        .file(
            "/test/project/pkg/unrelated.py",
            "UNUSED = 'ignored'\ndef broken(\n",
        )
        .install(&mut db)
        .expect("focused-occurrence fixture should install");
    let file = db
        .file(Utf8Path::new("/test/project/pkg/tags.py"))
        .expect("focused-occurrence registration source should exist");
    let key = TemplateLibraryId::new(
        &db,
        Some(file),
        PythonModuleName::parse("pkg.tags").expect("fixture module name should be valid"),
    );
    let symbol = template_library_definition_facts(&db, key)
        .symbol(TemplateSymbolKind::Tag, "focused")
        .expect("the exact registration should survive the unrelated recovered read");

    assert!(template_symbol_source(&db, symbol).is_some());
}

#[test]
fn invoking_an_imported_module_member_invalidates_resolution() {
    let (db, file, module) = imported_registration_fixture(
        "",
        "from django import template\nfrom . import implementation\nregister = template.Library()\nimplementation.configure()\nregister.tag(implementation.TAG, implementation.compile_tag)\n",
        "TAG = 'member_call'\ndef configure(): pass\ndef compile_tag(parser, token): pass\n",
    )
    .expect("module-member-call fixture should install");
    let key = TemplateLibraryId::new(&db, Some(file), module);

    assert!(
        template_library_definition_facts(&db, key)
            .symbol(TemplateSymbolKind::Tag, "member_call")
            .is_none()
    );
}

#[test]
fn invoking_an_alias_of_an_imported_module_member_invalidates_resolution() {
    let (db, file, module) = imported_registration_fixture(
        "",
        "from django import template\nfrom . import implementation\nregister = template.Library()\nhook = implementation.configure\nhook()\nregister.tag(implementation.TAG, implementation.compile_tag)\n",
        "TAG = 'aliased_member_call'\ndef configure(): pass\ndef compile_tag(parser, token): pass\n",
    )
    .expect("aliased module-member-call fixture should install");
    let key = TemplateLibraryId::new(&db, Some(file), module);

    assert!(
        template_library_definition_facts(&db, key)
            .symbol(TemplateSymbolKind::Tag, "aliased_member_call")
            .is_none()
    );
}

#[test]
fn invoking_an_indirect_imported_module_callee_invalidates_resolution() {
    let (db, file, module) = imported_registration_fixture(
        "",
        "from django import template\nfrom . import implementation\nregister = template.Library()\nimplementation.HOOKS[0]()\nregister.tag(implementation.TAG, implementation.compile_tag)\n",
        "TAG = 'indirect_member_call'\nHOOKS = []\ndef compile_tag(parser, token): pass\n",
    )
    .expect("indirect module-callee fixture should install");
    let key = TemplateLibraryId::new(&db, Some(file), module);

    assert!(
        template_library_definition_facts(&db, key)
            .symbol(TemplateSymbolKind::Tag, "indirect_member_call")
            .is_none()
    );
}

#[test]
fn invoking_a_wrapped_imported_module_member_invalidates_resolution() {
    let (db, file, module) = imported_registration_fixture(
        "",
        "from django import template\nfrom . import implementation\nregister = template.Library()\n(implementation.configure if enabled else noop)()\nregister.tag(implementation.TAG, implementation.compile_tag)\n",
        "TAG = 'wrapped_member_call'\ndef configure(): pass\ndef compile_tag(parser, token): pass\n",
    )
    .expect("wrapped module-member-call fixture should install");
    let key = TemplateLibraryId::new(&db, Some(file), module);

    assert!(
        template_library_definition_facts(&db, key)
            .symbol(TemplateSymbolKind::Tag, "wrapped_member_call")
            .is_none()
    );
}

#[test]
fn passing_an_imported_module_object_to_a_call_invalidates_resolution() {
    let (db, file, module) = imported_registration_fixture(
        "",
        "from django import template\nfrom . import implementation\nregister = template.Library()\nconsume(implementation)\nregister.tag(implementation.TAG, implementation.compile_tag)\n",
        "TAG = 'escaped_call'\ndef compile_tag(parser, token): pass\n",
    )
    .expect("module-call-escape fixture should install");
    let key = TemplateLibraryId::new(&db, Some(file), module);

    assert!(
        template_library_definition_facts(&db, key)
            .symbol(TemplateSymbolKind::Tag, "escaped_call")
            .is_none()
    );
}

#[test]
fn escaped_imported_module_invalidates_aliased_resolution() {
    let (db, file, module) = imported_registration_fixture(
        "",
        "from django import template\nfrom . import implementation\nregister = template.Library()\nbox = [implementation]\nbox[0].TAG = dynamic_name\nregister.tag(implementation.TAG, implementation.compile_tag)\n",
        "TAG = 'escaped'\ndef compile_tag(parser, token): pass\n",
    )
    .expect("module-escape fixture should install");
    let key = TemplateLibraryId::new(&db, Some(file), module);

    assert!(
        template_library_definition_facts(&db, key)
            .symbol(TemplateSymbolKind::Tag, "escaped")
            .is_none()
    );
}

#[test]
fn unconditional_import_failure_discards_prior_module_values() {
    let (db, file, module) = imported_registration_fixture(
        "",
        "from django import template\nfrom . import implementation\nregister = template.Library()\nregister.tag(implementation.TAG, implementation.compile_tag)\n",
        "TAG = 'never_imported'\ndef compile_tag(parser, token): pass\nraise RuntimeError()\n",
    )
    .expect("import-failure fixture should install");
    let key = TemplateLibraryId::new(&db, Some(file), module);

    assert!(
        template_library_definition_facts(&db, key)
            .symbol(TemplateSymbolKind::Tag, "never_imported")
            .is_none()
    );
}

#[test]
fn imported_duplicate_function_uses_the_resolved_definition_span() {
    let implementation = "TAG = 'duplicate'\ndef compile_tag(parser, token):\n    bits = token.split_contents()\n    if len(bits) != 2: raise ValueError()\ndef compile_tag(parser, token):\n    bits = token.split_contents()\n    if len(bits) != 3: raise ValueError()\n";
    let (db, file, module) = imported_registration_fixture(
        "",
        "from django import template\nfrom . import implementation\nregister = template.Library()\nregister.tag(implementation.TAG, implementation.compile_tag)\n",
        implementation,
    )
    .expect("duplicate-function fixture should install");
    let key = TemplateLibraryId::new(&db, Some(file), module);

    let rule =
        &template_library_tag_facts(&db, key).tag_rules()[&SymbolKey::tag("pkg.tags", "duplicate")];
    assert_eq!(
        rule.arg_constraints,
        vec![ArgumentCountConstraint::Exact(3)]
    );
    let symbol = template_library_definition_facts(&db, key)
        .symbol(TemplateSymbolKind::Tag, "duplicate")
        .expect("duplicate function registration should resolve");
    let source =
        template_symbol_source(&db, symbol).expect("exact final definition should navigate");
    assert!(
        source.definition_span().start_usize()
            > implementation
                .find("def compile_tag")
                .expect("fixture should contain the first definition")
    );
}

#[test]
fn imported_source_edits_invalidate_registration_products() {
    let (mut db, file, module) = imported_registration_fixture(
        "",
        "from django import template\nfrom . import implementation\nregister = template.Library()\nregister.tag(implementation.TAG, implementation.compile_tag)\n",
        "TAG = 'before'\ndef compile_tag(parser, token):\n    bits = token.split_contents()\n    if len(bits) != 2: raise ValueError()\n",
    )
    .expect("imported-edit fixture should install");
    {
        let key = TemplateLibraryId::new(&db, Some(file), module.clone());
        let symbol = template_library_definition_facts(&db, key)
            .symbol(TemplateSymbolKind::Tag, "before")
            .expect("the imported registration should use its initial name");
        let source = template_symbol_source(&db, symbol)
            .expect("the imported registration should retain callable identity");
        assert_eq!(
            source.file(),
            db.file(Utf8Path::new("/test/project/pkg/implementation.py"))
                .expect("imported implementation source should exist")
        );
    }

    let implementation_path = Utf8Path::new("/test/project/pkg/implementation.py");
    db.add_file(
        implementation_path.as_str(),
        "TAG = 'after'\ndef compile_tag(parser, token):\n    bits = token.split_contents()\n    if len(bits) != 3: raise ValueError()\n",
    )
    .expect("updated imported implementation should be written");
    SourceChanges::new([ChangeEvent::ContentChanged(
        implementation_path.to_path_buf(),
    )])
    .apply(&mut db);

    let key = TemplateLibraryId::new(&db, Some(file), module);
    let definitions = template_library_definition_facts(&db, key);
    assert!(
        definitions
            .symbol(TemplateSymbolKind::Tag, "before")
            .is_none()
    );
    let after = definitions
        .symbol(TemplateSymbolKind::Tag, "after")
        .expect("the imported registration should use its updated name");
    let source = template_symbol_source(&db, after)
        .expect("the updated registration should retain callable identity");
    assert_eq!(
        source.file(),
        db.file(implementation_path)
            .expect("updated implementation source should exist")
    );
    assert_eq!(
        template_library_tag_facts(&db, key).tag_rules()[&SymbolKey::tag("pkg.tags", "after")]
            .arg_constraints,
        vec![ArgumentCountConstraint::Exact(3)]
    );
}

#[test]
fn nested_helper_sources_are_dependencies_and_same_length_edits_change_rules() {
    let (mut db, file, module) = imported_registration_fixture(
        "",
        r#"from django import template
from .implementation import first
register = template.Library()
@register.tag(name="nested")
def compile_tag(parser, token):
    bits = first(token)
    if len(bits) != 2:
        raise template.TemplateSyntaxError("wrong count")
    return template.Node()
"#,
        "from .helper import second\ndef first(token):\n    return second(token)\n",
    )
    .expect("nested-helper fixture should install");
    let helper_path = Utf8Path::new("/test/project/pkg/helper.py");
    let helper_source = "def second(token):\n    return token.split_contents()[1:]\n";
    db.add_file(helper_path.as_str(), helper_source)
        .expect("helper source should install");
    let helper_file = db.file(helper_path).expect("helper file should exist");
    {
        let key = TemplateLibraryId::new(&db, Some(file), module.clone());
        assert_eq!(
            template_library_tag_facts(&db, key).tag_rules()[&SymbolKey::tag("pkg.tags", "nested")]
                .arg_constraints,
            vec![ArgumentCountConstraint::Exact(3)]
        );
        let dependencies = template_library_registration_dependencies(&db, key);
        assert!(
            dependencies.contains(&helper_file),
            "nested source must enter priming coverage"
        );
        assert!(
            dependencies.contains(
                &db.file(Utf8Path::new("/test/project/pkg/implementation.py"))
                    .expect("outer helper should exist")
            )
        );
    }
    let changed = helper_source.replace("[1:]", "[2:]");
    assert_eq!(changed.len(), helper_source.len());
    db.add_file(helper_path.as_str(), &changed)
        .expect("changed helper should be written");
    SourceChanges::new([ChangeEvent::ContentChanged(helper_path.to_path_buf())]).apply(&mut db);
    {
        let key = TemplateLibraryId::new(&db, Some(file), module.clone());
        assert_eq!(
            template_library_tag_facts(&db, key).tag_rules()[&SymbolKey::tag("pkg.tags", "nested")]
                .arg_constraints,
            vec![ArgumentCountConstraint::Exact(4)]
        );
    }
    db.add_file(helper_path.as_str(), &format!("{changed}\ndef broken(\n"))
        .expect("recovered helper source should be written");
    SourceChanges::new([ChangeEvent::ContentChanged(helper_path.to_path_buf())]).apply(&mut db);
    let key = TemplateLibraryId::new(&db, Some(file), module);
    assert!(
        template_library_tag_facts(&db, key)
            .tag_rules()
            .get(&SymbolKey::tag("pkg.tags", "nested"))
            .is_none_or(|rule| rule.arg_constraints.is_empty())
    );
    assert!(
        template_library_registration_dependencies(&db, key).contains(&helper_file),
        "an uncertain dependency must remain covered so repairing it triggers extraction"
    );
}

#[test]
fn repeated_helper_resolution_keeps_each_calls_mutation_prefix() {
    let (db, file, module) = imported_registration_fixture(
        "",
        "from django import template\nfrom . import implementation as helpers\nregister = template.Library()\n\n@register.tag\ndef changing(parser, token):\n    before = helpers.bits(token)\n    if len(before) != 2:\n        raise template.TemplateSyntaxError('first count')\n    helpers.bits = unknown_replacement\n    after = helpers.bits(token)\n    if len(after) != 3:\n        raise template.TemplateSyntaxError('second count')\n    return template.Node()\n",
        "def bits(token):\n    return token.split_contents()\n",
    ).expect("helper fixture should install");
    let library = TemplateLibraryId::new(&db, Some(file), module);
    assert!(
        template_library_definition_facts(&db, library)
            .symbol(TemplateSymbolKind::Tag, "changing")
            .is_some()
    );
    assert_eq!(
        template_library_tag_facts(&db, library).tag_rules()
            [&SymbolKey::tag("pkg.tags", "changing")]
            .arg_constraints,
        vec![ArgumentCountConstraint::Exact(2)]
    );
}

#[test]
fn local_binding_can_replace_an_unknown_module_call_target() {
    let (db, file, module) = imported_registration_fixture(
        "",
        r#"from django import template
from . import implementation
register = template.Library()
class chooser:
    pass
@register.tag
def checked(parser, token):
    chooser = implementation.bits
    values = chooser(token)
    if len(values) != 2:
        raise template.TemplateSyntaxError("wrong count")
    return template.Node()
"#,
        "def bits(token):\n    return token.split_contents()[1:]\n",
    )
    .expect("local binding fixture should install");
    let library = TemplateLibraryId::new(&db, Some(file), module);
    assert!(
        template_library_definition_facts(&db, library)
            .symbol(TemplateSymbolKind::Tag, "checked")
            .is_some()
    );
    assert_eq!(
        template_library_tag_facts(&db, library).tag_rules()
            [&SymbolKey::tag("pkg.tags", "checked")]
            .arg_constraints,
        vec![ArgumentCountConstraint::Exact(3)]
    );
}

#[test]
fn unknown_module_call_target_keeps_dependency_and_repair_evidence() {
    let (mut db, file, module) = imported_registration_fixture(
        "",
        r#"from django import template
from . import implementation
chooser = implementation.bits
register = template.Library()
@register.tag
def checked(parser, token):
    values = chooser(token)
    if len(values) != 2:
        raise template.TemplateSyntaxError("wrong count")
    body = parser.parse(("endchecked",))
    return template.Node(body)
"#,
        "class bits:\n    pass\n",
    )
    .expect("unknown call fixture should install");
    let helper_path = Utf8Path::new("/test/project/pkg/implementation.py");
    let helper_file = db.file(helper_path).expect("helper should exist");
    let library = TemplateLibraryId::new(&db, Some(file), module.clone());
    let key = SymbolKey::tag("pkg.tags", "checked");
    assert!(
        template_library_definition_facts(&db, library)
            .symbol(TemplateSymbolKind::Tag, "checked")
            .is_some()
    );
    assert!(
        template_library_tag_facts(&db, library)
            .tag_rules()
            .get(&key)
            .is_none_or(|rule| rule.arg_constraints.is_empty())
    );
    assert!(template_library_registration_dependencies(&db, library).contains(&helper_file));
    db.add_file(
        helper_path.as_str(),
        "def bits(token):\n    return token.split_contents()[1:]\n",
    )
    .expect("helper repair should be written");
    SourceChanges::new([ChangeEvent::ContentChanged(helper_path.to_path_buf())]).apply(&mut db);
    let library = TemplateLibraryId::new(&db, Some(file), module);
    assert_eq!(
        template_library_tag_facts(&db, library).tag_rules()[&key].arg_constraints,
        vec![ArgumentCountConstraint::Exact(3)]
    );
}

#[test]
fn helper_known_and_unknown_return_branches_remain_unknown() {
    let source = r#"
from django import template
register = template.Library()

def maybe_bits(token):
    if runtime_condition():
        return token.split_contents()
    return runtime_value()

@register.tag
def conditional(parser, token):
    bits = maybe_bits(token)
    if len(bits) != 2:
        raise template.TemplateSyntaxError("wrong count")
    body = parser.parse(("endconditional",))
    return template.Node(body)
"#;
    let result = extract_source(source, "helper_returns").expect("fixture should extract");
    let key = SymbolKey::tag("helper_returns", "conditional");
    assert!(result.block_specs.as_map().contains_key(&key));
    assert!(
        result
            .tag_rules
            .get(&key)
            .is_none_or(|rule| rule.arg_constraints.is_empty())
    );
}

#[test]
fn helper_distinct_return_values_join_to_unknown() {
    let source = r#"
from django import template
register = template.Library()

def choose_bits(token):
    if runtime_condition():
        return token.split_contents()
    return token.split_contents()[1:]

@register.tag
def distinct(parser, token):
    bits = choose_bits(token)
    if len(bits) != 2:
        raise template.TemplateSyntaxError("wrong count")
    body = parser.parse(("enddistinct",))
    return template.Node(body)
"#;
    let result = extract_source(source, "helper_returns").expect("fixture should extract");
    let key = SymbolKey::tag("helper_returns", "distinct");
    assert!(result.block_specs.as_map().contains_key(&key));
    assert!(
        result
            .tag_rules
            .get(&key)
            .is_none_or(|rule| rule.arg_constraints.is_empty())
    );
}

#[test]
fn unreachable_later_return_does_not_change_helper_value() {
    let source = r#"
from django import template
register = template.Library()

def choose_bits(token):
    return token.split_contents()
    return token.split_contents()[1:]

@register.tag
def early(parser, token):
    bits = choose_bits(token)
    if len(bits) != 2:
        raise template.TemplateSyntaxError("wrong count")
    return template.Node()
"#;
    let result = extract_source(source, "helper_returns").expect("fixture should extract");
    let rule = &result.tag_rules[&SymbolKey::tag("helper_returns", "early")];
    assert_eq!(
        rule.arg_constraints,
        vec![ArgumentCountConstraint::Exact(2)]
    );
}

#[test]
fn bare_and_implicit_helper_returns_join_to_unknown() {
    for (name, ending) in [("bare", "    return\n"), ("implicit", "")] {
        let source = format!(
            r#"
from django import template
register = template.Library()

def maybe_bits(token):
    if runtime_condition():
        return token.split_contents()
{ending}
@register.tag(name="{name}")
def compile_tag(parser, token):
    bits = maybe_bits(token)
    if len(bits) != 2:
        raise template.TemplateSyntaxError("wrong count")
    body = parser.parse(("end{name}",))
    return template.Node(body)
"#
        );
        let result = extract_source(&source, "helper_returns").expect("fixture should extract");
        let key = SymbolKey::tag("helper_returns", name);
        assert!(result.block_specs.as_map().contains_key(&key));
        assert!(
            result
                .tag_rules
                .get(&key)
                .is_none_or(|rule| rule.arg_constraints.is_empty()),
            "helper: {name}"
        );
    }
}

#[test]
fn finally_restores_or_overrides_the_saved_return_value() {
    let source = r#"
from django import template
register = template.Library()

def preserved_index():
    index = 1
    try:
        return index
    finally:
        index = 2

def overridden_index():
    try:
        return 1
    finally:
        return 2

@register.tag(name="preserved")
def preserved(parser, token):
    bits = token.split_contents()
    index = preserved_index()
    argument = bits[index]
    if argument != "required":
        raise template.TemplateSyntaxError("wrong argument")
    return template.Node()

@register.tag(name="overridden")
def overridden(parser, token):
    bits = token.split_contents()
    index = overridden_index()
    argument = bits[index]
    if argument != "required":
        raise template.TemplateSyntaxError("wrong argument")
    return template.Node()
"#;
    let result = extract_source(source, "helper_returns").expect("fixture should extract");
    assert_eq!(
        result.tag_rules[&SymbolKey::tag("helper_returns", "preserved")].required_keywords[0]
            .position,
        djls_project::SplitPosition::Forward(1)
    );
    assert_eq!(
        result.tag_rules[&SymbolKey::tag("helper_returns", "overridden")].required_keywords[0]
            .position,
        djls_project::SplitPosition::Forward(2)
    );
}

#[test]
fn finalizer_mutation_does_not_restore_a_stale_returned_list() {
    let source = r#"
from django import template
register = template.Library()

def helper(token):
    bits = token.split_contents()
    try:
        return bits
    finally:
        bits.pop(0)

@register.tag
def mutated(parser, token):
    bits = helper(token)
    if len(bits) != 2:
        raise template.TemplateSyntaxError("wrong count")
    body = parser.parse(("endmutated",))
    return template.Node(body)
"#;
    let result = extract_source(source, "helper_returns").expect("fixture should extract");
    let key = SymbolKey::tag("helper_returns", "mutated");
    assert!(result.block_specs.as_map().contains_key(&key));
    // The returned list has lost its tag-name bit. Exact(2) would reject the
    // valid three-bit opener. Unknown is valid when the alias cannot be tracked.
    assert!(result.tag_rules.get(&key).is_none_or(|rule| {
        !rule
            .arg_constraints
            .contains(&ArgumentCountConstraint::Exact(2))
    }));
}

#[test]
fn return_expression_pop_is_visible_to_the_finalizer() {
    let source = r#"
from django import template
register = template.Library()

def helper(token):
    bits = token.split_contents()
    try:
        return bits.pop(0)
    finally:
        return bits

@register.tag
def popped(parser, token):
    bits = helper(token)
    if len(bits) != 2:
        raise template.TemplateSyntaxError("wrong count")
    body = parser.parse(("endpopped",))
    return template.Node(body)
"#;
    let result = extract_source(source, "helper_returns").expect("fixture should extract");
    let key = SymbolKey::tag("helper_returns", "popped");
    assert!(result.block_specs.as_map().contains_key(&key));
    assert!(result.tag_rules.get(&key).is_none_or(|rule| {
        !rule
            .arg_constraints
            .contains(&ArgumentCountConstraint::Exact(2))
    }));
}

#[test]
fn direct_nested_helper_returns_keep_dependency_values() {
    let source = r#"
from django import template
register = template.Library()

def deepest(token):
    return token.split_contents()

def middle(token):
    return deepest(token)

def outer(token):
    return middle(token)

@register.tag
def nested(parser, token):
    bits = outer(token)
    if len(bits) != 2:
        raise template.TemplateSyntaxError("wrong count")
    return template.Node()
"#;
    let result = extract_source(source, "helper_returns").expect("fixture should extract");
    assert_eq!(
        result.tag_rules[&SymbolKey::tag("helper_returns", "nested")].arg_constraints,
        vec![ArgumentCountConstraint::Exact(2)]
    );
}

#[test]
fn same_length_imported_function_rename_invalidates_callable_only_name() {
    let (mut db, file, module) = imported_registration_fixture(
        "",
        "from django import template\nfrom .implementation import alpha\nregister = template.Library()\nregister.tag(alpha)\n",
        "def alpha(parser, token): pass\n",
    )
    .expect("function-rename fixture should install");
    {
        let key = TemplateLibraryId::new(&db, Some(file), module.clone());
        assert!(
            template_library_definition_facts(&db, key)
                .symbol(TemplateSymbolKind::Tag, "alpha")
                .is_some()
        );
    }

    let registration_path = Utf8Path::new("/test/project/pkg/tags.py");
    let implementation_path = Utf8Path::new("/test/project/pkg/implementation.py");
    db.add_file(
        registration_path.as_str(),
        "from django import template\nfrom .implementation import bravo\nregister = template.Library()\nregister.tag(bravo)\n",
    )
    .expect("renamed registration source should be written");
    db.add_file(
        implementation_path.as_str(),
        "def bravo(parser, token): pass\n",
    )
    .expect("renamed implementation source should be written");
    SourceChanges::new([
        ChangeEvent::ContentChanged(registration_path.to_path_buf()),
        ChangeEvent::ContentChanged(implementation_path.to_path_buf()),
    ])
    .apply(&mut db);

    let key = TemplateLibraryId::new(&db, Some(file), module);
    let definitions = template_library_definition_facts(&db, key);
    assert!(
        definitions
            .symbol(TemplateSymbolKind::Tag, "alpha")
            .is_none()
    );
    assert!(
        definitions
            .symbol(TemplateSymbolKind::Tag, "bravo")
            .is_some()
    );
}

#[test]
fn malformed_imported_registration_keywords_fail_closed() {
    let (db, file, module) = imported_registration_fixture(
        "",
        "from django import template\nfrom . import implementation\nregister = template.Library()\n@register.simple_tag\ndef retained(): pass\nregister.tag(implementation.TAG, implementation.compile_tag, nonsense=True)\n",
        "TAG = 'malformed'\ndef compile_tag(parser, token): pass\n",
    )
    .expect("malformed-registration fixture should install");
    let key = TemplateLibraryId::new(&db, Some(file), module);
    let facts = template_library_definition_facts(&db, key);
    assert!(facts.symbol(TemplateSymbolKind::Tag, "malformed").is_none());
    let retained = facts
        .symbol(TemplateSymbolKind::Tag, "retained")
        .expect("prior exact registration should survive");
    assert_eq!(template_symbol_source(&db, retained), None);
}

#[test]
fn imported_module_attribute_mutation_invalidates_later_resolution() {
    let (db, file, module) = imported_registration_fixture(
        "",
        "from django import template\nfrom . import implementation\nregister = template.Library()\nimplementation.TAG = dynamic_name\nregister.tag(implementation.TAG, implementation.compile_tag)\n",
        "TAG = 'mutated'\ndef compile_tag(parser, token): pass\n",
    )
    .expect("attribute-mutation fixture should install");
    let key = TemplateLibraryId::new(&db, Some(file), module);

    assert!(
        template_library_definition_facts(&db, key)
            .symbol(TemplateSymbolKind::Tag, "mutated")
            .is_none()
    );
}

#[test]
fn recovered_import_retains_positive_facts_but_opens_inventory_and_navigation() {
    let (db, file, module) = imported_registration_fixture(
        "",
        "from django import template\nfrom . import implementation\nregister = template.Library()\n@register.simple_tag\ndef retained(): pass\nregister.tag(implementation.TAG, implementation.compile_tag)\nregister.simple_tag(implementation.simple, name='recovered_simple')\n",
        "TAG = 'recovered'\ndef compile_tag(parser, token):\n    bits = token.split_contents()\n    if len(bits) != 1: raise ValueError()\ndef simple(value): return value\ndef broken(\n",
    )
    .expect("recovered-import fixture should install");
    let key = TemplateLibraryId::new(&db, Some(file), module);
    let facts = template_library_definition_facts(&db, key);
    let imported = facts
        .symbol(TemplateSymbolKind::Tag, "recovered")
        .expect("recovered imported positive fact should survive");
    let retained = facts
        .symbol(TemplateSymbolKind::Tag, "retained")
        .expect("other exact registrations should survive");
    assert!(
        facts
            .symbol(TemplateSymbolKind::Tag, "recovered_simple")
            .is_some(),
        "a recovered callable keeps its positive Tag Definition"
    );
    assert_eq!(template_symbol_source(&db, imported), None);
    assert_eq!(template_symbol_source(&db, retained), None);
    let tag_rules = template_library_tag_facts(&db, key).tag_rules();
    assert!(tag_rules.contains_key(&SymbolKey::tag("pkg.tags", "recovered")));
    assert!(matches!(
        &tag_rules[&SymbolKey::tag("pkg.tags", "retained")].argument_syntax,
        TagArgumentSyntax::Signature {
            parameters,
            variadic_keyword: None,
            ..
        } if parameters.is_empty()
    ));
    assert!(matches!(
        &tag_rules[&SymbolKey::tag("pkg.tags", "recovered_simple")].argument_syntax,
        TagArgumentSyntax::Parameters(parameters)
            if parameters.len() == 1 && parameters[0].name == "value"
    ));
}

#[test]
fn recovered_registration_options_do_not_taint_independent_signatures() {
    let (db, file, module) = imported_registration_fixture(
        "",
        "from django import template\nfrom . import implementation\nregister = template.Library()\n@register.simple_tag(takes_context=implementation.TAKES_CONTEXT, name='first')\ndef first(context, value): return value\n@register.simple_tag(takes_context=implementation.TAKES_CONTEXT, name='second')\ndef second(context, value): return value\n@register.simple_tag\ndef retained(value): return value\n",
        "TAKES_CONTEXT = True\ndef broken(\n",
    )
    .expect("recovered registration-option fixture should install");
    let key = TemplateLibraryId::new(&db, Some(file), module);
    let rules = template_library_tag_facts(&db, key).tag_rules();

    for name in ["first", "second"] {
        assert!(matches!(
            &rules[&SymbolKey::tag("pkg.tags", name)].argument_syntax,
            TagArgumentSyntax::Parameters(parameters)
                if parameters.len() == 1 && parameters[0].name == "value"
        ));
    }
    assert!(matches!(
        &rules[&SymbolKey::tag("pkg.tags", "retained")].argument_syntax,
        TagArgumentSyntax::Signature {
            parameters,
            variadic_keyword: None,
            ..
        } if parameters.len() == 1 && parameters[0].name == "value"
    ));
}

#[test]
fn literal_names_survive_unresolved_callables_across_call_shapes() {
    let (db, file, module) = imported_registration_fixture(
        "",
        "from django import template\nfrom missing import implementation\nregister = template.Library()\nregister.tag('literal_tag', implementation.compile_tag)\nregister.filter('literal_filter', implementation.filter)\nregister.simple_tag(implementation.simple, name='literal_simple')\nregister.inclusion_tag('partial.html', name='literal_inclusion')(implementation.inclusion)\nregister.simple_block_tag(name='literal_block', func=implementation.block)\nregister.inclusion_tag('unused.html', name='unused_inclusion')\n",
        "",
    )
    .expect("unresolved-callable fixture should install");
    let key = TemplateLibraryId::new(&db, Some(file), module);
    let facts = template_library_definition_facts(&db, key);

    for name in [
        "literal_tag",
        "literal_simple",
        "literal_inclusion",
        "literal_block",
    ] {
        let symbol = facts
            .symbol(TemplateSymbolKind::Tag, name)
            .unwrap_or_else(|| panic!("known Tag name `{name}` should survive"));
        assert_eq!(template_symbol_source(&db, symbol), None);
    }
    assert!(
        facts
            .symbol(TemplateSymbolKind::Tag, "unused_inclusion")
            .is_none(),
        "an unused inclusion_tag decorator must not register a Tag"
    );
    let filter = facts
        .symbol(TemplateSymbolKind::Filter, "literal_filter")
        .expect("known Filter name should survive");
    assert_eq!(template_symbol_source(&db, filter), None);
    assert!(template_library_tag_facts(&db, key).tag_rules().is_empty());
    assert!(
        template_library_filter_facts(&db, key)
            .filter_arities()
            .is_empty()
    );
}

#[test]
fn imported_callable_only_registrations_use_resolved_function_names() {
    let (db, file, module) = imported_registration_fixture(
        "",
        "from django import template\nfrom .implementation import compile_tag as tag_callable, imported_filter as filter_callable, keyword_tag, keyword_filter, simple, inclusion, block\nregister = template.Library()\nregister.tag(tag_callable)\nregister.filter(filter_callable)\nregister.tag(compile_function=keyword_tag)\nregister.filter(filter_func=keyword_filter)\nregister.simple_tag(func=simple)\nregister.inclusion_tag('partial.html')(inclusion)\nregister.simple_block_tag(func=block)\nregister.inclusion_tag('unused.html', name='unused_inclusion')\n",
        "def compile_tag(parser, token): pass\ndef imported_filter(value): return value\ndef keyword_tag(parser, token): pass\ndef keyword_filter(value): return value\ndef simple(): pass\ndef inclusion(): pass\ndef block(): pass\n",
    )
    .expect("callable-only fixture should install");
    let key = TemplateLibraryId::new(&db, Some(file), module);
    let facts = template_library_definition_facts(&db, key);

    for name in ["compile_tag", "simple", "inclusion", "block"] {
        assert!(
            facts.symbol(TemplateSymbolKind::Tag, name).is_some(),
            "resolved callable-only Tag `{name}` should use its function name"
        );
    }
    assert!(
        facts
            .symbol(TemplateSymbolKind::Filter, "imported_filter")
            .is_some()
    );
    assert!(
        facts
            .symbol(TemplateSymbolKind::Tag, "keyword_tag")
            .is_none()
    );
    assert!(
        facts
            .symbol(TemplateSymbolKind::Filter, "keyword_filter")
            .is_none()
    );
    assert!(
        facts
            .symbol(TemplateSymbolKind::Tag, "unused_inclusion")
            .is_none(),
        "an unused inclusion_tag decorator must not register a Tag"
    );
}

#[test]
fn project_backed_registration_options_resolve_without_inventing_count_facts() {
    let (db, file, module) = imported_registration_fixture(
        "",
        r#"from django import template
from . import implementation
register = template.Library()

register.simple_tag(
    implementation.context_tag,
    takes_context=implementation.TAKES_CONTEXT,
    name="imported_context",
)

@register.simple_block_tag(
    takes_context=implementation.TAKES_CONTEXT,
    name=implementation.PANEL_NAME,
    end_name=implementation.END_NAME,
)
def panel(context, content, title): pass

@register.simple_tag(takes_context=UNKNOWN, name="unknown_context")
def unknown_context(context, value): pass

@register.simple_tag(name="malformed", unsupported=True)
def malformed(value): pass
"#,
        "TAKES_CONTEXT = True\nPANEL_NAME = 'imported_panel'\nEND_NAME = 'finish_panel'\ndef context_tag(context, value): pass\n",
    )
    .expect("registration-option fixture should install");
    let key = TemplateLibraryId::new(&db, Some(file), module);
    let definitions = template_library_definition_facts(&db, key);
    for name in ["imported_context", "imported_panel", "unknown_context"] {
        assert!(
            definitions.symbol(TemplateSymbolKind::Tag, name).is_some(),
            "known registration `{name}` should survive"
        );
    }
    assert!(
        definitions
            .symbol(TemplateSymbolKind::Tag, "malformed")
            .is_none(),
        "an unsupported decorator must not invent a registration"
    );

    let tag_facts = template_library_tag_facts(&db, key);
    for name in ["imported_context", "imported_panel"] {
        assert_eq!(
            tag_facts.tag_rules()[&SymbolKey::tag("pkg.tags", name)].argument_syntax,
            TagArgumentSyntax::Signature {
                parameters: vec![djls_project::TagArgument {
                    name: if name == "imported_context" {
                        "value".to_string()
                    } else {
                        "title".to_string()
                    },
                    requirement: ParameterRequirement::Required,
                    kind: TagArgumentKind::Variable,
                }],
                positional_only: 0,
                variadic_keyword: None,
            },
            "resolved takes_context must remove framework parameters for `{name}`"
        );
    }
    assert_eq!(
        tag_facts.block_specs().as_map()[&SymbolKey::tag("pkg.tags", "imported_panel")]
            .end_tag
            .as_deref(),
        Some("finish_panel")
    );
    assert!(
        !tag_facts
            .tag_rules()
            .contains_key(&SymbolKey::tag("pkg.tags", "unknown_context")),
        "unknown options must not produce definite count facts"
    );
    assert!(
        !tag_facts
            .tag_rules()
            .contains_key(&SymbolKey::tag("pkg.tags", "malformed")),
        "malformed decorators must not produce count facts"
    );
}

// The fixture deliberately keeps all released django-bird registration shapes together so the
// cross-module name, callable, rule, block, arity, source, and dependency contracts stay visible.
#[allow(clippy::too_many_lines)]
#[test]
fn imported_registration_resolution_extracts_django_bird_shapes_and_coverage() {
    let registration_source = r#"from django import template
from . import asset, bird, load, prop, slot, var
from .filters import imported_filter

register = template.Library()
register.tag(asset.AssetTag.CSS.value, asset.do_asset)
register.tag(asset.AssetTag.JS.value, asset.do_asset)
register.tag(bird.TAG, bird.do_bird)
register.tag(load.TAG, load.do_load)
register.tag(prop.TAG, prop.do_prop)
register.tag(slot.TAG, slot.do_slot)
register.tag(var.TAG, var.do_var)
register.tag(var.END_TAG, var.do_end_var)
register.filter("bird_filter", imported_filter)
"#;
    let bird_source = r#"TAG = "bird"

def split_bits(token):
    return token.split_contents()

def do_bird(parser, token):
    bits = split_bits(token)
    if len(bits) != 2:
        raise ValueError("bird takes one argument")
    nodelist = parser.parse(("endbird",))
    parser.delete_first_token()
    return nodelist
"#;
    let asset_source = r#"from enum import Enum

class AssetTag(Enum):
    CSS = "bird:css"
    JS = "bird:js"

def do_asset(parser, token):
    bits = token.split_contents()
    if len(bits) != 1:
        raise ValueError("asset takes no arguments")
"#;
    let filter_source = "def imported_filter(value, argument):\n    return value\n";
    let mut db = TestDatabase::new();
    ProjectFixture::new("/test/project")
        .django_settings_module("settings")
        .file("/test/project/settings.py", "INSTALLED_APPS = []\n")
        .file("/test/project/app/__init__.py", "")
        .file("/test/project/app/templatetags/__init__.py", "")
        .file(
            "/test/project/app/templatetags/bird_tags.py",
            registration_source,
        )
        .file("/test/project/app/templatetags/bird.py", bird_source)
        .file("/test/project/app/templatetags/asset.py", asset_source)
        .file(
            "/test/project/app/templatetags/load.py",
            "TAG = 'bird:load'\ndef do_load(parser, token): pass\n",
        )
        .file(
            "/test/project/app/templatetags/prop.py",
            "TAG = 'bird:prop'\ndef do_prop(parser, token): pass\n",
        )
        .file(
            "/test/project/app/templatetags/slot.py",
            "TAG = 'bird:slot'\ndef do_slot(parser, token):\n    nodelist = parser.parse(('endbird:slot',))\n    parser.delete_first_token()\n",
        )
        .file(
            "/test/project/app/templatetags/var.py",
            "TAG = 'bird:var'\nEND_TAG = 'endbird:var'\ndef do_var(parser, token): pass\ndef do_end_var(parser, token): pass\n",
        )
        .file("/test/project/app/templatetags/filters.py", filter_source)
        .install(&mut db)
        .expect("multi-file registration fixture should install");

    let registration_file = db
        .file(Utf8Path::new("/test/project/app/templatetags/bird_tags.py"))
        .expect("registration source should exist");
    let key = TemplateLibraryId::new(
        &db,
        Some(registration_file),
        PythonModuleName::parse("app.templatetags.bird_tags")
            .expect("fixture module name should be valid"),
    );
    let definitions = template_library_definition_facts(&db, key);
    for name in [
        "bird",
        "bird:css",
        "bird:js",
        "bird:load",
        "bird:prop",
        "bird:slot",
        "bird:var",
        "endbird:var",
    ] {
        assert!(
            definitions.symbol(TemplateSymbolKind::Tag, name).is_some(),
            "imported Tag `{name}` should be registered"
        );
    }
    assert!(
        definitions
            .symbol(TemplateSymbolKind::Filter, "bird_filter")
            .is_some()
    );

    let tag_facts = template_library_tag_facts(&db, key);
    let bird_key = SymbolKey::tag("app.templatetags.bird_tags", "bird");
    assert_eq!(
        tag_facts.tag_rules()[&bird_key].arg_constraints,
        vec![ArgumentCountConstraint::Exact(2)]
    );
    assert_eq!(
        tag_facts.block_specs().as_map()[&bird_key]
            .end_tag
            .as_deref(),
        Some("endbird")
    );
    assert_eq!(
        tag_facts.block_specs().as_map()
            [&SymbolKey::tag("app.templatetags.bird_tags", "bird:slot")]
            .end_tag
            .as_deref(),
        Some("endbird:slot")
    );
    let filter_key = SymbolKey::filter("app.templatetags.bird_tags", "bird_filter");
    assert_eq!(
        template_library_filter_facts(&db, key)
            .filter_arities()
            .get(&filter_key),
        Some(&FilterArity::RequiredArgument)
    );

    let bird_symbol = definitions
        .symbol(TemplateSymbolKind::Tag, "bird")
        .expect("imported bird Tag should exist");
    let source = template_symbol_source(&db, bird_symbol)
        .expect("exact imported callable should have a source");
    assert_eq!(
        source.file().path(&db),
        Utf8Path::new("/test/project/app/templatetags/bird.py")
    );
    assert_eq!(
        &bird_source[source.name_span().start_usize()..source.name_span().end_usize()],
        "do_bird"
    );
    assert_eq!(
        &bird_source[source.definition_span().start_usize()..source.definition_span().end_usize()],
        "def do_bird(parser, token):\n    bits = split_bits(token)\n    if len(bits) != 2:\n        raise ValueError(\"bird takes one argument\")\n    nodelist = parser.parse((\"endbird\",))\n    parser.delete_first_token()\n    return nodelist"
    );

    let covered_paths = template_library_registration_dependencies(&db, key)
        .iter()
        .map(|file| file.path(&db).as_str())
        .collect::<Vec<_>>();
    for path in [
        "/test/project/app/__init__.py",
        "/test/project/app/templatetags/__init__.py",
        "/test/project/app/templatetags/asset.py",
        "/test/project/app/templatetags/bird.py",
        "/test/project/app/templatetags/filters.py",
        "/test/project/app/templatetags/load.py",
        "/test/project/app/templatetags/prop.py",
        "/test/project/app/templatetags/slot.py",
        "/test/project/app/templatetags/var.py",
    ] {
        assert!(
            covered_paths.contains(&path),
            "coverage should include {path}"
        );
    }
}

#[test]
fn unresolved_imported_registration_opens_only_its_library_inventory() {
    let mut db = TestDatabase::new();
    ProjectFixture::new("/test/project")
        .django_settings_module("settings")
        .file("/test/project/settings.py", "INSTALLED_APPS = []\n")
        .file("/test/project/known.py", "from django import template\nregister = template.Library()\n@register.simple_tag\ndef retained(): pass\n")
        .file("/test/project/dynamic.py", "from django import template\nfrom missing import names, functions\nregister = template.Library()\n@register.simple_tag\ndef retained(): pass\nregister.tag(names.TAG, functions.compile_tag)\n")
        .install(&mut db)
        .expect("uncertain registration fixture should install");

    let dynamic_file = db
        .file(Utf8Path::new("/test/project/dynamic.py"))
        .expect("dynamic library should exist");
    let dynamic = TemplateLibraryId::new(
        &db,
        Some(dynamic_file),
        PythonModuleName::parse("dynamic").expect("fixture module should be valid"),
    );
    let dynamic_facts = template_library_definition_facts(&db, dynamic);
    let dynamic_retained = dynamic_facts
        .symbol(TemplateSymbolKind::Tag, "retained")
        .expect("exact registration should survive uncertainty");
    assert_eq!(template_symbol_source(&db, dynamic_retained), None);
    assert!(
        dynamic_facts
            .symbol(TemplateSymbolKind::Tag, "TAG")
            .is_none()
    );

    let known_file = db
        .file(Utf8Path::new("/test/project/known.py"))
        .expect("known library should exist");
    let known = TemplateLibraryId::new(
        &db,
        Some(known_file),
        PythonModuleName::parse("known").expect("fixture module should be valid"),
    );
    let known_facts = template_library_definition_facts(&db, known);
    let known_retained = known_facts
        .symbol(TemplateSymbolKind::Tag, "retained")
        .expect("closed library should retain its exact registration");
    assert!(template_symbol_source(&db, known_retained).is_some());
}

// (b) Edge case — valid Python with no registrations
#[test]
fn extract_bundle_no_registrations() {
    let source = r"
def regular_function():
    pass

class MyClass:
    pass
";
    let result = extract_source(source, "test.module")
        .expect("unregistered-function extraction fixture should build");
    assert!(result.is_empty());
}

// Corpus: defaulttags.py has both tags and filters (via `cycle` tag +
// querystring simple_tag). Validates multiple registration kinds extracted.
#[test]
fn extract_bundle_multiple_registrations() {
    let result = extract_source(DEFAULTTAGS_SOURCE, "django.template.defaulttags")
        .expect("multiple-registration extraction fixture should build");
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
fn extract_bundle_call_style_registration_no_func_def() {
    let source = r#"
from django import template
from somewhere import do_for
register = template.Library()

register.tag("for", do_for)
"#;
    let result = extract_source(source, "test.module")
        .expect("call-style registration extraction fixture should build");
    assert!(result.tag_rules.is_empty());
    assert!(result.block_specs.is_empty());
}

// Vendored corpus-snippet golden tests — full pipeline extraction on pinned snippets.
// These snapshot the complete extraction output for each fixture.

// Corpus: django/template/defaulttags.py — the largest built-in templatetag
// module. Exercises bare @register.tag, @register.tag("name"),
// @register.tag(name="name"), @register.simple_tag, len checks (exact, min,
// max, not-in), keyword position checks, option loops, block specs with
// intermediates, opaque blocks, dynamic end tags, and multiple raise statements.
#[test]
fn golden_defaulttags() {
    let result = extract_source(DEFAULTTAGS_SOURCE, "django.template.defaulttags")
        .expect("default-tags extraction fixture should build");
    insta::assert_yaml_snapshot!(
        sorted_snapshot(&result).expect("default-tags extraction snapshot should serialize")
    );
}

// Corpus: django/template/loader_tags.py — block, extends, include tags.
// Exercises simple block (endblock), option loop (include with/only),
// and non-block tags (extends).
#[test]
fn golden_loader_tags() {
    let result = extract_source(LOADER_TAGS_SOURCE, "django.template.loader_tags")
        .expect("loader-tags extraction fixture should build");
    insta::assert_yaml_snapshot!(
        sorted_snapshot(&result).expect("loader-tags extraction snapshot should serialize")
    );
}

// Corpus: django/template/defaultfilters.py — all built-in filters.
// Exercises @register.filter (bare), @register.filter("name"),
// @register.filter(is_safe=True), filters with no arg, required arg,
// and optional arg (default parameter).
#[test]
fn golden_defaultfilters() {
    let result = extract_source(DEFAULTFILTERS_SOURCE, "django.template.defaultfilters")
        .expect("default-filters extraction fixture should build");
    insta::assert_yaml_snapshot!(
        sorted_snapshot(&result).expect("default-filters extraction snapshot should serialize")
    );
}

// Corpus: django/templatetags/i18n.py — i18n tags.
// Exercises @register.tag("name"), @register.filter, and the
// blocktranslate next_token loop pattern.
#[test]
fn golden_i18n() {
    let result = extract_source(I18N_SOURCE, "django.templatetags.i18n")
        .expect("i18n extraction fixture should build");
    insta::assert_yaml_snapshot!(
        sorted_snapshot(&result).expect("i18n extraction snapshot should serialize")
    );
}

// Corpus: tests/template_tests/templatetags/inclusion.py — inclusion tags.
// Exercises @register.inclusion_tag with and without takes_context,
// various arg counts, and keyword-only defaults.
#[test]
fn golden_inclusion_tags() {
    let result = extract_source(
        INCLUSION_SOURCE,
        "tests.template_tests.templatetags.inclusion",
    )
    .expect("inclusion-tag extraction fixture should build");
    insta::assert_yaml_snapshot!(
        sorted_snapshot(&result).expect("inclusion-tag extraction snapshot should serialize")
    );
}

// Corpus: tests/template_tests/templatetags/custom.py — simple tags.
// Exercises @register.simple_tag with and without takes_context,
// @register.simple_tag(name="..."), @register.simple_block_tag,
// @register.filter, and various arg patterns.
#[test]
fn golden_custom_tags() {
    let result = extract_source(CUSTOM_SOURCE, "tests.template_tests.templatetags.custom")
        .expect("custom-tag extraction fixture should build");
    insta::assert_yaml_snapshot!(
        sorted_snapshot(&result).expect("custom-tag extraction snapshot should serialize")
    );
}

// Corpus: tests/template_tests/templatetags/testtags.py — call-style
// registrations. Exercises register.tag("name", func) and
// register.filter("name", func) call-style patterns.
#[test]
fn golden_testtags() {
    let result = extract_source(
        TESTTAGS_SOURCE,
        "tests.template_tests.templatetags.testtags",
    )
    .expect("call-style tag extraction fixture should build");
    insta::assert_yaml_snapshot!(
        sorted_snapshot(&result).expect("call-style tag extraction snapshot should serialize")
    );
}

// Corpus: django-allauth/allauth/templatetags/allauth.py — custom block tag.
// Exercises helper-based argument parsing and explicit end tag extraction.
#[test]
fn golden_allauth_tags() {
    let result = extract_source(ALLAUTH_TAGS_SOURCE, "allauth.templatetags.allauth")
        .expect("allauth extraction fixture should build");
    insta::assert_yaml_snapshot!(
        sorted_snapshot(&result).expect("allauth extraction snapshot should serialize")
    );
}

// Corpus: wagtail/admin/templatetags/wagtailadmin_tags.py — call-style
// registrations. Exercises register.tag("name", Class.handle) and
// register.filter("name", func) without local function definitions.
#[test]
fn golden_wagtailadmin_tags() {
    let result = extract_source(
        WAGTAILADMIN_TAGS_SOURCE,
        "wagtail.admin.templatetags.wagtailadmin_tags",
    )
    .expect("Wagtail admin extraction fixture should build");
    insta::assert_yaml_snapshot!(
        sorted_snapshot(&result).expect("Wagtail admin extraction snapshot should serialize")
    );
}

// Corpus: django/templatetags/tz.py — timezone tags.
// Exercises simple tags and block tags with conventional end tags.
#[test]
fn golden_django_tz() {
    let result = extract_source(TZ_SOURCE, "django.templatetags.tz")
        .expect("timezone extraction fixture should build");
    insta::assert_yaml_snapshot!(
        sorted_snapshot(&result).expect("timezone extraction snapshot should serialize")
    );
}

// Corpus: django/contrib/admin/templatetags/admin_urls.py — admin URL helpers.
// Exercises simple_tag with takes_context and optional function parameters.
#[test]
fn golden_django_admin_urls() {
    let result = extract_source(
        ADMIN_URLS_SOURCE,
        "django.contrib.admin.templatetags.admin_urls",
    )
    .expect("Django admin URL extraction fixture should build");
    insta::assert_yaml_snapshot!(
        sorted_snapshot(&result).expect("Django admin URL extraction snapshot should serialize")
    );
}

// Pattern-specific corpus assertions — validate specific extraction
// behaviors using real Django code, complementing the full-module snapshots.

// Corpus: `autoescape` in defaulttags.py — bare @register.tag decorator.
// Registration name defaults to function name.
#[test]
fn corpus_decorator_bare_tag() {
    let result = extract_source(DEFAULTTAGS_SOURCE, "django.template.defaulttags")
        .expect("bare-decorator extraction fixture should build");
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
    let result = extract_source(DEFAULTTAGS_SOURCE, "django.template.defaulttags")
        .expect("explicit-name decorator extraction fixture should build");
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
    let result = extract_source(DEFAULTTAGS_SOURCE, "django.template.defaulttags")
        .expect("name-keyword decorator extraction fixture should build");
    let key = SymbolKey::tag("django.template.defaulttags", "partialdef");
    assert!(
        result.tag_rules.contains_key(&key) || result.block_specs.as_map().contains_key(&key),
        "partialdef should be extracted (name from kwarg)"
    );
}

// Corpus: `no_params` in custom.py — @register.simple_tag with zero user args.
#[test]
fn corpus_simple_tag_no_args() {
    let result = extract_source(CUSTOM_SOURCE, "tests.template_tests.templatetags.custom")
        .expect("no-argument simple-tag extraction fixture should build");
    let key = SymbolKey::tag("tests.template_tests.templatetags.custom", "no_params");
    assert!(result.tag_rules.contains_key(&key));
    let rule = &result.tag_rules[&key];
    assert!(
        rule.argument_syntax
            .parameters()
            .expect("expected parameter syntax")
            .is_empty()
    );
}

// Corpus: `one_param` in custom.py — @register.simple_tag with one required arg.
#[test]
fn corpus_simple_tag_with_args() {
    let result = extract_source(CUSTOM_SOURCE, "tests.template_tests.templatetags.custom")
        .expect("simple-tag argument extraction fixture should build");
    let key = SymbolKey::tag("tests.template_tests.templatetags.custom", "one_param");
    assert!(result.tag_rules.contains_key(&key));
    let rule = &result.tag_rules[&key];
    assert_eq!(
        rule.argument_syntax
            .parameters()
            .expect("expected parameter syntax")
            .len(),
        1
    );
    assert!(
        rule.argument_syntax
            .parameters()
            .expect("expected parameter syntax")[0]
            .requirement
            .is_required()
    );
}

// Corpus: `no_params_with_context` in custom.py —
// @register.simple_tag(takes_context=True), context param excluded from args.
#[test]
fn corpus_simple_tag_takes_context() {
    let result = extract_source(CUSTOM_SOURCE, "tests.template_tests.templatetags.custom")
        .expect("context simple-tag extraction fixture should build");
    let key = SymbolKey::tag(
        "tests.template_tests.templatetags.custom",
        "no_params_with_context",
    );
    assert!(result.tag_rules.contains_key(&key));
    let rule = &result.tag_rules[&key];
    assert!(
        rule.argument_syntax
            .parameters()
            .expect("expected parameter syntax")
            .is_empty(),
        "context param should not appear as extracted arg"
    );
}

// Corpus: `inclusion_one_param` in inclusion.py — @register.inclusion_tag
// with one required arg.
#[test]
fn corpus_inclusion_tag() {
    let result = extract_source(
        INCLUSION_SOURCE,
        "tests.template_tests.templatetags.inclusion",
    )
    .expect("inclusion-tag extraction fixture should build");
    let key = SymbolKey::tag(
        "tests.template_tests.templatetags.inclusion",
        "inclusion_one_param",
    );
    assert!(result.tag_rules.contains_key(&key));
    let rule = &result.tag_rules[&key];
    assert_eq!(
        rule.argument_syntax
            .parameters()
            .expect("expected parameter syntax")
            .len(),
        1
    );
    assert!(
        rule.argument_syntax
            .parameters()
            .expect("expected parameter syntax")[0]
            .requirement
            .is_required()
    );
}

// Corpus: `inclusion_no_params_with_context` in inclusion.py —
// @register.inclusion_tag with takes_context=True.
#[test]
fn corpus_inclusion_tag_takes_context() {
    let result = extract_source(
        INCLUSION_SOURCE,
        "tests.template_tests.templatetags.inclusion",
    )
    .expect("context inclusion-tag extraction fixture should build");
    let key = SymbolKey::tag(
        "tests.template_tests.templatetags.inclusion",
        "inclusion_no_params_with_context",
    );
    assert!(result.tag_rules.contains_key(&key));
    let rule = &result.tag_rules[&key];
    assert!(
        rule.argument_syntax
            .parameters()
            .expect("expected parameter syntax")
            .is_empty(),
        "context param should not appear as extracted arg"
    );
}

// Corpus: `inclusion_one_default` in inclusion.py — inclusion_tag with
// one required + one optional arg.
#[test]
fn corpus_inclusion_tag_with_args() {
    let result = extract_source(
        INCLUSION_SOURCE,
        "tests.template_tests.templatetags.inclusion",
    )
    .expect("inclusion-tag argument extraction fixture should build");
    let key = SymbolKey::tag(
        "tests.template_tests.templatetags.inclusion",
        "inclusion_one_default",
    );
    assert!(result.tag_rules.contains_key(&key));
    let rule = &result.tag_rules[&key];
    assert_eq!(
        rule.argument_syntax
            .parameters()
            .expect("expected parameter syntax")
            .len(),
        2
    );
    assert!(
        rule.argument_syntax
            .parameters()
            .expect("expected parameter syntax")[0]
            .requirement
            .is_required()
    );
    assert!(
        !rule
            .argument_syntax
            .parameters()
            .expect("expected parameter syntax")[1]
            .requirement
            .is_required()
    );
}

// Corpus: `querystring` in defaulttags.py — @register.simple_tag(name="querystring",
// takes_context=True) with name kwarg on simple_tag.
#[test]
fn corpus_simple_tag_with_name_kwarg() {
    let result = extract_source(DEFAULTTAGS_SOURCE, "django.template.defaulttags")
        .expect("named simple-tag extraction fixture should build");
    let key = SymbolKey::tag("django.template.defaulttags", "querystring");
    assert!(
        result.tag_rules.contains_key(&key),
        "querystring should be extracted via name kwarg"
    );
}

#[test]
fn unprojected_form_preserves_loader_argument_hint() {
    let result = extract_source(LOADER_TAGS_SOURCE, "django.template.loader_tags")
        .expect("loader-tag source should extract");
    let rule = &result.tag_rules[&SymbolKey::tag("django.template.loader_tags", "include")];
    let parameters = rule
        .argument_syntax
        .parameters()
        .expect("known argument hints remain useful without complete forms");
    assert_eq!(parameters.len(), 1);
    assert!(parameters[0].requirement.is_required());
    assert_eq!(parameters[0].kind, djls_project::TagArgumentKind::Variable);
}

// Corpus: `for` in defaulttags.py derives its two correlated forms from the
// reversed predicate and variable `in_index` subscript.
#[test]
fn corpus_for_reversed_forms() {
    let result = extract_source(DEFAULTTAGS_SOURCE, "django.template.defaulttags")
        .expect("for-tag extraction fixture should build");
    let key = SymbolKey::tag("django.template.defaulttags", "for");
    let rule = result.tag_rules.get(&key).expect("for should be extracted");
    let (forms, coverage) = rule
        .argument_syntax
        .forms()
        .expect("for should retain correlated forms");
    assert_eq!(coverage, ArgumentFormCoverage::Complete);
    assert_eq!(forms.len(), 2);

    let non_reversed = forms
        .iter()
        .find(|form| {
            matches!(
                form.pattern().last().map(|argument| &argument.kind),
                Some(TagArgumentPatternKind::VariableExcept(value)) if value == "reversed"
            )
        })
        .expect("non-reversed form should exclude the reversed discriminator");
    assert_eq!(
        non_reversed
            .pattern()
            .iter()
            .map(|argument| &argument.kind)
            .collect::<Vec<_>>(),
        vec![
            &TagArgumentPatternKind::VariableWidth { minimum: 1 },
            &TagArgumentPatternKind::Literal("in".to_string()),
            &TagArgumentPatternKind::VariableExcept("reversed".to_string()),
        ]
    );

    let reversed = forms
        .iter()
        .find(|form| {
            matches!(
                form.pattern().last().map(|argument| &argument.kind),
                Some(TagArgumentPatternKind::Literal(value)) if value == "reversed"
            )
        })
        .expect("reversed form should require its discriminator");
    assert_eq!(
        reversed
            .pattern()
            .iter()
            .map(|argument| &argument.kind)
            .collect::<Vec<_>>(),
        vec![
            &TagArgumentPatternKind::VariableWidth { minimum: 0 },
            &TagArgumentPatternKind::Literal("in".to_string()),
            &TagArgumentPatternKind::Variable,
            &TagArgumentPatternKind::Literal("reversed".to_string()),
        ]
    );
    assert_eq!(non_reversed.pattern()[0].name, "arguments");
    assert_eq!(non_reversed.pattern()[2].name, "arg_from_end_1");
    assert_eq!(reversed.pattern()[0].name, "arguments");
    assert_eq!(reversed.pattern()[2].name, "arg_from_end_2");
    assert!(rule.diagnostic_messages.is_none());
    for form in [non_reversed, reversed] {
        assert!(matches!(
            &form.pattern()[1].mismatch_message,
            Some(ExtractedMessageTemplate::PercentFormat { template, args })
                if template == "'for' statements should use the format 'for x in y': %s"
                    && args == &[ExtractedMessageArg::TokenContents]
        ));
    }
}

#[test]
fn token_contents_message_argument_requires_the_tracked_token_value() {
    let accepted = [
        (
            "",
            "raise template.TemplateSyntaxError('bad %% form: %r' % token.contents)",
        ),
        (
            "source_token = token",
            "raise template.TemplateSyntaxError('bad %% form: %r' % source_token.contents)",
        ),
    ];
    for (prelude, raised) in accepted {
        let source = format!(
            "from django import template\nregister = template.Library()\n@register.tag('loop')\ndef compile_loop(parser, token):\n    bits = token.split_contents()\n    {prelude}\n    if bits[1] != 'in':\n        {raised}\n    return Node()\n"
        );
        let result = extract_source(&source, "message_tags").expect("fixture should extract");
        let rule = &result.tag_rules[&SymbolKey::tag("message_tags", "loop")];
        assert!(
            rule.diagnostic_messages.as_deref().is_some_and(|messages| {
                messages.iter().any(|message| {
                    matches!(
                        &message.message,
                        ExtractedMessageTemplate::PercentFormat { template, args }
                            if template == "bad %% form: %r"
                                && args == &[ExtractedMessageArg::TokenContents]
                    )
                })
            }),
            "raise body: {raised}; rule: {rule:#?}"
        );
    }

    let rejected = [
        (
            "other = runtime_token()",
            "raise template.TemplateSyntaxError('bad: %s' % other.contents)",
        ),
        (
            "token = runtime_token()",
            "raise template.TemplateSyntaxError('bad: %s' % token.contents)",
        ),
        (
            "token.contents = 'changed'",
            "raise template.TemplateSyntaxError('bad: %s' % token.contents)",
        ),
        (
            "",
            "raise template.TemplateSyntaxError('bad: %s' % runtime_contents())",
        ),
    ];
    for (prelude, raised) in rejected {
        let source = format!(
            "from django import template\nregister = template.Library()\n@register.tag('loop')\ndef compile_loop(parser, token):\n    bits = token.split_contents()\n    {prelude}\n    if bits[1] != 'in':\n        {raised}\n    return Node()\n"
        );
        let result = extract_source(&source, "message_tags").expect("fixture should extract");
        let rule = &result.tag_rules[&SymbolKey::tag("message_tags", "loop")];
        assert!(rule.diagnostic_messages.as_ref().is_none_or(Vec::is_empty));
    }
}

// Corpus: `widthratio` in defaulttags.py uses Django's real exhaustive
// `if len(bits) == 4 / elif len(bits) == 6 / else: raise` dispatch.
#[test]
fn corpus_len_exact_check() {
    let result = extract_source(DEFAULTTAGS_SOURCE, "django.template.defaulttags")
        .expect("exact-length extraction fixture should build");
    let key = SymbolKey::tag("django.template.defaulttags", "widthratio");
    assert!(
        result.tag_rules.contains_key(&key),
        "widthratio should be extracted"
    );
    let rule = &result.tag_rules[&key];
    let (forms, coverage) = rule
        .argument_syntax
        .forms()
        .expect("widthratio should retain correlated forms");
    assert_eq!(coverage, ArgumentFormCoverage::Complete);
    assert_eq!(
        forms
            .iter()
            .map(|form| form.pattern().len())
            .collect::<Vec<_>>(),
        vec![3, 5]
    );
    assert_eq!(
        forms[1].pattern()[3].kind,
        TagArgumentPatternKind::Literal("as".into())
    );
    assert_eq!(forms[1].pattern()[4].name, "asvar");
}

// Corpus: `cycle` in defaulttags.py — accepted forms retain the one-argument
// named-cycle branch while every projected form satisfies `len(args) >= 2`.
#[test]
fn corpus_len_min_check() {
    let result = extract_source(DEFAULTTAGS_SOURCE, "django.template.defaulttags")
        .expect("minimum-length extraction fixture should build");
    let key = SymbolKey::tag("django.template.defaulttags", "cycle");
    let rule = &result.tag_rules[&key];
    let (forms, _) = rule.argument_syntax.forms().expect("cycle forms");
    assert!(forms.iter().all(|form| form.minimum_len() >= 1));
    assert!(forms.iter().any(|form| form.exact_len() == Some(1)));
}

// Corpus: `templatetag` in defaulttags.py — `len(bits) != 2` → Exact(2).
// Real `debug` tag has no split_contents, so we use `templatetag` which
// has a clean `len(bits) != 2` check for the exact constraint pattern.
#[test]
fn corpus_len_exact_check_templatetag() {
    let result = extract_source(DEFAULTTAGS_SOURCE, "django.template.defaulttags")
        .expect("template-tag length extraction fixture should build");
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
    let result = extract_source(DEFAULTTAGS_SOURCE, "django.template.defaulttags")
        .expect("multiple-raise extraction fixture should build");
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
    let result = extract_source(LOADER_TAGS_SOURCE, "django.template.loader_tags")
        .expect("option-loop extraction fixture should build");
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
    let result = extract_source(DEFAULTTAGS_SOURCE, "django.template.defaulttags")
        .expect("for-tag extraction fixture should build");
    let key = SymbolKey::tag("django.template.defaulttags", "for");
    assert!(result.block_specs.as_map().contains_key(&key));
    let spec = &result.block_specs.as_map()[&key];
    assert_eq!(spec.end_tag.as_deref(), Some("endfor"));
    assert!(spec.intermediates.contains(&"empty".to_string()));
}

// Corpus: `do_if` in defaulttags.py — block with elif/else intermediates.
#[test]
fn corpus_block_with_intermediates() {
    let result = extract_source(DEFAULTTAGS_SOURCE, "django.template.defaulttags")
        .expect("intermediate-tag extraction fixture should build");
    let key = SymbolKey::tag("django.template.defaulttags", "if");
    assert!(result.block_specs.as_map().contains_key(&key));
    let spec = &result.block_specs.as_map()[&key];
    assert_eq!(spec.end_tag.as_deref(), Some("endif"));
    assert!(spec.intermediates.contains(&"elif".to_string()));
    assert!(spec.intermediates.contains(&"else".to_string()));
}

// Corpus: `comment` in defaulttags.py uses skip_past.
#[test]
fn corpus_opaque_block() {
    let result = extract_source(DEFAULTTAGS_SOURCE, "django.template.defaulttags")
        .expect("opaque-block extraction fixture should build");
    let key = SymbolKey::tag("django.template.defaulttags", "comment");
    assert!(result.block_specs.as_map().contains_key(&key));
    let spec = &result.block_specs.as_map()[&key];
    assert_eq!(spec.body_analysis_evidence, BodyAnalysisEvidence::SkipPast);
    assert_eq!(spec.end_tag.as_deref(), Some("endcomment"));
}

// Corpus: `verbatim` in defaulttags.py uses parser.parse(), so extraction
// records no skip evidence. Semantic builtin policy still makes its body opaque.
#[test]
fn corpus_verbatim_has_no_skip_evidence() {
    let result = extract_source(DEFAULTTAGS_SOURCE, "django.template.defaulttags")
        .expect("non-opaque extraction fixture should build");
    let key = SymbolKey::tag("django.template.defaulttags", "verbatim");
    assert!(result.block_specs.as_map().contains_key(&key));
    let spec = &result.block_specs.as_map()[&key];
    assert_eq!(
        spec.body_analysis_evidence,
        BodyAnalysisEvidence::NotDetected
    );
    assert_eq!(spec.end_tag.as_deref(), Some("endverbatim"));
}

// Corpus: `spaceless` in defaulttags.py — uses parser.parse(("endspaceless",))
// with a literal end tag.
#[test]
fn corpus_literal_end_tag() {
    let result = extract_source(DEFAULTTAGS_SOURCE, "django.template.defaulttags")
        .expect("literal-end-tag extraction fixture should build");
    let key = SymbolKey::tag("django.template.defaulttags", "spaceless");
    assert!(result.block_specs.as_map().contains_key(&key));
    let spec = &result.block_specs.as_map()[&key];
    assert_eq!(spec.end_tag.as_deref(), Some("endspaceless"));
}

// Edge case — genuinely unknowable dynamic f-string end tag through the full
// extraction path. Ensures ambiguous closers remain unknown instead of being
// re-synthesized from the registered tag name.
#[test]
fn ambiguous_closer_stays_unknown_after_extraction() {
    let source = r#"
from django import template
register = template.Library()

@register.tag("mystery")
def do_block(parser, token):
    options = {"name": "mystery"}
    nodelist = parser.parse((f"end{options['name']}",))
    parser.delete_first_token()
    return BlockNode(nodelist)
"#;
    let result = extract_source(source, "app.templatetags.custom")
        .expect("unknown-end-tag extraction fixture should build");
    let key = SymbolKey::tag("app.templatetags.custom", "mystery");
    let spec = &result.block_specs.as_map()[&key];
    assert!(spec.end_tag.is_none());
}

#[test]
fn self_named_dynamic_closer_concretizes_per_registration_name() {
    let source = r#"
from django import template
register = template.Library()

@register.tag("mystery")
def do_block(parser, token):
    tag_name, *rest = token.split_contents()
    nodelist = parser.parse((f"end{tag_name}",))
    parser.delete_first_token()
    return BlockNode(tag_name, nodelist)
"#;
    let result = extract_source(source, "app.templatetags.custom")
        .expect("conventional-end-tag extraction fixture should build");
    let key = SymbolKey::tag("app.templatetags.custom", "mystery");
    let spec = &result.block_specs.as_map()[&key];
    assert_eq!(spec.end_tag.as_deref(), Some("endmystery"));
}

// Corpus: `do_block` in loader_tags.py — simple block tag with endblock.
#[test]
fn corpus_simple_block() {
    let result = extract_source(LOADER_TAGS_SOURCE, "django.template.loader_tags")
        .expect("simple-block extraction fixture should build");
    let key = SymbolKey::tag("django.template.loader_tags", "block");
    assert!(result.block_specs.as_map().contains_key(&key));
    let spec = &result.block_specs.as_map()[&key];
    assert_eq!(spec.end_tag.as_deref(), Some("endblock"));
    assert!(spec.intermediates.is_empty());
    assert_eq!(
        spec.body_analysis_evidence,
        BodyAnalysisEvidence::NotDetected
    );
}

// Corpus: `title` in defaultfilters.py — filter with no arg (value only).
#[test]
fn corpus_filter_no_arg() {
    let result = extract_source(DEFAULTFILTERS_SOURCE, "django.template.defaultfilters")
        .expect("no-argument filter extraction fixture should build");
    let key = SymbolKey::filter("django.template.defaultfilters", "title");
    assert_eq!(
        result.filter_arities.get(&key),
        Some(&FilterArity::NoArgument)
    );
}

// Corpus: `default` in defaultfilters.py — filter with required arg.
#[test]
fn corpus_filter_required_arg() {
    let result = extract_source(DEFAULTFILTERS_SOURCE, "django.template.defaultfilters")
        .expect("required-argument filter extraction fixture should build");
    let key = SymbolKey::filter("django.template.defaultfilters", "default");
    assert_eq!(
        result.filter_arities.get(&key),
        Some(&FilterArity::RequiredArgument)
    );
}

// Corpus: `date` in defaultfilters.py — filter with optional arg (arg=None).
#[test]
fn corpus_filter_optional_arg() {
    let result = extract_source(DEFAULTFILTERS_SOURCE, "django.template.defaultfilters")
        .expect("optional-argument filter extraction fixture should build");
    let key = SymbolKey::filter("django.template.defaultfilters", "date");
    assert_eq!(
        result.filter_arities.get(&key),
        Some(&FilterArity::OptionalArgument)
    );
}

// Corpus: `escapejs` in defaultfilters.py — @register.filter("escapejs")
// with positional string name, bare filter decorator with no user arg.
#[test]
fn corpus_filter_bare_decorator() {
    let result = extract_source(DEFAULTFILTERS_SOURCE, "django.template.defaultfilters")
        .expect("bare filter-decorator extraction fixture should build");
    let key = SymbolKey::filter("django.template.defaultfilters", "lower");
    assert!(result.filter_arities.contains_key(&key));
}

// Corpus: `escapejs` in defaultfilters.py — @register.filter("escapejs")
// demonstrates named filter via positional string arg.
#[test]
fn corpus_filter_with_name() {
    let result = extract_source(DEFAULTFILTERS_SOURCE, "django.template.defaultfilters")
        .expect("named-filter extraction fixture should build");
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
    let result = extract_source(DEFAULTFILTERS_SOURCE, "django.template.defaultfilters")
        .expect("safe-filter extraction fixture should build");
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
    let result = extract_source(source, "app.templatetags.filters")
        .expect("call-style filter extraction fixture should build");
    insta::assert_yaml_snapshot!(
        sorted_snapshot(&result).expect("call-style filter extraction snapshot should serialize")
    );
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
    let result = extract_source(source, "app.templatetags.custom")
        .expect("custom parser extraction fixture should build");
    insta::assert_yaml_snapshot!(
        sorted_snapshot(&result).expect("custom parser extraction snapshot should serialize")
    );
}

// (b) Edge case — empty source
#[test]
fn golden_empty_source() {
    let result = extract_source("", "test.module").expect("empty extraction fixture should build");
    insta::assert_yaml_snapshot!(
        sorted_snapshot(&result).expect("empty extraction snapshot should serialize")
    );
}

// (b) Edge case — invalid Python
#[test]
fn golden_invalid_python() {
    let result = extract_source("def {invalid", "test.module")
        .expect("invalid-Python extraction fixture should build");
    insta::assert_yaml_snapshot!(
        sorted_snapshot(&result).expect("invalid-Python extraction snapshot should serialize")
    );
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
    let result = extract_source(source, "test.module")
        .expect("unregistered-source extraction fixture should build");
    insta::assert_yaml_snapshot!(
        sorted_snapshot(&result).expect("unregistered-source extraction snapshot should serialize")
    );
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
    let result = extract_source(source, "test.module")
        .expect("missing-definition extraction fixture should build");
    insta::assert_yaml_snapshot!(
        sorted_snapshot(&result).expect("missing-definition extraction snapshot should serialize")
    );
}

fn project_assignment_rule(
    caller_source: &str,
    alter_base: impl FnOnce(String) -> String,
) -> Result<Option<std::sync::Arc<djls_project::TagRule>>, Box<dyn std::error::Error>> {
    let corpus = Corpus::require()?;
    let base_path = corpus
        .root()
        .join("repos/django-5.2/django/template/base.py");
    let base = fs::read_to_string(base_path.as_std_path())?;
    let base = alter_base(base);
    let mut db = TestDatabase::new();
    ProjectFixture::new("/test/project")
        .django_settings_module("settings")
        .pythonpath("/test/site-packages")
        .file("/test/project/settings.py", "INSTALLED_APPS = []\n")
        .file("/test/project/app/__init__.py", "")
        .file("/test/project/app/templatetags/__init__.py", "")
        .file(
            "/test/project/app/implementation.py",
            r#"
def compile_assignment(parser, token):
    return object()

if enabled:
    globals()["compile_assignment"] = replacement
"#,
        )
        .file(
            "/test/project/app/helper2.py",
            "from django.template.base import token_kwargs\n",
        )
        .file("/test/project/app/templatetags/example.py", caller_source)
        .file("/test/site-packages/django/__init__.py", "")
        .file("/test/site-packages/django/template/__init__.py", "")
        .file("/test/site-packages/django/template/base.py", base)
        .install(&mut db)?;
    let file = db.file(Utf8Path::new("/test/project/app/templatetags/example.py"))?;
    let library = TemplateLibraryId::new(
        &db,
        Some(file),
        PythonModuleName::parse("app.templatetags.example")?,
    );
    Ok(template_library_tag_facts(&db, library)
        .tag_rules()
        .get(&SymbolKey::tag(
            "app.templatetags.example",
            "assignment_tag",
        ))
        .cloned())
}

const ASSIGNMENT_TAG_SOURCE: &str = r#"
from django import template
from django.template.base import token_kwargs
register = template.Library()

@register.tag(name="assignment_tag")
def compile_assignment(parser, token):
    bits = token.split_contents()[1:]
    values = token_kwargs(bits, parser, support_legacy=True)
    if not values:
        raise template.TemplateSyntaxError("assignment_tag needs assignments")
    if bits:
        raise template.TemplateSyntaxError("assignment_tag has trailing bits")
    return template.Node()
"#;

#[test]
fn canonical_token_kwargs_derives_assignment_operand_from_guards() {
    let rule = project_assignment_rule(ASSIGNMENT_TAG_SOURCE, |source| source)
        .expect("assignment fixture should install")
        .expect("canonical helper should derive a Tag Rule");
    assert!(
        matches!(
            &rule.argument_syntax,
            TagArgumentSyntax::Assignments { operand }
                if operand.mode == AssignmentMode::ModernOrLegacy
                    && operand.cardinality == UniqueKeyCardinality::AtLeastOne
                    && operand.remainder == RemainderPolicy::Reject
                    && operand.empty_message == Some(ExtractedMessageTemplate::Static(
                        "assignment_tag needs assignments".to_string()
                    ))
                    && operand.remainder_message == Some(ExtractedMessageTemplate::Static(
                        "assignment_tag has trailing bits".to_string()
                    ))
        ),
        "{rule:#?}"
    );
}

#[test]
fn canonical_assignment_helper_aliases_keep_the_same_operand() {
    for (import, call) in [
        (
            "from django.template.base import token_kwargs as parse_kwargs",
            "parse_kwargs",
        ),
        ("import django.template.base as base", "base.token_kwargs"),
        ("from app.helper2 import token_kwargs", "token_kwargs"),
    ] {
        let source = ASSIGNMENT_TAG_SOURCE
            .replace("from django.template.base import token_kwargs", import)
            .replace("values = token_kwargs(", &format!("values = {call}("));
        let rule = project_assignment_rule(&source, |source| source)
            .expect("assignment fixture should install")
            .expect("canonical alias should derive a rule");
        assert!(
            matches!(&rule.argument_syntax, TagArgumentSyntax::Assignments { operand }
            if operand.cardinality == UniqueKeyCardinality::AtLeastOne
                && operand.remainder == RemainderPolicy::Reject),
            "{import}: {rule:#?}"
        );
    }
}

#[test]
fn repeated_assignment_guards_keep_the_strongest_constraint_and_first_message() {
    let source = ASSIGNMENT_TAG_SOURCE.replace(
        "    if not values:",
        "    if len(values) != 1:\n        raise template.TemplateSyntaxError(\"exactly one\")\n    if not values:",
    ).replace(
        "    return template.Node()",
        "    if bits:\n        raise template.TemplateSyntaxError(\"later remainder\")\n    return template.Node()",
    );
    let rule = project_assignment_rule(&source, |source| source)
        .expect("assignment fixture should install")
        .expect("guarded assignment should derive a rule");
    assert!(
        matches!(&rule.argument_syntax, TagArgumentSyntax::Assignments { operand }
        if operand.cardinality == UniqueKeyCardinality::ExactlyOne
            && operand.empty_message == Some(ExtractedMessageTemplate::Static("exactly one".to_string()))
            && operand.multiple_message == Some(ExtractedMessageTemplate::Static("exactly one".to_string()))
            && operand.remainder_message == Some(ExtractedMessageTemplate::Static("assignment_tag has trailing bits".to_string()))),
        "{rule:#?}"
    );
}

#[test]
fn empty_and_multiple_assignment_guards_keep_their_own_messages() {
    let source = ASSIGNMENT_TAG_SOURCE.replace(
        "    if bits:",
        "    if len(values) != 1:\n        raise template.TemplateSyntaxError(\"exactly one\")\n    if bits:",
    );
    let rule = project_assignment_rule(&source, |source| source)
        .expect("assignment fixture should install")
        .expect("guarded assignment should derive a rule");
    assert!(
        matches!(&rule.argument_syntax, TagArgumentSyntax::Assignments { operand }
        if operand.cardinality == UniqueKeyCardinality::ExactlyOne
            && operand.empty_message == Some(ExtractedMessageTemplate::Static("assignment_tag needs assignments".to_string()))
            && operand.multiple_message == Some(ExtractedMessageTemplate::Static("exactly one".to_string()))),
        "{rule:#?}"
    );
}

#[test]
fn assignment_arguments_cannot_restore_a_mutated_input() {
    let source = ASSIGNMENT_TAG_SOURCE.replace(
        "token_kwargs(bits, parser, support_legacy=True)",
        "token_kwargs(bits, (consume(bits), parser)[1], support_legacy=True)",
    );
    let rule = project_assignment_rule(&source, |source| source)
        .expect("assignment fixture should install");
    assert!(
        rule.is_none_or(|rule| !matches!(
            rule.argument_syntax,
            TagArgumentSyntax::Assignments { .. }
        )),
        "later argument mutation must invalidate the earlier list value"
    );
}

#[test]
fn rebound_canonical_token_kwargs_is_not_native() {
    let rule = project_assignment_rule(ASSIGNMENT_TAG_SOURCE, |mut source| {
        source.push_str("\ndef replacement(bits, parser, support_legacy=False): return {}\ntoken_kwargs = replacement\n");
        source
    }).expect("assignment fixture should install");
    assert!(
        rule.is_none_or(|rule| !matches!(
            rule.argument_syntax,
            TagArgumentSyntax::Assignments { .. }
        ))
    );
}

#[test]
fn replaced_or_recovered_canonical_exports_do_not_gain_native_semantics() {
    for suffix in [
        "\ndef token_kwargs(bits, parser, support_legacy=False): return {}\n",
        "\nfrom app.implementation import compile_assignment as token_kwargs\n",
        "\ndef broken(\n",
    ] {
        let rule = project_assignment_rule(ASSIGNMENT_TAG_SOURCE, |mut source| {
            source.push_str(suffix);
            source
        })
        .expect("assignment fixture should install");
        assert!(
            rule.is_none_or(|rule| !matches!(
                rule.argument_syntax,
                TagArgumentSyntax::Assignments { .. }
            )),
            "unproven export: {suffix}"
        );
    }
}

#[test]
fn first_party_django_shadow_does_not_gain_native_assignment_semantics() {
    let corpus = Corpus::require().expect("synced corpus should be available");
    let base = fs::read_to_string(
        corpus
            .root()
            .join("repos/django-5.2/django/template/base.py"),
    )
    .expect("locked Django source should be readable");
    let mut db = TestDatabase::new();
    ProjectFixture::new("/test/project")
        .django_settings_module("settings")
        .file("/test/project/settings.py", "INSTALLED_APPS = []\n")
        .file("/test/project/django/__init__.py", "")
        .file("/test/project/django/template/__init__.py", "")
        .file("/test/project/django/template/base.py", base)
        .file("/test/project/tags.py", ASSIGNMENT_TAG_SOURCE)
        .install(&mut db)
        .expect("first-party shadow fixture should install");
    let file = db
        .file(Utf8Path::new("/test/project/tags.py"))
        .expect("library should exist");
    let library = TemplateLibraryId::new(
        &db,
        Some(file),
        PythonModuleName::parse("tags").expect("module name should be valid"),
    );
    assert!(
        template_library_definition_facts(&db, library)
            .symbol(TemplateSymbolKind::Tag, "assignment_tag")
            .is_some()
    );
    let rules = template_library_tag_facts(&db, library).tag_rules();
    assert!(
        rules
            .get(&SymbolKey::tag("tags", "assignment_tag"))
            .is_none_or(|rule| !matches!(
                rule.argument_syntax,
                TagArgumentSyntax::Assignments { .. }
            ))
    );
}

#[test]
fn uncertain_assignment_map_branch_does_not_prove_cardinality() {
    let source = ASSIGNMENT_TAG_SOURCE.replace(
        "    if not values:",
        "    if enabled:\n        values = fallback\n    if not values:",
    );
    let rule = project_assignment_rule(&source, |source| source)
        .expect("assignment fixture should install");
    assert!(rule.is_none_or(|rule| !matches!(&rule.argument_syntax,
        TagArgumentSyntax::Assignments { operand }
            if operand.cardinality != UniqueKeyCardinality::Any)));
}

#[test]
fn caller_parameter_shadowing_is_not_native() {
    let source = ASSIGNMENT_TAG_SOURCE.replace(
        "def compile_assignment(parser, token):",
        "def compile_assignment(parser, token, token_kwargs):",
    );
    let rule = project_assignment_rule(&source, |source| source)
        .expect("assignment fixture should install");
    assert!(
        rule.is_none_or(|rule| !matches!(
            rule.argument_syntax,
            TagArgumentSyntax::Assignments { .. }
        ))
    );
}

#[test]
fn compound_dynamic_module_write_keeps_registration_callable_unknown() {
    let source = r#"
from django import template
from app.implementation import compile_assignment
register = template.Library()
register.tag("assignment_tag", compile_assignment)
"#;
    assert!(
        project_assignment_rule(source, |source| source)
            .expect("assignment fixture should install")
            .is_none()
    );
}

#[test]
fn mutated_module_member_is_not_native_at_direct_or_aliased_call() {
    for call in [
        "base.token_kwargs(bits, parser, support_legacy=True)",
        "parse_kwargs(bits, parser, support_legacy=True)",
    ] {
        let source = ASSIGNMENT_TAG_SOURCE
            .replace(
                "from django.template.base import token_kwargs",
                "import django.template.base as base",
            )
            .replace(
                "    values = token_kwargs(bits, parser, support_legacy=True)",
                &format!(
                    "    base.token_kwargs = replacement\n    parse_kwargs = base.token_kwargs\n    values = {call}"
                ),
            );
        let rule = project_assignment_rule(&source, |source| source)
            .expect("assignment fixture should install");
        assert!(rule.is_none_or(|rule| {
            !matches!(rule.argument_syntax, TagArgumentSyntax::Assignments { .. })
        }));
    }
}

#[test]
fn mutated_assignment_map_does_not_prove_input_cardinality() {
    for mutation in [
        "    values['default'] = 1",
        "    alias = values\n    consume(alias)",
    ] {
        let source = ASSIGNMENT_TAG_SOURCE.replace(
            "    if not values:",
            &format!("{mutation}\n    if not values:"),
        );
        let rule = project_assignment_rule(&source, |source| source)
            .expect("assignment fixture should install");
        assert!(
            rule.is_none_or(|rule| !matches!(
                &rule.argument_syntax,
                TagArgumentSyntax::Assignments { operand }
                    if operand.cardinality != UniqueKeyCardinality::Any
            )),
            "map mutation must invalidate later cardinality evidence: {mutation}"
        );
    }
}

#[test]
fn mutated_assignment_remainder_does_not_prove_full_consumption() {
    for mutation in ["    bits[:] = []", "    alias = bits\n    consume(alias)"] {
        let source = ASSIGNMENT_TAG_SOURCE.replace(
            "    if not values:",
            &format!("{mutation}\n    if not values:"),
        );
        let rule = project_assignment_rule(&source, |source| source)
            .expect("assignment fixture should install");
        assert!(
            rule.is_none_or(|rule| !matches!(
                &rule.argument_syntax,
                TagArgumentSyntax::Assignments { operand }
                    if operand.remainder == RemainderPolicy::Reject
            )),
            "remainder mutation must invalidate later consumption evidence: {mutation}"
        );
    }
}
