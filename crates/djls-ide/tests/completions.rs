use std::borrow::Cow;
use std::collections::HashMap;

use camino::Utf8Path;
use djls_ide::completion;
use djls_project::StaticKnowledge;
use djls_project::TemplateLibraries;
use djls_semantic::TagSpec;
use djls_semantic::TagSpecs;
use djls_source::Offset;
use djls_source::PositionEncoding;
use djls_testing::TestDatabase;
use djls_testing::builtin_tag;
use djls_testing::library_filter;
use djls_testing::library_tag;
use djls_testing::make_template_libraries_with_knowledge;
use tower_lsp_server::ls_types;

fn tag_libraries() -> TemplateLibraries {
    tag_libraries_with_knowledge(StaticKnowledge::Known)
}

fn tag_libraries_with_knowledge(knowledge: StaticKnowledge) -> TemplateLibraries {
    let tags = vec![
        builtin_tag("if", "django.template.defaulttags"),
        library_tag("trans", "i18n", "django.templatetags.i18n"),
        library_tag("blocktrans", "i18n", "django.templatetags.i18n"),
    ];
    let libraries = HashMap::from([(
        "i18n".to_string(),
        "django.templatetags.i18n".to_string(),
    )]);
    let builtins = vec!["django.template.defaulttags".to_string()];

    make_template_libraries_with_knowledge(&tags, &[], &libraries, &builtins, knowledge)
}

fn filter_libraries() -> TemplateLibraries {
    let filters = vec![library_filter(
        "trans",
        "i18n",
        "django.templatetags.i18n",
    )];
    let libraries = HashMap::from([(
        "i18n".to_string(),
        "django.templatetags.i18n".to_string(),
    )]);

    make_template_libraries_with_knowledge(
        &[],
        &filters,
        &libraries,
        &[],
        StaticKnowledge::Known,
    )
}

fn project_only_specs() -> TagSpecs {
    let mut specs = TagSpecs::default();
    specs.insert(
        "project_only".to_string(),
        TagSpec::new(
            Cow::Borrowed("project.templatetags.project_only"),
            None,
            Cow::Borrowed(&[]),
            false,
        ),
    );
    specs
}

fn source_and_offset(marked_source: &str) -> (String, Offset) {
    let offset = marked_source
        .find('§')
        .expect("test source should contain a cursor marker");
    let mut source = marked_source.to_string();
    source.remove(offset);
    (source, Offset::new(u32::try_from(offset).unwrap()))
}

fn completion_labels(
    marked_source: &str,
    template_libraries: TemplateLibraries,
    tag_specs: TagSpecs,
) -> Vec<String> {
    let (source, offset) = source_and_offset(marked_source);
    let db = TestDatabase::new()
        .with_template_libraries(template_libraries)
        .with_specs(tag_specs);
    db.add_file("template.html", &source);
    let file = db.get_or_create_file(Utf8Path::new("template.html"));

    let Some(response) = completion(&db, file, offset, PositionEncoding::Utf16, false) else {
        return Vec::new();
    };

    let items = match response {
        ls_types::CompletionResponse::Array(items) => items,
        ls_types::CompletionResponse::List(list) => list.items,
    };
    items.into_iter().map(|item| item.label).collect()
}

#[test]
fn tag_completions_respect_load_position() {
    let before_load = completion_labels(
        "{% § %}\n{% load i18n %}",
        tag_libraries(),
        TagSpecs::default(),
    );
    let mut after_load = completion_labels(
        "{% load i18n %}\n{% § %}",
        tag_libraries(),
        TagSpecs::default(),
    );
    after_load.sort_unstable();

    assert_eq!(before_load, vec!["if"]);
    assert_eq!(after_load, vec!["blocktrans", "if", "trans"]);
}

#[test]
fn partial_tag_completions_use_known_libraries_not_raw_specs() {
    let libraries = tag_libraries_with_knowledge(StaticKnowledge::Partial);

    let labels = completion_labels("{% project§ %}", libraries, project_only_specs());

    assert!(labels.is_empty());
}

#[test]
fn filter_completions_respect_load_position() {
    let before_load = completion_labels(
        "{{ value|tr§ }}\n{% load i18n %}",
        filter_libraries(),
        TagSpecs::default(),
    );
    let after_load = completion_labels(
        "{% load i18n %}\n{{ value|tr§ }}",
        filter_libraries(),
        TagSpecs::default(),
    );

    assert!(before_load.is_empty());
    assert_eq!(after_load, vec!["trans"]);
}
