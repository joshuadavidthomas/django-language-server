use camino::Utf8Path;
use djls_ide::collect_folding_ranges;
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
