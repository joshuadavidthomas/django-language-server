use camino::Utf8Path;
use djls_source::File;
use djls_source::Origin;

use crate::db::Db as ProjectDb;
use crate::python::PythonModuleName;
use crate::python::evaluation::BranchConstraints;
use crate::python::evaluation::MappingEntryEvidence;
use crate::python::evaluation::MappingOverride;
use crate::python::evaluation::PythonBindingState;
use crate::python::evaluation::PythonBoundValue;
use crate::python::evaluation::PythonMapping;
use crate::python::evaluation::PythonMaterializedSequence;
use crate::python::evaluation::PythonModuleFacts;
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
use crate::settings::types::InstalledAppEvidence;
use crate::settings::types::InstalledApps;
use crate::settings::types::InstalledAppsAlternatives;
use crate::settings::types::MAX_EXACT_SETTING_ALTERNATIVES;
use crate::settings::types::MergeDynamicEvidence;
use crate::settings::types::MergeEvidence;
use crate::settings::types::PartialInstalledApps;
use crate::settings::types::PartialTemplateBackend;
use crate::settings::types::PartialTemplateBackends;
use crate::settings::types::PartialTemplateDirectories;
use crate::settings::types::SettingAlternatives;
use crate::settings::types::SettingCase;
use crate::settings::types::SettingFieldEvidence;
use crate::settings::types::SettingIssue;
use crate::settings::types::SettingIssueKind;
use crate::settings::types::TemplateBackend;
use crate::settings::types::TemplateBackendEvidence;
use crate::settings::types::TemplateBackends;
use crate::settings::types::TemplateContextProcessorPath;
use crate::settings::types::TemplateDirectoryPath;
use crate::settings::types::TemplateSettingAlternatives;
use crate::settings::types::WithOrigins;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SupportedSetting {
    InstalledApps,
    Templates,
}

impl SupportedSetting {
    const fn name(self) -> &'static str {
        match self {
            Self::InstalledApps => "INSTALLED_APPS",
            Self::Templates => "TEMPLATES",
        }
    }
}

struct ConstrainedNamespaceIssue {
    issue: SettingIssue,
    constraints: BranchConstraints,
}

pub(crate) fn settings_from_values(
    db: &dyn ProjectDb,
    file: File,
    facts: &PythonModuleFacts,
) -> DjangoSettings {
    let namespace_dynamic = facts.namespace_remainder.as_ref().map(|remainder| {
        remainder
            .as_slice()
            .iter()
            .map(|cause| ConstrainedNamespaceIssue {
                issue: issue(SettingIssueKind::DynamicNamespace, cause.unknown.origins()),
                constraints: cause.constraints.clone(),
            })
            .collect::<Vec<_>>()
    });
    let syntax_issues = |setting: SupportedSetting| {
        facts
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
        installed_apps: installed_apps(facts, namespace_dynamic.as_deref()),
        templates: templates(db, facts, namespace_dynamic.as_deref()),
    };

    let issues = syntax_issues(SupportedSetting::InstalledApps);
    if !issues.is_empty() {
        settings
            .installed_apps
            .add(SettingCase::Dynamic(PartialInstalledApps {
                evidence: issues
                    .into_iter()
                    .map(InstalledAppEvidence::Issue)
                    .collect(),
            }));
    }
    let issues = syntax_issues(SupportedSetting::Templates);
    if !issues.is_empty() {
        settings
            .templates
            .add(SettingCase::Dynamic(PartialTemplateBackends {
                evidence: issues
                    .into_iter()
                    .map(TemplateBackendEvidence::Issue)
                    .collect(),
            }));
    }
    settings
}

