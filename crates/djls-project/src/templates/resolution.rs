use std::borrow::Cow;
use std::fmt;
use std::io;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_source::File;
use djls_source::FileError;
use djls_source::RootWalk;
use djls_source::Utf8PathClean;
use djls_source::WalkEntryKind;
use djls_source::WalkOptions;
use djls_source::path_to_file;
use djls_source::safe_join;
use rustc_hash::FxHashMap;
use rustc_hash::FxHashSet;

use crate::db::Db as ProjectDb;
use crate::project::Project;
use crate::python::resolve_package_dirs;
use crate::settings::EvaluatedPath;
use crate::settings::django_settings;
use crate::settings::settings_module_file;
use crate::settings::types::InstalledAppEvidence;
use crate::settings::types::PathListEvidence;
use crate::settings::types::SettingCase;
use crate::settings::types::TemplateListEvidence;
use crate::settings::types::template_backend_evidence_slots;
use crate::templates::installed_app_package_module;

/// The feasible ordered template-root search sequences extracted from settings.
///
/// Settings alternatives remain separate so resolution can compare the winner of each complete
/// configuration rather than treating roots from mutually exclusive branches as one loader list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateDirectories(Vec<TemplateDirectoryAlternative>);

#[derive(Clone, Debug, PartialEq, Eq)]
struct TemplateDirectoryAlternative(Vec<RootEntry>);

#[derive(Clone, Debug, PartialEq, Eq)]
enum RootEntry {
    Known {
        root: Utf8PathBuf,
        backend: usize,
    },
    /// One unenumerable element at this exact position; roots before and after it keep
    /// their ordering guarantees. A missing backend means the backend itself is unknown.
    Unknown {
        backend: Option<usize>,
    },
}

impl TemplateDirectoryAlternative {
    fn push_root(&mut self, root: Utf8PathBuf, backend: usize) {
        self.0.push(RootEntry::Known { root, backend });
    }

    fn mark_unknown_roots(&mut self, backend: Option<usize>) {
        if !matches!(self.0.last(), Some(RootEntry::Unknown { backend: existing }) if *existing == backend)
        {
            self.0.push(RootEntry::Unknown { backend });
        }
    }
}

impl TemplateDirectories {
    pub fn known_roots(&self) -> impl Iterator<Item = &Utf8Path> {
        let mut seen = FxHashSet::default();
        self.0
            .iter()
            .flat_map(|alternative| &alternative.0)
            .filter_map(move |entry| match entry {
                RootEntry::Known { root, .. } if seen.insert(root.as_path()) => {
                    Some(root.as_path())
                }
                RootEntry::Known { .. } | RootEntry::Unknown { .. } => None,
            })
    }

    #[must_use]
    pub fn configuration_may_omit_roots(&self) -> bool {
        self.0.len() > 1
            || self.0.iter().any(|alternative| {
                alternative
                    .0
                    .iter()
                    .any(|entry| matches!(entry, RootEntry::Unknown { .. }))
            })
    }

    fn alternatives(&self) -> &[TemplateDirectoryAlternative] {
        &self.0
    }
}

enum InstalledAppEntry {
    Known(String),
    Unknown,
}

struct InstalledAppsProjection(Vec<InstalledAppEntry>);

fn project_installed_apps(
    case: &SettingCase<
        crate::settings::types::InstalledAppsValue,
        crate::settings::types::DynamicInstalledApps,
        crate::settings::types::MalformedInstalledApps,
    >,
) -> InstalledAppsProjection {
    match case {
        SettingCase::Known(value) => InstalledAppsProjection(
            value
                .apps
                .iter()
                .map(|app| InstalledAppEntry::Known(app.value.clone()))
                .collect(),
        ),
        SettingCase::Dynamic(value) => InstalledAppsProjection(
            value
                .apps
                .evidence
                .iter()
                .map(|evidence| match evidence {
                    InstalledAppEvidence::Known(app) => InstalledAppEntry::Known(app.value.clone()),
                    InstalledAppEvidence::Issue(_) => InstalledAppEntry::Unknown,
                })
                .collect(),
        ),
        SettingCase::Malformed(value) => InstalledAppsProjection(
            value
                .apps
                .evidence
                .iter()
                .map(|evidence| match evidence {
                    InstalledAppEvidence::Known(app) => InstalledAppEntry::Known(app.value.clone()),
                    InstalledAppEvidence::Issue(_) => InstalledAppEntry::Unknown,
                })
                .collect(),
        ),
        SettingCase::Unset => InstalledAppsProjection(Vec::new()),
    }
}

