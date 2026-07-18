use camino::Utf8Path;
use djls_source::File;
use djls_source::Origin;

use crate::db::Db as ProjectDb;
use crate::python::evaluation::BranchConstraints;
use crate::python::evaluation::MappingOverride;
use crate::python::evaluation::MappingStringEntry;
use crate::python::evaluation::PythonBindingState;
use crate::python::evaluation::PythonBoundValue;
use crate::python::evaluation::PythonMapping;
use crate::python::evaluation::PythonMaterializedSequence;
use crate::python::evaluation::PythonModuleValues;
use crate::python::evaluation::PythonMutation;
use crate::python::evaluation::PythonMutationOperation;
use crate::python::evaluation::PythonMutationPathSegment;
use crate::python::evaluation::PythonSequence;
use crate::python::evaluation::PythonSequenceAlternativeRef;
use crate::python::evaluation::PythonSequenceItem;
use crate::python::evaluation::PythonUnknown;
use crate::python::evaluation::PythonUnknownCause;
use crate::python::evaluation::PythonValue;
use crate::settings::types::DjangoSettings;
use crate::settings::types::EvaluatedPath;
use crate::settings::types::InstalledAppEvidence;
use crate::settings::types::InstalledAppsAlternatives;
use crate::settings::types::InstalledAppsValue;
use crate::settings::types::MAX_EXACT_SETTING_ALTERNATIVES;
use crate::settings::types::MergeDynamicEvidence;
use crate::settings::types::MergeEvidence;
use crate::settings::types::OrderedInstalledApps;
use crate::settings::types::OrderedPathList;
use crate::settings::types::OrderedTemplateList;
use crate::settings::types::PartialInstalledApps;
use crate::settings::types::PartialSettingField;
use crate::settings::types::PartialTemplateBackend;
use crate::settings::types::PartialTemplates;
use crate::settings::types::SettingAlternatives;
use crate::settings::types::SettingCase;
use crate::settings::types::SettingIssue;
use crate::settings::types::SettingIssueKind;
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
}

impl KnownSetting {
    const fn name(self) -> &'static str {
        match self {
            Self::InstalledApps => "INSTALLED_APPS",
            Self::Templates => "TEMPLATES",
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
            .as_slice()
            .iter()
            .map(|cause| NamespaceDynamicEvidence {
                issue: issue(SettingIssueKind::DynamicNamespace, cause.unknown.origins()),
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
    };

    let issues = syntax_issues(KnownSetting::InstalledApps);
    if !issues.is_empty() {
        settings
            .installed_apps
            .add(SettingCase::Dynamic(PartialInstalledApps {
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
            .add(SettingCase::Dynamic(PartialTemplates {
                templates: OrderedTemplateList {
                    evidence: issues
                        .into_iter()
                        .map(TemplateListEvidence::Issue)
                        .collect(),
                },
            }));
    }
    settings
}

fn binding_cases<T, P>(
    values: &PythonModuleValues,
    setting: KnownSetting,
    namespace_dynamic: Option<&[NamespaceDynamicEvidence]>,
    mut bound: impl FnMut(
        &PythonBoundValue,
        &BranchConstraints,
    ) -> Vec<(SettingCase<T, P>, BranchConstraints)>,
    dynamic: impl Fn(Vec<SettingIssue>) -> P,
) -> SettingAlternatives<T, P>
where
    T: MergeEvidence,
    P: MergeEvidence + MergeDynamicEvidence,
{
    let mut cases: Vec<(SettingCase<T, P>, BranchConstraints)> =
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
                            .representative_binding_origin()
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
            SettingCase::Dynamic(dynamic(vec![alternative_limit_issue([origin])])),
            BranchConstraints::unconstrained(),
        ));
    }
    SettingAlternatives::from_correlated(cases)
}

fn correlated_cases<T, P>(
    cases: Vec<SettingCase<T, P>>,
    correlation: &BranchConstraints,
) -> Vec<(SettingCase<T, P>, BranchConstraints)> {
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
            let Some(sequence) = collection_sequence(&bound.value) else {
                let case = if bound.value.unknown_value().is_some() {
                    SettingCase::Dynamic(PartialInstalledApps {
                        apps: OrderedInstalledApps {
                            evidence: vec![InstalledAppEvidence::Issue(unknown_value_issue(
                                &bound.value,
                            ))],
                        },
                    })
                } else {
                    SettingCase::Malformed(PartialInstalledApps {
                        apps: OrderedInstalledApps {
                            evidence: vec![InstalledAppEvidence::Issue(value_issue(
                                SettingIssueKind::InvalidShape,
                                &bound.value,
                            ))],
                        },
                    })
                };
                return correlated_cases(vec![case], constraints);
            };

            sequence
                .alternatives()
                .map(|alternative| match alternative {
                    PythonSequenceAlternativeRef::Exact {
                        items,
                        constraints: alternative_constraints,
                    } => (
                        installed_apps_list_case(items, &mutation_issues),
                        constraints.intersection(alternative_constraints),
                    ),
                    PythonSequenceAlternativeRef::Remainder {
                        origins,
                        constraints: remainder_constraints,
                    } => {
                        let mut apps = OrderedInstalledApps {
                            evidence: vec![InstalledAppEvidence::Issue(alternative_limit_issue(
                                origins.iter().copied(),
                            ))],
                        };
                        apps.evidence.extend(
                            mutation_issues
                                .iter()
                                .cloned()
                                .map(InstalledAppEvidence::Issue),
                        );
                        (
                            SettingCase::Dynamic(PartialInstalledApps { apps }),
                            constraints.intersection(remainder_constraints),
                        )
                    }
                })
                .collect()
        },
        |issues| PartialInstalledApps {
            apps: OrderedInstalledApps {
                evidence: issues
                    .into_iter()
                    .map(InstalledAppEvidence::Issue)
                    .collect(),
            },
        },
    )
}

