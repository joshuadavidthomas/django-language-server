use camino::Utf8Path;
use djls_ide::collect_folding_ranges;
use djls_ide::document_symbols;
use djls_semantic::BodyAnalysis;
use djls_semantic::EndTag;
use djls_semantic::IntermediateTag;
use djls_semantic::TagSpec;
use djls_semantic::bundled_django_tag_specs;
use djls_source::ChangeEvent;
use djls_source::PositionEncoding;
use djls_source::SourceChanges;
use djls_testing::ProjectFixture;
use djls_testing::ProjectSettings;
use djls_testing::TestDatabase;
use tower_lsp_server::ls_types;

#[test]
fn folding_ranges_include_top_level_and_nested_template_blocks() {
    let source = r"{% load static %}
{% load i18n %}
{# comment
continued #}
<!DOCTYPE html>
<title>
  {% block title %}
    Django Test App
  {% endblock %}
</title>
<main>
  {% block content %}
    {% if items %}
      <ul>
        {% for item in items %}
          <li>{{ item.name }}</li>
        {% endfor %}
      </ul>
    {% else %}
      <p>No items found.</p>
    {% endif %}
  {% endblock %}
</main>
";
    let db = TestDatabase::new();
    db.add_file("template.html", source)
        .expect("template fixture should be added");
    let file = db
        .file(Utf8Path::new("template.html"))
        .expect("template fixture file should exist");
    let ranges = collect_folding_ranges(&db, file);

    let expected = [
        (0, 1, ls_types::FoldingRangeKind::Imports),
        (2, 3, ls_types::FoldingRangeKind::Comment),
        (6, 8, ls_types::FoldingRangeKind::Region),
        (11, 21, ls_types::FoldingRangeKind::Region),
        (12, 20, ls_types::FoldingRangeKind::Region),
        (14, 16, ls_types::FoldingRangeKind::Region),
    ]
    .into_iter()
    .map(|(start_line, end_line, kind)| ls_types::FoldingRange {
        start_line,
        end_line,
        start_character: None,
        end_character: None,
        kind: Some(kind),
        collapsed_text: None,
    })
    .collect::<Vec<_>>();
    assert_eq!(ranges, expected);
}

#[test]
fn folding_ranges_ignore_closer_looking_tags_inside_opaque_content() {
    let source = r"{% if outer %}
{% verbatim %}
{% endif %}
body
{% endverbatim %}
{% endif %}
";
    let db = TestDatabase::new();
    db.add_file("template.html", source)
        .expect("template fixture should be added");
    let file = db
        .file(Utf8Path::new("template.html"))
        .expect("template fixture file should exist");
    let ranges = collect_folding_ranges(&db, file);

    let expected = [(0, 5), (1, 4)]
        .into_iter()
        .map(|(start_line, end_line)| ls_types::FoldingRange {
            start_line,
            end_line,
            start_character: None,
            end_character: None,
            kind: Some(ls_types::FoldingRangeKind::Region),
            collapsed_text: None,
        })
        .collect::<Vec<_>>();
    assert_eq!(ranges, expected);
}

#[test]
fn opaque_comment_and_verbatim_folds_keep_their_kinds() {
    let source = "{% comment %}\n{% if ignored %}\n{% endcomment %}\n{% verbatim %}\nraw\n{% endverbatim %}\n";
    let projectless = TestDatabase::new();
    projectless
        .add_file("template.html", source)
        .expect("fixture should be added");
    let mut project = TestDatabase::new();
    ProjectFixture::new("/project")
        .settings(&ProjectSettings {
            dirs: vec!["/project/templates".to_string()],
            ..ProjectSettings::default()
        })
        .file("/project/templates/template.html", source)
        .install(&mut project)
        .expect("project fixture should install");

    for (db, path) in [
        (&projectless, "template.html"),
        (&project, "/project/templates/template.html"),
    ] {
        let file = db.file(Utf8Path::new(path)).expect("fixture file");
        let ranges = collect_folding_ranges(db, file);
        assert_eq!(
            ranges
                .iter()
                .map(|range| (range.start_line, range.end_line, range.kind.clone()))
                .collect::<Vec<_>>(),
            [
                (0, 2, Some(ls_types::FoldingRangeKind::Comment)),
                (3, 5, Some(ls_types::FoldingRangeKind::Region)),
            ]
        );
    }
}

#[test]
fn opaque_opener_with_intermediate_produces_comment_fold() {
    let mut specs = bundled_django_tag_specs().clone();
    specs.insert(
        "opaque_if".to_string(),
        TagSpec::new(
            "test".into(),
            Some(EndTag {
                name: "endopaque_if".into(),
                required: true,
            }),
            vec![IntermediateTag {
                name: "opaque_else".into(),
            }]
            .into(),
            BodyAnalysis::Opaque,
        ),
    );
    let db = TestDatabase::new().with_projectless_tag_specs(specs);
    db.add_file(
        "template.html",
        "{% opaque_if %}\nfirst\n{% opaque_else %}\nsecond\n{% endopaque_if %}",
    )
    .expect("fixture should be added");
    let file = db
        .file(Utf8Path::new("template.html"))
        .expect("fixture file");

    assert_eq!(
        collect_folding_ranges(&db, file),
        [ls_types::FoldingRange {
            start_line: 0,
            end_line: 4,
            start_character: None,
            end_character: None,
            kind: Some(ls_types::FoldingRangeKind::Comment),
            collapsed_text: None,
        }]
    );
}

#[test]
fn project_backed_folds_and_symbols_follow_presentation_edits() {
    let path = "/project/templates/page.html";
    let body = "{% block content %}\n  é😀 <b>{{ user|default:'x' }}</b>\n  {% include 'card.html' %}\n{% endblock %}\n";
    let before = format!("text\n{body}");
    let after = format!("é😀\n\ntext extended\n{body}");
    let mut db = TestDatabase::new();
    ProjectFixture::new("/project")
        .settings(&ProjectSettings {
            dirs: vec!["/project/templates".to_string()],
            ..ProjectSettings::default()
        })
        .file(path, &before)
        .install(&mut db)
        .expect("project fixture should install");
    let file = db.file(Utf8Path::new(path)).expect("fixture should exist");

    let ranges = collect_folding_ranges(&db, file);
    assert_eq!((ranges[0].start_line, ranges[0].end_line), (1, 4));
    let symbols = document_symbols(&db, file, PositionEncoding::Utf16);
    assert_eq!(
        symbols[0].selection_range.start,
        ls_types::Position::new(1, 9)
    );
    let children = symbols[0].children.as_deref().expect("block children");
    assert_eq!(children[0].name, "user");
    assert_eq!(
        children[0].selection_range.start,
        ls_types::Position::new(2, 12)
    );
    assert_eq!(
        children[0].children.as_ref().expect("filter children")[0]
            .selection_range
            .start,
        ls_types::Position::new(2, 17)
    );
    assert_eq!(children[1].name, "card.html");
    assert_eq!(
        children[1].selection_range.start,
        ls_types::Position::new(3, 14)
    );

    db.add_file(path, &after)
        .expect("edit should be applied to the filesystem");
    SourceChanges::new([ChangeEvent::ContentChanged(path.into())]).apply(&mut db);
    let ranges = collect_folding_ranges(&db, file);
    assert_eq!((ranges[0].start_line, ranges[0].end_line), (3, 6));
    let symbols = document_symbols(&db, file, PositionEncoding::Utf16);
    assert_eq!(
        symbols[0].selection_range.start,
        ls_types::Position::new(3, 9)
    );
    let children = symbols[0].children.as_deref().expect("block children");
    assert_eq!(
        children[0].selection_range.start,
        ls_types::Position::new(4, 12)
    );
    assert_eq!(
        children[0].children.as_ref().expect("filter children")[0]
            .selection_range
            .start,
        ls_types::Position::new(4, 17)
    );
    assert_eq!(
        children[1].selection_range.start,
        ls_types::Position::new(5, 14)
    );
}