fn binding_cases<T, P>(
    facts: &PythonModuleFacts,
    setting: SupportedSetting,
    namespace_dynamic: Option<&[ConstrainedNamespaceIssue]>,
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
        let mut add_case = |case, constraints: BranchConstraints, origin| {
            for (existing, existing_constraints) in &mut cases {
                if existing.merge_evidence(&case) {
                    existing_constraints.merge_evidence(&constraints);
                    return;
                }
            }
            if cases.len() < MAX_EXACT_SETTING_ALTERNATIVES {
                cases.push((case, constraints));
            } else {
                overflow_origin = overflow_origin.or(origin);
            }
        };
        let mut unbound_constraints = Vec::new();
        if let Some(binding) = facts.bindings.get(setting.name()) {
            for (alternative, constraints) in binding.alternatives_with_constraints() {
                match alternative {
                    PythonBindingState::Bound(value) => {
                        let origin = value
                            .representative_binding_origin()
                            .or_else(|| value.value.origins().next());
                        for (case, constraints) in bound(value, constraints) {
                            add_case(case, constraints, origin);
                        }
                    }
                    PythonBindingState::Unbound => {
                        let constraints = constraints.clone();
                        unbound_constraints.push(constraints.clone());
                        add_case(SettingCase::Unset, constraints, None);
                    }
                }
            }
        } else {
            let constraints = BranchConstraints::unconstrained();
            unbound_constraints.push(constraints.clone());
            add_case(SettingCase::Unset, constraints, None);
        }
        if let Some(evidence) = namespace_dynamic {
            for unbound in &unbound_constraints {
                let mut groups: Vec<(BranchConstraints, Vec<SettingIssue>)> = Vec::new();
                for cause in evidence {
                    let constraints = unbound.intersection(&cause.constraints);
                    if constraints.is_impossible() {
                        continue;
                    }
                    if let Some((_, issues)) = groups
                        .iter_mut()
                        .find(|(existing, _)| *existing == constraints)
                    {
                        issues.push(cause.issue.clone());
                    } else {
                        groups.push((constraints, vec![cause.issue.clone()]));
                    }
                }
                for (constraints, issues) in groups {
                    let origin = issues
                        .iter()
                        .find_map(|issue| issue.origins.first())
                        .copied();
                    add_case(SettingCase::Dynamic(dynamic(issues)), constraints, origin);
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
    SettingAlternatives::from_constrained(cases)
}

fn constrained_cases<T, P>(
    cases: Vec<SettingCase<T, P>>,
    constraints: &BranchConstraints,
) -> Vec<(SettingCase<T, P>, BranchConstraints)> {
    cases
        .into_iter()
        .map(|case| (case, constraints.clone()))
        .collect()
}

fn installed_apps(
    facts: &PythonModuleFacts,
    namespace_dynamic: Option<&[ConstrainedNamespaceIssue]>,
) -> InstalledAppsAlternatives {
    binding_cases(
        facts,
        SupportedSetting::InstalledApps,
        namespace_dynamic,
        |bound, constraints| {
            let mutation_issues =
                unsupported_mutation_issues(facts, SupportedSetting::InstalledApps, &bound.value);
            let Some(sequence) = collection_sequence(&bound.value) else {
                let case = if bound.value.unknown_value().is_some() {
                    SettingCase::Dynamic(PartialInstalledApps {
                        evidence: vec![InstalledAppEvidence::Issue(unknown_value_issue(
                            &bound.value,
                        ))],
                    })
                } else {
                    SettingCase::Malformed(PartialInstalledApps {
                        evidence: vec![InstalledAppEvidence::Issue(value_issue(
                            SettingIssueKind::InvalidShape,
                            &bound.value,
                        ))],
                    })
                };
                return constrained_cases(vec![case], constraints);
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
                        let mut evidence = vec![InstalledAppEvidence::Issue(
                            alternative_limit_issue(origins.iter().copied()),
                        )];
                        evidence.extend(
                            mutation_issues
                                .iter()
                                .cloned()
                                .map(InstalledAppEvidence::Issue),
                        );
                        (
                            SettingCase::Dynamic(PartialInstalledApps { evidence }),
                            constraints.intersection(remainder_constraints),
                        )
                    }
                })
                .collect()
        },
        |issues| PartialInstalledApps {
            evidence: issues
                .into_iter()
                .map(InstalledAppEvidence::Issue)
                .collect(),
        },
    )
}

fn installed_apps_list_case(
    items: &[PythonSequenceItem],
    mutation_issues: &[SettingIssue],
) -> SettingCase<InstalledApps, PartialInstalledApps> {
    let (mut evidence, malformed) = string_list_items(items);
    evidence.extend(
        mutation_issues
            .iter()
            .cloned()
            .map(InstalledAppEvidence::Issue),
    );
    if malformed {
        SettingCase::Malformed(PartialInstalledApps { evidence })
    } else if evidence
        .iter()
        .all(|evidence| matches!(evidence, InstalledAppEvidence::Known(_)))
    {
        SettingCase::Known(InstalledApps {
            apps: evidence
                .into_iter()
                .filter_map(|evidence| match evidence {
                    InstalledAppEvidence::Known(app) => Some(app),
                    InstalledAppEvidence::Issue(_) => None,
                })
                .collect(),
        })
    } else {
        SettingCase::Dynamic(PartialInstalledApps { evidence })
    }
}

fn string_list_items(items: &[PythonSequenceItem]) -> (Vec<InstalledAppEvidence>, bool) {
    let mut evidence = Vec::new();
    let mut malformed = false;
    for item in items {
        let item = match item {
            PythonSequenceItem::Value(value) => {
                if let Some(scalar) = value.known_scalar()
                    && let Some(text) = scalar.string_value()
                {
                    InstalledAppEvidence::Known(WithOrigins::new(
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
    (evidence, malformed)
}

fn templates(
    db: &dyn ProjectDb,
    facts: &PythonModuleFacts,
    namespace_dynamic: Option<&[ConstrainedNamespaceIssue]>,
) -> TemplateSettingAlternatives {
    binding_cases(
        facts,
        SupportedSetting::Templates,
        namespace_dynamic,
        |bound, constraints| {
            let mutation_issues =
                unsupported_mutation_issues(facts, SupportedSetting::Templates, &bound.value);
            if let Some(sequence) = collection_sequence(&bound.value) {
                template_list(db, sequence, &mutation_issues, constraints)
            } else if bound.value.unknown_value().is_some() {
                constrained_cases(
                    vec![SettingCase::Dynamic(PartialTemplateBackends {
                        evidence: vec![TemplateBackendEvidence::Issue(unknown_value_issue(
                            &bound.value,
                        ))],
                    })],
                    constraints,
                )
            } else {
                constrained_cases(
                    vec![SettingCase::Malformed(PartialTemplateBackends {
                        evidence: vec![TemplateBackendEvidence::Issue(value_issue(
                            SettingIssueKind::InvalidShape,
                            &bound.value,
                        ))],
                    })],
                    constraints,
                )
            }
        },
        |issues| PartialTemplateBackends {
            evidence: issues
                .into_iter()
                .map(TemplateBackendEvidence::Issue)
                .collect(),
        },
    )
}

fn template_list(
    db: &dyn ProjectDb,
    sequence: PythonMaterializedSequence<'_>,
    issues: &[SettingIssue],
    outer_constraints: &BranchConstraints,
) -> Vec<(
    SettingCase<TemplateBackends, PartialTemplateBackends>,
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
                cases.extend(expansion.exact.into_iter().filter_map(|case| {
                    let constraints = outer_constraints
                        .intersection(constraints)
                        .intersection(&case.constraints);
                    (!constraints.is_impossible()).then_some((case.case, constraints))
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
                        .map(TemplateBackendEvidence::Issue)
                        .collect::<Vec<_>>();
                    evidence.push(TemplateBackendEvidence::Issue(alternative_limit_issue(
                        origins.iter().copied(),
                    )));
                    let case =
                        template_settings_case(evidence, false, BranchConstraints::unconstrained());
                    let constraints = outer_constraints
                        .intersection(constraints)
                        .intersection(&case.constraints);
                    if !constraints.is_impossible() {
                        cases.push((case.case, constraints));
                    }
                }
            }
        }
    }
    if let Some(origins) = overflow_origins {
        cases.push((
            SettingCase::Dynamic(PartialTemplateBackends {
                evidence: vec![TemplateBackendEvidence::Issue(alternative_limit_issue(
                    origins,
                ))],
            }),
            BranchConstraints::unconstrained(),
        ));
    }
    cases
}

struct ConstrainedTemplateCase {
    case: SettingCase<TemplateBackends, PartialTemplateBackends>,
    constraints: BranchConstraints,
}

fn template_list_alternative(
    db: &dyn ProjectDb,
    items: &[PythonSequenceItem],
    issues: &[SettingIssue],
    limit: usize,
) -> CappedExpansion<ConstrainedTemplateCase> {
    let initial_evidence = issues
        .iter()
        .cloned()
        .map(TemplateBackendEvidence::Issue)
        .collect::<Vec<_>>();
    let mut cases = vec![(initial_evidence, false, BranchConstraints::unconstrained())];
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
                            let constraints = backend.constraints.clone();
                            (
                                TemplateBackendEvidence::Backend(Box::new(backend)),
                                malformed,
                                constraints,
                            )
                        })
                        .collect()
                } else if value.unknown_value().is_some() {
                    vec![(
                        TemplateBackendEvidence::Issue(unknown_value_issue(value)),
                        false,
                        BranchConstraints::unconstrained(),
                    )]
                } else {
                    vec![(
                        TemplateBackendEvidence::Issue(value_issue(
                            SettingIssueKind::InvalidShape,
                            value,
                        )),
                        true,
                        BranchConstraints::unconstrained(),
                    )]
                }
            }
            PythonSequenceItem::UnknownElement(unknown) => vec![(
                TemplateBackendEvidence::Issue(issue(
                    SettingIssueKind::UnknownElement,
                    unknown.origins(),
                )),
                false,
                BranchConstraints::unconstrained(),
            )],
            PythonSequenceItem::UnknownUnpack(unknown) => vec![(
                TemplateBackendEvidence::Issue(issue(
                    SettingIssueKind::UnknownUnpack,
                    unknown.origins(),
                )),
                false,
                BranchConstraints::unconstrained(),
            )],
        };
        let item_origin = python_list_item_origin(item);
        let mut next =
            Vec::with_capacity(limit.min(cases.len().saturating_mul(alternatives.len())));
        for (evidence, malformed, constraints) in &cases {
            for (item, item_malformed, item_constraints) in &alternatives {
                let constraints = constraints.intersection(item_constraints);
                if constraints.is_impossible() {
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
                next.push((evidence, *malformed || *item_malformed, constraints));
            }
        }
        cases = next;
    }

    CappedExpansion {
        exact: cases
            .into_iter()
            .map(|(evidence, malformed, constraints)| {
                template_settings_case(evidence, malformed, constraints)
            })
            .collect(),
        overflow_origins,
    }
}

fn template_settings_case(
    evidence: Vec<TemplateBackendEvidence>,
    malformed: bool,
    constraints: BranchConstraints,
) -> ConstrainedTemplateCase {
    let case = if malformed {
        SettingCase::Malformed(PartialTemplateBackends { evidence })
    } else if evidence.iter().any(|evidence| match evidence {
        TemplateBackendEvidence::Backend(backend) => backend.has_issues(),
        TemplateBackendEvidence::Issue(_) => true,
    }) {
        SettingCase::Dynamic(PartialTemplateBackends { evidence })
    } else {
        SettingCase::Known(TemplateBackends {
            backends: evidence
                .into_iter()
                .filter_map(|evidence| match evidence {
                    TemplateBackendEvidence::Backend(backend) => {
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
                    TemplateBackendEvidence::Issue(_) => None,
                })
                .collect(),
        })
    };
    ConstrainedTemplateCase { case, constraints }
}

fn partial_backend(
    db: &dyn ProjectDb,
    mapping: PythonMapping<'_>,
) -> CappedExpansion<PartialTemplateBackend> {
    let mut backend = PartialTemplateBackend {
        constraints: BranchConstraints::unconstrained(),
        backend: SettingFieldEvidence::new(None),
        dirs: PartialTemplateDirectories::new(),
        app_dirs: SettingFieldEvidence::new(None),
        options: SettingFieldEvidence::new(()),
        libraries: SettingFieldEvidence::new(Vec::new()),
        builtins: SettingFieldEvidence::new(Vec::new()),
        context_processors: SettingFieldEvidence::new(Vec::new()),
    };

    let (backend_value, mut issues) = dict_field(mapping, "BACKEND");
    backend.backend.issues.append(&mut issues);
    match backend_value {
        Some(value) => {
            if let Some(scalar) = value.known_scalar()
                && let Some(name) = scalar.string_value()
            {
                backend.backend.known = Some(WithOrigins::new(
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
            backend.app_dirs.known = Some(WithOrigins::new(
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
        || CappedExpansion::one(TemplateDirectoryCase::empty()),
        |value| path_list_capped(db, value),
    );
    CappedExpansion {
        exact: dirs
            .exact
            .into_iter()
            .map(|projection| {
                let mut projected = backend.clone();
                projected.constraints = projection.constraints;
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
                                Ok(path) => {
                                    backend.context_processors.known.push(WithOrigins::new(
                                        path,
                                        scalar.first_origin(),
                                        scalar.additional_origins(),
                                    ));
                                }
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
    libraries: &mut SettingFieldEvidence<Vec<(String, WithOrigins<PythonModuleName>)>>,
) {
    for entry in mapping.effective_string_entries() {
        match entry {
            MappingEntryEvidence::Value { key: alias, value } => {
                if let Some(scalar) = value.known_scalar()
                    && let Some(module) = scalar.string_value()
                {
                    match PythonModuleName::parse(module) {
                        Ok(module) => libraries.known.push((
                            alias.to_string(),
                            WithOrigins::new(
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
            MappingEntryEvidence::UnknownKey(key) => {
                libraries.issues.push(unknown_value_issue(key));
            }
            MappingEntryEvidence::InvalidKey(key) => {
                libraries
                    .issues
                    .push(value_issue(SettingIssueKind::InvalidShape, key));
            }
            MappingEntryEvidence::UnknownUnpack(unknown) => {
                libraries
                    .issues
                    .push(issue(SettingIssueKind::UnknownUnpack, unknown.origins()));
            }
        }
    }
}

fn module_name_list(
    value: &PythonValue,
) -> (Vec<WithOrigins<PythonModuleName>>, Vec<SettingIssue>, bool) {
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
                    if let Ok(name) = PythonModuleName::parse(name) {
                        known.push(WithOrigins::new(
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
struct TemplateDirectoryCase {
    paths: PartialTemplateDirectories,
    malformed: bool,
    constraints: BranchConstraints,
}

impl TemplateDirectoryCase {
    fn empty() -> Self {
        Self {
            paths: PartialTemplateDirectories::new(),
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
) -> CappedExpansion<TemplateDirectoryCase> {
    let Some(sequence) = collection_sequence(value) else {
        let mut projection = TemplateDirectoryCase::empty();
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
                    let mut projection = TemplateDirectoryCase::empty();
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
) -> CappedExpansion<TemplateDirectoryCase> {
    let mut cases = vec![TemplateDirectoryCase::empty()];
    let mut overflow_origins = None;
    for item in items {
        match item {
            PythonSequenceItem::Value(value) => {
                let evaluated = constrained_template_directories(db, value);
                if evaluated.is_empty() {
                    for projection in &mut cases {
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
                    for projection in &mut cases {
                        projection.paths.push_known(path.path.clone());
                        projection.constraints =
                            projection.constraints.intersection(&path.constraints);
                    }
                    continue;
                }

                let mut next =
                    Vec::with_capacity(limit.min(cases.len().saturating_mul(evaluated.len())));
                for projection in &cases {
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
                cases = next;
            }
            PythonSequenceItem::UnknownElement(unknown) => {
                for projection in &mut cases {
                    projection
                        .paths
                        .push_issue(issue(SettingIssueKind::UnknownElement, unknown.origins()));
                }
            }
            PythonSequenceItem::UnknownUnpack(unknown) => {
                for projection in &mut cases {
                    projection
                        .paths
                        .push_issue(issue(SettingIssueKind::UnknownUnpack, unknown.origins()));
                }
            }
        }
    }
    CappedExpansion {
        exact: cases,
        overflow_origins,
    }
}

struct ConstrainedTemplateDirectory {
    path: WithOrigins<TemplateDirectoryPath>,
    constraints: BranchConstraints,
}

fn constrained_template_directories(
    db: &dyn ProjectDb,
    value: &PythonValue,
) -> Vec<ConstrainedTemplateDirectory> {
    if let Some(path) = value.path_value() {
        return value
            .origins_with_constraints()
            .map(|(origin, constraints)| ConstrainedTemplateDirectory {
                path: WithOrigins::new(TemplateDirectoryPath::new(path.to_path_buf()), origin, []),
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
                TemplateDirectoryPath::new(path.to_path_buf())
            } else {
                TemplateDirectoryPath::new(origin.file.path(db).parent()?.join(path))
            };
            Some(ConstrainedTemplateDirectory {
                path: WithOrigins::new(resolved, origin, []),
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

fn setting_accepts_mutation(setting: SupportedSetting, mutation: &PythonMutation) -> bool {
    match setting {
        SupportedSetting::InstalledApps => {
            mutation.path.is_empty()
                && matches!(
                    mutation.operation,
                    PythonMutationOperation::Append
                        | PythonMutationOperation::Extend
                        | PythonMutationOperation::Insert
                        | PythonMutationOperation::Remove
                )
        }
        SupportedSetting::Templates => {
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
    facts: &PythonModuleFacts,
    setting: SupportedSetting,
    value: &PythonValue,
) -> Vec<SettingIssue> {
    facts
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
        | PythonUnknownCause::ModuleAttribute { .. }
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
