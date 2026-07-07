use camino::Utf8Path;
use djls_ide::collect_folding_ranges;
use djls_testing::TestDatabase;
use tower_lsp_server::ls_types;

#[test]
fn folding_ranges_include_top_level_and_nested_template_blocks() {
    let source = r"{% load static %}

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
    db.add_file("template.html", source);
    let file = db.file(Utf8Path::new("template.html"));
    let ranges = collect_folding_ranges(&db, file);

    assert!(ranges.iter().any(|range| {
        range.start_line == 4
            && range.end_line == 6
            && range.kind == Some(ls_types::FoldingRangeKind::Region)
    }));
    assert!(ranges.iter().any(|range| {
        range.start_line == 9
            && range.end_line == 19
            && range.kind == Some(ls_types::FoldingRangeKind::Region)
    }));
    assert!(ranges.iter().any(|range| {
        range.start_line == 10
            && range.end_line == 18
            && range.kind == Some(ls_types::FoldingRangeKind::Region)
    }));
    assert!(ranges.iter().any(|range| {
        range.start_line == 12
            && range.end_line == 14
            && range.kind == Some(ls_types::FoldingRangeKind::Region)
    }));
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
    db.add_file("template.html", source);
    let file = db.file(Utf8Path::new("template.html"));
    let ranges = collect_folding_ranges(&db, file);

    assert!(ranges.iter().any(|range| {
        range.start_line == 0
            && range.end_line == 5
            && range.kind == Some(ls_types::FoldingRangeKind::Region)
    }));
    assert!(ranges.iter().any(|range| {
        range.start_line == 1
            && range.end_line == 4
            && range.kind == Some(ls_types::FoldingRangeKind::Region)
    }));
    assert!(!ranges.iter().any(|range| {
        range.start_line == 0
            && range.end_line == 2
            && range.kind == Some(ls_types::FoldingRangeKind::Region)
    }));
}
