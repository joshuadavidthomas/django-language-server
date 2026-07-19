use camino::Utf8Path;
use djls_project::ArgumentCountConstraint;
use djls_project::PythonModuleName;
use djls_project::SymbolKey;
use djls_project::TemplateLibraryKey;
use djls_project::TemplateSymbolKind;
use djls_project::template_library_definition_facts;
use djls_project::template_library_filter_facts;
use djls_project::template_library_tag_facts;
use djls_project::testing::PythonSyntaxErrorClass;
use djls_project::testing::python_syntax_errors;
use djls_source::ChangeEvent;
use djls_source::SourceChanges;
use djls_source::Span;
use djls_testing::ExtractionBundle;
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

fn extract_source(source: &str, module_name: &str) -> ExtractionBundle {
    let db = TestDatabase::new();
    let path = Utf8Path::new("/test/extraction.py");
    db.add_file(path.as_str(), source);
    let file = db.file(path);
    extract_bundle(&db, file, PythonModuleName::parse(module_name).unwrap())
}

fn execution_count(db: &TestDatabase, events: &[salsa::Event], query_name: &str) -> usize {
    events
        .iter()
        .filter(|event| match &event.kind {
            salsa::EventKind::WillExecute { database_key } => db
                .ingredient_debug_name(database_key.ingredient_index())
                .ends_with(query_name),
            _ => false,
        })
        .count()
}

// Corpus: `no_params` in tests/template_tests/templatetags/custom.py —
// `@register.simple_tag` with no user args, exercises simple_tag pipeline
#[test]
fn extract_bundle_simple_tag() {
    let result = extract_source(CUSTOM_SOURCE, "tests.template_tests.templatetags.custom");
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
    let result = extract_source(DEFAULTFILTERS_SOURCE, "django.template.defaultfilters");
    let key = SymbolKey::filter("django.template.defaultfilters", "lower");
    assert!(result.filter_arities.contains_key(&key));
    let arity = &result.filter_arities[&key];
    assert!(!arity.expects_arg);
}

// Corpus: `default` in django/template/defaultfilters.py — filter with
// required arg (value, arg)
#[test]
fn extract_bundle_filter_with_arg() {
    let result = extract_source(DEFAULTFILTERS_SOURCE, "django.template.defaultfilters");
    let key = SymbolKey::filter("django.template.defaultfilters", "default");
    assert!(result.filter_arities.contains_key(&key));
    let arity = &result.filter_arities[&key];
    assert!(arity.expects_arg);
    assert!(!arity.arg_optional);
}

// Corpus: `block` in django/template/loader_tags.py — `@register.tag("block")`
// with parser.parse(("endblock",)) block spec
#[test]
fn extract_bundle_block_tag() {
    let result = extract_source(LOADER_TAGS_SOURCE, "django.template.loader_tags");
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
    let result = extract_source("", "test.module");
    assert!(result.is_empty());
}

// (b) Edge case — invalid Python returns empty result
#[test]
fn extract_bundle_invalid_python() {
    let result = extract_source("def {invalid python", "test.module");
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
    db.add_file(path.as_str(), source);
    let file = db.file(path);
    let module_name = PythonModuleName::parse("test.templatetags.known").unwrap();

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
        Span::new(u32::try_from(source.len()).unwrap(), 0)
    );
    assert!(!errors[0].message.is_empty());
}

#[test]
fn parser_distinguishes_empty_python_from_non_python() {
    let db = TestDatabase::new();
    db.add_file("/test/empty.py", "");
    db.add_file("/test/notes.txt", "");

    assert_eq!(
        python_syntax_errors(&db, db.file(Utf8Path::new("/test/empty.py"))),
        Some(Vec::new())
    );
    assert_eq!(
        python_syntax_errors(&db, db.file(Utf8Path::new("/test/notes.txt"))),
        None
    );
}

