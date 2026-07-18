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
use crate::settings::types::InstalledAppEvidence;
use crate::templates::configurations::TemplateBackendConfiguration;
use crate::templates::configurations::TemplateBackendId;
use crate::templates::configurations::TemplateConfigurationId;
use crate::templates::configurations::TemplateConfigurationSlot;
use crate::templates::configurations::TemplateDirectoryEvidence;
use crate::templates::configurations::template_configurations;
use crate::templates::installed_app_package_module;

/// The feasible ordered template-root search sequences extracted from settings.
///
/// Settings alternatives remain separate so resolution can compare the winner of each complete
/// configuration rather than treating roots from mutually exclusive branches as one loader list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateDirectories(Vec<TemplateDirectoryAlternative>);

#[derive(Clone, Debug, PartialEq, Eq)]
struct TemplateDirectoryAlternative {
    configuration: TemplateConfigurationId,
    roots: Vec<RootEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum RootEntry {
    Known {
        root: Utf8PathBuf,
        backend: TemplateBackendId,
    },
    /// One unenumerable element at this exact position; roots before and after it keep
    /// their ordering guarantees.
    Unknown { selection: TemplateBackendSelection },
}

impl TemplateDirectoryAlternative {
    fn push_root(&mut self, root: Utf8PathBuf, backend: TemplateBackendId) {
        self.roots.push(RootEntry::Known { root, backend });
    }

    fn mark_unknown_roots(&mut self, selection: TemplateBackendSelection) {
        if !matches!(self.roots.last(), Some(RootEntry::Unknown { selection: existing }) if *existing == selection)
        {
            self.roots.push(RootEntry::Unknown { selection });
        }
    }
}

impl TemplateDirectories {
    pub fn known_roots(&self) -> impl Iterator<Item = &Utf8Path> {
        let mut seen = FxHashSet::default();
        self.0
            .iter()
            .flat_map(|alternative| &alternative.roots)
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
                    .roots
                    .iter()
                    .any(|entry| matches!(entry, RootEntry::Unknown { .. }))
            })
    }

    fn alternatives(&self) -> &[TemplateDirectoryAlternative] {
        &self.0
    }
}