fn add_backend_roots(
    db: &dyn ProjectDb,
    project: Project,
    backend: &crate::settings::types::PartialTemplateBackend,
    apps: &InstalledAppsProjection,
    backend_index: usize,
    alternative: &mut TemplateDirectoryAlternative,
) {
    if backend
        .backend
        .known
        .as_ref()
        .is_none_or(|name| name.value != "django.template.backends.django.DjangoTemplates")
    {
        return;
    }
    for evidence in &backend.dirs.evidence {
        match evidence {
            PathListEvidence::Known(dir) => {
                let EvaluatedPath::Resolved(path) = &dir.value;
                alternative.push_root(path.clone(), backend_index);
            }
            PathListEvidence::Issue(_) => alternative.mark_unknown_roots(Some(backend_index)),
        }
    }
    if !backend.app_dirs.issues.is_empty() {
        alternative.mark_unknown_roots(Some(backend_index));
    }
    if backend
        .app_dirs
        .known
        .as_ref()
        .is_some_and(|app_dirs| app_dirs.value)
    {
        for entry in &apps.0 {
            let InstalledAppEntry::Known(app) = entry else {
                alternative.mark_unknown_roots(Some(backend_index));
                continue;
            };
            let Some(package_module) = installed_app_package_module(db, project, app) else {
                alternative.mark_unknown_roots(Some(backend_index));
                continue;
            };
            let package_dirs = resolve_package_dirs(db, project, package_module);
            if package_dirs.dirs.is_empty() {
                alternative.mark_unknown_roots(Some(backend_index));
            }
            for package_dir in package_dirs.dirs {
                alternative.push_root(package_dir.join("templates"), backend_index);
            }
        }
    }
}

#[salsa::tracked(returns(ref))]
pub fn template_directories(db: &dyn ProjectDb, project: Project) -> TemplateDirectories {
    project.touch_search_path_roots(db);

    if settings_module_file(db, project).is_none() {
        return TemplateDirectories(vec![TemplateDirectoryAlternative(vec![
            RootEntry::Unknown { backend: None },
        ])]);
    }

    let settings = django_settings(db, project);
    let mut alternatives = Vec::new();

    for configuration in settings.feasible_configurations() {
        let apps = project_installed_apps(configuration.installed_apps);
        let mut alternative = TemplateDirectoryAlternative(Vec::new());

        match configuration.templates {
            SettingCase::Known(value) => {
                for (backend_index, backend) in value.backends.iter().enumerate() {
                    let partial = crate::settings::types::PartialTemplateBackend::from_complete(
                        backend.clone(),
                    );
                    add_backend_roots(
                        db,
                        project,
                        &partial,
                        &apps,
                        backend_index,
                        &mut alternative,
                    );
                }
            }
            SettingCase::Dynamic(value) => add_partial_backend_roots(
                db,
                project,
                &value.templates.evidence,
                &apps,
                &mut alternative,
            ),
            SettingCase::Malformed(value) => add_partial_backend_roots(
                db,
                project,
                &value.templates.evidence,
                &apps,
                &mut alternative,
            ),
            SettingCase::Unset => {}
        }
        // Keep one entry per feasible settings configuration. Environment correlation relies on
        // this index matching the Template Library configuration derived from the same branch.
        alternatives.push(alternative);
    }

    TemplateDirectories(alternatives)
}

fn add_partial_backend_roots(
    db: &dyn ProjectDb,
    project: Project,
    evidence: &[TemplateListEvidence],
    apps: &InstalledAppsProjection,
    alternative: &mut TemplateDirectoryAlternative,
) {
    for (backend_index, evidence) in template_backend_evidence_slots(evidence) {
        match evidence {
            TemplateListEvidence::Backend(backend) => {
                if !backend.backend.issues.is_empty() {
                    alternative.mark_unknown_roots(None);
                }
                add_backend_roots(db, project, backend, apps, backend_index, alternative);
            }
            TemplateListEvidence::Issue(_) => {
                alternative.mark_unknown_roots(None);
            }
        }
    }
}

#[salsa::interned]
#[derive(Debug)]
pub struct TemplateName {
    #[returns(ref)]
    pub name: String,
}

#[must_use]
pub fn resolve_relative_name<'a>(
    current_template_name: Option<&str>,
    target: &'a str,
    allow_self: bool,
) -> Option<Cow<'a, str>> {
    if !is_relative_template_name(target) {
        return Some(Cow::Borrowed(target));
    }

    let current_template_name = current_template_name?;
    let current_template_name = current_template_name.trim_start_matches('/');
    let current_dir = current_template_name
        .rsplit_once('/')
        .map_or("", |(dir, _name)| dir);
    let mut stack = Vec::new();

    for segment in current_dir.split('/').chain(target.split('/')) {
        match segment {
            "" | "." => {}
            ".." => {
                stack.pop()?;
            }
            segment => stack.push(segment),
        }
    }

    let normalized = if stack.is_empty() {
        ".".to_string()
    } else {
        stack.join("/")
    };

    if !allow_self && normalized == current_template_name {
        return None;
    }

    Some(Cow::Owned(normalized))
}

