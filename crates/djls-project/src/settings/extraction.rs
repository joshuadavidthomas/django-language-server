use camino::Utf8Path;
use djls_source::File;
use djls_source::Origin;

use crate::db::Db as ProjectDb;
use crate::python::evaluation::BranchConstraints;
use crate::python::evaluation::PythonBindingState;
use crate::python::evaluation::PythonBoundValue;
use crate::python::evaluation::PythonDict;
use crate::python::evaluation::PythonDictItem;
use crate::python::evaluation::PythonList;
use crate::python::evaluation::PythonListItem;
use crate::python::evaluation::PythonModuleValues;
use crate::python::evaluation::PythonMutation;
use crate::python::evaluation::PythonMutationOperation;
use crate::python::evaluation::PythonMutationPathSegment;
use crate::python::evaluation::PythonUnknown;
use crate::python::evaluation::PythonUnknownCause;
use crate::python::evaluation::PythonValue;
use crate::python::evaluation::PythonValueKind;
use crate::settings::types::DjangoSettings;
use crate::settings::types::DynamicInstalledApps;
use crate::settings::types::DynamicScalarSetting;
use crate::settings::types::DynamicStaticFilesDirs;
use crate::settings::types::DynamicTemplates;
use crate::settings::types::EvaluatedPath;
use crate::settings::types::InstalledAppEvidence;
use crate::settings::types::InstalledAppsAlternatives;
use crate::settings::types::InstalledAppsValue;
use crate::settings::types::MAX_EXACT_SETTING_ALTERNATIVES;
use crate::settings::types::MalformedInstalledApps;
use crate::settings::types::MalformedScalarSetting;
use crate::settings::types::MalformedStaticFilesDirs;
use crate::settings::types::MalformedTemplates;
use crate::settings::types::MergeDynamicEvidence;
use crate::settings::types::MergeEvidence;
use crate::settings::types::OrderedInstalledApps;
use crate::settings::types::OrderedPathList;
use crate::settings::types::OrderedTemplateList;
use crate::settings::types::PartialSettingField;
use crate::settings::types::PartialTemplateBackend;
use crate::settings::types::PathListEvidence;
use crate::settings::types::SettingAlternatives;
use crate::settings::types::SettingCase;
use crate::settings::types::SettingIssue;
use crate::settings::types::SettingIssueKind;
use crate::settings::types::StaticFilesDirsAlternatives;
use crate::settings::types::StaticFilesDirsValue;
use crate::settings::types::StaticFilesSettings;
use crate::settings::types::StaticRoot;
use crate::settings::types::StaticRootAlternatives;
use crate::settings::types::StaticUrl;
use crate::settings::types::StaticUrlAlternatives;
use crate::settings::types::TemplateAlternatives;
use crate::settings::types::TemplateBackend;
use crate::settings::types::TemplateContextProcessorPath;
use crate::settings::types::TemplateListEvidence;
use crate::settings::types::TemplatesValue;
use crate::settings::types::WithOrigin;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KnownSetting {
    InstalledApps,
    Templates,
    StaticUrl,
    StaticRoot,
    StaticFilesDirs,
}

impl KnownSetting {
    const fn name(self) -> &'static str {
        match self {
            Self::InstalledApps => "INSTALLED_APPS",
            Self::Templates => "TEMPLATES",
            Self::StaticUrl => "STATIC_URL",
            Self::StaticRoot => "STATIC_ROOT",
            Self::StaticFilesDirs => "STATICFILES_DIRS",
        }
    }
}

struct NamespaceDynamicEvidence {
    issue: SettingIssue,
    constraints: BranchConstraints,
}

pub(crate) fn settings_from_values(
    db: &dyn ProjectDb,
    file: File,
    values: &PythonModuleValues,
) -> DjangoSettings {
    let namespace_dynamic = values.namespace_remainder.as_ref().map(|remainder| {
        remainder
            .causes
            .iter()
            .map(|cause| NamespaceDynamicEvidence {
                issue: issue(SettingIssueKind::DynamicNamespace, cause.unknown.origin),
                constraints: cause.constraints.clone(),
            })
            .collect::<Vec<_>>()
    });
    let syntax_issues = |setting: KnownSetting| {
        values
            .syntax_impacts
            .iter()
            .filter(|impact| impact.affects(setting.name()))
            .map(|impact| {
                issue(
                    SettingIssueKind::SyntaxError,
                    Some(Origin::new(file, impact.error.span)),
                )
            })
            .collect::<Vec<_>>()
    };

    let mut settings = DjangoSettings {
        installed_apps: installed_apps(values, namespace_dynamic.as_deref()),
        templates: templates(db, values, namespace_dynamic.as_deref()),
        staticfiles: StaticFilesSettings {
            static_url: static_url(values, namespace_dynamic.as_deref()),
            static_root: static_root(db, values, namespace_dynamic.as_deref()),
            staticfiles_dirs: staticfiles_dirs(db, values, namespace_dynamic.as_deref()),
        },
    };

    let issues = syntax_issues(KnownSetting::InstalledApps);
    if !issues.is_empty() {
        settings
            .installed_apps
            .add(SettingCase::Dynamic(DynamicInstalledApps {
                apps: OrderedInstalledApps {
                    evidence: issues
                        .into_iter()
                        .map(InstalledAppEvidence::Issue)
                        .collect(),
                },
            }));
    }
    let issues = syntax_issues(KnownSetting::Templates);
    if !issues.is_empty() {
        settings
            .templates
            .add(SettingCase::Dynamic(DynamicTemplates {
                templates: OrderedTemplateList {
                    evidence: issues
                        .into_iter()
                        .map(TemplateListEvidence::Issue)
                        .collect(),
                },
            }));
    }
    let issues = syntax_issues(KnownSetting::StaticUrl);
    if !issues.is_empty() {
        settings
            .staticfiles
            .static_url
            .add(SettingCase::Dynamic(DynamicScalarSetting { issues }));
    }
    let issues = syntax_issues(KnownSetting::StaticRoot);
    if !issues.is_empty() {
        settings
            .staticfiles
            .static_root
            .add(SettingCase::Dynamic(DynamicScalarSetting { issues }));
    }
    let issues = syntax_issues(KnownSetting::StaticFilesDirs);
    if !issues.is_empty() {
        settings
            .staticfiles
            .staticfiles_dirs
            .add(SettingCase::Dynamic(DynamicStaticFilesDirs {
                paths: OrderedPathList {
                    evidence: issues.into_iter().map(PathListEvidence::Issue).collect(),
                },
            }));
    }
    settings
}