fn known_roots_for_scope<'a>(
    directories: &'a TemplateDirectories,
    scope: &TemplateBackendScope,
) -> Vec<&'a Utf8Path> {
    let selections = match scope.kind() {
        TemplateBackendScopeKind::ProjectInventory => {
            return directories.known_roots().collect();
        }
        TemplateBackendScopeKind::Selected(selections) => selections.as_slice(),
    };
    let mut roots = Vec::new();
    for selection in selections {
        let TemplateBackendSelection::Backend(backend) = *selection else {
            // A configuration remainder has no concrete roots to report as tried.
            continue;
        };
        for entry in directories
            .alternatives()
            .iter()
            .flat_map(|alternative| &alternative.roots)
        {
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
    roots
}

fn add_backend_roots(
    db: &dyn ProjectDb,
    project: Project,
    backend: &TemplateBackendConfiguration,
    installed_apps: &[InstalledAppEvidence],
    alternative: &mut TemplateDirectoryAlternative,
) {
    if backend.backend_state().is_open() {
        alternative.mark_unknown_roots(TemplateBackendSelection::Backend(backend.id()));
    }
    if backend.backend_name() != Some("django.template.backends.django.DjangoTemplates") {
        return;
    }
    for evidence in backend.directories() {
        match evidence {
            TemplateDirectoryEvidence::Path(path) => {
                alternative.push_root(path.clone(), backend.id());
            }
            TemplateDirectoryEvidence::Unknown => {
                alternative.mark_unknown_roots(TemplateBackendSelection::Backend(backend.id()));
            }
        }
    }
    if backend.app_directories_state().is_open() {
        alternative.mark_unknown_roots(TemplateBackendSelection::Backend(backend.id()));
    }
    if backend.app_directories() == Some(true) {
        for evidence in installed_apps {
            let InstalledAppEvidence::Known(app) = evidence else {
                alternative.mark_unknown_roots(TemplateBackendSelection::Backend(backend.id()));
                continue;
            };
            let Some(package_module) = installed_app_package_module(db, project, &app.value) else {
                alternative.mark_unknown_roots(TemplateBackendSelection::Backend(backend.id()));
                continue;
            };
            let package_dirs = resolve_package_dirs(db, project, package_module);
            if package_dirs.dirs.is_empty() {
                alternative.mark_unknown_roots(TemplateBackendSelection::Backend(backend.id()));
            }
            for package_dir in package_dirs.dirs {
                alternative.push_root(package_dir.join("templates"), backend.id());
            }
        }
    }
}

#[salsa::tracked(returns(ref))]
pub fn template_directories(db: &dyn ProjectDb, project: Project) -> TemplateDirectories {
    project.touch_search_path_roots(db);

    let configurations = template_configurations(db, project);
    let alternatives = configurations
        .configurations()
        .iter()
        .map(|configuration| {
            let mut alternative = TemplateDirectoryAlternative {
                configuration: configuration.id(),
                roots: Vec::new(),
            };
            for slot in configuration.slots() {
                match *slot {
                    TemplateConfigurationSlot::Backend(backend) => {
                        let backend = configurations
                            .backend(backend)
                            .expect("a canonical backend slot should resolve");
                        add_backend_roots(
                            db,
                            project,
                            backend,
                            configuration.installed_apps(),
                            &mut alternative,
                        );
                    }
                    TemplateConfigurationSlot::Remainder => alternative.mark_unknown_roots(
                        TemplateBackendSelection::ConfigurationRemainder(configuration.id()),
                    ),
                }
            }
            alternative
        })
        .collect();

    TemplateDirectories(alternatives)
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

    #[must_use]
    pub(crate) fn backend_scope_for_file(
        self,
        db: &'db dyn ProjectDb,
        file: File,
    ) -> TemplateBackendScope {
        template_directory_index(db, self)
            .backend_scopes_by_file(db)
            .get(&file)
            .cloned()
            .unwrap_or_else(TemplateBackendScope::project_inventory)
    }

    #[must_use]
    pub fn backend_scope_for_origin(
        self,
        db: &'db dyn ProjectDb,
        origin: TemplateOrigin<'db>,
    ) -> TemplateBackendScope {
        template_directory_index(db, self)
            .backend_scopes_by_origin(db)
            .get(&origin.file(db))
            .and_then(|by_name| by_name.get(origin.template_name(db).name(db)))
            .cloned()
            .unwrap_or_else(TemplateBackendScope::project_inventory)
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
        self.resolve_excluding_in_scope(
            db,
            name,
            excluded,
            &TemplateBackendScope::project_inventory(),
        )
    }

    #[must_use]
    pub fn resolve_excluding_origins_in_scope(
        self,
        db: &'db dyn ProjectDb,
        name: TemplateName<'db>,
        excluded: &[TemplateOrigin<'db>],
        scope: &TemplateBackendScope,
    ) -> FindTemplateResult<'db> {
        // Django's skip set follows loader origin identity, not the Template Name alias used to
        // reach that origin. A physical source must therefore stay excluded across all aliases.
        let excluded_files = excluded
            .iter()
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
        let outcomes = match scope.kind() {
            TemplateBackendScopeKind::ProjectInventory => index
                .searches(db)
                .iter()
                .map(|search| resolve_alternative(db, &search.evidence, name, &excluded))
                .collect::<Vec<_>>(),
            TemplateBackendScopeKind::Selected(selections) => {
                let configurations = template_configurations(db, self.project(db));
                selections
                    .as_slice()
                    .iter()
                    .map(|selection| match *selection {
                        TemplateBackendSelection::Backend(backend) => {
                            let Some(configuration) = configurations
                                .backend(backend)
                                .map(TemplateBackendConfiguration::configuration)
                            else {
                                return AlternativeOutcome::Inconclusive {
                                    origins: Vec::new(),
                                };
                            };
                            let Some(search) = index
                                .searches(db)
                                .iter()
                                .find(|search| search.configuration == configuration)
                            else {
                                return AlternativeOutcome::Inconclusive {
                                    origins: Vec::new(),
                                };
                            };
                            let filtered = search
                                .evidence
                                .iter()
                                .filter(|evidence| evidence.matches_backend(backend))
                                .cloned()
                                .collect::<Vec<_>>();
                            resolve_alternative(db, &filtered, name, &excluded)
                        }
                        TemplateBackendSelection::ConfigurationRemainder(_) => {
                            AlternativeOutcome::Inconclusive {
                                origins: Vec::new(),
                            }
                        }
                    })
                    .collect::<Vec<_>>()
            }
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
            let tried = known_roots_for_scope(directories, scope)
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

#[cfg(test)]
#[salsa::tracked]
fn test_template_origin<'db>(
    db: &'db dyn ProjectDb,
    name: TemplateName<'db>,
    file: File,
) -> TemplateOrigin<'db> {
    TemplateOrigin::new(db, name, file)
}

#[cfg(test)]
fn collect_backend_selections_scan<'db>(
    db: &'db dyn ProjectDb,
    search: &[TemplateSearchEvidence<'db>],
    name: TemplateName<'db>,
    file: File,
    selections: &mut Vec<TemplateBackendSelection>,
) {
    let file_path = file.path(db);
    for evidence in search {
        let selection = match evidence {
            TemplateSearchEvidence::Origin { origin, backend }
                if origin.template_name(db) == name && origin.file(db) == file =>
            {
                TemplateBackendSelection::Backend(*backend)
            }
            TemplateSearchEvidence::UnknownRoots { selection } => *selection,
            TemplateSearchEvidence::Issue {
                issue: TemplateSearchIssue::Walk { root, .. },
                backend,
            } if file_path
                .strip_prefix(root)
                .is_ok_and(|relative| relative.clean().as_str() == name.name(db)) =>
            {
                TemplateBackendSelection::Backend(*backend)
            }
            TemplateSearchEvidence::Issue {
                issue:
                    TemplateSearchIssue::File {
                        name: issue_name,
                        path,
                        ..
                    },
                backend,
            } if issue_name == name.name(db) && path == file_path => {
                TemplateBackendSelection::Backend(*backend)
            }
            TemplateSearchEvidence::Origin { .. } | TemplateSearchEvidence::Issue { .. } => {
                continue;
            }
        };
        if !selections.contains(&selection) {
            selections.push(selection);
        }
    }
}

#[derive(Default)]
struct BackendSelectionEvidenceIndex<'db> {
    concrete_by_origin: FxHashMap<(File, TemplateName<'db>), Vec<TemplateBackendSelection>>,
    global: Vec<TemplateBackendSelection>,
    file_issues: FxHashMap<Utf8PathBuf, FxHashMap<String, Vec<TemplateBackendSelection>>>,
    walk_issues: Vec<(Utf8PathBuf, TemplateBackendSelection)>,
    canonical_order: Vec<TemplateBackendSelection>,
}

struct BackendSelectionIndexes {
    by_origin: FxHashMap<File, FxHashMap<String, TemplateBackendScope>>,
    by_file: FxHashMap<File, TemplateBackendScope>,
}

impl<'db> BackendSelectionEvidenceIndex<'db> {
    fn record(&mut self, db: &'db dyn ProjectDb, evidence: &TemplateSearchEvidence<'db>) {
        let selection = evidence.selection();
        if !self.canonical_order.contains(&selection) {
            self.canonical_order.push(selection);
        }
        match evidence {
            TemplateSearchEvidence::Origin { origin, backend } => {
                self.concrete_by_origin
                    .entry((origin.file(db), origin.template_name(db)))
                    .or_default()
                    .push(TemplateBackendSelection::Backend(*backend));
            }
            // Unknown roots are feasible for every discovered (file, name) pair. Keep one compact
            // canonical selection rather than expanding them while consuming origins.
            TemplateSearchEvidence::UnknownRoots { selection } => self.global.push(*selection),
            // File failures affect exactly one physical path and Template Name.
            TemplateSearchEvidence::Issue {
                issue: TemplateSearchIssue::File { name, path, .. },
                backend,
            } => {
                self.file_issues
                    .entry(path.clone())
                    .or_default()
                    .entry(name.clone())
                    .or_default()
                    .push(TemplateBackendSelection::Backend(*backend));
            }
            // Walk failures stay compact by root. They are matched only against discovered
            // (file, name) pairs after all concrete origins have been consumed.
            TemplateSearchEvidence::Issue {
                issue: TemplateSearchIssue::Walk { root, .. },
                backend,
            } => self
                .walk_issues
                .push((root.clone(), TemplateBackendSelection::Backend(*backend))),
        }
    }

    fn finish(
        mut self,
        db: &'db dyn ProjectDb,
        names_by_file: &FxHashMap<File, Vec<TemplateName<'db>>>,
    ) -> BackendSelectionIndexes {
        stable_deduplicate_backend_selections(&mut self.global);

        let mut by_origin = FxHashMap::default();
        let mut by_file = FxHashMap::default();
        for (&file, names) in names_by_file {
            let file_path = file.path(db);
            let mut file_selections = Vec::new();

            for &name in names {
                let key = (file, name);
                let mut selections = self.concrete_by_origin.remove(&key).unwrap_or_default();
                selections.extend_from_slice(&self.global);

                if let Some(issue_selections) = self
                    .file_issues
                    .get(file_path)
                    .and_then(|by_name| by_name.get(name.name(db)))
                {
                    selections.extend_from_slice(issue_selections);
                }

                for (root, selection) in &self.walk_issues {
                    if file_path
                        .strip_prefix(root)
                        .is_ok_and(|relative| relative.clean().as_str() == name.name(db))
                    {
                        selections.push(*selection);
                    }
                }

                let selections =
                    backend_selections_in_canonical_order(&self.canonical_order, &selections);
                file_selections.extend_from_slice(&selections);
                if let Some(scope) = TemplateBackendScope::selected(selections) {
                    by_origin
                        .entry(file)
                        .or_insert_with(FxHashMap::default)
                        .insert(name.name(db).clone(), scope);
                }
            }

            file_selections =
                backend_selections_in_canonical_order(&self.canonical_order, &file_selections);
            if let Some(scope) = TemplateBackendScope::selected(file_selections) {
                by_file.insert(file, scope);
            }
        }

        BackendSelectionIndexes { by_origin, by_file }
    }
}

fn stable_deduplicate_backend_selections(selections: &mut Vec<TemplateBackendSelection>) {
    let mut unique = Vec::with_capacity(selections.len());
    for selection in selections.drain(..) {
        if !unique.contains(&selection) {
            unique.push(selection);
        }
    }
    *selections = unique;
}

fn backend_selections_in_canonical_order(
    canonical_order: &[TemplateBackendSelection],
    selections: &[TemplateBackendSelection],
) -> Vec<TemplateBackendSelection> {
    let mut ordered = canonical_order
        .iter()
        .copied()
        .filter(|selection| selections.contains(selection))
        .collect::<Vec<_>>();
    ordered.extend_from_slice(selections);
    stable_deduplicate_backend_selections(&mut ordered);
    ordered
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScopedTemplateReferenceResolution<'db> {
    pub source: TemplateOrigin<'db>,
    pub target_name: TemplateName<'db>,
    pub result: FindTemplateResult<'db>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct TemplateBackendScope(TemplateBackendScopeKind);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum TemplateBackendScopeKind {
    ProjectInventory,
    Selected(SelectedTemplateBackends),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SelectedTemplateBackends(Vec<TemplateBackendSelection>);

impl SelectedTemplateBackends {
    pub(super) fn as_slice(&self) -> &[TemplateBackendSelection] {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TemplateBackendSelection {
    Backend(TemplateBackendId),
    /// A feasible settings configuration whose backend and roots cannot be enumerated.
    ConfigurationRemainder(TemplateConfigurationId),
}

impl fmt::Debug for TemplateBackendScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            TemplateBackendScopeKind::ProjectInventory => {
                f.write_str("TemplateBackendScope::ProjectInventory")
            }
            TemplateBackendScopeKind::Selected(_) => f.write_str("TemplateBackendScope::Selected"),
        }
    }
}

static PROJECT_INVENTORY_SCOPE: TemplateBackendScope =
    TemplateBackendScope(TemplateBackendScopeKind::ProjectInventory);

impl TemplateBackendScope {
    pub(super) const fn project_inventory() -> Self {
        Self(TemplateBackendScopeKind::ProjectInventory)
    }

    pub(super) const fn project_inventory_ref() -> &'static Self {
        &PROJECT_INVENTORY_SCOPE
    }

    pub(super) fn selected(mut selections: Vec<TemplateBackendSelection>) -> Option<Self> {
        stable_deduplicate_backend_selections(&mut selections);
        (!selections.is_empty()).then_some({
            Self(TemplateBackendScopeKind::Selected(
                SelectedTemplateBackends(selections),
            ))
        })
    }

    pub(super) const fn kind(&self) -> &TemplateBackendScopeKind {
        &self.0
    }
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
    /// Equality-bearing direct scope evidence for each discovered Template Origin.
    #[tracked]
    #[returns(ref)]
    backend_scopes_by_origin: FxHashMap<File, FxHashMap<String, TemplateBackendScope>>,
    /// Equality-bearing direct scope evidence for each discovered physical Template.
    ///
    /// This is derived beside the origin/search indexes so per-Template semantic analysis does
    /// not rescan every configuration and compare every candidate path.
    #[tracked]
    #[returns(ref)]
    backend_scopes_by_file: FxHashMap<File, TemplateBackendScope>,
    #[tracked]
    #[returns(ref)]
    searches: Vec<ConfigurationSearch<'db>>,
}

#[derive(Clone, Debug, PartialEq, Eq, salsa::Update)]
struct ConfigurationSearch<'db> {
    configuration: TemplateConfigurationId,
    evidence: Vec<TemplateSearchEvidence<'db>>,
}

