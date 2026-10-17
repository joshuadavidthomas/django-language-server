//! Fact provenance for every tag in the bundled Django archives.
//!
//! Each row shows a fused Tag Spec fact and where it came from: extraction from Django source,
//! the hand-written fallback table (`builtin_tag_specs`), or nowhere. A new hand-written Django
//! fact appears here as a `Fallback` cell; deleting a duplicated one removes a `*` marker.

use std::fmt::Write as _;

use djls_conf::DjangoVersion;
use djls_project::BlockSpec;
use djls_project::BodyAnalysisEvidence;
use djls_project::Db as _;
use djls_project::Project;
use djls_project::ScopedTemplateLibraries;
use djls_project::SymbolKey;
use djls_project::TagArgumentSyntax;
use djls_project::TemplateLibrary;
use djls_project::TemplateSymbolKind;
use djls_project::template_library_catalog;
use djls_project::template_library_tag_facts;
use djls_semantic::BodyAnalysis;
use djls_semantic::TagSpec;
use djls_semantic::TagSpecs;
use djls_semantic::builtin_tag_specs;
use djls_semantic::library_tag_specs;
use djls_testing::bundled_django_database;

const LEGEND: &str = "\
Extracted  = derived from Django source by djls-project
Extracted* = derived from Django source; builtin_tag_specs duplicates it
Extracted! = derived from Django source; overrides a different builtin_tag_specs value
Fallback   = supplied only by the hand-written builtin_tag_specs table in djls-semantic
Other      = supplied by neither (update this test to attribute the new source)
-          = no fact
";

#[derive(Clone, Copy)]
enum Source {
    Extracted,
    ExtractedDuplicated,
    ExtractedOverriding,
    Fallback,
    Other,
}

impl Source {
    fn attribute<T: PartialEq>(fused: &T, extracted: Option<&T>, fallback: Option<&T>) -> Self {
        match (extracted, fallback) {
            (Some(extracted), Some(fallback)) if extracted == fused && fallback == fused => {
                Self::ExtractedDuplicated
            }
            (Some(extracted), Some(_)) if extracted == fused => Self::ExtractedOverriding,
            (Some(extracted), None) if extracted == fused => Self::Extracted,
            (_, Some(fallback)) if fallback == fused => Self::Fallback,
            _ => Self::Other,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Extracted => "Extracted",
            Self::ExtractedDuplicated => "Extracted*",
            Self::ExtractedOverriding => "Extracted!",
            Self::Fallback => "Fallback",
            Self::Other => "Other",
        }
    }
}

fn cell(source: Source, value: &str) -> String {
    format!("{} {value}", source.label())
}

fn structure(spec: &TagSpec) -> Option<String> {
    let end = spec.end_tag.as_ref()?.name.to_string();
    let intermediates = spec
        .intermediate_tags()
        .iter()
        .map(|tag| tag.name.as_ref())
        .collect::<Vec<_>>();
    Some(if intermediates.is_empty() {
        end
    } else {
        format!("{end} [{}]", intermediates.join(", "))
    })
}

fn block_structure(block: &BlockSpec) -> Option<String> {
    let end = block.end_tag.clone()?;
    Some(if block.intermediates.is_empty() {
        end
    } else {
        format!("{end} [{}]", block.intermediates.join(", "))
    })
}

fn syntax_kind(syntax: &TagArgumentSyntax) -> &'static str {
    match syntax {
        TagArgumentSyntax::Unknown => "Unknown",
        TagArgumentSyntax::Signature { .. } => "Signature",
        TagArgumentSyntax::Parameters(_) => "Parameters",
        TagArgumentSyntax::Forms { .. } => "Forms",
        TagArgumentSyntax::Assignments { .. } => "Assignments",
    }
}

fn library_label(library: &TemplateLibrary<'_>) -> String {
    library.load_name().map_or_else(
        || {
            library
                .module_name_str()
                .rsplit('.')
                .next()
                .unwrap_or_default()
                .to_string()
        },
        |name| name.as_str().to_string(),
    )
}