fn binding_cases<T, D, I>(
    values: &PythonModuleValues,
    setting: KnownSetting,
    namespace_dynamic: Option<&[NamespaceDynamicEvidence]>,
    mut bound: impl FnMut(
        &PythonBoundValue,
        &BranchConstraints,
    ) -> Vec<(SettingCase<T, D, I>, BranchConstraints)>,
    dynamic: impl Fn(Vec<SettingIssue>) -> D,
) -> SettingAlternatives<T, D, I>
where
    T: MergeEvidence,
    D: MergeEvidence + MergeDynamicEvidence,
    I: MergeEvidence,
{
    let mut cases: Vec<(SettingCase<T, D, I>, BranchConstraints)> =
        Vec::with_capacity(MAX_EXACT_SETTING_ALTERNATIVES + 1);
    let mut overflow_origin = None;
    {
        let mut add_case = |case, correlation: BranchConstraints, origin| {
            for (existing, existing_correlation) in &mut cases {
                if existing.merge_evidence(&case) {
                    existing_correlation.merge_evidence(&correlation);
                    return;
                }
            }
            if cases.len() < MAX_EXACT_SETTING_ALTERNATIVES {
                cases.push((case, correlation));
            } else {
                overflow_origin = overflow_origin.or(origin);
            }
        };
        let mut unbound_constraints = Vec::new();
        if let Some(binding) = values.bindings.get(setting.name()) {
            for (alternative, constraints) in binding.alternatives_with_constraints() {
                match alternative {
                    PythonBindingState::Bound(value) => {
                        let origin = value
                            .binding_origins
                            .first()
                            .copied()
                            .or_else(|| value.value.origins().next());
                        for (case, correlation) in bound(value, constraints) {
                            add_case(case, correlation, origin);
                        }
                    }
                    PythonBindingState::Unbound => {
                        let correlation = constraints.clone();
                        unbound_constraints.push(correlation.clone());
                        add_case(SettingCase::Unset, correlation, None);
                    }
                }
            }
        } else {
            let correlation = BranchConstraints::unconstrained();
            unbound_constraints.push(correlation.clone());
            add_case(SettingCase::Unset, correlation, None);
        }
        if let Some(evidence) = namespace_dynamic {
            for unbound in &unbound_constraints {
                let mut groups: Vec<(BranchConstraints, Vec<SettingIssue>)> = Vec::new();
                for cause in evidence {
                    let correlation = unbound.intersection(&cause.constraints);
                    if correlation.is_impossible() {
                        continue;
                    }
                    if let Some((_, issues)) = groups
                        .iter_mut()
                        .find(|(existing, _)| *existing == correlation)
                    {
                        issues.push(cause.issue.clone());
                    } else {
                        groups.push((correlation, vec![cause.issue.clone()]));
                    }
                }
                for (correlation, issues) in groups {
                    let origin = issues
                        .iter()
                        .find_map(|issue| issue.origins.first())
                        .copied();
                    add_case(SettingCase::Dynamic(dynamic(issues)), correlation, origin);
                }
            }
        }
    }
    if let Some(origin) = overflow_origin {
        cases.push((
            SettingCase::Dynamic(dynamic(vec![alternative_limit_issue(origin)])),
            BranchConstraints::unconstrained(),
        ));
    }
    SettingAlternatives::from_correlated(cases)
}

fn correlated_cases<T, D, I>(
    cases: Vec<SettingCase<T, D, I>>,
    correlation: &BranchConstraints,
) -> Vec<(SettingCase<T, D, I>, BranchConstraints)> {
    cases
        .into_iter()
        .map(|case| (case, correlation.clone()))
        .collect()
}

fn installed_apps(
    values: &PythonModuleValues,
    namespace_dynamic: Option<&[NamespaceDynamicEvidence]>,
) -> InstalledAppsAlternatives {
    binding_cases(
        values,
        KnownSetting::InstalledApps,
        namespace_dynamic,
        |bound, constraints| {
            let mutation_issues =
                unsupported_mutation_issues(values, KnownSetting::InstalledApps, &bound.value);
            let PythonValueKind::List(list) = &bound.value.kind else {
                let case = match &bound.value.kind {
                    PythonValueKind::Unknown(_) => SettingCase::Dynamic(DynamicInstalledApps {
                        apps: OrderedInstalledApps {
                            evidence: vec![InstalledAppEvidence::Issue(unknown_value_issue(
                                &bound.value,
                            ))],
                        },
                    }),
                    _ => SettingCase::Malformed(MalformedInstalledApps {
                        apps: OrderedInstalledApps {
                            evidence: vec![InstalledAppEvidence::Issue(value_issue(
                                SettingIssueKind::InvalidShape,
                                &bound.value,
                            ))],
                        },
                    }),
                };
                return correlated_cases(vec![case], constraints);
            };

            list.correlated_variants()
                .map(|(items, variant_constraints)| {
                    let (mut apps, malformed) = string_list_items(items);
                    apps.evidence.extend(
                        mutation_issues
                            .iter()
                            .cloned()
                            .map(InstalledAppEvidence::Issue),
                    );
                    let case = if malformed {
                        SettingCase::Malformed(MalformedInstalledApps { apps })
                    } else if apps
                        .evidence
                        .iter()
                        .all(|evidence| matches!(evidence, InstalledAppEvidence::Known(_)))
                    {
                        SettingCase::Known(InstalledAppsValue {
                            apps: apps
                                .evidence
                                .into_iter()
                                .filter_map(|evidence| match evidence {
                                    InstalledAppEvidence::Known(app) => Some(app),
                                    InstalledAppEvidence::Issue(_) => None,
                                })
                                .collect(),
                        })
                    } else {
                        SettingCase::Dynamic(DynamicInstalledApps { apps })
                    };
                    (case, constraints.intersection(variant_constraints))
                })
                .collect()
        },
        |issues| DynamicInstalledApps {
            apps: OrderedInstalledApps {
                evidence: issues
                    .into_iter()
                    .map(InstalledAppEvidence::Issue)
                    .collect(),
            },
        },
    )
}

