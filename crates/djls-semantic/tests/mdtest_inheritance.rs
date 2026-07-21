use std::path::Path;

use camino::Utf8Path;
use djls_semantic::BlockSite;
use djls_semantic::ChainEnd;
use djls_semantic::ExtendsTarget;
use djls_semantic::TemplateInheritance;
use djls_semantic::block_overrides;
use djls_semantic::inherited_blocks;
use djls_semantic::parent_block;
use djls_semantic::template_inheritance;
use djls_semantic::template_symbols;
use djls_source::File;
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
    )
    .expect("inheritance mdtest suite should run");
}

fn render_inheritance(scenario: &Scenario) -> anyhow::Result<String> {
    let db = TestDatabase::new();
    let project = project_for_scenario(&db, scenario)?;

    let primary = scenario.primary_file()?;
    let primary_path = template_path(&primary.path);
    let file = db.file(Utf8Path::new(&primary_path))?;
    let nodelist = match parse_template(&db, file) {
        djls_templates::TemplateParseResult::Parsed(nodelist) => nodelist,
        djls_templates::TemplateParseResult::NotTemplate => {
            return Ok("file is not a template".to_string());
        }
        djls_templates::TemplateParseResult::Unreadable(error) => {
            return Ok(format!("template could not be read: {error}"));
        }
    };
    let symbols = template_symbols(&db, file, nodelist);
    let inheritance = template_inheritance(&db, project, file);

    let mut output = vec![
        format!("extends: {}", render_extends(symbols.extends())),
        "blocks:".to_string(),
    ];
    if symbols.blocks().is_empty() {
        output.push("  none".to_string());
    } else {
        for block in symbols.blocks() {
            output.push(format!(
                "  - {} name@{} full@{}",
                block.name,
                render_span(block.name_span),
                render_span(block.full_span)
            ));
        }
    }
    output.push("partials:".to_string());
    if symbols.partials().is_empty() {
        output.push("  none".to_string());
    } else {
        for partial in symbols.partials() {
            output.push(format!(
                "  - {} name@{} full@{}",
                partial.name,
                render_span(partial.name_span),
                render_span(partial.full_span)
            ));
        }
    }
    output.push("chain:".to_string());
    render_chain(&mut output, &db, inheritance);
    output.push("block queries:".to_string());
    render_block_queries(&mut output, &db, project, file, symbols.blocks());

    Ok(output.join("\n"))
}

fn project_for_scenario(
    db: &TestDatabase,
    scenario: &Scenario,
) -> anyhow::Result<djls_project::Project> {
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

fn render_chain(output: &mut Vec<String>, db: &TestDatabase, inheritance: TemplateInheritance<'_>) {
    if inheritance.ancestors(db).is_empty() {
        output.push("  ancestors: none".to_string());
    } else {
        output.push("  ancestors:".to_string());
        for ancestor in inheritance.ancestors(db) {
            let name = ancestor.template_name(db).name(db);
            output.push(format!("    - {name}"));
        }
    }
    output.push(format!("  end: {}", render_chain_end(inheritance.end(db))));
}

fn render_block_queries(
    output: &mut Vec<String>,
    db: &TestDatabase,
    project: djls_project::Project,
    file: File,
    blocks: &[djls_semantic::BlockDef],
) {
    output.push("  parent blocks:".to_string());
    if blocks.is_empty() {
        output.push("    none".to_string());
    } else {
        for block in blocks {
            let parent = parent_block(db, project, file, &block.name)
                .map_or_else(|| "none".to_string(), |site| render_block_site(db, site));
            output.push(format!("    - {} -> {parent}", block.name));
        }
    }

    output.push("  inherited blocks:".to_string());
    let inherited = inherited_blocks(db, project, file);
    if inherited.is_empty() {
        output.push("    none".to_string());
    } else {
        for (name, site) in inherited {
            output.push(format!("    - {name} -> {}", render_block_site(db, site)));
        }
    }

    output.push("  overrides:".to_string());
    if blocks.is_empty() {
        output.push("    none".to_string());
    } else {
        for block in blocks {
            let overrides = block_overrides(db, project, file, &block.name);
            if overrides.is_empty() {
                output.push(format!("    - {}: none", block.name));
            } else {
                output.push(format!("    - {}:", block.name));
                for site in overrides {
                    output.push(format!("      - {}", render_block_site(db, site)));
                }
            }
        }
    }
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
        ChainEnd::InconclusiveParent { name } => {
            format!("inconclusive-parent {name:?}")
        }
        ChainEnd::Cycle => "cycle".to_string(),
    }
}

fn render_span(span: Span) -> String {
    format!("{}..{}", span.start_usize(), span.end_usize())
}

fn render_block_site(db: &TestDatabase, site: BlockSite) -> String {
    format!(
        "{} name@{} full@{}",
        render_file(db, site.file),
        render_span(site.name_span),
        render_span(site.full_span)
    )
}

fn render_file(db: &TestDatabase, file: File) -> String {
    file.path(db)
        .strip_prefix(TEMPLATE_ROOT)
        .map_or_else(|_| file.path(db).as_str(), Utf8Path::as_str)
        .trim_start_matches('/')
        .to_string()
}
