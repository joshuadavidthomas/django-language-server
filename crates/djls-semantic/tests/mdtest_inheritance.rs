use std::fmt::Write as _;
use std::path::Path;

use camino::Utf8Path;
use djls_semantic::ChainEnd;
use djls_semantic::ExtendsTarget;
use djls_semantic::TemplateInheritance;
use djls_semantic::template_inheritance;
use djls_semantic::template_symbols;
use djls_source::Span;
use djls_templates::parse_template;
use djls_testing::ProjectFixture;
use djls_testing::Scenario;
use djls_testing::TestDatabase;

const PROJECT_ROOT: &str = "/test/project";
const TEMPLATE_ROOT: &str = "/test/project/templates";

#[test]
fn mdtest_inheritance() {
    djls_testing::run_suite_with(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/mdtest/inheritance"),
        render_inheritance,
    );
}

fn render_inheritance(scenario: &Scenario) -> String {
    let db = TestDatabase::new();
    let project = project_for_scenario(&db, scenario);

    let primary = scenario.primary_file();
    let primary_path = template_path(&primary.path);
    let file = db.get_or_create_file(Utf8Path::new(&primary_path));
    let nodelist = parse_template(&db, file).expect("should parse");
    let symbols = template_symbols(&db, nodelist);
    let inheritance = template_inheritance(&db, project, file);

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
    writeln!(&mut output, "chain:").unwrap();
    render_chain(&mut output, &db, inheritance);

    output.trim_end().to_string()
}

fn project_for_scenario(db: &TestDatabase, scenario: &Scenario) -> djls_project::Project {
    let settings_source = format!(
        "INSTALLED_APPS = []\nTEMPLATES = [{{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['{TEMPLATE_ROOT}'], 'APP_DIRS': False}}]\n"
    );
    let fixture = ProjectFixture::new(PROJECT_ROOT)
        .django_settings_module("testproject.settings")
        .file("/test/project/testproject/settings.py", settings_source);

    scenario
        .files
        .iter()
        .fold(fixture, |fixture, file| {
            fixture.file(template_path(&file.path), file.source.clone())
        })
        .build(db)
}

fn template_path(relative_path: &str) -> String {
    format!("{TEMPLATE_ROOT}/{relative_path}")
}

fn render_chain(output: &mut String, db: &TestDatabase, inheritance: TemplateInheritance<'_>) {
    if inheritance.ancestors(db).is_empty() {
        writeln!(output, "  ancestors: none").unwrap();
    } else {
        writeln!(output, "  ancestors:").unwrap();
        for ancestor in inheritance.ancestors(db) {
            let name = ancestor.template_name(db).name(db);
            writeln!(output, "    - {name}").unwrap();
        }
    }
    writeln!(output, "  end: {}", render_chain_end(inheritance.end(db))).unwrap();
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

fn render_chain_end(end: ChainEnd) -> String {
    match end {
        ChainEnd::Root => "root".to_string(),
        ChainEnd::Dynamic { span } => format!("dynamic @{}", render_span(span)),
        ChainEnd::Unresolved { name } => format!("unresolved {name:?}"),
        ChainEnd::IncompleteDirs => "incomplete-dirs".to_string(),
        ChainEnd::Cycle => "cycle".to_string(),
    }
}

fn render_span(span: Span) -> String {
    format!("{}..{}", span.start_usize(), span.end_usize())
}