fn string_list_items(items: &[PythonListItem]) -> (OrderedInstalledApps, bool) {
    let mut evidence = Vec::new();
    let mut malformed = false;
    for item in items {
        let item = match item {
            PythonListItem::Value(value) => match &value.kind {
                PythonValueKind::Str(text) => InstalledAppEvidence::Known(WithOrigin::new(
                    text.clone(),
                    value.origins().collect(),
                )),
                PythonValueKind::Unknown(_) => {
                    InstalledAppEvidence::Issue(unknown_value_issue(value))
                }
                _ => {
                    malformed = true;
                    InstalledAppEvidence::Issue(value_issue(SettingIssueKind::InvalidShape, value))
                }
            },
            PythonListItem::UnknownElement(unknown) => {
                InstalledAppEvidence::Issue(issue(SettingIssueKind::UnknownElement, unknown.origin))
            }
            PythonListItem::UnknownUnpack(unknown) => {
                InstalledAppEvidence::Issue(issue(SettingIssueKind::UnknownUnpack, unknown.origin))
            }
        };
        evidence.push(item);
    }
    (OrderedInstalledApps { evidence }, malformed)
}

fn templates(
    db: &dyn ProjectDb,
    values: &PythonModuleValues,
    namespace_dynamic: Option<&[NamespaceDynamicEvidence]>,
) -> TemplateAlternatives {
    binding_cases(
        values,
        KnownSetting::Templates,
        namespace_dynamic,
        |bound, constraints| {
            let mutation_issues =
                unsupported_mutation_issues(values, KnownSetting::Templates, &bound.value);
            match &bound.value.kind {
                PythonValueKind::List(list) => {
                    template_list(db, list, &mutation_issues, constraints)
                }
                PythonValueKind::Unknown(_) => correlated_cases(
                    vec![SettingCase::Dynamic(DynamicTemplates {
                        templates: OrderedTemplateList {
                            evidence: vec![TemplateListEvidence::Issue(unknown_value_issue(
                                &bound.value,
                            ))],
                        },
                    })],
                    constraints,
                ),
                _ => correlated_cases(
                    vec![SettingCase::Malformed(MalformedTemplates {
                        templates: OrderedTemplateList {
                            evidence: vec![TemplateListEvidence::Issue(value_issue(
                                SettingIssueKind::InvalidShape,
                                &bound.value,
                            ))],
                        },
                    })],
                    constraints,
                ),
            }
        },
        |issues| DynamicTemplates {
            templates: OrderedTemplateList {
                evidence: issues
                    .into_iter()
                    .map(TemplateListEvidence::Issue)
                    .collect(),
            },
        },
    )
}

fn template_list(
    db: &dyn ProjectDb,
    list: &PythonList,
    issues: &[SettingIssue],
    outer_correlation: &BranchConstraints,
) -> Vec<(
    SettingCase<TemplatesValue, DynamicTemplates, MalformedTemplates>,
    BranchConstraints,
)> {
    let mut cases = Vec::with_capacity(MAX_EXACT_SETTING_ALTERNATIVES + 1);
    let mut overflow_origin = None;
    for (items, constraints) in list.correlated_variants() {
        if cases.len() == MAX_EXACT_SETTING_ALTERNATIVES {
            overflow_origin = overflow_origin.or_else(|| list_item_origin(items));
            break;
        }
        let expansion = template_list_variant(
            db,
            items,
            issues,
            MAX_EXACT_SETTING_ALTERNATIVES - cases.len(),
        );
        cases.extend(expansion.exact.into_iter().filter_map(|configuration| {
            let correlation = outer_correlation
                .intersection(constraints)
                .intersection(&configuration.correlation);
            (!correlation.is_impossible()).then_some((configuration.case, correlation))
        }));
        overflow_origin = overflow_origin.or(expansion.overflow_origin);
    }
    if let Some(origin) = overflow_origin {
        cases.push((
            SettingCase::Dynamic(DynamicTemplates {
                templates: OrderedTemplateList {
                    evidence: vec![TemplateListEvidence::Issue(alternative_limit_issue(origin))],
                },
            }),
            BranchConstraints::unconstrained(),
        ));
    }
    cases
}

struct CorrelatedTemplateConfiguration {
    case: SettingCase<TemplatesValue, DynamicTemplates, MalformedTemplates>,
    correlation: BranchConstraints,
}