fn is_relative_template_name(target: &str) -> bool {
    target.starts_with("./") || target.starts_with("../")
}

#[salsa::tracked]
#[derive(Debug)]
pub struct TemplateOrigin<'db> {
    resolved_template_name: TemplateName<'db>,
    template_file: File,
}

impl<'db> TemplateOrigin<'db> {
    pub fn template_name(self, db: &'db dyn ProjectDb) -> TemplateName<'db> {
        self.resolved_template_name(db)
    }

    pub fn file(self, db: &'db dyn ProjectDb) -> File {
        self.template_file(db)
    }

    pub fn path_buf(self, db: &'db dyn ProjectDb) -> &'db Utf8PathBuf {
        self.file(db).path(db)
    }
}

#[salsa::tracked]
pub struct TemplateResolution<'db> {
    project: Project,
}

impl<'db> TemplateResolution<'db> {
    pub fn origins(
        self,
        db: &'db dyn ProjectDb,
    ) -> impl Iterator<Item = TemplateOrigin<'db>> + 'db {
        template_directory_index(db, self)
            .ordered(db)
            .iter()
            .copied()
    }

    pub fn template_names(
        self,
        db: &'db dyn ProjectDb,
    ) -> impl Iterator<Item = TemplateName<'db>> + 'db {
        template_directory_index(db, self)
            .by_template_name(db)
            .keys()
            .copied()
    }

    /// Returns template names with a concrete match in the backends that can render `file`.
    pub fn template_names_for_backend_scope(
        self,
        db: &'db dyn ProjectDb,
        file: File,
    ) -> Vec<TemplateName<'db>> {
        let scope = self.backend_scope_for_file(db, file);
        self.template_names(db)
            .filter(
                |name| match self.resolve_excluding_in_scope(db, *name, &[], &scope) {
                    FindTemplateResult::Found(_) => true,
                    FindTemplateResult::Inconclusive(search) => !search.possible_origins.is_empty(),
                    FindTemplateResult::DoesNotExist(_) => false,
                },
            )
            .collect()
    }

