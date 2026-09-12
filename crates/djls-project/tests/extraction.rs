use camino::Utf8Path;
use djls_project::ArgumentCountConstraint;
use djls_project::ArgumentFormCoverage;
use djls_project::BodyAnalysisEvidence;
use djls_project::FilterArity;
use djls_project::ParameterRequirement;
use djls_project::PythonModuleName;
use djls_project::SymbolKey;
use djls_project::TagArgumentKind;
use djls_project::TagArgumentSyntax;
use djls_project::TemplateLibraryId;
use djls_project::TemplateSymbolKind;
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
                .is_none_or(|rule| rule.arg_constraints.is_empty())
        );
    }
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
    let key = SymbolKey::tag("helper_returns", "early");
    assert!(
        result.tag_rules.contains_key(&key),
        "{:#?}",
        result.tag_rules
    );
    assert_eq!(
        result.tag_rules[&key].arg_constraints,
        vec![ArgumentCountConstraint::Exact(2)]
    );
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
        assert!(
            template_library_definition_facts(&db, key)
                .symbol(TemplateSymbolKind::Tag, "before")
                .is_some()
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
    assert!(
        definitions
            .symbol(TemplateSymbolKind::Tag, "after")
            .is_some()
    );
    assert_eq!(
        template_library_tag_facts(&db, key).tag_rules()[&SymbolKey::tag("pkg.tags", "after")]
            .arg_constraints,
        vec![ArgumentCountConstraint::Exact(3)]
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
            .map(|form| form.arguments.len())
            .collect::<Vec<_>>(),
        vec![3, 5]
    );
    assert_eq!(
        forms[1].arguments[3].kind,
        TagArgumentKind::Literal("as".into())
    );
    assert_eq!(forms[1].arguments[4].name, "asvar");
}

// Corpus: `cycle` in defaulttags.py — `len(args) < 2` → Min(2).
#[test]
fn corpus_len_min_check() {
    let result = extract_source(DEFAULTTAGS_SOURCE, "django.template.defaulttags")
        .expect("minimum-length extraction fixture should build");
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