fn template_list_variant(
    db: &dyn ProjectDb,
    items: &[PythonListItem],
    issues: &[SettingIssue],
    limit: usize,
) -> CappedExpansion<CorrelatedTemplateConfiguration> {
    let initial_evidence = issues
        .iter()
        .cloned()
        .map(TemplateListEvidence::Issue)
        .collect::<Vec<_>>();
    let mut configurations = vec![(initial_evidence, false, BranchConstraints::unconstrained())];
    let mut overflow_origin = None;
    for item in items {
        let alternatives = match item {
            PythonListItem::Value(value) => match &value.kind {
                PythonValueKind::Dict(dict) => {
                    let expansion = partial_backend(db, dict);
                    overflow_origin = overflow_origin.or(expansion.overflow_origin);
                    expansion
                        .exact
                        .into_iter()
                        .map(|backend| {
                            let malformed = backend.is_malformed();
                            let correlation = backend.correlation.clone();
                            (
                                TemplateListEvidence::Backend(Box::new(backend)),
                                malformed,
                                correlation,
                            )
                        })
                        .collect()
                }
                PythonValueKind::Unknown(_) => vec![(
                    TemplateListEvidence::Issue(unknown_value_issue(value)),
                    false,
                    BranchConstraints::unconstrained(),
                )],
                _ => vec![(
                    TemplateListEvidence::Issue(value_issue(SettingIssueKind::InvalidShape, value)),
                    true,
                    BranchConstraints::unconstrained(),
                )],
            },
            PythonListItem::UnknownElement(unknown) => vec![(
                TemplateListEvidence::Issue(issue(
                    SettingIssueKind::UnknownElement,
                    unknown.origin,
                )),
                false,
                BranchConstraints::unconstrained(),
            )],
            PythonListItem::UnknownUnpack(unknown) => vec![(
                TemplateListEvidence::Issue(issue(SettingIssueKind::UnknownUnpack, unknown.origin)),
                false,
                BranchConstraints::unconstrained(),
            )],
        };
        let item_origin = python_list_item_origin(item);
        let mut next =
            Vec::with_capacity(limit.min(configurations.len().saturating_mul(alternatives.len())));
        for (evidence, malformed, correlation) in &configurations {
            for (item, item_malformed, item_correlation) in &alternatives {
                let correlation = correlation.intersection(item_correlation);
                if correlation.is_impossible() {
                    continue;
                }
                if next.len() == limit {
                    overflow_origin = overflow_origin.or(item_origin);
                    continue;
                }
                let mut evidence = evidence.clone();
                evidence.push(item.clone());
                next.push((evidence, *malformed || *item_malformed, correlation));
            }
        }
        configurations = next;
    }

    CappedExpansion {
        exact: configurations
            .into_iter()
            .map(|(evidence, malformed, correlation)| {
                template_configuration(evidence, malformed, correlation)
            })
            .collect(),
        overflow_origin,
    }
}

fn template_configuration(
    evidence: Vec<TemplateListEvidence>,
    malformed: bool,
    correlation: BranchConstraints,
) -> CorrelatedTemplateConfiguration {
    let templates = OrderedTemplateList { evidence };
    let case = if malformed {
        SettingCase::Malformed(MalformedTemplates { templates })
    } else if templates.evidence.iter().any(|evidence| match evidence {
        TemplateListEvidence::Backend(backend) => backend.has_issues(),
        TemplateListEvidence::Issue(_) => true,
    }) {
        SettingCase::Dynamic(DynamicTemplates { templates })
    } else {
        SettingCase::Known(TemplatesValue {
            backends: templates
                .evidence
                .into_iter()
                .filter_map(|evidence| match evidence {
                    TemplateListEvidence::Backend(backend) => {
                        let backend = *backend;
                        Some(TemplateBackend {
                            backend: backend.backend.known.expect("complete backend has BACKEND"),
                            dirs: backend.dirs.into_known(),
                            app_dirs: backend.app_dirs.known,
                            libraries: backend.libraries.known,
                            builtins: backend.builtins.known,
                            context_processors: backend.context_processors.known,
                        })
                    }
                    TemplateListEvidence::Issue(_) => None,
                })
                .collect(),
        })
    };
    CorrelatedTemplateConfiguration { case, correlation }
}

fn partial_backend(
    db: &dyn ProjectDb,
    dict: &PythonDict,
) -> CappedExpansion<PartialTemplateBackend> {
    let mut backend = PartialTemplateBackend {
        correlation: BranchConstraints::unconstrained(),
        backend: PartialSettingField::new(None),
        dirs: OrderedPathList::new(),
        app_dirs: PartialSettingField::new(None),
        options: PartialSettingField::new(()),
        libraries: PartialSettingField::new(Vec::new()),
        builtins: PartialSettingField::new(Vec::new()),
        context_processors: PartialSettingField::new(Vec::new()),
    };

    let (backend_value, mut issues) = dict_lookup(dict, "BACKEND");
    backend.backend.issues.append(&mut issues);
    match backend_value {
        Some(value) => match &value.kind {
            PythonValueKind::Str(name) => {
                backend.backend.known =
                    Some(WithOrigin::new(name.clone(), value.origins().collect()));
            }
            PythonValueKind::Unknown(_) => {
                backend.backend.issues.push(unknown_value_issue(value));
            }
            _ => backend
                .backend
                .issues
                .push(value_issue(SettingIssueKind::InvalidShape, value)),
        },
        None => backend
            .backend
            .issues
            .push(issue(SettingIssueKind::MissingBackend, None)),
    }

    let (dirs_value, dirs_issues) = dict_lookup(dict, "DIRS");

    let (app_dirs_value, mut issues) = dict_lookup(dict, "APP_DIRS");
    backend.app_dirs.issues.append(&mut issues);
    if let Some(value) = app_dirs_value {
        match &value.kind {
            PythonValueKind::Bool(flag) => {
                backend.app_dirs.known = Some(WithOrigin::new(*flag, value.origins().collect()));
            }
            PythonValueKind::Unknown(_) => {
                backend.app_dirs.issues.push(unknown_value_issue(value));
            }
            _ => backend
                .app_dirs
                .issues
                .push(value_issue(SettingIssueKind::InvalidShape, value)),
        }
    }

    let (options_value, mut issues) = dict_lookup(dict, "OPTIONS");
    backend.options.issues.append(&mut issues);
    if let Some(options) = options_value {
        match &options.kind {
            PythonValueKind::Dict(options) => extract_options(options, &mut backend),
            PythonValueKind::Unknown(_) => {
                backend.options.issues.push(unknown_value_issue(options));
            }
            _ => backend
                .options
                .issues
                .push(value_issue(SettingIssueKind::InvalidShape, options)),
        }
    }

    let dirs = dirs_value.map_or_else(
        || CappedExpansion::one(PathListProjection::empty()),
        |value| path_list_capped(db, value),
    );
    CappedExpansion {
        exact: dirs
            .exact
            .into_iter()
            .map(|projection| {
                let mut projected = backend.clone();
                projected.correlation = projection.constraints;
                projected.dirs.extend_issues(dirs_issues.clone());
                projected.dirs.evidence.extend(projection.paths.evidence);
                projected
            })
            .collect(),
        overflow_origin: dirs.overflow_origin,
    }
}