    pub fn origins_for_name(
        self,
        db: &'db dyn ProjectDb,
        template_name: TemplateName<'db>,
    ) -> &'db [TemplateOrigin<'db>] {
        template_directory_index(db, self)
            .by_template_name(db)
            .get(&template_name)
            .map_or(&[], Vec::as_slice)
    }

    pub fn template_names_for_file(
        self,
        db: &'db dyn ProjectDb,
        file: File,
    ) -> &'db [TemplateName<'db>] {
        template_directory_index(db, self)
            .names_by_file(db)
            .get(&file)
            .map_or(&[], Vec::as_slice)
    }

    #[must_use]
    pub fn resolve(
        self,
        db: &'db dyn ProjectDb,
        name: TemplateName<'db>,
    ) -> FindTemplateResult<'db> {
        self.resolve_excluding(db, name, &[])
    }

    pub(crate) fn backend_selections_for_file(
        self,
        db: &'db dyn ProjectDb,
        file: File,
    ) -> Vec<BackendSelection> {
        let index = template_directory_index(db, self);
        let mut selections = Vec::new();

        for name in self.template_names_for_file(db, file) {
            for (configuration, search) in index.searches(db).iter().enumerate() {
                collect_backend_selections(db, search, *name, file, configuration, &mut selections);
            }
        }

        selections
    }

    #[must_use]
    fn backend_scope_for_file(self, db: &'db dyn ProjectDb, file: File) -> TemplateBackendScope {
        TemplateBackendScope(self.backend_selections_for_file(db, file))
    }

    #[must_use]
    pub fn backend_scope_for_origin(
        self,
        db: &'db dyn ProjectDb,
        origin: TemplateOrigin<'db>,
    ) -> TemplateBackendScope {
        let index = template_directory_index(db, self);
        let mut selections = Vec::new();
        let name = origin.template_name(db);
        let file = origin.file(db);

        for (configuration, search) in index.searches(db).iter().enumerate() {
            collect_backend_selections(db, search, name, file, configuration, &mut selections);
        }
        TemplateBackendScope(selections)
    }

    #[must_use]
    pub fn resolve_for_file(
        self,
        db: &'db dyn ProjectDb,
        name: TemplateName<'db>,
        file: File,
    ) -> FindTemplateResult<'db> {
        self.resolve_excluding_in_scope(db, name, &[], &self.backend_scope_for_file(db, file))
    }

    /// Normalize and resolve a raw reference from one concrete source origin.
    ///
    /// Keeping the source origin in this interface preserves both the template name used as the
    /// relative-reference anchor and the settings/backend selections under which that origin is
    /// renderable. Callers handling a physical file with multiple names must invoke this for each
    /// origin and join the scoped outcomes rather than choosing one file-level name.
    #[must_use]
    pub fn resolve_reference_from_origin(
        self,
        db: &'db dyn ProjectDb,
        source: TemplateOrigin<'db>,
        raw_name: TemplateName<'db>,
        excluded: &[TemplateOrigin<'db>],
        allow_self: bool,
    ) -> Option<ScopedTemplateReferenceResolution<'db>> {
        self.resolve_reference_from_origin_in_scope(
            db,
            source,
            raw_name,
            excluded,
            allow_self,
            &self.backend_scope_for_origin(db, source),
        )
    }

    /// Resolve from an origin name while retaining a scope established by an earlier lookup.
    ///
    /// Inheritance uses this to keep the child's backend selections throughout the ancestor chain
    /// instead of widening to every backend that can independently render a shared ancestor file.
    #[must_use]
    pub fn resolve_reference_from_origin_in_scope(
        self,
        db: &'db dyn ProjectDb,
        source: TemplateOrigin<'db>,
        raw_name: TemplateName<'db>,
        excluded: &[TemplateOrigin<'db>],
        allow_self: bool,
        scope: &TemplateBackendScope,
    ) -> Option<ScopedTemplateReferenceResolution<'db>> {
        let resolved = resolve_relative_name(
            Some(source.template_name(db).name(db)),
            raw_name.name(db),
            allow_self,
        )?;
        let target_name = match resolved {
            Cow::Borrowed(_) => raw_name,
            Cow::Owned(name) => TemplateName::new(db, name),
        };
        let result = self.resolve_excluding_origins_in_scope(db, target_name, excluded, scope);
        Some(ScopedTemplateReferenceResolution {
            source,
            target_name,
            result,
        })
    }

    /// Finds the first non-excluded origin in Django loader order.
    ///
    /// Exclusions model Django's inheritance `skip` set: a shadowing template that has already
    /// participated in the inheritance chain is ignored and lookup continues at the next origin.
    #[must_use]
    pub fn resolve_excluding(
        self,
        db: &'db dyn ProjectDb,
        name: TemplateName<'db>,
        excluded: &[File],
    ) -> FindTemplateResult<'db> {
        self.resolve_excluding_in_scope(db, name, excluded, &TemplateBackendScope::default())
    }

    #[must_use]
    pub fn resolve_excluding_origins_in_scope(
        self,
        db: &'db dyn ProjectDb,
        name: TemplateName<'db>,
        excluded: &[TemplateOrigin<'db>],
        scope: &TemplateBackendScope,
    ) -> FindTemplateResult<'db> {
        let excluded_files = excluded
            .iter()
            .filter(|origin| origin.template_name(db) == name)
            .map(|origin| origin.file(db))
            .collect::<Vec<_>>();
        self.resolve_excluding_in_scope(db, name, &excluded_files, scope)
    }

    #[must_use]
    fn resolve_excluding_in_scope(
        self,
        db: &'db dyn ProjectDb,
        name: TemplateName<'db>,
        excluded: &[File],
        scope: &TemplateBackendScope,
    ) -> FindTemplateResult<'db> {
        let index = template_directory_index(db, self);
        let excluded: FxHashSet<_> = excluded.iter().copied().collect();
        let outcomes = if scope.0.is_empty() {
            index
                .searches(db)
                .iter()
                .map(|search| resolve_alternative(db, search, name, &excluded))
                .collect::<Vec<_>>()
        } else {
            scope
                .0
                .iter()
                .map(|selection| match *selection {
                    BackendSelection::Known {
                        configuration,
                        backend,
                    } => {
                        let search = &index.searches(db)[configuration];
                        let filtered = search
                            .iter()
                            .filter(|evidence| evidence.matches_backend(backend))
                            .cloned()
                            .collect::<Vec<_>>();
                        resolve_alternative(db, &filtered, name, &excluded)
                    }
                    BackendSelection::Unknown { .. } => AlternativeOutcome::Inconclusive {
                        origins: Vec::new(),
                    },
                })
                .collect::<Vec<_>>()
        };

        let unanimous_file = outcomes.first().and_then(|outcome| match outcome {
            AlternativeOutcome::Found { origin, .. } => Some(origin.file(db)),
            AlternativeOutcome::DoesNotExist | AlternativeOutcome::Inconclusive { .. } => None,
        });
        if let Some(file) = unanimous_file
            && outcomes.iter().all(
                |outcome| matches!(outcome, AlternativeOutcome::Found { origin, .. } if origin.file(db) == file),
            )
        {
            let AlternativeOutcome::Found { origin, .. } = outcomes[0] else {
                unreachable!("the unanimous outcome was checked as found")
            };
            return FindTemplateResult::Found(origin);
        }

        if outcomes
            .iter()
            .all(|outcome| matches!(outcome, AlternativeOutcome::DoesNotExist))
        {
            let directories = template_directories(db, self.project(db));
            let mut roots = Vec::new();
            if scope.0.is_empty() {
                roots.extend(directories.known_roots());
            } else {
                for selection in &scope.0 {
                    let BackendSelection::Known {
                        configuration,
                        backend,
                    } = *selection
                    else {
                        // An open backend has no concrete roots to report as tried.
                        continue;
                    };
                    let Some(alternative) = directories.alternatives().get(configuration) else {
                        continue;
                    };
                    for entry in &alternative.0 {
                        let RootEntry::Known {
                            root,
                            backend: candidate,
                        } = entry
                        else {
                            continue;
                        };
                        if *candidate == backend && !roots.contains(&root.as_path()) {
                            roots.push(root.as_path());
                        }
                    }
                }
            }
            let tried = roots
                .into_iter()
                .filter_map(|root| safe_join(root, name.name(db)).ok())
                .collect();
            return FindTemplateResult::DoesNotExist(TemplateDoesNotExist { name, tried });
        }

        FindTemplateResult::Inconclusive(InconclusiveTemplateSearch {
            name,
            possible_origins: possible_origins(db, outcomes),
        })
    }
}

