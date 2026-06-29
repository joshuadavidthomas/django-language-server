use std::borrow::Cow;

use camino::Utf8Path;
use djls_ide::completion;
use djls_project::BuiltinLibrarySource;
use djls_project::LibraryName;
use djls_project::LoadableLibrarySource;
use djls_project::PythonModulePath;
use djls_project::StaticKnowledge;
use djls_project::SymbolDefinition;
use djls_project::TemplateLibraries;
use djls_project::TemplateSymbol;
use djls_project::TemplateSymbolKind;
use djls_project::TemplateSymbolName;
use djls_semantic::TagSpec;
use djls_semantic::TagSpecs;
use djls_source::Offset;
use djls_source::PositionEncoding;
use djls_testing::TestDatabase;
use tower_lsp_server::ls_types;

fn template_symbol(
    kind: TemplateSymbolKind,
    name: &str,
    module: &PythonModulePath,
) -> TemplateSymbol {
    TemplateSymbol {
        kind,
        name: TemplateSymbolName::parse(name).unwrap(),
        definition: SymbolDefinition::Module(module.clone()),
        doc: None,
    }
}

fn tag_libraries() -> TemplateLibraries {
    let builtin_module = PythonModulePath::parse("django.template.defaulttags").unwrap();
    let i18n_name = LibraryName::parse("i18n").unwrap();
    let i18n_module = PythonModulePath::parse("django.templatetags.i18n").unwrap();

    TemplateLibraries::builder()
        .knowledge(StaticKnowledge::Known)
        .builtin_untracked(
            BuiltinLibrarySource::DjangoDefault,
            builtin_module.clone(),
            true,
            vec![template_symbol(
                TemplateSymbolKind::Tag,
                "if",
                &builtin_module,
            )],
        )
        .loadable_untracked(
            i18n_name,
            LoadableLibrarySource::ConfiguredAlias,
            i18n_module.clone(),
            true,
            vec![
                template_symbol(TemplateSymbolKind::Tag, "trans", &i18n_module),
                template_symbol(TemplateSymbolKind::Tag, "blocktrans", &i18n_module),
            ],
        )
        .build()
}

fn filter_libraries() -> TemplateLibraries {
    let library_name = LibraryName::parse("i18n").unwrap();
    let module = PythonModulePath::parse("django.templatetags.i18n").unwrap();

    TemplateLibraries::builder()
        .knowledge(StaticKnowledge::Known)
        .loadable_untracked(
            library_name,
            LoadableLibrarySource::ConfiguredAlias,
            module.clone(),
            true,
            vec![template_symbol(
                TemplateSymbolKind::Filter,
                "trans",
                &module,
            )],
        )
        .build()
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
    let libraries = tag_libraries().with_knowledge(StaticKnowledge::Partial);

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