fn extract_options(options: &PythonDict, backend: &mut PartialTemplateBackend) {
    let (libraries_value, mut issues) = dict_lookup(options, "libraries");
    backend.libraries.issues.append(&mut issues);
    if let Some(value) = libraries_value {
        match &value.kind {
            PythonValueKind::Dict(dict) => extract_libraries(dict, &mut backend.libraries),
            PythonValueKind::Unknown(_) => {
                backend.libraries.issues.push(unknown_value_issue(value));
            }
            _ => backend
                .libraries
                .issues
                .push(value_issue(SettingIssueKind::InvalidShape, value)),
        }
    }

    let (builtins_value, mut issues) = dict_lookup(options, "builtins");
    backend.builtins.issues.append(&mut issues);
    if let Some(value) = builtins_value {
        let (known, mut issues, _) = module_name_list(value);
        backend.builtins.known = known;
        backend.builtins.issues.append(&mut issues);
    }

    let (processors_value, mut issues) = dict_lookup(options, "context_processors");
    backend.context_processors.issues.append(&mut issues);
    if let Some(value) = processors_value {
        match &value.kind {
            PythonValueKind::List(list) => {
                for item in list.semantic_items() {
                    match item {
                        PythonListItem::Value(value) => match &value.kind {
                            PythonValueKind::Str(path) => {
                                match TemplateContextProcessorPath::parse(path) {
                                    Ok(path) => backend
                                        .context_processors
                                        .known
                                        .push(WithOrigin::new(path, value.origins().collect())),
                                    Err(_) => backend.context_processors.issues.push(value_issue(
                                        SettingIssueKind::InvalidModuleName,
                                        value,
                                    )),
                                }
                            }
                            PythonValueKind::Unknown(_) => backend
                                .context_processors
                                .issues
                                .push(unknown_value_issue(value)),
                            _ => backend
                                .context_processors
                                .issues
                                .push(value_issue(SettingIssueKind::InvalidShape, value)),
                        },
                        PythonListItem::UnknownElement(unknown) => backend
                            .context_processors
                            .issues
                            .push(issue(SettingIssueKind::UnknownElement, unknown.origin)),
                        PythonListItem::UnknownUnpack(unknown) => backend
                            .context_processors
                            .issues
                            .push(issue(SettingIssueKind::UnknownUnpack, unknown.origin)),
                    }
                }
            }
            PythonValueKind::Unknown(_) => backend
                .context_processors
                .issues
                .push(unknown_value_issue(value)),
            _ => backend
                .context_processors
                .issues
                .push(value_issue(SettingIssueKind::InvalidShape, value)),
        }
    }
}

fn extract_libraries(
    dict: &PythonDict,
    libraries: &mut PartialSettingField<Vec<(String, WithOrigin<crate::python::PythonModuleName>)>>,
) {
    let mut seen = std::collections::BTreeSet::new();
    let mut unknown_unpack_has_authority = false;
    for item in dict.items.iter().rev() {
        match item {
            PythonDictItem::Entry { key, value } => {
                let PythonValueKind::Str(alias) = &key.kind else {
                    let issue = match &key.kind {
                        PythonValueKind::Unknown(_) => {
                            unknown_unpack_has_authority = true;
                            unknown_value_issue(key)
                        }
                        _ => value_issue(SettingIssueKind::InvalidShape, key),
                    };
                    libraries.issues.insert(0, issue);
                    continue;
                };
                if !seen.insert(alias.clone()) || unknown_unpack_has_authority {
                    continue;
                }
                match &value.kind {
                    PythonValueKind::Str(module) => {
                        match crate::python::PythonModuleName::parse(module) {
                            Ok(module) => libraries.known.insert(
                                0,
                                (
                                    alias.clone(),
                                    WithOrigin::new(module, value.origins().collect()),
                                ),
                            ),
                            Err(_) => libraries
                                .issues
                                .insert(0, value_issue(SettingIssueKind::InvalidModuleName, value)),
                        }
                    }
                    PythonValueKind::Unknown(_) => {
                        libraries.issues.insert(0, unknown_value_issue(value));
                    }
                    _ => libraries
                        .issues
                        .insert(0, value_issue(SettingIssueKind::InvalidShape, value)),
                }
            }
            PythonDictItem::UnknownUnpack(unknown) => {
                unknown_unpack_has_authority = true;
                libraries
                    .issues
                    .insert(0, issue(SettingIssueKind::UnknownUnpack, unknown.origin));
            }
        }
    }
}

