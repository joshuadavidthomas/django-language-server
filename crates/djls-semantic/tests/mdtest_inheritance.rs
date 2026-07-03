use std::fmt::Write as _;
use std::path::Path;

use camino::Utf8Path;
use djls_semantic::ExtendsTarget;
use djls_semantic::template_symbols;
use djls_source::Span;
use djls_templates::parse_template;
use djls_testing::Scenario;
use djls_testing::TestDatabase;

#[test]
fn mdtest_inheritance() {
    djls_testing::run_suite_with(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/mdtest/inheritance"),
        render_inheritance,
    );
}

fn render_inheritance(scenario: &Scenario) -> String {
    let db = TestDatabase::new();
    for file in &scenario.files {
        db.add_file(&file.path, &file.source);
    }

    let primary = scenario.primary_file();
    let file = db.get_or_create_file(Utf8Path::new(&primary.path));
    let nodelist = parse_template(&db, file).expect("should parse");
    let symbols = template_symbols(&db, nodelist);

    let mut output = String::new();
    writeln!(
        &mut output,
        "extends: {}",
        render_extends(symbols.extends())
    )
    .unwrap();
    writeln!(&mut output, "blocks:").unwrap();
    if symbols.blocks().is_empty() {
        writeln!(&mut output, "  none").unwrap();
    } else {
        for block in symbols.blocks() {
            writeln!(
                &mut output,
                "  - {} name@{} full@{}",
                block.name,
                render_span(block.name_span),
                render_span(block.full_span)
            )
            .unwrap();
        }
    }
    writeln!(&mut output, "partials:").unwrap();
    if symbols.partials().is_empty() {
        writeln!(&mut output, "  none").unwrap();
    } else {
        for partial in symbols.partials() {
            writeln!(
                &mut output,
                "  - {} name@{} full@{}",
                partial.name,
                render_span(partial.name_span),
                render_span(partial.full_span)
            )
            .unwrap();
        }
    }

    output.trim_end().to_string()
}

fn render_extends(target: Option<&ExtendsTarget>) -> String {
    match target {
        Some(ExtendsTarget::Literal { name, span }) => {
            format!("literal {name:?} @{}", render_span(*span))
        }
        Some(ExtendsTarget::Dynamic { span }) => format!("dynamic @{}", render_span(*span)),
        None => "none".to_string(),
    }
}

fn render_span(span: Span) -> String {
    format!("{}..{}", span.start_usize(), span.end_usize())
}