#[test]
fn comment_only_edit_backdates_parsed_body_consumers() {
    let event_log = SalsaEventLog::default();
    let mut db = TestDatabase::with_event_log(event_log.clone());
    let path = Utf8Path::new("/test/templatetags/known.py");
    let source = "from django import template\nregister = template.Library()\n@register.simple_tag\ndef known():\n    return 'known'\n";
    db.add_file(path.as_str(), source);
    let file = db.file(path);
    let module_name = PythonModuleName::parse("test.templatetags.known").unwrap();

    let key = TemplateLibraryKey::new(&db, Some(file), module_name);
    assert!(!template_library_tag_facts(&db, key).tag_rules().is_empty());
    let _ = event_log.take();

    db.add_file(path.as_str(), &format!("{source}# comment only\n"));
    SourceChanges::new([ChangeEvent::ContentChanged(path.to_path_buf())]).apply(&mut db);

    assert!(!template_library_tag_facts(&db, key).tag_rules().is_empty());
    let events = event_log.take();
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

    db.add_file("/test/defaulttags.py", DEFAULTTAGS_SOURCE);
    let tags_file = db.file(Utf8Path::new("/test/defaulttags.py"));
    let tags_module = PythonModuleName::parse("django.template.defaulttags").unwrap();
    let tags_key = TemplateLibraryKey::new(&db, Some(tags_file), tags_module);
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

    let events = event_log.take();
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

    db.add_file("/test/defaultfilters.py", DEFAULTFILTERS_SOURCE);
    let filters_file = db.file(Utf8Path::new("/test/defaultfilters.py"));
    let filters_key = TemplateLibraryKey::new(
        &db,
        Some(filters_file),
        PythonModuleName::parse("django.template.defaultfilters").unwrap(),
    );
    let filters = template_library_filter_facts(&db, filters_key);
    assert!(
        filters
            .filter_arities()
            .keys()
            .any(|key| key.name == "lower"
                && key.registration_module == "django.template.defaultfilters")
    );

    let events = event_log.take();
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
        execution_count(&db, &event_log.take(), "template_library_filter_facts"),
        0,
        "same-revision extraction should be memoized",
    );
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
    let result = extract_source(source, "test.module");
    assert!(result.is_empty());
}

// Corpus: defaulttags.py has both tags and filters (via `cycle` tag +
// querystring simple_tag). Validates multiple registration kinds extracted.
#[test]
fn extract_bundle_multiple_registrations() {
    let result = extract_source(DEFAULTTAGS_SOURCE, "django.template.defaulttags");
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
    let result = extract_source(source, "test.module");
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
    let result = extract_source(DEFAULTTAGS_SOURCE, "django.template.defaulttags");
    insta::assert_yaml_snapshot!(sorted_snapshot(&result));
}

// Corpus: django/template/loader_tags.py — block, extends, include tags.
// Exercises simple block (endblock), option loop (include with/only),
// and non-block tags (extends).
#[test]
fn golden_loader_tags() {
    let result = extract_source(LOADER_TAGS_SOURCE, "django.template.loader_tags");
    insta::assert_yaml_snapshot!(sorted_snapshot(&result));
}

// Corpus: django/template/defaultfilters.py — all built-in filters.
// Exercises @register.filter (bare), @register.filter("name"),
// @register.filter(is_safe=True), filters with no arg, required arg,
// and optional arg (default parameter).
#[test]
fn golden_defaultfilters() {
    let result = extract_source(DEFAULTFILTERS_SOURCE, "django.template.defaultfilters");
    insta::assert_yaml_snapshot!(sorted_snapshot(&result));
}

// Corpus: django/templatetags/i18n.py — i18n tags.
// Exercises @register.tag("name"), @register.filter, and the
// blocktranslate next_token loop pattern.
#[test]
fn golden_i18n() {
    let result = extract_source(I18N_SOURCE, "django.templatetags.i18n");
    insta::assert_yaml_snapshot!(sorted_snapshot(&result));
}

// Corpus: tests/template_tests/templatetags/inclusion.py — inclusion tags.
// Exercises @register.inclusion_tag with and without takes_context,
// various arg counts, and keyword-only defaults.
#[test]
fn golden_inclusion_tags() {
    let result = extract_source(
        INCLUSION_SOURCE,
        "tests.template_tests.templatetags.inclusion",
    );
    insta::assert_yaml_snapshot!(sorted_snapshot(&result));
}

// Corpus: tests/template_tests/templatetags/custom.py — simple tags.
// Exercises @register.simple_tag with and without takes_context,
// @register.simple_tag(name="..."), @register.simple_block_tag,
// @register.filter, and various arg patterns.
#[test]
fn golden_custom_tags() {
    let result = extract_source(CUSTOM_SOURCE, "tests.template_tests.templatetags.custom");
    insta::assert_yaml_snapshot!(sorted_snapshot(&result));
}

// Corpus: tests/template_tests/templatetags/testtags.py — call-style
// registrations. Exercises register.tag("name", func) and
// register.filter("name", func) call-style patterns.
#[test]
fn golden_testtags() {
    let result = extract_source(
        TESTTAGS_SOURCE,
        "tests.template_tests.templatetags.testtags",
    );
    insta::assert_yaml_snapshot!(sorted_snapshot(&result));
}