fn module_name_list(
    value: &PythonValue,
) -> (
    Vec<WithOrigin<crate::python::PythonModuleName>>,
    Vec<SettingIssue>,
    bool,
) {
    let mut known = Vec::new();
    let mut issues = Vec::new();
    let mut malformed = false;
    let PythonValueKind::List(list) = &value.kind else {
        return match &value.kind {
            PythonValueKind::Unknown(_) => (known, vec![unknown_value_issue(value)], false),
            _ => (
                known,
                vec![value_issue(SettingIssueKind::InvalidShape, value)],
                true,
            ),
        };
    };
    for item in list.semantic_items() {
        match item {
            PythonListItem::Value(value) => match &value.kind {
                PythonValueKind::Str(name) => {
                    if let Ok(name) = crate::python::PythonModuleName::parse(name) {
                        known.push(WithOrigin::new(name, value.origins().collect()));
                    } else {
                        malformed = true;
                        issues.push(value_issue(SettingIssueKind::InvalidModuleName, value));
                    }
                }
                PythonValueKind::Unknown(_) => {
                    issues.push(unknown_value_issue(value));
                }
                _ => {
                    malformed = true;
                    issues.push(value_issue(SettingIssueKind::InvalidShape, value));
                }
            },
            PythonListItem::UnknownElement(unknown) => {
                issues.push(issue(SettingIssueKind::UnknownElement, unknown.origin));
            }
            PythonListItem::UnknownUnpack(unknown) => {
                issues.push(issue(SettingIssueKind::UnknownUnpack, unknown.origin));
            }
        }
    }
    (known, issues, malformed)
}

fn static_url(
    values: &PythonModuleValues,
    namespace_dynamic: Option<&[NamespaceDynamicEvidence]>,
) -> StaticUrlAlternatives {
    binding_cases(
        values,
        KnownSetting::StaticUrl,
        namespace_dynamic,
        |bound, constraints| {
            correlated_cases(
                vec![match &bound.value.kind {
                    PythonValueKind::Str(value) => SettingCase::Known(WithOrigin::new(
                        StaticUrl(value.clone()),
                        bound.value.origins().collect(),
                    )),
                    PythonValueKind::Unknown(_) => SettingCase::Dynamic(DynamicScalarSetting {
                        issues: vec![unknown_value_issue(&bound.value)],
                    }),
                    _ => SettingCase::Malformed(MalformedScalarSetting {
                        issues: vec![value_issue(SettingIssueKind::InvalidShape, &bound.value)],
                    }),
                }],
                constraints,
            )
        },
        |issues| DynamicScalarSetting { issues },
    )
}

fn static_root(
    db: &dyn ProjectDb,
    values: &PythonModuleValues,
    namespace_dynamic: Option<&[NamespaceDynamicEvidence]>,
) -> StaticRootAlternatives {
    binding_cases(
        values,
        KnownSetting::StaticRoot,
        namespace_dynamic,
        |bound, constraints| {
            let paths = evaluated_paths(db, &bound.value);
            if paths.is_empty() {
                let case = if let PythonValueKind::Unknown(_) = &bound.value.kind {
                    SettingCase::Dynamic(DynamicScalarSetting {
                        issues: vec![unknown_value_issue(&bound.value)],
                    })
                } else {
                    SettingCase::Malformed(MalformedScalarSetting {
                        issues: vec![value_issue(SettingIssueKind::InvalidShape, &bound.value)],
                    })
                };
                correlated_cases(vec![case], constraints)
            } else {
                paths
                    .into_iter()
                    .map(|path| {
                        (
                            SettingCase::Known(WithOrigin::new(
                                StaticRoot::new(path.path.value),
                                path.path.origins,
                            )),
                            constraints.intersection(&path.constraints),
                        )
                    })
                    .collect()
            }
        },
        |issues| DynamicScalarSetting { issues },
    )
}

fn staticfiles_dirs(
    db: &dyn ProjectDb,
    values: &PythonModuleValues,
    namespace_dynamic: Option<&[NamespaceDynamicEvidence]>,
) -> StaticFilesDirsAlternatives {
    binding_cases(
        values,
        KnownSetting::StaticFilesDirs,
        namespace_dynamic,
        |bound, constraints| {
            let mutation_issues =
                unsupported_mutation_issues(values, KnownSetting::StaticFilesDirs, &bound.value);
            match &bound.value.kind {
                PythonValueKind::List(_) if !mutation_issues.is_empty() => correlated_cases(
                    vec![SettingCase::Dynamic(DynamicStaticFilesDirs {
                        paths: OrderedPathList {
                            evidence: mutation_issues
                                .into_iter()
                                .map(PathListEvidence::Issue)
                                .collect(),
                        },
                    })],
                    constraints,
                ),
                PythonValueKind::List(_) => path_list(db, &bound.value)
                    .into_iter()
                    .map(|projection| {
                        let projection_correlation = projection.constraints;
                        let paths = projection.paths;
                        let case = if projection.malformed {
                            SettingCase::Malformed(MalformedStaticFilesDirs { paths })
                        } else if paths.has_issues() {
                            SettingCase::Dynamic(DynamicStaticFilesDirs { paths })
                        } else {
                            SettingCase::Known(StaticFilesDirsValue {
                                dirs: paths.into_known(),
                            })
                        };
                        (case, constraints.intersection(&projection_correlation))
                    })
                    .collect(),
                PythonValueKind::Unknown(_) => correlated_cases(
                    vec![SettingCase::Dynamic(DynamicStaticFilesDirs {
                        paths: OrderedPathList {
                            evidence: vec![PathListEvidence::Issue(unknown_value_issue(
                                &bound.value,
                            ))],
                        },
                    })],
                    constraints,
                ),
                _ => correlated_cases(
                    vec![SettingCase::Malformed(MalformedStaticFilesDirs {
                        paths: OrderedPathList {
                            evidence: vec![PathListEvidence::Issue(value_issue(
                                SettingIssueKind::InvalidShape,
                                &bound.value,
                            ))],
                        },
                    })],
                    constraints,
                ),
            }
        },
        |issues| DynamicStaticFilesDirs {
            paths: OrderedPathList {
                evidence: issues.into_iter().map(PathListEvidence::Issue).collect(),
            },
        },
    )
}