fn collect_backend_selections<'db>(
    db: &'db dyn ProjectDb,
    search: &[TemplateSearchEvidence<'db>],
    name: TemplateName<'db>,
    file: File,
    configuration: usize,
    selections: &mut Vec<BackendSelection>,
) {
    let file_path = file.path(db);
    for evidence in search {
        let selection = match evidence {
            TemplateSearchEvidence::Origin { origin, backend }
                if origin.template_name(db) == name && origin.file(db) == file =>
            {
                BackendSelection::Known {
                    configuration,
                    backend: *backend,
                }
            }
            // Unknown roots can contain this file under the same template name. Retain that
            // feasible configuration even when another configuration supplied the concrete name.
            TemplateSearchEvidence::UnknownRoots {
                backend: Some(backend),
            } => BackendSelection::Known {
                configuration,
                backend: *backend,
            },
            TemplateSearchEvidence::UnknownRoots { backend: None } => {
                BackendSelection::Unknown { configuration }
            }
            // A failed walk can contribute only when its known root can physically contain this
            // file, and a failed file lookup only when it names this exact path and template name.
            TemplateSearchEvidence::Issue {
                issue: TemplateSearchIssue::Walk { root, .. },
                backend,
            } if file_path
                .strip_prefix(root)
                .is_ok_and(|relative| relative.clean().as_str() == name.name(db)) =>
            {
                BackendSelection::Known {
                    configuration,
                    backend: *backend,
                }
            }
            TemplateSearchEvidence::Issue {
                issue:
                    TemplateSearchIssue::File {
                        name: issue_name,
                        path,
                        ..
                    },
                backend,
            } if issue_name == name.name(db) && path == file_path => BackendSelection::Known {
                configuration,
                backend: *backend,
            },
            TemplateSearchEvidence::Origin { .. } | TemplateSearchEvidence::Issue { .. } => {
                continue;
            }
        };
        if !selections.contains(&selection) {
            selections.push(selection);
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScopedTemplateReferenceResolution<'db> {
    pub source: TemplateOrigin<'db>,
    pub target_name: TemplateName<'db>,
    pub result: FindTemplateResult<'db>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TemplateBackendScope(Vec<BackendSelection>);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BackendSelection {
    Known {
        configuration: usize,
        backend: usize,
    },
    /// A feasible settings configuration whose backend and roots cannot be enumerated.
    Unknown { configuration: usize },
}

#[salsa::tracked]
pub fn template_resolution(db: &dyn ProjectDb, project: Project) -> TemplateResolution<'_> {
    let _ = template_directories(db, project);
    TemplateResolution::new(db, project)
}

#[salsa::tracked]
struct TemplateDirectoryIndex<'db> {
    #[tracked]
    #[returns(ref)]
    ordered: Vec<TemplateOrigin<'db>>,
    #[tracked]
    #[returns(ref)]
    by_template_name: FxHashMap<TemplateName<'db>, Vec<TemplateOrigin<'db>>>,
    #[tracked]
    #[returns(ref)]
    names_by_file: FxHashMap<File, Vec<TemplateName<'db>>>,
    #[tracked]
    #[returns(ref)]
    searches: Vec<Vec<TemplateSearchEvidence<'db>>>,
}