// Corpus: django-allauth/allauth/templatetags/allauth.py — custom block tag.
// Exercises helper-based argument parsing and explicit end tag extraction.
#[test]
fn golden_allauth_tags() {
    let result = extract_source(ALLAUTH_TAGS_SOURCE, "allauth.templatetags.allauth");
    insta::assert_yaml_snapshot!(sorted_snapshot(&result));
}

// Corpus: wagtail/admin/templatetags/wagtailadmin_tags.py — call-style
// registrations. Exercises register.tag("name", Class.handle) and
// register.filter("name", func) without local function definitions.
#[test]
fn golden_wagtailadmin_tags() {
    let result = extract_source(
        WAGTAILADMIN_TAGS_SOURCE,
        "wagtail.admin.templatetags.wagtailadmin_tags",
    );
    insta::assert_yaml_snapshot!(sorted_snapshot(&result));
}

// Corpus: django/templatetags/tz.py — timezone tags.
// Exercises simple tags and block tags with conventional end tags.
#[test]
fn golden_django_tz() {
    let result = extract_source(TZ_SOURCE, "django.templatetags.tz");
    insta::assert_yaml_snapshot!(sorted_snapshot(&result));
}

// Corpus: django/contrib/admin/templatetags/admin_urls.py — admin URL helpers.
// Exercises simple_tag with takes_context and optional function parameters.
#[test]
fn golden_django_admin_urls() {
    let result = extract_source(
        ADMIN_URLS_SOURCE,
        "django.contrib.admin.templatetags.admin_urls",
    );
    insta::assert_yaml_snapshot!(sorted_snapshot(&result));
}

// Pattern-specific corpus assertions — validate specific extraction
// behaviors using real Django code, complementing the full-module snapshots.

// Corpus: `autoescape` in defaulttags.py — bare @register.tag decorator.
// Registration name defaults to function name.
#[test]
fn corpus_decorator_bare_tag() {
    let result = extract_source(DEFAULTTAGS_SOURCE, "django.template.defaulttags");
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
    let result = extract_source(DEFAULTTAGS_SOURCE, "django.template.defaulttags");
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
    let result = extract_source(DEFAULTTAGS_SOURCE, "django.template.defaulttags");
    let key = SymbolKey::tag("django.template.defaulttags", "partialdef");
    assert!(
        result.tag_rules.contains_key(&key) || result.block_specs.as_map().contains_key(&key),
        "partialdef should be extracted (name from kwarg)"
    );
}

// Corpus: `no_params` in custom.py — @register.simple_tag with zero user args.
#[test]
fn corpus_simple_tag_no_args() {
    let result = extract_source(CUSTOM_SOURCE, "tests.template_tests.templatetags.custom");
    let key = SymbolKey::tag("tests.template_tests.templatetags.custom", "no_params");
    assert!(result.tag_rules.contains_key(&key));
    let rule = &result.tag_rules[&key];
    assert!(rule.extracted_args.is_empty());
}

// Corpus: `one_param` in custom.py — @register.simple_tag with one required arg.
#[test]
fn corpus_simple_tag_with_args() {
    let result = extract_source(CUSTOM_SOURCE, "tests.template_tests.templatetags.custom");
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
    let result = extract_source(CUSTOM_SOURCE, "tests.template_tests.templatetags.custom");
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
    let result = extract_source(
        INCLUSION_SOURCE,
        "tests.template_tests.templatetags.inclusion",
    );
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
    let result = extract_source(
        INCLUSION_SOURCE,
        "tests.template_tests.templatetags.inclusion",
    );
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
    let result = extract_source(
        INCLUSION_SOURCE,
        "tests.template_tests.templatetags.inclusion",
    );
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
    let result = extract_source(DEFAULTTAGS_SOURCE, "django.template.defaulttags");
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
    let result = extract_source(DEFAULTTAGS_SOURCE, "django.template.defaulttags");
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
    let result = extract_source(DEFAULTTAGS_SOURCE, "django.template.defaulttags");
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
    let result = extract_source(DEFAULTTAGS_SOURCE, "django.template.defaulttags");
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
    let result = extract_source(DEFAULTTAGS_SOURCE, "django.template.defaulttags");
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
    let result = extract_source(LOADER_TAGS_SOURCE, "django.template.loader_tags");
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
    let result = extract_source(DEFAULTTAGS_SOURCE, "django.template.defaulttags");
    let key = SymbolKey::tag("django.template.defaulttags", "for");
    assert!(result.block_specs.as_map().contains_key(&key));
    let spec = &result.block_specs.as_map()[&key];
    assert_eq!(spec.end_tag.as_deref(), Some("endfor"));
    assert!(spec.intermediates.contains(&"empty".to_string()));
}