#[derive(Clone)]
struct PathListProjection {
    paths: OrderedPathList,
    malformed: bool,
    constraints: BranchConstraints,
}

impl PathListProjection {
    fn empty() -> Self {
        Self {
            paths: OrderedPathList::new(),
            malformed: false,
            constraints: BranchConstraints::unconstrained(),
        }
    }
}

struct CappedExpansion<T> {
    exact: Vec<T>,
    overflow_origin: Option<Origin>,
}

impl<T> CappedExpansion<T> {
    fn one(value: T) -> Self {
        Self {
            exact: vec![value],
            overflow_origin: None,
        }
    }
}

fn path_list(db: &dyn ProjectDb, value: &PythonValue) -> Vec<PathListProjection> {
    let expansion = path_list_capped(db, value);
    let mut projections = expansion.exact;
    if let Some(origin) = expansion.overflow_origin {
        let mut overflow = PathListProjection::empty();
        overflow.paths.push_issue(alternative_limit_issue(origin));
        projections.push(overflow);
    }
    projections
}

fn path_list_capped(
    db: &dyn ProjectDb,
    value: &PythonValue,
) -> CappedExpansion<PathListProjection> {
    let PythonValueKind::List(list) = &value.kind else {
        let mut projection = PathListProjection::empty();
        if let PythonValueKind::Unknown(_) = &value.kind {
            projection.paths.push_issue(unknown_value_issue(value));
        } else {
            projection
                .paths
                .push_issue(value_issue(SettingIssueKind::InvalidShape, value));
            projection.malformed = true;
        }
        return CappedExpansion::one(projection);
    };

    let mut projections = Vec::with_capacity(MAX_EXACT_SETTING_ALTERNATIVES);
    let mut overflow_origin = None;
    for (items, constraints) in list.correlated_variants() {
        if projections.len() == MAX_EXACT_SETTING_ALTERNATIVES {
            overflow_origin = overflow_origin
                .or_else(|| list_item_origin(items))
                .or_else(|| value.origins().next());
            break;
        }
        let expansion = path_list_variant(
            db,
            items,
            MAX_EXACT_SETTING_ALTERNATIVES - projections.len(),
        );
        projections.extend(expansion.exact.into_iter().map(|mut projection| {
            projection.constraints = projection.constraints.intersection(constraints);
            projection
        }));
        overflow_origin = overflow_origin.or(expansion.overflow_origin);
    }
    CappedExpansion {
        exact: projections,
        overflow_origin,
    }
}

fn path_list_variant(
    db: &dyn ProjectDb,
    items: &[PythonListItem],
    limit: usize,
) -> CappedExpansion<PathListProjection> {
    let mut configurations = vec![PathListProjection::empty()];
    let mut overflow_origin = None;
    for item in items {
        match item {
            PythonListItem::Value(value) => {
                let evaluated = evaluated_paths(db, value);
                if evaluated.is_empty() {
                    for projection in &mut configurations {
                        if let PythonValueKind::Unknown(_) = &value.kind {
                            projection.paths.push_issue(unknown_value_issue(value));
                        } else {
                            projection.malformed = true;
                            projection
                                .paths
                                .push_issue(value_issue(SettingIssueKind::InvalidShape, value));
                        }
                    }
                    continue;
                }
                if evaluated.len() == 1 {
                    let path = evaluated.into_iter().next().expect("one evaluated path");
                    for projection in &mut configurations {
                        projection.paths.push_known(path.path.clone());
                        projection.constraints =
                            projection.constraints.intersection(&path.constraints);
                    }
                    continue;
                }

                let mut next = Vec::with_capacity(
                    limit.min(configurations.len().saturating_mul(evaluated.len())),
                );
                for projection in &configurations {
                    for path in &evaluated {
                        if next.len() == limit {
                            overflow_origin.get_or_insert_with(|| path.path.origin());
                            continue;
                        }
                        let constraints = projection.constraints.intersection(&path.constraints);
                        if constraints.is_impossible() {
                            continue;
                        }
                        let mut projection = projection.clone();
                        projection.paths.push_known(path.path.clone());
                        projection.constraints = constraints;
                        next.push(projection);
                    }
                }
                configurations = next;
            }
            PythonListItem::UnknownElement(unknown) => {
                for projection in &mut configurations {
                    projection
                        .paths
                        .push_issue(issue(SettingIssueKind::UnknownElement, unknown.origin));
                }
            }
            PythonListItem::UnknownUnpack(unknown) => {
                for projection in &mut configurations {
                    projection
                        .paths
                        .push_issue(issue(SettingIssueKind::UnknownUnpack, unknown.origin));
                }
            }
        }
    }
    CappedExpansion {
        exact: configurations,
        overflow_origin,
    }
}

struct EvaluatedPathCandidate {
    path: WithOrigin<EvaluatedPath>,
    constraints: BranchConstraints,
}

fn evaluated_paths(db: &dyn ProjectDb, value: &PythonValue) -> Vec<EvaluatedPathCandidate> {
    match &value.kind {
        PythonValueKind::Path(path) => value
            .origins_with_constraints()
            .map(|(origin, constraints)| EvaluatedPathCandidate {
                path: WithOrigin::new(EvaluatedPath::Resolved(path.clone()), vec![origin]),
                constraints: constraints.clone(),
            })
            .collect(),
        PythonValueKind::Str(path) => {
            let path = Utf8Path::new(path);
            value
                .origins_with_constraints()
                .filter_map(|(origin, constraints)| {
                    let resolved = if path.is_absolute() {
                        EvaluatedPath::Resolved(path.to_path_buf())
                    } else {
                        EvaluatedPath::Resolved(origin.file.path(db).parent()?.join(path))
                    };
                    Some(EvaluatedPathCandidate {
                        path: WithOrigin::new(resolved, vec![origin]),
                        constraints: constraints.clone(),
                    })
                })
                .collect()
        }
        _ => Vec::new(),
    }
}