fn provenance_rows(
    db: &dyn djls_semantic::Db,
    project: Project,
    library: &TemplateLibrary<'_>,
    fallback: &TagSpecs,
) -> Vec<[String; 6]> {
    let module = library.module_name_str();
    let facts = template_library_tag_facts(db, library.id());
    let fused = library_tag_specs(db, project, library.id());

    let mut names = library
        .symbols()
        .iter()
        .filter(|symbol| symbol.kind == TemplateSymbolKind::Tag)
        .map(|symbol| symbol.name().to_string())
        .collect::<Vec<_>>();
    names.extend(
        fallback
            .iter()
            .filter(|(_, spec)| spec.module() == module)
            .map(|(name, _)| name.clone()),
    );
    names.sort();
    names.dedup();

    names
        .into_iter()
        .map(|name| {
            let key = SymbolKey::tag(module, name.as_str());
            let registered = if library.symbol(TemplateSymbolKind::Tag, &name).is_some() {
                name.clone()
            } else {
                format!("{name} (unregistered)")
            };
            let Some(spec) = fused.get(&name) else {
                return [
                    library_label(library),
                    registered,
                    "-".into(),
                    "-".into(),
                    "-".into(),
                    "-".into(),
                ];
            };
            let hand = fallback.get(&name).filter(|spec| spec.module() == module);
            let block = facts.block_specs().as_map().get(&key);
            let rule = facts.tag_rules().get(&key);

            let structure_cell = structure(spec).map_or_else(
                || "-".to_string(),
                |fused| {
                    let source = Source::attribute(
                        &fused,
                        block.and_then(block_structure).as_ref(),
                        hand.and_then(structure).as_ref(),
                    );
                    cell(source, &fused)
                },
            );

            let opaque = |body: BodyAnalysis| body == BodyAnalysis::Opaque;
            let body_cell = if opaque(spec.body_analysis()) {
                let source = Source::attribute(
                    &BodyAnalysis::Opaque,
                    block
                        .filter(|block| {
                            block.body_analysis_evidence == BodyAnalysisEvidence::SkipPast
                        })
                        .map(|_| &BodyAnalysis::Opaque),
                    hand.map(TagSpec::body_analysis)
                        .filter(|body| opaque(*body))
                        .as_ref(),
                );
                cell(source, "opaque")
            } else {
                "-".to_string()
            };

            let syntax = spec.argument_syntax();
            let syntax_cell = if rule.is_none() && *syntax == TagArgumentSyntax::Unknown {
                "-".to_string()
            } else {
                let source = Source::attribute(
                    syntax,
                    rule.map(|rule| &rule.argument_syntax),
                    hand.map(TagSpec::argument_syntax)
                        .filter(|syntax| **syntax != TagArgumentSyntax::Unknown),
                );
                cell(source, syntax_kind(syntax))
            };

            let role_cell = spec.role().map_or_else(
                || "-".to_string(),
                |role| {
                    let source =
                        Source::attribute(&role, None, hand.and_then(TagSpec::role).as_ref());
                    cell(source, &format!("{role:?}"))
                },
            );

            [
                library_label(library),
                registered,
                structure_cell,
                body_cell,
                syntax_cell,
                role_cell,
            ]
        })
        .collect()
}

#[test]
fn bundled_django_tag_fact_provenance() {
    let fallback = builtin_tag_specs();
    for version in [
        DjangoVersion::Django52,
        DjangoVersion::Django60,
        DjangoVersion::Django61,
    ] {
        let db = bundled_django_database(version).expect("bundled Django project should install");
        let project = db.project().expect("bundled Django project");
        let catalog = template_library_catalog(&db, project);
        let mut libraries = ScopedTemplateLibraries::from_project_inventory(catalog)
            .resolved_libraries()
            .into_iter()
            .filter(|library| library.module_name_str().starts_with("django."))
            .collect::<Vec<_>>();
        libraries.sort_by_key(|library| library.module_name_str().to_string());

        let mut rows =
            vec![["library", "tag", "structure", "body", "rule", "role"].map(String::from)];
        for library in libraries {
            rows.extend(provenance_rows(&db, project, library, &fallback));
        }

        let mut widths = [0; 6];
        for row in &rows {
            for (width, value) in widths.iter_mut().zip(row) {
                *width = (*width).max(value.len());
            }
        }
        let mut table = format!("{LEGEND}\n");
        for row in &rows {
            let line = row
                .iter()
                .zip(widths)
                .map(|(value, width)| format!("{value:<width$}"))
                .collect::<Vec<_>>()
                .join("  ");
            writeln!(table, "{}", line.trim_end()).expect("write to string");
        }
        writeln!(table).expect("write to string");
        for (column, title) in rows[0].iter().enumerate().skip(2) {
            let mut counts = std::collections::BTreeMap::<&str, usize>::new();
            for row in &rows[1..] {
                if let Some((label, _)) = row[column].split_once(' ') {
                    *counts.entry(label).or_default() += 1;
                }
            }
            let counts = counts
                .iter()
                .map(|(label, count)| format!("{count} {label}"))
                .collect::<Vec<_>>()
                .join(", ");
            writeln!(table, "{title}: {counts}").expect("write to string");
        }
        insta::assert_snapshot!(version.as_str(), table);
    }
}