// Corpus: `do_if` in defaulttags.py — block with elif/else intermediates.
#[test]
fn corpus_block_with_intermediates() {
    let result = extract_source(DEFAULTTAGS_SOURCE, "django.template.defaulttags");
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
    let result = extract_source(DEFAULTTAGS_SOURCE, "django.template.defaulttags");
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
    let result = extract_source(DEFAULTTAGS_SOURCE, "django.template.defaulttags");
    let key = SymbolKey::tag("django.template.defaulttags", "verbatim");
    assert!(result.block_specs.as_map().contains_key(&key));
    let spec = &result.block_specs.as_map()[&key];
    assert!(
        !spec.opaque,
        "real verbatim uses parser.parse(), not skip_past"
    );
    assert_eq!(spec.end_tag.as_deref(), Some("endverbatim"));
}

// Corpus: `spaceless` in defaulttags.py — uses parser.parse(("endspaceless",))
// with a literal end tag.
#[test]
fn corpus_literal_end_tag() {
    let result = extract_source(DEFAULTTAGS_SOURCE, "django.template.defaulttags");
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
    let result = extract_source(source, "app.templatetags.custom");
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
    let result = extract_source(source, "app.templatetags.custom");
    let key = SymbolKey::tag("app.templatetags.custom", "mystery");
    let spec = &result.block_specs.as_map()[&key];
    assert_eq!(spec.end_tag.as_deref(), Some("endmystery"));
}

// Corpus: `do_block` in loader_tags.py — simple block tag with endblock.
#[test]
fn corpus_simple_block() {
    let result = extract_source(LOADER_TAGS_SOURCE, "django.template.loader_tags");
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
    let result = extract_source(DEFAULTFILTERS_SOURCE, "django.template.defaultfilters");
    let key = SymbolKey::filter("django.template.defaultfilters", "title");
    assert!(result.filter_arities.contains_key(&key));
    let arity = &result.filter_arities[&key];
    assert!(!arity.expects_arg);
}

// Corpus: `default` in defaultfilters.py — filter with required arg.
#[test]
fn corpus_filter_required_arg() {
    let result = extract_source(DEFAULTFILTERS_SOURCE, "django.template.defaultfilters");
    let key = SymbolKey::filter("django.template.defaultfilters", "default");
    assert!(result.filter_arities.contains_key(&key));
    let arity = &result.filter_arities[&key];
    assert!(arity.expects_arg);
    assert!(!arity.arg_optional);
}

// Corpus: `date` in defaultfilters.py — filter with optional arg (arg=None).
#[test]
fn corpus_filter_optional_arg() {
    let result = extract_source(DEFAULTFILTERS_SOURCE, "django.template.defaultfilters");
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
    let result = extract_source(DEFAULTFILTERS_SOURCE, "django.template.defaultfilters");
    let key = SymbolKey::filter("django.template.defaultfilters", "lower");
    assert!(result.filter_arities.contains_key(&key));
}

// Corpus: `escapejs` in defaultfilters.py — @register.filter("escapejs")
// demonstrates named filter via positional string arg.
#[test]
fn corpus_filter_with_name() {
    let result = extract_source(DEFAULTFILTERS_SOURCE, "django.template.defaultfilters");
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
    let result = extract_source(DEFAULTFILTERS_SOURCE, "django.template.defaultfilters");
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
    let result = extract_source(source, "app.templatetags.filters");
    insta::assert_yaml_snapshot!(sorted_snapshot(&result));
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
    let result = extract_source(source, "app.templatetags.custom");
    insta::assert_yaml_snapshot!(sorted_snapshot(&result));
}

// (b) Edge case — empty source
#[test]
fn golden_empty_source() {
    let result = extract_source("", "test.module");
    insta::assert_yaml_snapshot!(sorted_snapshot(&result));
}

// (b) Edge case — invalid Python
#[test]
fn golden_invalid_python() {
    let result = extract_source("def {invalid", "test.module");
    insta::assert_yaml_snapshot!(sorted_snapshot(&result));
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
    let result = extract_source(source, "test.module");
    insta::assert_yaml_snapshot!(sorted_snapshot(&result));
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
    let result = extract_source(source, "test.module");
    insta::assert_yaml_snapshot!(sorted_snapshot(&result));
}