#[derive(Clone, Debug, PartialEq, Eq, salsa::Update)]
enum TemplateSearchEvidence<'db> {
    Origin {
        origin: TemplateOrigin<'db>,
        backend: TemplateBackendId,
    },
    UnknownRoots {
        selection: TemplateBackendSelection,
    },
    Issue {
        issue: TemplateSearchIssue,
        backend: TemplateBackendId,
    },
}

impl TemplateSearchEvidence<'_> {
    fn selection(&self) -> TemplateBackendSelection {
        match self {
            Self::Origin { backend, .. } | Self::Issue { backend, .. } => {
                TemplateBackendSelection::Backend(*backend)
            }
            Self::UnknownRoots { selection } => *selection,
        }
    }

    fn matches_backend(&self, backend: TemplateBackendId) -> bool {
        match self {
            Self::Origin {
                backend: candidate, ..
            }
            | Self::Issue {
                backend: candidate, ..
            } => *candidate == backend,
            Self::UnknownRoots { selection } => {
                *selection == TemplateBackendSelection::Backend(backend)
            }
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
    let mut backend_selection_evidence = BackendSelectionEvidenceIndex::default();

    for alternative in files.searches() {
        let mut search = Vec::new();
        for evidence in &alternative.evidence {
            let evidence = match evidence {
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
                    TemplateSearchEvidence::Origin {
                        origin,
                        backend: *backend,
                    }
                }
                ProjectTemplateSearchEvidence::UnknownRoots { selection } => {
                    TemplateSearchEvidence::UnknownRoots {
                        selection: *selection,
                    }
                }
                ProjectTemplateSearchEvidence::Issue { issue, backend } => {
                    TemplateSearchEvidence::Issue {
                        issue: issue.clone(),
                        backend: *backend,
                    }
                }
            };
            backend_selection_evidence.record(db, &evidence);
            search.push(evidence);
        }
        searches.push(ConfigurationSearch {
            configuration: alternative.configuration,
            evidence: search,
        });
    }

    let BackendSelectionIndexes {
        by_origin: backend_scopes_by_origin,
        by_file: backend_scopes_by_file,
    } = backend_selection_evidence.finish(db, &names_by_file);

    tracing::debug!("Discovered {} total template origins", ordered.len());

    TemplateDirectoryIndex::new(
        db,
        ordered,
        by_template_name,
        names_by_file,
        backend_scopes_by_origin,
        backend_scopes_by_file,
        searches,
    )
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
            TemplateSearchEvidence::UnknownRoots { selection } => {
                if !open_selections.contains(selection) {
                    open_selections.push(*selection);
                }
            }
            TemplateSearchEvidence::Issue {
                issue: TemplateSearchIssue::Walk { .. },
                backend,
            } => {
                let selection = TemplateBackendSelection::Backend(*backend);
                if !open_selections.contains(&selection) {
                    open_selections.push(selection);
                }
            }
            TemplateSearchEvidence::Issue {
                issue:
                    TemplateSearchIssue::File {
                        name: issue_name, ..
                    },
                backend,
            } if issue_name == name.name(db) => {
                let selection = TemplateBackendSelection::Backend(*backend);
                if !open_selections.contains(&selection) {
                    open_selections.push(selection);
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
    searches: Vec<ProjectConfigurationSearch>,
}

impl ProjectTemplateFiles {
    fn searches(&self) -> &[ProjectConfigurationSearch] {
        &self.searches
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ProjectConfigurationSearch {
    configuration: TemplateConfigurationId,
    evidence: Vec<ProjectTemplateSearchEvidence>,
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
        backend: TemplateBackendId,
    },
    UnknownRoots {
        selection: TemplateBackendSelection,
    },
    Issue {
        issue: TemplateSearchIssue,
        backend: TemplateBackendId,
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
            Self::UnknownRoots { selection } => f
                .debug_struct("UnknownRoots")
                .field("selection", selection)
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
        for entry in &alternative.roots {
            let (root, backend) = match entry {
                RootEntry::Unknown { selection } => {
                    search.push(ProjectTemplateSearchEvidence::UnknownRoots {
                        selection: *selection,
                    });
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
        searches.push(ProjectConfigurationSearch {
            configuration: alternative.configuration,
            evidence: search,
        });
    }

    ProjectTemplateFiles { searches }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use djls_testing::TestDatabase;

    use super::*;
    use crate::templates::configurations::TemplateConfigurations;

    fn scan_selection_indexes<'db>(
        db: &'db TestDatabase,
        searches: &[Vec<TemplateSearchEvidence<'db>>],
        names_by_file: &FxHashMap<File, Vec<TemplateName<'db>>>,
    ) -> BackendSelectionIndexes {
        let mut canonical_order = Vec::new();
        for evidence in searches.iter().flatten() {
            let selection = evidence.selection();
            if !canonical_order.contains(&selection) {
                canonical_order.push(selection);
            }
        }
        let mut by_origin = FxHashMap::default();
        let mut by_file = FxHashMap::default();

        for (&file, names) in names_by_file {
            let mut file_selections = Vec::new();
            for &name in names {
                let mut selections = Vec::new();
                for search in searches {
                    collect_backend_selections_scan(db, search, name, file, &mut selections);
                }
                let selections =
                    backend_selections_in_canonical_order(&canonical_order, &selections);
                file_selections.extend_from_slice(&selections);
                if let Some(scope) = TemplateBackendScope::selected(selections) {
                    by_origin
                        .entry(file)
                        .or_insert_with(FxHashMap::default)
                        .insert(name.name(db).clone(), scope);
                }
            }
            file_selections =
                backend_selections_in_canonical_order(&canonical_order, &file_selections);
            if let Some(scope) = TemplateBackendScope::selected(file_selections) {
                by_file.insert(file, scope);
            }
        }

        BackendSelectionIndexes { by_origin, by_file }
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn direct_backend_selection_indexes_match_scan_for_all_evidence() {
        let db = TestDatabase::new();
        db.add_file("/templates/a/page.html", "a");
        db.add_file("/templates/b/page.html", "b");
        db.add_file("/outside/page.html", "outside");
        let file_a = db.file(Utf8Path::new("/templates/a/page.html"));
        let file_b = db.file(Utf8Path::new("/templates/b/page.html"));
        let outside = db.file(Utf8Path::new("/outside/page.html"));
        let page = TemplateName::new(&db, "page.html".to_string());
        let alias = TemplateName::new(&db, "alias.html".to_string());
        let origin_a = test_template_origin(&db, page, file_a);
        let origin_a_alias = test_template_origin(&db, alias, file_a);
        let origin_b = test_template_origin(&db, page, file_b);
        let identities = TemplateConfigurations::for_testing(&[7, 2], false);
        let first_configuration = &identities.configurations()[0];
        let second_configuration = &identities.configurations()[1];
        let first_backends = first_configuration
            .backends()
            .iter()
            .map(TemplateBackendConfiguration::id)
            .collect::<Vec<_>>();
        let second_backends = second_configuration
            .backends()
            .iter()
            .map(TemplateBackendConfiguration::id)
            .collect::<Vec<_>>();

        let searches = vec![
            vec![
                TemplateSearchEvidence::Origin {
                    origin: origin_a,
                    backend: first_backends[0],
                },
                TemplateSearchEvidence::Origin {
                    origin: origin_b,
                    backend: first_backends[1],
                },
                TemplateSearchEvidence::UnknownRoots {
                    selection: TemplateBackendSelection::Backend(first_backends[2]),
                },
                TemplateSearchEvidence::UnknownRoots {
                    selection: TemplateBackendSelection::ConfigurationRemainder(
                        first_configuration.id(),
                    ),
                },
                TemplateSearchEvidence::Issue {
                    issue: TemplateSearchIssue::Walk {
                        root: Utf8PathBuf::from("/templates/a"),
                        kind: io::ErrorKind::PermissionDenied,
                    },
                    backend: first_backends[3],
                },
                TemplateSearchEvidence::Issue {
                    issue: TemplateSearchIssue::Walk {
                        root: Utf8PathBuf::from("/unrelated"),
                        kind: io::ErrorKind::PermissionDenied,
                    },
                    backend: first_backends[4],
                },
                TemplateSearchEvidence::Issue {
                    issue: TemplateSearchIssue::File {
                        name: "alias.html".to_string(),
                        path: Utf8PathBuf::from("/templates/a/page.html"),
                        error: FileError::NotFound,
                    },
                    backend: first_backends[5],
                },
                TemplateSearchEvidence::Issue {
                    issue: TemplateSearchIssue::File {
                        name: "other.html".to_string(),
                        path: Utf8PathBuf::from("/templates/a/page.html"),
                        error: FileError::NotFound,
                    },
                    backend: first_backends[6],
                },
            ],
            vec![
                TemplateSearchEvidence::Origin {
                    origin: origin_a_alias,
                    backend: second_backends[0],
                },
                TemplateSearchEvidence::UnknownRoots {
                    selection: TemplateBackendSelection::Backend(second_backends[1]),
                },
            ],
        ];
        let names_by_file =
            FxHashMap::from_iter([(file_a, vec![page, alias]), (file_b, vec![page])]);

        let mut evidence_index = BackendSelectionEvidenceIndex::default();
        for search in &searches {
            for evidence in search {
                evidence_index.record(&db, evidence);
            }
        }
        let indexed = evidence_index.finish(&db, &names_by_file);
        let scanned = scan_selection_indexes(&db, &searches, &names_by_file);

        assert_eq!(indexed.by_origin, scanned.by_origin);
        assert_eq!(indexed.by_file, scanned.by_file);
        assert!(indexed.by_file.contains_key(&file_a));
        assert!(
            !indexed.by_file.contains_key(&outside),
            "a file with no concrete Template Origin must retain ProjectInventory scope"
        );
        assert!(
            matches!(
                indexed.by_origin[&file_a]["page.html"].kind(),
                TemplateBackendScopeKind::Selected(selections)
                    if selections
                        .as_slice()
                        .contains(&TemplateBackendSelection::Backend(first_backends[3]))
            ),
            "the matching walk issue must contribute uncertainty"
        );
        assert!(
            matches!(
                indexed.by_origin[&file_a]["alias.html"].kind(),
                TemplateBackendScopeKind::Selected(selections)
                    if selections
                        .as_slice()
                        .contains(&TemplateBackendSelection::Backend(first_backends[5]))
            ),
            "the exact (path, name) file issue must contribute uncertainty"
        );
        assert!(
            matches!(
                indexed.by_origin[&file_a]["page.html"].kind(),
                TemplateBackendScopeKind::Selected(selections)
                    if !selections
                        .as_slice()
                        .contains(&TemplateBackendSelection::Backend(first_backends[5]))
            ),
            "file issue evidence for an alias must not leak to another Template Name"
        );
    }

    #[test]
    fn selected_backend_scope_stably_deduplicates_canonical_order() {
        let configurations = TemplateConfigurations::for_testing(&[2], true);
        let configuration = &configurations.configurations()[0];
        let first = configuration.backends()[0].id();
        let second = configuration.backends()[1].id();
        let remainder = TemplateBackendSelection::ConfigurationRemainder(configuration.id());
        let scope = TemplateBackendScope::selected(vec![
            TemplateBackendSelection::Backend(first),
            remainder,
            TemplateBackendSelection::Backend(second),
            remainder,
            TemplateBackendSelection::Backend(first),
        ])
        .expect("canonical selections should form a scope");

        let TemplateBackendScopeKind::Selected(selections) = scope.kind() else {
            panic!("non-empty selections should retain Selected scope")
        };
        assert_eq!(
            selections.as_slice(),
            [
                TemplateBackendSelection::Backend(first),
                remainder,
                TemplateBackendSelection::Backend(second),
            ]
        );
        assert!(
            TemplateBackendScope::selected(Vec::new()).is_none(),
            "an empty selection cannot construct a Selected scope"
        );
    }

    #[test]
    fn backend_scope_debug_is_opaque() {
        let configurations = TemplateConfigurations::for_testing(&[1], false);
        let backend = configurations.configurations()[0].backends()[0].id();
        let scope =
            TemplateBackendScope::selected(vec![TemplateBackendSelection::Backend(backend)])
                .expect("one backend selection should form a scope");

        assert_eq!(format!("{scope:?}"), "TemplateBackendScope::Selected");
        assert_eq!(
            format!("{:?}", TemplateBackendScope::project_inventory()),
            "TemplateBackendScope::ProjectInventory"
        );
    }

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