fn installed_apps_list_case(
    items: &[PythonSequenceItem],
    mutation_issues: &[SettingIssue],
) -> SettingCase<InstalledAppsValue, PartialInstalledApps> {
    let (mut apps, malformed) = string_list_items(items);
    apps.evidence.extend(
        mutation_issues
            .iter()
            .cloned()
            .map(InstalledAppEvidence::Issue),
    );
    if malformed {
        SettingCase::Malformed(PartialInstalledApps { apps })
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
        SettingCase::Dynamic(PartialInstalledApps { apps })
    }
}

fn string_list_items(items: &[PythonSequenceItem]) -> (OrderedInstalledApps, bool) {
    let mut evidence = Vec::new();
    let mut malformed = false;
    for item in items {
        let item = match item {
            PythonSequenceItem::Value(value) => {
                if let Some(scalar) = value.known_scalar()
                    && let Some(text) = scalar.string_value()
                {
                    InstalledAppEvidence::Known(WithOrigin::new(
                        text.to_string(),
                        scalar.first_origin(),
                        scalar.additional_origins(),
                    ))
                } else if value.unknown_value().is_some() {
                    InstalledAppEvidence::Issue(unknown_value_issue(value))
                } else {
                    malformed = true;
                    InstalledAppEvidence::Issue(value_issue(SettingIssueKind::InvalidShape, value))
                }
            }
            PythonSequenceItem::UnknownElement(unknown) => InstalledAppEvidence::Issue(issue(
                SettingIssueKind::UnknownElement,
                unknown.origins(),
            )),
            PythonSequenceItem::UnknownUnpack(unknown) => InstalledAppEvidence::Issue(issue(
                SettingIssueKind::UnknownUnpack,
                unknown.origins(),
            )),
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
            if let Some(sequence) = collection_sequence(&bound.value) {
                template_list(db, sequence, &mutation_issues, constraints)
            } else if bound.value.unknown_value().is_some() {
                correlated_cases(
                    vec![SettingCase::Dynamic(PartialTemplates {
                        templates: OrderedTemplateList {
                            evidence: vec![TemplateListEvidence::Issue(unknown_value_issue(
                                &bound.value,
                            ))],
                        },
                    })],
                    constraints,
                )
            } else {
                correlated_cases(
                    vec![SettingCase::Malformed(PartialTemplates {
                        templates: OrderedTemplateList {
                            evidence: vec![TemplateListEvidence::Issue(value_issue(
                                SettingIssueKind::InvalidShape,
                                &bound.value,
                            ))],
                        },
                    })],
                    constraints,
                )
            }
        },
        |issues| PartialTemplates {
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
    sequence: PythonMaterializedSequence<'_>,
    issues: &[SettingIssue],
    outer_correlation: &BranchConstraints,
) -> Vec<(
    SettingCase<TemplatesValue, PartialTemplates>,
    BranchConstraints,
)> {
    let mut cases = Vec::with_capacity(MAX_EXACT_SETTING_ALTERNATIVES + 1);
    let mut overflow_origins: Option<Vec<Origin>> = None;
    for alternative in sequence.alternatives() {
        match alternative {
            PythonSequenceAlternativeRef::Exact { items, constraints } => {
                if cases.len() == MAX_EXACT_SETTING_ALTERNATIVES {
                    overflow_origins = overflow_origins
                        .or_else(|| list_item_origin(items).map(|origin| vec![origin]));
                    break;
                }
                let expansion = template_list_alternative(
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
                if overflow_origins.is_none() {
                    overflow_origins = expansion.overflow_origins;
                }
            }
            PythonSequenceAlternativeRef::Remainder {
                origins,
                constraints,
            } => {
                if cases.len() == MAX_EXACT_SETTING_ALTERNATIVES {
                    if overflow_origins.is_none() {
                        overflow_origins = Some(origins.to_vec());
                    }
                } else {
                    let mut evidence = issues
                        .iter()
                        .cloned()
                        .map(TemplateListEvidence::Issue)
                        .collect::<Vec<_>>();
                    evidence.push(TemplateListEvidence::Issue(alternative_limit_issue(
                        origins.iter().copied(),
                    )));
                    let configuration =
                        template_configuration(evidence, false, BranchConstraints::unconstrained());
                    let correlation = outer_correlation
                        .intersection(constraints)
                        .intersection(&configuration.correlation);
                    if !correlation.is_impossible() {
                        cases.push((configuration.case, correlation));
                    }
                }
            }
        }
    }
    if let Some(origins) = overflow_origins {
        cases.push((
            SettingCase::Dynamic(PartialTemplates {
                templates: OrderedTemplateList {
                    evidence: vec![TemplateListEvidence::Issue(alternative_limit_issue(
                        origins,
                    ))],
                },
            }),
            BranchConstraints::unconstrained(),
        ));
    }
    cases
}

struct CorrelatedTemplateConfiguration {
    case: SettingCase<TemplatesValue, PartialTemplates>,
    correlation: BranchConstraints,
}

fn template_list_alternative(
    db: &dyn ProjectDb,
    items: &[PythonSequenceItem],
    issues: &[SettingIssue],
    limit: usize,
) -> CappedExpansion<CorrelatedTemplateConfiguration> {
    let initial_evidence = issues
        .iter()
        .cloned()
        .map(TemplateListEvidence::Issue)
        .collect::<Vec<_>>();
    let mut configurations = vec![(initial_evidence, false, BranchConstraints::unconstrained())];
    let mut overflow_origins = None;
    for item in items {
        let alternatives = match item {
            PythonSequenceItem::Value(value) => {
                if let Some(mapping) = value.mapping() {
                    let expansion = partial_backend(db, mapping);
                    if overflow_origins.is_none() {
                        overflow_origins = expansion.overflow_origins;
                    }
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
                } else if value.unknown_value().is_some() {
                    vec![(
                        TemplateListEvidence::Issue(unknown_value_issue(value)),
                        false,
                        BranchConstraints::unconstrained(),
                    )]
                } else {
                    vec![(
                        TemplateListEvidence::Issue(value_issue(
                            SettingIssueKind::InvalidShape,
                            value,
                        )),
                        true,
                        BranchConstraints::unconstrained(),
                    )]
                }
            }
            PythonSequenceItem::UnknownElement(unknown) => vec![(
                TemplateListEvidence::Issue(issue(
                    SettingIssueKind::UnknownElement,
                    unknown.origins(),
                )),
                false,
                BranchConstraints::unconstrained(),
            )],
            PythonSequenceItem::UnknownUnpack(unknown) => vec![(
                TemplateListEvidence::Issue(issue(
                    SettingIssueKind::UnknownUnpack,
                    unknown.origins(),
                )),
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
                    if overflow_origins.is_none() {
                        overflow_origins = item_origin.map(|origin| vec![origin]);
                    }
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
        overflow_origins,
    }
}

fn template_configuration(
    evidence: Vec<TemplateListEvidence>,
    malformed: bool,
    correlation: BranchConstraints,
) -> CorrelatedTemplateConfiguration {
    let templates = OrderedTemplateList { evidence };
    let case = if malformed {
        SettingCase::Malformed(PartialTemplates { templates })
    } else if templates.evidence.iter().any(|evidence| match evidence {
        TemplateListEvidence::Backend(backend) => backend.has_issues(),
        TemplateListEvidence::Issue(_) => true,
    }) {
        SettingCase::Dynamic(PartialTemplates { templates })
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
    mapping: PythonMapping<'_>,
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

    let (backend_value, mut issues) = dict_field(mapping, "BACKEND");
    backend.backend.issues.append(&mut issues);
    match backend_value {
        Some(value) => {
            if let Some(scalar) = value.known_scalar()
                && let Some(name) = scalar.string_value()
            {
                backend.backend.known = Some(WithOrigin::new(
                    name.to_string(),
                    scalar.first_origin(),
                    scalar.additional_origins(),
                ));
            } else if value.unknown_value().is_some() {
                backend.backend.issues.push(unknown_value_issue(value));
            } else {
                backend
                    .backend
                    .issues
                    .push(value_issue(SettingIssueKind::InvalidShape, value));
            }
        }
        None => backend
            .backend
            .issues
            .push(issue(SettingIssueKind::MissingBackend, None)),
    }

    let (dirs_value, dirs_issues) = dict_field(mapping, "DIRS");

    let (app_dirs_value, mut issues) = dict_field(mapping, "APP_DIRS");
    backend.app_dirs.issues.append(&mut issues);
    if let Some(value) = app_dirs_value {
        if let Some(scalar) = value.known_scalar()
            && let Some(flag) = scalar.bool_value()
        {
            backend.app_dirs.known = Some(WithOrigin::new(
                flag,
                scalar.first_origin(),
                scalar.additional_origins(),
            ));
        } else if value.unknown_value().is_some() {
            backend.app_dirs.issues.push(unknown_value_issue(value));
        } else {
            backend
                .app_dirs
                .issues
                .push(value_issue(SettingIssueKind::InvalidShape, value));
        }
    }

    let (options_value, mut issues) = dict_field(mapping, "OPTIONS");
    backend.options.issues.append(&mut issues);
    if let Some(options) = options_value {
        if let Some(mapping) = options.mapping() {
            extract_options(mapping, &mut backend);
        } else if options.unknown_value().is_some() {
            backend.options.issues.push(unknown_value_issue(options));
        } else {
            backend
                .options
                .issues
                .push(value_issue(SettingIssueKind::InvalidShape, options));
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
        overflow_origins: dirs.overflow_origins,
    }
}

fn extract_options(options: PythonMapping<'_>, backend: &mut PartialTemplateBackend) {
    let (libraries_value, mut issues) = dict_field(options, "libraries");
    backend.libraries.issues.append(&mut issues);
    if let Some(value) = libraries_value {
        if let Some(mapping) = value.mapping() {
            extract_libraries(mapping, &mut backend.libraries);
        } else if value.unknown_value().is_some() {
            backend.libraries.issues.push(unknown_value_issue(value));
        } else {
            backend
                .libraries
                .issues
                .push(value_issue(SettingIssueKind::InvalidShape, value));
        }
    }

    let (builtins_value, mut issues) = dict_field(options, "builtins");
    backend.builtins.issues.append(&mut issues);
    if let Some(value) = builtins_value {
        let (known, mut issues, _) = module_name_list(value);
        backend.builtins.known = known;
        backend.builtins.issues.append(&mut issues);
    }

    let (processors_value, mut issues) = dict_field(options, "context_processors");
    backend.context_processors.issues.append(&mut issues);
    if let Some(value) = processors_value {
        if let Some(sequence) = collection_sequence(value) {
            for item in sequence.semantic_items() {
                match item {
                    PythonSequenceItem::Value(value) => {
                        if let Some(scalar) = value.known_scalar()
                            && let Some(path) = scalar.string_value()
                        {
                            match TemplateContextProcessorPath::parse(path) {
                                Ok(path) => backend.context_processors.known.push(WithOrigin::new(
                                    path,
                                    scalar.first_origin(),
                                    scalar.additional_origins(),
                                )),
                                Err(_) => backend
                                    .context_processors
                                    .issues
                                    .push(value_issue(SettingIssueKind::InvalidModuleName, value)),
                            }
                        } else if value.unknown_value().is_some() {
                            backend
                                .context_processors
                                .issues
                                .push(unknown_value_issue(value));
                        } else {
                            backend
                                .context_processors
                                .issues
                                .push(value_issue(SettingIssueKind::InvalidShape, value));
                        }
                    }
                    PythonSequenceItem::UnknownElement(unknown) => backend
                        .context_processors
                        .issues
                        .push(issue(SettingIssueKind::UnknownElement, unknown.origins())),
                    PythonSequenceItem::UnknownUnpack(unknown) => backend
                        .context_processors
                        .issues
                        .push(issue(SettingIssueKind::UnknownUnpack, unknown.origins())),
                }
            }
        } else if value.unknown_value().is_some() {
            backend
                .context_processors
                .issues
                .push(unknown_value_issue(value));
        } else {
            backend
                .context_processors
                .issues
                .push(value_issue(SettingIssueKind::InvalidShape, value));
        }
    }
}

fn extract_libraries(
    mapping: PythonMapping<'_>,
    libraries: &mut PartialSettingField<Vec<(String, WithOrigin<crate::python::PythonModuleName>)>>,
) {
    for entry in mapping.effective_string_entries() {
        match entry {
            MappingStringEntry::Value { key: alias, value } => {
                if let Some(scalar) = value.known_scalar()
                    && let Some(module) = scalar.string_value()
                {
                    match crate::python::PythonModuleName::parse(module) {
                        Ok(module) => libraries.known.push((
                            alias.to_string(),
                            WithOrigin::new(
                                module,
                                scalar.first_origin(),
                                scalar.additional_origins(),
                            ),
                        )),
                        Err(_) => libraries
                            .issues
                            .push(value_issue(SettingIssueKind::InvalidModuleName, value)),
                    }
                } else if value.unknown_value().is_some() {
                    libraries.issues.push(unknown_value_issue(value));
                } else {
                    libraries
                        .issues
                        .push(value_issue(SettingIssueKind::InvalidShape, value));
                }
            }
            MappingStringEntry::UnknownKey(key) => {
                libraries.issues.push(unknown_value_issue(key));
            }
            MappingStringEntry::InvalidKey(key) => {
                libraries
                    .issues
                    .push(value_issue(SettingIssueKind::InvalidShape, key));
            }
            MappingStringEntry::UnknownUnpack(unknown) => {
                libraries
                    .issues
                    .push(issue(SettingIssueKind::UnknownUnpack, unknown.origins()));
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
    let Some(sequence) = collection_sequence(value) else {
        return if value.unknown_value().is_some() {
            (known, vec![unknown_value_issue(value)], false)
        } else {
            (
                known,
                vec![value_issue(SettingIssueKind::InvalidShape, value)],
                true,
            )
        };
    };
    for item in sequence.semantic_items() {
        match item {
            PythonSequenceItem::Value(value) => {
                if let Some(scalar) = value.known_scalar()
                    && let Some(name) = scalar.string_value()
                {
                    if let Ok(name) = crate::python::PythonModuleName::parse(name) {
                        known.push(WithOrigin::new(
                            name,
                            scalar.first_origin(),
                            scalar.additional_origins(),
                        ));
                    } else {
                        malformed = true;
                        issues.push(value_issue(SettingIssueKind::InvalidModuleName, value));
                    }
                } else if value.unknown_value().is_some() {
                    issues.push(unknown_value_issue(value));
                } else {
                    malformed = true;
                    issues.push(value_issue(SettingIssueKind::InvalidShape, value));
                }
            }
            PythonSequenceItem::UnknownElement(unknown) => {
                issues.push(issue(SettingIssueKind::UnknownElement, unknown.origins()));
            }
            PythonSequenceItem::UnknownUnpack(unknown) => {
                issues.push(issue(SettingIssueKind::UnknownUnpack, unknown.origins()));
            }
        }
    }
    (known, issues, malformed)
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
    overflow_origins: Option<Vec<Origin>>,
}

impl<T> CappedExpansion<T> {
    fn one(value: T) -> Self {
        Self {
            exact: vec![value],
            overflow_origins: None,
        }
    }
}

fn path_list_capped(
    db: &dyn ProjectDb,
    value: &PythonValue,
) -> CappedExpansion<PathListProjection> {
    let Some(sequence) = collection_sequence(value) else {
        let mut projection = PathListProjection::empty();
        if value.unknown_value().is_some() {
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
    let mut overflow_origins = None;
    for alternative in sequence.alternatives() {
        match alternative {
            PythonSequenceAlternativeRef::Exact { items, constraints } => {
                if projections.len() == MAX_EXACT_SETTING_ALTERNATIVES {
                    overflow_origins = overflow_origins.or_else(|| {
                        list_item_origin(items)
                            .or_else(|| value.origins().next())
                            .map(|origin| vec![origin])
                    });
                    break;
                }
                let expansion = path_list_alternative(
                    db,
                    items,
                    MAX_EXACT_SETTING_ALTERNATIVES - projections.len(),
                );
                projections.extend(expansion.exact.into_iter().map(|mut projection| {
                    projection.constraints = projection.constraints.intersection(constraints);
                    projection
                }));
                if overflow_origins.is_none() {
                    overflow_origins = expansion.overflow_origins;
                }
            }
            PythonSequenceAlternativeRef::Remainder {
                origins,
                constraints,
            } => {
                if projections.len() == MAX_EXACT_SETTING_ALTERNATIVES {
                    if overflow_origins.is_none() {
                        overflow_origins = Some(origins.to_vec());
                    }
                } else {
                    let mut projection = PathListProjection::empty();
                    projection
                        .paths
                        .push_issue(alternative_limit_issue(origins.iter().copied()));
                    projection.constraints = projection.constraints.intersection(constraints);
                    projections.push(projection);
                }
            }
        }
    }
    CappedExpansion {
        exact: projections,
        overflow_origins,
    }
}

fn path_list_alternative(
    db: &dyn ProjectDb,
    items: &[PythonSequenceItem],
    limit: usize,
) -> CappedExpansion<PathListProjection> {
    let mut configurations = vec![PathListProjection::empty()];
    let mut overflow_origins = None;
    for item in items {
        match item {
            PythonSequenceItem::Value(value) => {
                let evaluated = evaluated_paths(db, value);
                if evaluated.is_empty() {
                    for projection in &mut configurations {
                        if value.unknown_value().is_some() {
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
                        let constraints = projection.constraints.intersection(&path.constraints);
                        if constraints.is_impossible() {
                            continue;
                        }
                        if next.len() == limit {
                            overflow_origins.get_or_insert_with(|| vec![path.path.origin()]);
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
            PythonSequenceItem::UnknownElement(unknown) => {
                for projection in &mut configurations {
                    projection
                        .paths
                        .push_issue(issue(SettingIssueKind::UnknownElement, unknown.origins()));
                }
            }
            PythonSequenceItem::UnknownUnpack(unknown) => {
                for projection in &mut configurations {
                    projection
                        .paths
                        .push_issue(issue(SettingIssueKind::UnknownUnpack, unknown.origins()));
                }
            }
        }
    }
    CappedExpansion {
        exact: configurations,
        overflow_origins,
    }
}

struct EvaluatedPathCandidate {
    path: WithOrigin<EvaluatedPath>,
    constraints: BranchConstraints,
}

fn evaluated_paths(db: &dyn ProjectDb, value: &PythonValue) -> Vec<EvaluatedPathCandidate> {
    if let Some(path) = value.path_value() {
        return value
            .origins_with_constraints()
            .map(|(origin, constraints)| EvaluatedPathCandidate {
                path: WithOrigin::new(EvaluatedPath::Resolved(path.clone()), origin, []),
                constraints: constraints.clone(),
            })
            .collect();
    }
    let Some(scalar) = value.known_scalar() else {
        return Vec::new();
    };
    let Some(path) = scalar.string_value() else {
        return Vec::new();
    };
    let path = Utf8Path::new(path);
    scalar
        .origins_with_constraints()
        .filter_map(|(origin, constraints)| {
            let resolved = if path.is_absolute() {
                EvaluatedPath::Resolved(path.to_path_buf())
            } else {
                EvaluatedPath::Resolved(origin.file.path(db).parent()?.join(path))
            };
            Some(EvaluatedPathCandidate {
                path: WithOrigin::new(resolved, origin, []),
                constraints: constraints.clone(),
            })
        })
        .collect()
}

fn dict_field<'a>(
    mapping: PythonMapping<'a>,
    wanted: &str,
) -> (Option<&'a PythonValue>, Vec<SettingIssue>) {
    let lookup = mapping.lookup_string_key(wanted);
    let issues = lookup
        .overrides()
        .iter()
        .map(|override_| match override_ {
            MappingOverride::UnknownUnpack(unknown) => {
                issue(SettingIssueKind::UnknownUnpack, unknown.origins())
            }
            MappingOverride::UnknownKey(key) => unknown_value_issue(key),
        })
        .collect();
    (lookup.value(), issues)
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
                PythonMutationOperation::Append
                    | PythonMutationOperation::Extend
                    | PythonMutationOperation::Insert
                    | PythonMutationOperation::Remove
            ) && matches!(mutation.path.as_slice(), [PythonMutationPathSegment::Index(_), PythonMutationPathSegment::Key(key)] if key == "DIRS")
        }
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
                && value.contains_origin(mutation.origin)
        })
        .map(|mutation| issue(SettingIssueKind::UnsupportedMutation, Some(mutation.origin)))
        .collect()
}

/// The list-or-tuple sequence a collection-shaped setting accepts. Strings are
/// honest Python sequences, but a bare string is not a valid collection
/// setting, so [`PythonSequence::String`] is explicitly rejected here at the
/// settings boundary rather than hidden by the value model.
fn collection_sequence(value: &PythonValue) -> Option<PythonMaterializedSequence<'_>> {
    let sequence = value.sequence()?;
    match sequence {
        PythonSequence::List(_) | PythonSequence::Tuple(_) => sequence.materialized(),
        PythonSequence::String(_) => None,
    }
}

fn list_item_origin(items: &[PythonSequenceItem]) -> Option<Origin> {
    items.iter().find_map(python_list_item_origin)
}

fn python_list_item_origin(item: &PythonSequenceItem) -> Option<Origin> {
    match item {
        PythonSequenceItem::Value(value) => value.origins().next(),
        PythonSequenceItem::UnknownElement(unknown)
        | PythonSequenceItem::UnknownUnpack(unknown) => unknown.origins().next(),
    }
}

fn alternative_limit_issue(origins: impl IntoIterator<Item = Origin>) -> SettingIssue {
    issue(SettingIssueKind::DynamicExpression, origins)
}

fn unknown_issue(unknown: &PythonUnknown) -> SettingIssue {
    issue(unknown_issue_kind(unknown), unknown.origins())
}

fn unknown_value_issue(value: &PythonValue) -> SettingIssue {
    let unknown = value
        .unknown_value()
        .expect("unknown value issue requires an unknown value");
    unknown_issue(unknown)
}

fn unknown_issue_kind(unknown: &PythonUnknown) -> SettingIssueKind {
    match unknown.cause {
        PythonUnknownCause::Unreadable(_) => SettingIssueKind::Unreadable,
        PythonUnknownCause::SyntaxErrors(_) => SettingIssueKind::SyntaxError,
        PythonUnknownCause::UnsupportedMutation => SettingIssueKind::UnsupportedMutation,
        PythonUnknownCause::UnsupportedExpression
        | PythonUnknownCause::InvalidImport(_)
        | PythonUnknownCause::ImportNotFound(_)
        | PythonUnknownCause::MissingImportMember { .. }
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
fn issue(kind: SettingIssueKind, origins: impl IntoIterator<Item = Origin>) -> SettingIssue {
    SettingIssue {
        kind,
        origins: origins.into_iter().collect(),
    }
}
