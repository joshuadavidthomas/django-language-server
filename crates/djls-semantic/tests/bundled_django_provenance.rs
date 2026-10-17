//! Fact provenance for every tag in the bundled Django archives.
//!
//! Each row shows a fused Tag Spec fact and where it came from: extraction from Django source, a
//! Fallback Rule in `djls-project`'s `templates/tags/fallback.rs`, or nowhere. A new Fallback Rule
//! appears here as a `Fallback` cell; replacing one with extraction turns it into `Extracted`.

use std::fmt::Write as _;

use djls_conf::DjangoVersion;
use djls_project::BlockSpec;
use djls_project::BodyAnalysisEvidence;
use djls_project::BodyLexing;
use djls_project::Db as _;
use djls_project::FactProvenance;
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
use djls_semantic::library_tag_specs;
use djls_testing::bundled_django_database;

const LEGEND: &str = "\
Extracted = derived from Django source by djls-project
Fallback  = supplied by a Fallback Rule in djls-project's templates/tags/fallback.rs
Other     = supplied by neither (update this test to attribute the new source)
-         = no fact
";

#[derive(Clone, Copy)]
enum Source {
    Extracted,
    Fallback,
    Other,
}

impl Source {
    fn attribute<T: PartialEq>(fused: &T, extracted: Option<&T>, fallback: Option<&T>) -> Self {
        if extracted == Some(fused) {
            Self::Extracted
        } else if fallback == Some(fused) {
            Self::Fallback
        } else {
            Self::Other
        }
    }

    fn from_provenance(provenance: Option<FactProvenance>) -> Self {
        match provenance {
            Some(FactProvenance::Extracted) => Self::Extracted,
            Some(FactProvenance::Fallback) => Self::Fallback,
            None => Self::Other,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Extracted => "Extracted",
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

#[allow(clippy::too_many_lines)]
fn provenance_rows(
    db: &dyn djls_semantic::Db,
    project: Project,
    library: &TemplateLibrary<'_>,
) -> Vec<[String; 7]> {
    let module = library.module_name_str();
    let facts = template_library_tag_facts(db, library.id());
    let fused = library_tag_specs(db, project, library.id());

    let mut names = library
        .symbols()
        .iter()
        .filter(|symbol| symbol.kind == TemplateSymbolKind::Tag)
        .map(|symbol| symbol.name().to_string())
        .collect::<Vec<_>>();
    names.sort();
    names.dedup();

    names
        .into_iter()
        .map(|name| {
            let key = SymbolKey::tag(module, name.as_str());
            let Some(spec) = fused.get(&name) else {
                return [
                    library_label(library),
                    name,
                    "-".into(),
                    "-".into(),
                    "-".into(),
                    "-".into(),
                    "-".into(),
                ];
            };
            let block = facts.block_specs().as_map().get(&key);
            let rule = facts.tag_rules().get(&key);
            let meaning = facts.tag_meanings().get(&key);
            let verbatim =
                meaning.is_some_and(|meaning| meaning.body_lexing() == BodyLexing::Verbatim);

            // A verbatim Fallback Rule supplies `end<name>` when extraction found no closer.
            let structure_cell = structure(spec).map_or_else(
                || "-".to_string(),
                |fused| {
                    let source = Source::attribute(
                        &fused,
                        block.and_then(block_structure).as_ref(),
                        verbatim.then(|| format!("end{name}")).as_ref(),
                    );
                    cell(source, &fused)
                },
            );

            let body_cell = match spec.body_analysis() {
                BodyAnalysis::Analyze => "-".to_string(),
                BodyAnalysis::Opaque => {
                    let skip_past = block.is_some_and(|block| {
                        block.body_analysis_evidence == BodyAnalysisEvidence::SkipPast
                    });
                    let source = if skip_past {
                        Source::Extracted
                    } else {
                        Source::Other
                    };
                    cell(source, "opaque")
                }
                BodyAnalysis::Verbatim => {
                    let source = if verbatim {
                        Source::Fallback
                    } else {
                        Source::Other
                    };
                    cell(source, "verbatim")
                }
            };

            let syntax = spec.argument_syntax();
            let syntax_cell = if rule.is_none() && *syntax == TagArgumentSyntax::Unknown {
                "-".to_string()
            } else {
                let source =
                    Source::attribute(syntax, rule.map(|rule| &rule.argument_syntax), None);
                cell(source, syntax_kind(syntax))
            };

            let role_cell = spec.role().map_or_else(
                || "-".to_string(),
                |role| {
                    let source = Source::from_provenance(
                        meaning
                            .filter(|meaning| meaning.role() == Some(role))
                            .and_then(djls_project::TagMeaning::role_provenance),
                    );
                    cell(source, &format!("{role:?}"))
                },
            );

            // Condition arguments come only from Fallback Rules until extraction records them.
            let condition_cell = meaning.and_then(|meaning| meaning.condition()).map_or_else(
                || "-".to_string(),
                |condition| {
                    let mut tags = vec![name.clone()];
                    tags.extend(condition.intermediates().iter().cloned());
                    cell(Source::Fallback, &tags.join(", "))
                },
            );

            [
                library_label(library),
                name,
                structure_cell,
                body_cell,
                syntax_cell,
                role_cell,
                condition_cell,
            ]
        })
        .collect()
}

#[test]
fn bundled_django_tag_fact_provenance() {
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

        let mut rows = vec![
            [
                "library",
                "tag",
                "structure",
                "body",
                "rule",
                "role",
                "condition",
            ]
            .map(String::from),
        ];
        for library in libraries {
            rows.extend(provenance_rows(&db, project, library));
        }

        let mut widths = [0; 7];
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