fn dict_lookup<'a>(
    dict: &'a PythonDict,
    wanted: &str,
) -> (Option<&'a PythonValue>, Vec<SettingIssue>) {
    let mut value = None;
    let mut issues = Vec::new();
    for item in &dict.items {
        match item {
            PythonDictItem::Entry {
                key,
                value: candidate,
            } if matches!(&key.kind, PythonValueKind::Str(key) if key == wanted) => {
                value = Some(candidate);
                issues.clear();
            }
            PythonDictItem::UnknownUnpack(unknown) => {
                issues.push(issue(SettingIssueKind::UnknownUnpack, unknown.origin));
            }
            PythonDictItem::Entry { key, .. }
                if matches!(key.kind, PythonValueKind::Unknown(_)) =>
            {
                issues.push(unknown_value_issue(key));
            }
            PythonDictItem::Entry { .. } => {}
        }
    }
    (value, issues)
}

fn setting_accepts_mutation(setting: KnownSetting, mutation: &PythonMutation) -> bool {
    match setting {
        KnownSetting::InstalledApps => {
            mutation.path.is_empty()
                && matches!(
                    mutation.operation,
                    PythonMutationOperation::Append
                        | PythonMutationOperation::Extend
                        | PythonMutationOperation::Insert
                        | PythonMutationOperation::Remove
                )
        }
        KnownSetting::Templates => {
            matches!(
                mutation.operation,
                PythonMutationOperation::Append | PythonMutationOperation::Extend
            ) && matches!(mutation.path.as_slice(), [PythonMutationPathSegment::Index(_), PythonMutationPathSegment::Key(key)] if key == "DIRS")
        }
        KnownSetting::StaticUrl | KnownSetting::StaticRoot | KnownSetting::StaticFilesDirs => false,
    }
}

fn unsupported_mutation_issues(
    values: &PythonModuleValues,
    setting: KnownSetting,
    value: &PythonValue,
) -> Vec<SettingIssue> {
    values
        .mutations
        .iter()
        .filter(|mutation| {
            mutation.binding == setting.name()
                && !setting_accepts_mutation(setting, mutation)
                && value_contains_origin(value, mutation.origin)
        })
        .map(|mutation| issue(SettingIssueKind::UnsupportedMutation, Some(mutation.origin)))
        .collect()
}

fn value_contains_origin(value: &PythonValue, wanted: Origin) -> bool {
    value.origins().any(|origin| origin == wanted)
        || match &value.kind {
            PythonValueKind::List(list) => list.semantic_items().iter().any(|item| match item {
                PythonListItem::Value(value) => value_contains_origin(value, wanted),
                PythonListItem::UnknownElement(unknown)
                | PythonListItem::UnknownUnpack(unknown) => unknown.origin == Some(wanted),
            }),
            PythonValueKind::Dict(dict) => dict.items.iter().any(|item| match item {
                PythonDictItem::Entry { key, value } => {
                    value_contains_origin(key, wanted) || value_contains_origin(value, wanted)
                }
                PythonDictItem::UnknownUnpack(unknown) => unknown.origin == Some(wanted),
            }),
            PythonValueKind::Unknown(unknown) => unknown.origin == Some(wanted),
            PythonValueKind::Str(_) | PythonValueKind::Bool(_) | PythonValueKind::Path(_) => false,
        }
}

fn list_item_origin(items: &[PythonListItem]) -> Option<Origin> {
    items.iter().find_map(python_list_item_origin)
}

fn python_list_item_origin(item: &PythonListItem) -> Option<Origin> {
    match item {
        PythonListItem::Value(value) => value.origins().next(),
        PythonListItem::UnknownElement(unknown) | PythonListItem::UnknownUnpack(unknown) => {
            unknown.origin
        }
    }
}

fn alternative_limit_issue(origin: Origin) -> SettingIssue {
    unknown_issue(&PythonUnknown {
        cause: PythonUnknownCause::AlternativeLimitExceeded,
        origin: Some(origin),
    })
}

fn unknown_issue(unknown: &PythonUnknown) -> SettingIssue {
    issue(unknown_issue_kind(unknown), unknown.origin)
}

fn unknown_value_issue(value: &PythonValue) -> SettingIssue {
    let PythonValueKind::Unknown(unknown) = &value.kind else {
        unreachable!("unknown value issue requires PythonValueKind::Unknown");
    };
    value_issue(unknown_issue_kind(unknown), value)
}

fn unknown_issue_kind(unknown: &PythonUnknown) -> SettingIssueKind {
    match unknown.cause {
        PythonUnknownCause::Unreadable(_) => SettingIssueKind::Unreadable,
        PythonUnknownCause::SyntaxErrors(_) => SettingIssueKind::SyntaxError,
        PythonUnknownCause::UnsupportedMutation => SettingIssueKind::UnsupportedMutation,
        PythonUnknownCause::UnsupportedExpression
        | PythonUnknownCause::InvalidImport(_)
        | PythonUnknownCause::ImportNotFound(_)
        | PythonUnknownCause::SkippedExternal(_)
        | PythonUnknownCause::Cycle
        | PythonUnknownCause::AlternativeLimitExceeded => SettingIssueKind::DynamicExpression,
    }
}

fn value_issue(kind: SettingIssueKind, value: &PythonValue) -> SettingIssue {
    SettingIssue {
        kind,
        origins: value.origins().collect(),
    }
}
fn issue(kind: SettingIssueKind, origin: Option<Origin>) -> SettingIssue {
    SettingIssue {
        kind,
        origins: origin.into_iter().collect(),
    }
}