#[derive(Clone, Debug, PartialEq, Eq, salsa::Update)]
enum TemplateSearchEvidence<'db> {
    Origin {
        origin: TemplateOrigin<'db>,
        backend: usize,
    },
    UnknownRoots {
        backend: Option<usize>,
    },
    Issue {
        issue: TemplateSearchIssue,
        backend: usize,
    },
}

impl TemplateSearchEvidence<'_> {
    fn matches_backend(&self, backend: usize) -> bool {
        match self {
            Self::Origin {
                backend: candidate, ..
            }
            | Self::Issue {
                backend: candidate, ..
            } => *candidate == backend,
            Self::UnknownRoots { backend: candidate } => *candidate == Some(backend),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, salsa::Update)]
enum TemplateSearchIssue {
    Walk {
        root: Utf8PathBuf,
        kind: io::ErrorKind,
    },
    File {
        name: String,
        path: Utf8PathBuf,
        error: FileError,
    },
}

#[salsa::tracked]
fn template_directory_index<'db>(
    db: &'db dyn ProjectDb,
    resolution: TemplateResolution<'db>,
) -> TemplateDirectoryIndex<'db> {
    let project = resolution.project(db);
    let files = project_template_files(db, project);
    let mut ordered = Vec::new();
    let mut by_template_name = FxHashMap::default();
    let mut names_by_file = FxHashMap::default();
    let mut searches = Vec::new();
    let mut origins = FxHashMap::default();

    for alternative in files.searches() {
        let mut search = Vec::new();
        for evidence in alternative {
            match evidence {
                ProjectTemplateSearchEvidence::File { template, backend } => {
                    let template_name = TemplateName::new(db, template.name().to_string());
                    let origin = *origins
                        .entry((template_name, template.file()))
                        .or_insert_with(|| {
                            let origin = TemplateOrigin::new(db, template_name, template.file());
                            let file_names = names_by_file
                                .entry(template.file())
                                .or_insert_with(Vec::new);
                            if !file_names.contains(&template_name) {
                                file_names.push(template_name);
                            }
                            by_template_name
                                .entry(template_name)
                                .or_insert_with(Vec::new)
                                .push(origin);
                            ordered.push(origin);
                            origin
                        });
                    search.push(TemplateSearchEvidence::Origin {
                        origin,
                        backend: *backend,
                    });
                }
                ProjectTemplateSearchEvidence::UnknownRoots { backend } => {
                    search.push(TemplateSearchEvidence::UnknownRoots { backend: *backend });
                }
                ProjectTemplateSearchEvidence::Issue { issue, backend } => {
                    search.push(TemplateSearchEvidence::Issue {
                        issue: issue.clone(),
                        backend: *backend,
                    });
                }
            }
        }
        searches.push(search);
    }

    tracing::debug!("Discovered {} total template origins", ordered.len());

    TemplateDirectoryIndex::new(db, ordered, by_template_name, names_by_file, searches)
}

