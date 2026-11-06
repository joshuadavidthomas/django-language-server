//! Ask DJLS the questions Django already answered, and record each disagreement.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fmt::Write as _;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_project::FilterArity;
use djls_project::LoadableLibraryLookup;
use djls_project::Project;
use djls_project::ScopedTemplateLibraries;
use djls_project::TemplateLibrary;
use djls_project::TemplateName;
use djls_project::TemplateResolutionResult;
use djls_project::TemplateSymbolKind;
use djls_project::template_library_catalog;
use djls_project::template_resolution;
use djls_semantic::collect_template_diagnostics;
use djls_semantic::library_filter_specs;
use djls_semantic::library_tag_specs;
use djls_semantic::scoped_template_libraries_for_file;
use djls_source::File;
use djls_templates::Node;
use djls_templates::TemplateParseResult;
use djls_templates::parse_template;
use serde::Deserialize;
use serde::Serialize;

use crate::db::OsTestDatabase;
use crate::oracle::SettingsMode;
use crate::oracle::django::Answers;
use crate::oracle::django::LibraryErrorKind;
use crate::oracle::django::ProbeOutcome;
use crate::oracle::report::Family;
use crate::oracle::report::Finding;
use crate::oracle::report::FindingClass;

/// One generated template, its Django verdict, and the symbol it exercises.
pub(super) struct ProbeCase {
    pub(super) path: Utf8PathBuf,
    pub(super) template: String,
    library: String,
    symbol: String,
    kind: TemplateSymbolKind,
    pub(super) django: ProbeOutcome,
    pub(super) error: Option<String>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Coverage {
    pub libraries: usize,
    pub symbols: usize,
    pub filters_with_arity: usize,
    pub blocks: usize,
    pub probes: usize,
    pub templates: usize,
    pub subjects: BTreeSet<String>,
}

pub(super) struct Comparison {
    pub(super) coverage: Coverage,
    pub(super) findings: Vec<Finding>,
    pub(super) probe_verdicts: Vec<ProbeVerdict>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct ProbeVerdict {
    template: String,
    library: String,
    symbol: String,
    django: String,
    error: Option<String>,
    diagnostics: Vec<String>,
    scoped_symbols: BTreeMap<String, String>,
}

pub(super) fn probe_files(answers: &Answers, dir: &Utf8Path) -> Vec<ProbeCase> {
    let mut cases = Vec::new();
    for library in answers.libraries.iter().filter(|library| library.probed) {
        let tags = library.tags.iter().flat_map(|(name, tag)| {
            tag.probes
                .iter()
                .map(move |probe| (name, TemplateSymbolKind::Tag, probe))
        });
        let filters = library.filters.iter().flat_map(|(name, filter)| {
            filter
                .probes
                .iter()
                .map(move |probe| (name, TemplateSymbolKind::Filter, probe))
        });
        for (name, kind, probe) in tags.chain(filters) {
            cases.push(ProbeCase {
                path: dir.join(format!("{:05}.html", cases.len())),
                template: probe.template.clone(),
                library: library.module.clone(),
                symbol: name.clone(),
                kind,
                django: probe.django,
                error: probe.error.clone(),
            });
        }
    }
    cases
}

pub(super) fn seed_files(root: &Utf8Path) -> anyhow::Result<Vec<ProbeCase>> {
    let mut entries = std::fs::read_dir(root)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    entries
        .into_iter()
        .map(|entry| {
            let path = Utf8PathBuf::from_path_buf(entry.path())
                .map_err(|path| anyhow::anyhow!("non-UTF8 seed path: {}", path.display()))?;
            Ok(ProbeCase {
                template: std::fs::read_to_string(&path)?,
                symbol: entry.file_name().to_string_lossy().into_owned(),
                library: "compilation-seed".to_string(),
                kind: TemplateSymbolKind::Tag,
                path,
                django: ProbeOutcome::Unchecked,
                error: None,
            })
        })
        .collect()
}

#[allow(clippy::too_many_lines)] // Keep the per-library comparison sequence together.
pub(super) fn compare(
    db: &OsTestDatabase,
    project: Project,
    answers: &Answers,
    mode: SettingsMode,
    probes: &[ProbeCase],
) -> anyhow::Result<Comparison> {
    let mut comparison = Comparison {
        coverage: Coverage::default(),
        findings: Vec::new(),
        probe_verdicts: Vec::new(),
    };
    let catalog = template_library_catalog(db, project);
    let scoped = ScopedTemplateLibraries::from_project_inventory(catalog);

    let builtins: BTreeMap<&str, &TemplateLibrary> = scoped
        .resolved_libraries()
        .into_iter()
        .filter(|library| library.load_name().is_none())
        .map(|library| (library.module_name_str(), library))
        .collect();
    for library in &answers.libraries {
        let first_finding = comparison.findings.len();
        comparison.coverage.libraries += 1;
        comparison.coverage.subjects.insert(format!(
            "library:{}:{:?}",
            library.module, library.load_name
        ));
        for (kind, names) in [
            ("tag", library.tags.keys().collect::<Vec<_>>()),
            ("filter", library.filters.keys().collect()),
        ] {
            for name in names {
                comparison
                    .coverage
                    .subjects
                    .insert(format!("{kind}:{}:{name}", library.module));
            }
        }
        for (name, tag) in &library.tags {
            comparison.coverage.subjects.insert(format!(
                "block-exploration:{}:{name}:{}",
                library.module,
                tag.block.is_some()
            ));
            if let Some(block) = &tag.block
                && !block.unexplored.is_empty()
            {
                comparison.findings.push(Finding::new(
                    FindingClass::Unchecked,
                    Family::BlockStructure,
                    Some(&library.module),
                    Some(name),
                    &format!("unexplored stop tags: {:?}", block.unexplored),
                    "not compared",
                    "A stop tag needs arguments or the exploration budget was exhausted.",
                ));
            }
            if library.probed
                && !tag
                    .probes
                    .iter()
                    .any(|probe| probe.django == ProbeOutcome::Compiled)
            {
                comparison.findings.push(Finding::new(
                    FindingClass::Unchecked,
                    Family::ArgumentRequiredness,
                    Some(&library.module),
                    Some(name),
                    "no accepted witness discovered",
                    "argument and block boundaries not verified",
                    "Generator exhausted its candidate shapes.",
                ));
            }
        }
        let djls = match &library.load_name {
            None => {
                if let Some(found) = builtins.get(library.module.as_str()) {
                    Some(*found)
                } else {
                    {
                        comparison.findings.push(Finding::new(
                            FindingClass::Imprecise,
                            Family::LibraryLoad,
                            Some(&library.module),
                            None,
                            "builtin library",
                            "builtin membership not proven",
                            "The enumerated inventory does not prove absence. Native diagnostic probes separately establish contradictions.",
                        ));
                        None
                    }
                }
            }
            Some(load_name) => loadable(&mut comparison, scoped, load_name, &library.module),
        };
        let Some(djls) = djls else {
            for finding in &mut comparison.findings[first_finding..] {
                write!(finding.key, ":load={:?}", library.load_name)?;
            }
            continue;
        };
        let source = djls.source_file().map(|file| file.path(db).to_string());
        compare_inventory(&mut comparison, library, djls, source.as_deref());
        compare_filters(&mut comparison, db, library, djls, source.as_deref());
        compare_blocks(
            &mut comparison,
            db,
            project,
            library,
            djls,
            source.as_deref(),
        );
        compare_requiredness(
            &mut comparison,
            db,
            project,
            library,
            djls,
            source.as_deref(),
        );
        for finding in &mut comparison.findings[first_finding..] {
            write!(finding.key, ":load={:?}", library.load_name)?;
        }
    }

    compare_probes(&mut comparison, db, probes)?;
    if mode == SettingsMode::Project {
        compare_templates(&mut comparison, db, project, answers);
    }
    Ok(comparison)
}

/// Modules Django refuses as libraries must not load in DJLS either.
pub(super) fn compare_not_libraries(
    db: &OsTestDatabase,
    project: Project,
    answers: &Answers,
    not_libraries: &BTreeMap<String, String>,
) -> Vec<Finding> {
    let catalog = template_library_catalog(db, project);
    let scoped = ScopedTemplateLibraries::from_project_inventory(catalog);
    not_libraries
        .iter()
        .filter_map(|(load_name, module)| {
            let message = answers
                .library_errors
                .get(module)
                .map_or("", |error| error.message.as_str());
            let djls = match scoped.loadable_library_str(load_name) {
                LoadableLibraryLookup::Found(_) => "loadable library",
                LoadableLibraryLookup::Ambiguous(_) => "ambiguous library",
                LoadableLibraryLookup::Inconclusive(_) | LoadableLibraryLookup::Absent => {
                    return None;
                }
            };
            Some(Finding::new(
                FindingClass::Wrong,
                Family::LibraryLoad,
                Some(module),
                None,
                "InvalidTemplateLibrary: no module-level `register`",
                djls,
                message,
            ))
        })
        .collect()
}

/// Libraries Django could not import in this environment: nothing was compared for them.
pub(super) fn import_failures(
    answers: &Answers,
    requested: &BTreeMap<String, String>,
) -> Vec<Finding> {
    requested
        .values()
        .filter_map(|module| {
            let error = answers.library_errors.get(module)?;
            (error.kind == LibraryErrorKind::ImportFailed).then(|| {
                Finding::new(
                    FindingClass::Unchecked,
                    Family::LibraryLoad,
                    Some(module),
                    None,
                    "import failed in this environment",
                    "not compared",
                    &error.message,
                )
            })
        })
        .collect()
}

fn loadable<'a, 'db>(
    comparison: &mut Comparison,
    scoped: ScopedTemplateLibraries<'a, 'db>,
    load_name: &str,
    module: &str,
) -> Option<&'a TemplateLibrary<'db>> {
    let (class, djls) = match scoped.loadable_library_str(load_name) {
        LoadableLibraryLookup::Found(library) if library.module_name_str() == module => {
            return Some(library);
        }
        LoadableLibraryLookup::Found(library) => (
            FindingClass::Wrong,
            format!("loads `{}`", library.module_name_str()),
        ),
        LoadableLibraryLookup::Ambiguous(libraries) => (
            FindingClass::Imprecise,
            format!("ambiguous between {}", module_list(&libraries)),
        ),
        LoadableLibraryLookup::Inconclusive(libraries) => (
            FindingClass::Imprecise,
            format!("inconclusive among {}", module_list(&libraries)),
        ),
        LoadableLibraryLookup::Absent => (FindingClass::Wrong, "absent".to_string()),
    };
    comparison.findings.push(Finding::new(
        class,
        Family::LibraryLoad,
        Some(module),
        Some(load_name),
        &format!("`{{% load {load_name} %}}` loads `{module}`"),
        &djls,
        "",
    ));
    None
}

fn module_list(libraries: &[&TemplateLibrary<'_>]) -> String {
    libraries
        .iter()
        .map(|library| format!("`{}`", library.module_name_str()))
        .collect::<Vec<_>>()
        .join(", ")
}

fn compare_inventory(
    comparison: &mut Comparison,
    library: &crate::oracle::django::Library,
    djls: &TemplateLibrary<'_>,
    source: Option<&str>,
) {
    let open = djls.symbols_are_unobserved();
    for (kind, names) in [
        (
            TemplateSymbolKind::Tag,
            library.tags.keys().collect::<BTreeSet<_>>(),
        ),
        (
            TemplateSymbolKind::Filter,
            library.filters.keys().collect::<BTreeSet<_>>(),
        ),
    ] {
        let label = kind_label(kind);
        comparison.coverage.symbols += names.len();
        for name in &names {
            if djls.symbol(kind, name).is_none() {
                let (class, answer) = if open {
                    (FindingClass::Imprecise, "not observed; inventory is open")
                } else {
                    (FindingClass::Wrong, "absent from a closed inventory")
                };
                comparison.findings.push(
                    Finding::new(
                        class,
                        Family::SymbolInventory,
                        Some(&library.module),
                        Some(&format!("{label}:{name}")),
                        &format!("registers {label} `{name}`"),
                        answer,
                        "",
                    )
                    .with_source(source),
                );
            }
        }
        for symbol in djls.symbols().iter().filter(|symbol| symbol.kind == kind) {
            if !names.contains(&symbol.name().to_string()) {
                comparison.findings.push(
                    Finding::new(
                        FindingClass::Wrong,
                        Family::SymbolInventory,
                        Some(&library.module),
                        Some(&format!("{label}:{}", symbol.name())),
                        &format!("no {label} `{}`", symbol.name()),
                        &format!("defines {label} `{}`", symbol.name()),
                        "",
                    )
                    .with_source(source),
                );
            }
        }
    }
}

fn compare_filters(
    comparison: &mut Comparison,
    db: &OsTestDatabase,
    library: &crate::oracle::django::Library,
    djls: &TemplateLibrary<'_>,
    source: Option<&str>,
) {
    let specs = library_filter_specs(db, djls.id());
    for (name, filter) in &library.filters {
        comparison.coverage.subjects.insert(format!(
            "filter-arity:{}:{name}:{:?}",
            library.module, filter.arity
        ));
        let Some(django) = filter.arity.as_deref() else {
            if library.probed {
                comparison.findings.push(Finding::new(
                    FindingClass::Unchecked,
                    Family::FilterArity,
                    Some(&library.module),
                    Some(name),
                    "runtime signature could not be inspected",
                    "not compared",
                    "FilterExpression.args_check raised an unexpected exception.",
                ));
            }
            continue;
        };
        if djls.symbol(TemplateSymbolKind::Filter, name).is_none() {
            continue;
        }
        comparison.coverage.filters_with_arity += 1;
        let answer = specs.get(name).map(|arity| match arity {
            FilterArity::NoArgument => "no_argument",
            FilterArity::RequiredArgument => "required_argument",
            FilterArity::OptionalArgument => "optional_argument",
        });
        let class = match answer {
            Some(answer) if answer == django => continue,
            Some(_) => FindingClass::Wrong,
            None => FindingClass::Imprecise,
        };
        comparison.findings.push(
            Finding::new(
                class,
                Family::FilterArity,
                Some(&library.module),
                Some(name),
                django,
                answer.unwrap_or("unknown"),
                "Django `FilterExpression.args_check` with zero and one argument.",
            )
            .with_source(source),
        );
    }
}

fn compare_blocks(
    comparison: &mut Comparison,
    db: &OsTestDatabase,
    project: Project,
    library: &crate::oracle::django::Library,
    djls: &TemplateLibrary<'_>,
    source: Option<&str>,
) {
    let specs = library_tag_specs(db, project, djls.id());
    for (name, tag) in &library.tags {
        let Some(block) = &tag.block else { continue };
        if block.end_tags.is_empty() || djls.symbol(TemplateSymbolKind::Tag, name).is_none() {
            continue;
        }
        comparison.coverage.blocks += 1;
        let opener = format!(
            "{{% {name}{} %}}",
            if block.opening_arguments.is_empty() {
                String::new()
            } else {
                format!(" {}", block.opening_arguments)
            }
        );
        let django = describe_block(&block.end_tags, &block.intermediates);
        let Some(spec) = specs.get(name) else {
            comparison.findings.push(
                Finding::new(
                    FindingClass::Imprecise,
                    Family::BlockStructure,
                    Some(&library.module),
                    Some(name),
                    &django,
                    "no proven closing structure",
                    &format!(
                        "`{opener}` waits for a closing tag in Django ({} kind).",
                        tag.kind
                    ),
                )
                .with_source(source),
            );
            continue;
        };
        let djls_intermediates: Vec<String> = spec
            .intermediate_tags()
            .iter()
            .map(|tag| tag.name.to_string())
            .collect();
        let end_tags: Vec<String> = spec
            .end_tag
            .iter()
            .map(|tag| tag.name.to_string())
            .collect();
        let closer_matches = end_tags.iter().any(|tag| block.end_tags.contains(tag));
        let missing: Vec<&String> = block
            .intermediates
            .iter()
            .filter(|tag| !djls_intermediates.contains(tag))
            .collect();
        if closer_matches && missing.is_empty() {
            continue;
        }
        comparison.findings.push(
            Finding::new(
                FindingClass::Wrong,
                Family::BlockStructure,
                Some(&library.module),
                Some(name),
                &django,
                &describe_block(&end_tags, &djls_intermediates),
                &format!("Explored from `{opener}`. A definite spec contradicts Django's observed closer or omits an observed intermediate."),
            )
            .with_source(source),
        );
    }
}

/// A parameter DJLS calls required must appear in every invocation Django accepts.
fn compare_requiredness(
    comparison: &mut Comparison,
    db: &OsTestDatabase,
    project: Project,
    library: &crate::oracle::django::Library,
    djls: &TemplateLibrary<'_>,
    source: Option<&str>,
) {
    let specs = library_tag_specs(db, project, djls.id());
    for (name, tag) in &library.tags {
        let Some(min_bits) = tag.min_compiled_bits else {
            continue;
        };
        comparison.coverage.subjects.insert(format!(
            "argument-witness:{}:{name}:{min_bits}",
            library.module
        ));
        let Some(parameters) = specs
            .get(name)
            .and_then(|spec| spec.argument_syntax().parameters())
        else {
            continue;
        };
        let required: Vec<&str> = parameters
            .iter()
            .filter(|parameter| parameter.requirement.is_required())
            .map(|parameter| parameter.name.as_str())
            .collect();
        if required.len() <= min_bits {
            continue;
        }
        let shortest = tag
            .probes
            .iter()
            .filter(|probe| probe.django == ProbeOutcome::Compiled)
            .min_by_key(|probe| probe.template.len())
            .map(|probe| format!("Django compiles `{}`.", probe.template))
            .unwrap_or_default();
        comparison.findings.push(
            Finding::new(
                FindingClass::Wrong,
                Family::ArgumentRequiredness,
                Some(&library.module),
                Some(name),
                &format!("compiles with {min_bits} argument(s)"),
                &format!(
                    "requires {} argument(s): {}",
                    required.len(),
                    required.join(", ")
                ),
                &shortest,
            )
            .with_source(source),
        );
    }
}

fn describe_block(end_tags: &[String], intermediates: &[String]) -> String {
    let closing = if end_tags.is_empty() {
        "non-block tag (no closing tag)".to_string()
    } else {
        format!("closes with {}", quoted(end_tags))
    };
    if intermediates.is_empty() {
        closing
    } else {
        format!("{closing}; intermediates {}", quoted(intermediates))
    }
}

fn quoted(names: &[String]) -> String {
    names
        .iter()
        .map(|name| format!("`{name}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Compare Django's compile verdict with DJLS's diagnostics, one group per symbol and direction.
fn compare_probes(
    comparison: &mut Comparison,
    db: &OsTestDatabase,
    probes: &[ProbeCase],
) -> anyhow::Result<()> {
    struct Group {
        count: usize,
        examples: Vec<String>,
    }
    let mut groups: BTreeMap<(String, String, TemplateSymbolKind, bool), Group> = BTreeMap::new();
    for probe in probes {
        comparison.coverage.probes += 1;
        comparison.coverage.subjects.insert(format!(
            "probe:{}:{}:{}:{}",
            kind_label(probe.kind),
            probe.library,
            probe.symbol,
            probe.template
        ));
        if probe.django == ProbeOutcome::Unchecked {
            let mut finding = Finding::new(
                FindingClass::Unchecked,
                Family::Validation,
                Some(&probe.library),
                Some(&probe.symbol),
                "unexpected native compiler exception",
                "not compared",
                probe.error.as_deref().unwrap_or("no runtime verdict"),
            );
            finding.evidence.push(probe.template.clone());
            write!(finding.key, ":{}", probe.template)?;
            comparison.findings.push(finding);
            continue;
        }
        let file = db.file(&probe.path)?;
        let diagnostics = collect_template_diagnostics(db, file);
        let djls_errors: Vec<String> = diagnostics
            .template_errors
            .iter()
            .map(|error| format!("{} {error}", error.diagnostic_code()))
            .chain(
                diagnostics
                    .validation_errors
                    .iter()
                    // S124 says DJLS could not read part of a library: a hint, not a rejection.
                    .filter(|error| error.code() != "S124")
                    .map(|error| format!("{} {error}", error.code())),
            )
            .collect();
        comparison
            .probe_verdicts
            .push(probe_verdict(db, file, probe, djls_errors.clone()));
        let false_positive = match (probe.django, djls_errors.is_empty()) {
            (ProbeOutcome::Compiled, false) => true,
            (ProbeOutcome::Rejected, true) => false,
            (ProbeOutcome::Compiled, true)
            | (ProbeOutcome::Rejected, false)
            | (ProbeOutcome::Unchecked, _) => continue,
        };
        let group = groups
            .entry((
                probe.library.clone(),
                probe.symbol.clone(),
                probe.kind,
                false_positive,
            ))
            .or_insert(Group {
                count: 0,
                examples: Vec::new(),
            });
        group.count += 1;
        group.examples.push(if false_positive {
            format!(
                "`{}`: Django compiles; DJLS reports {}",
                probe.template,
                djls_errors.join(" / ")
            )
        } else {
            format!(
                "`{}`: Django rejects ({}); DJLS reports nothing",
                probe.template,
                probe.error.as_deref().unwrap_or("error")
            )
        });
    }
    for ((library, symbol, kind, false_positive), group) in groups {
        let (class, django, djls) = if false_positive {
            (FindingClass::Wrong, "compiles", "reports an error")
        } else {
            (FindingClass::Missed, "rejects", "reports nothing")
        };
        let label = kind_label(kind);
        let mut finding = Finding::new(
            class,
            Family::Validation,
            Some(&library),
            Some(&symbol),
            &format!("{django} {} {label} probe(s)", group.count),
            djls,
            "",
        );
        write!(finding.key, ":{label}")?;
        finding.evidence = group.examples;
        comparison.findings.push(finding);
    }
    Ok(())
}

fn probe_verdict(
    db: &OsTestDatabase,
    file: File,
    probe: &ProbeCase,
    diagnostics: Vec<String>,
) -> ProbeVerdict {
    let scoped = scoped_template_libraries_for_file(db, file);
    let mut scoped_symbols = BTreeMap::new();
    if let TemplateParseResult::Parsed(nodes) = parse_template(db, file) {
        for node in nodes.nodelist(db) {
            if let Node::Tag { name, .. } = node {
                scoped_symbols.insert(
                    name.to_string(),
                    format!("{:?}", scoped.symbol(name, TemplateSymbolKind::Tag)),
                );
            }
        }
    }
    ProbeVerdict {
        template: probe.template.clone(),
        library: probe.library.clone(),
        symbol: probe.symbol.clone(),
        django: format!("{:?}", probe.django),
        error: probe.error.clone(),
        diagnostics,
        scoped_symbols,
    }
}

fn compare_templates(
    comparison: &mut Comparison,
    db: &OsTestDatabase,
    project: Project,
    answers: &Answers,
) {
    let Some(templates) = &answers.templates else {
        return;
    };
    let resolution = template_resolution(db, project);
    for (name, django) in &templates.resolved {
        comparison
            .coverage
            .subjects
            .insert(format!("template:{name}"));
        let django = if django.is_null() {
            None
        } else if let Some(path) = django.as_str() {
            Some(path.to_string())
        } else {
            comparison.findings.push(Finding::new(
                FindingClass::Unchecked,
                Family::TemplateResolution,
                None,
                Some(name),
                &django.to_string(),
                "not compared",
                "Native loader could not establish an origin.",
            ));
            continue;
        };
        comparison.coverage.templates += 1;
        let template_name = TemplateName::new(db, name.clone());
        let canonical = |path: &Utf8Path| {
            std::fs::canonicalize(path)
                .ok()
                .and_then(|path| Utf8PathBuf::from_path_buf(path).ok())
                .map_or_else(|| path.to_string(), |path| path.to_string())
        };
        let (class, djls) = match resolution.resolve(db, template_name) {
            TemplateResolutionResult::Found(origin) => {
                let path = canonical(origin.file(db).path(db));
                if django.as_deref() == Some(path.as_str()) {
                    continue;
                }
                (FindingClass::Wrong, format!("finds `{path}`"))
            }
            TemplateResolutionResult::DoesNotExist(_) => {
                if django.is_none() {
                    continue;
                }
                (FindingClass::Wrong, "does not exist".to_string())
            }
            TemplateResolutionResult::Inconclusive(search) => {
                let possible: Vec<String> = search
                    .possible_origins
                    .iter()
                    .map(|origin| canonical(origin.file(db).path(db)))
                    .collect();
                (
                    FindingClass::Imprecise,
                    format!(
                        "inconclusive, with non-exhaustive evidence {}",
                        quoted(&possible)
                    ),
                )
            }
        };
        comparison.findings.push(Finding::new(
            class,
            Family::TemplateResolution,
            None,
            Some(name),
            &django.map_or_else(
                || "does not exist".to_string(),
                |path| format!("finds `{path}`"),
            ),
            &djls,
            "",
        ));
    }
}

fn kind_label(kind: TemplateSymbolKind) -> &'static str {
    match kind {
        TemplateSymbolKind::Tag => "tag",
        TemplateSymbolKind::Filter => "filter",
    }
}