/// Outcome of an ordered template-name search.
///
/// `DoesNotExist` is only produced by an exhaustive search of every known root. `Inconclusive`
/// retains whatever positive evidence the incomplete search established; how much weight to
/// give that evidence is deliberately a per-consumer policy decision, not one this crate makes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FindTemplateResult<'db> {
    Found(TemplateOrigin<'db>),
    DoesNotExist(TemplateDoesNotExist<'db>),
    Inconclusive(InconclusiveTemplateSearch<'db>),
}

enum AlternativeOutcome<'db> {
    Found { origin: TemplateOrigin<'db> },
    DoesNotExist,
    Inconclusive { origins: Vec<TemplateOrigin<'db>> },
}

fn possible_origins<'db>(
    db: &'db dyn ProjectDb,
    outcomes: Vec<AlternativeOutcome<'db>>,
) -> Vec<TemplateOrigin<'db>> {
    let mut possible = Vec::new();
    let mut seen = FxHashSet::default();
    for outcome in outcomes {
        let origins = match outcome {
            AlternativeOutcome::Found { origin } => vec![origin],
            AlternativeOutcome::Inconclusive { origins } => origins,
            AlternativeOutcome::DoesNotExist => Vec::new(),
        };
        for origin in origins {
            if seen.insert(origin.file(db)) {
                possible.push(origin);
            }
        }
    }
    possible
}

fn resolve_alternative<'db>(
    db: &'db dyn ProjectDb,
    search: &[TemplateSearchEvidence<'db>],
    name: TemplateName<'db>,
    excluded: &FxHashSet<File>,
) -> AlternativeOutcome<'db> {
    let mut open_selections = Vec::new();
    for evidence in search {
        match evidence {
            TemplateSearchEvidence::Origin { origin, .. }
                if origin.template_name(db) == name && !excluded.contains(&origin.file(db)) =>
            {
                return if open_selections.is_empty() {
                    AlternativeOutcome::Found { origin: *origin }
                } else {
                    AlternativeOutcome::Inconclusive {
                        origins: vec![*origin],
                    }
                };
            }
            TemplateSearchEvidence::UnknownRoots { backend } => {
                if !open_selections.contains(backend) {
                    open_selections.push(*backend);
                }
            }
            TemplateSearchEvidence::Issue {
                issue: TemplateSearchIssue::Walk { .. },
                backend,
            } => {
                let backend = Some(*backend);
                if !open_selections.contains(&backend) {
                    open_selections.push(backend);
                }
            }
            TemplateSearchEvidence::Issue {
                issue:
                    TemplateSearchIssue::File {
                        name: issue_name, ..
                    },
                backend,
            } if issue_name == name.name(db) => {
                let backend = Some(*backend);
                if !open_selections.contains(&backend) {
                    open_selections.push(backend);
                }
            }
            TemplateSearchEvidence::Origin { .. } | TemplateSearchEvidence::Issue { .. } => {}
        }
    }
    if open_selections.is_empty() {
        AlternativeOutcome::DoesNotExist
    } else {
        AlternativeOutcome::Inconclusive {
            origins: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateDoesNotExist<'db> {
    pub name: TemplateName<'db>,
    pub tried: Vec<Utf8PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InconclusiveTemplateSearch<'db> {
    pub name: TemplateName<'db>,
    pub possible_origins: Vec<TemplateOrigin<'db>>,
}

/// Positive template files and ordered evidence from searching configured roots.
#[derive(Clone, Default, PartialEq, Eq)]
struct ProjectTemplateFiles {
    searches: Vec<Vec<ProjectTemplateSearchEvidence>>,
}

impl ProjectTemplateFiles {
    fn searches(&self) -> &[Vec<ProjectTemplateSearchEvidence>] {
        &self.searches
    }
}

impl fmt::Debug for ProjectTemplateFiles {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProjectTemplateFiles")
            .field("searches", &self.searches)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
enum ProjectTemplateSearchEvidence {
    File {
        template: ProjectTemplateFile,
        backend: usize,
    },
    UnknownRoots {
        backend: Option<usize>,
    },
    Issue {
        issue: TemplateSearchIssue,
        backend: usize,
    },
}

impl fmt::Debug for ProjectTemplateSearchEvidence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::File { template, backend } => f
                .debug_struct("File")
                .field("template", template)
                .field("backend", backend)
                .finish(),
            Self::UnknownRoots { backend } => f
                .debug_struct("UnknownRoots")
                .field("backend", backend)
                .finish(),
            Self::Issue { issue, backend } => f
                .debug_struct("Issue")
                .field("issue", issue)
                .field("backend", backend)
                .finish(),
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
struct ProjectTemplateFile {
    name: String,
    path: Utf8PathBuf,
    file: File,
}

impl ProjectTemplateFile {
    fn new(name: String, path: Utf8PathBuf, file: File) -> Self {
        Self { name, path, file }
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn file(&self) -> File {
        self.file
    }
}

impl fmt::Debug for ProjectTemplateFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProjectTemplateFile")
            .field("name", &self.name)
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

#[salsa::tracked(returns(ref))]
fn project_template_files(db: &dyn ProjectDb, project: Project) -> ProjectTemplateFiles {
    // Freshness boundary: template discovery re-runs when any search-path root revision is bumped
    // during Django Discovery. Directories outside registered roots are re-walked with the query.
    for search_path in project.search_paths(db).iter() {
        if let Some(root) = db.files().root(db, search_path.path()) {
            let _ = root.revision(db);
        } else {
            tracing::warn!(
                "Search path has no registered source root: {}",
                search_path.path()
            );
        }
    }

    let mut searches = Vec::new();
    let walk_options = WalkOptions::unrestricted();
    let directories = template_directories(db, project);

    for alternative in directories.alternatives() {
        let mut search = Vec::new();
        for entry in &alternative.0 {
            let (root, backend) = match entry {
                RootEntry::Unknown { backend } => {
                    search.push(ProjectTemplateSearchEvidence::UnknownRoots { backend: *backend });
                    continue;
                }
                RootEntry::Known { root, backend } => (root, *backend),
            };
            let (entries, issues) = match db.walk_root(root, &walk_options) {
                // Missing and file roots are exhaustively empty: nothing to load templates from.
                RootWalk::Missing | RootWalk::File(_) => continue,
                RootWalk::Directory { entries, issues } => (entries, issues),
                RootWalk::Inaccessible(kind) => (Vec::new(), vec![kind]),
            };
            let mut root_evidence = Vec::new();
            // A traversal issue can hide a matching file anywhere in this root, so it must precede
            // every positive retained from the same walk.
            for kind in issues {
                tracing::warn!(
                    "Failed to fully walk template directory {}: {:?}",
                    root,
                    kind
                );
                search.push(ProjectTemplateSearchEvidence::Issue {
                    issue: TemplateSearchIssue::Walk {
                        root: root.clone(),
                        kind,
                    },
                    backend,
                });
            }
            for entry in entries {
                if entry.kind != WalkEntryKind::File {
                    continue;
                }
                let name = entry.relative.clean().to_string();
                match path_to_file(db, &entry.path) {
                    Ok(file) => root_evidence.push(ProjectTemplateSearchEvidence::File {
                        template: ProjectTemplateFile::new(name, entry.path, file),
                        backend,
                    }),
                    Err(error) => {
                        tracing::warn!("Failed to index template file {}: {}", entry.path, error);
                        root_evidence.push(ProjectTemplateSearchEvidence::Issue {
                            issue: TemplateSearchIssue::File {
                                name,
                                path: entry.path,
                                error,
                            },
                            backend,
                        });
                    }
                }
            }

            root_evidence.sort_by(|a, b| match (a, b) {
                (
                    ProjectTemplateSearchEvidence::File { template: a, .. },
                    ProjectTemplateSearchEvidence::File { template: b, .. },
                ) => a.name.cmp(&b.name).then_with(|| a.path.cmp(&b.path)),
                (
                    ProjectTemplateSearchEvidence::Issue {
                        issue: TemplateSearchIssue::File { name: a, .. },
                        ..
                    },
                    ProjectTemplateSearchEvidence::Issue {
                        issue: TemplateSearchIssue::File { name: b, .. },
                        ..
                    },
                ) => a.cmp(b),
                (ProjectTemplateSearchEvidence::File { .. }, _) => std::cmp::Ordering::Less,
                (_, ProjectTemplateSearchEvidence::File { .. }) => std::cmp::Ordering::Greater,
                _ => std::cmp::Ordering::Equal,
            });
            search.extend(root_evidence);
        }
        searches.push(search);
    }

    ProjectTemplateFiles { searches }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use super::resolve_relative_name;

    #[test]
    fn non_relative_names_pass_through_borrowed() {
        assert_eq!(
            resolve_relative_name(Some("dir/page.html"), "x.html", false),
            Some(Cow::Borrowed("x.html"))
        );
        assert_eq!(
            resolve_relative_name(Some("dir/page.html"), "partials/./x.html", false),
            Some(Cow::Borrowed("partials/./x.html"))
        );
    }

    #[test]
    fn non_relative_name_without_current_template_passes_through_borrowed() {
        assert_eq!(
            resolve_relative_name(None, "x.html", false),
            Some(Cow::Borrowed("x.html"))
        );
    }

    #[test]
    fn relative_name_without_current_template_is_unresolvable() {
        assert_eq!(resolve_relative_name(None, "./x.html", false), None);
    }

    #[test]
    fn resolves_sibling_relative_name() {
        assert_eq!(
            resolve_relative_name(Some("dir/page.html"), "./x.html", false).as_deref(),
            Some("dir/x.html")
        );
    }

    #[test]
    fn resolves_parent_relative_name() {
        assert_eq!(
            resolve_relative_name(Some("a/b/page.html"), "../x.html", false).as_deref(),
            Some("a/x.html")
        );
    }

    #[test]
    fn rejects_relative_name_that_escapes_hierarchy() {
        assert_eq!(
            resolve_relative_name(Some("dir/page.html"), "../../x.html", false),
            None
        );
    }

    #[test]
    fn rejects_self_when_self_is_not_allowed() {
        assert_eq!(
            resolve_relative_name(Some("dir/page.html"), "./page.html", false),
            None
        );
    }

    #[test]
    fn allows_self_when_self_is_allowed() {
        assert_eq!(
            resolve_relative_name(Some("dir/page.html"), "./page.html", true).as_deref(),
            Some("dir/page.html")
        );
    }

    #[test]
    fn compares_self_against_current_name_without_leading_slash() {
        assert_eq!(
            resolve_relative_name(Some("/dir/page.html"), "./page.html", false),
            None
        );
        assert_eq!(
            resolve_relative_name(Some("/dir/page.html"), "./x.html", false).as_deref(),
            Some("dir/x.html")
        );
    }

    #[test]
    fn resolves_relative_name_from_root_template() {
        assert_eq!(
            resolve_relative_name(Some("page.html"), "./x.html", false).as_deref(),
            Some("x.html")
        );
    }
}
