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
use crate::templates::installed_app_package_module;

/// The ordered template-root search sequence Django Discovery extracted from settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateDirectories(Vec<RootEntry>);

#[derive(Clone, Debug, PartialEq, Eq)]
enum RootEntry {
    Known(Utf8PathBuf),
    Unknown(UnknownRoots),
}

/// Roots the configuration may contain that extraction could not enumerate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, salsa::Update)]
enum UnknownRoots {
    /// One unenumerable element at this exact position; roots before and after it keep
    /// their ordering guarantees.
    Positioned,
    /// Whole backends may be unextracted; later candidates are parallel alternatives,
    /// not an ordered tail.
    AlternativeBackends,
}

impl TemplateDirectories {
    pub fn known_roots(&self) -> impl Iterator<Item = &Utf8Path> {
        self.0.iter().filter_map(|entry| match entry {
            RootEntry::Known(root) => Some(root.as_path()),
            RootEntry::Unknown(_) => None,
        })
    }

    #[must_use]
    pub fn configuration_may_omit_roots(&self) -> bool {
        self.0
            .iter()
            .any(|entry| matches!(entry, RootEntry::Unknown(_)))
    }

    fn search(&self) -> &[RootEntry] {
        &self.0
    }

    fn unknown_roots_count(&self) -> usize {
        self.0
            .iter()
            .filter(|entry| matches!(entry, RootEntry::Unknown(_)))
            .count()
    }

    fn push_root(&mut self, root: Utf8PathBuf) {
        self.0.push(RootEntry::Known(root));
    }

    fn mark_unknown_roots(&mut self, kind: UnknownRoots) {
        let already_marked_here = self
            .0
            .iter()
            .rev()
            .take_while(|entry| matches!(entry, RootEntry::Unknown(_)))
            .any(|entry| *entry == RootEntry::Unknown(kind));
        if !already_marked_here {
            self.0.push(RootEntry::Unknown(kind));
        }
    }
}

#[salsa::tracked(returns(ref))]
pub fn template_directories(db: &dyn ProjectDb, project: Project) -> TemplateDirectories {
    project.touch_search_path_roots(db);

    let settings = django_settings(db, project);
    let mut discovery = TemplateDirectories(Vec::new());

    let templates_have_positioned_unknown_dir = settings.templates.backends.len() == 1
        && settings.templates.backends[0].is_django_templates_backend()
        && settings.templates.backends[0]
            .dirs
            .iter()
            .any(|dir| matches!(dir, EvaluatedPath::Unknown));
    if !settings.templates.is_fully_extracted() && !templates_have_positioned_unknown_dir {
        // Alternative or wholly unknown backend values may occur before any extracted backend.
        // A lone backend's unknown DIRS element is different: its exact position is retained below.
        discovery.mark_unknown_roots(UnknownRoots::AlternativeBackends);
    }

    for backend in settings
        .templates
        .backends
        .iter()
        .filter(|backend| backend.is_django_templates_backend())
    {
        let unknowns_before_backend = discovery.unknown_roots_count();

        for dir in &backend.dirs {
            match dir {
                EvaluatedPath::Resolved(path) => discovery.push_root(path.clone()),
                EvaluatedPath::Unknown => discovery.mark_unknown_roots(UnknownRoots::Positioned),
            }
        }

        if backend.app_dirs == Some(true) {
            if !settings.installed_apps.is_fully_extracted() {
                discovery.mark_unknown_roots(UnknownRoots::Positioned);
            }
            for app in &settings.installed_apps.values {
                let Some(package_module) = installed_app_package_module(db, project, app) else {
                    discovery.mark_unknown_roots(UnknownRoots::Positioned);
                    continue;
                };
                let package_dirs = resolve_package_dirs(db, project, package_module);
                if package_dirs.dirs.is_empty() {
                    discovery.mark_unknown_roots(UnknownRoots::Positioned);
                    continue;
                }

                for package_dir in package_dirs.dirs {
                    // Keep the candidate even when it appears absent. The detailed walk below
                    // distinguishes an exhaustively missing directory from a metadata failure.
                    discovery.push_root(package_dir.join("templates"));
                }
            }
        }

        if !backend.is_fully_extracted()
            && discovery.unknown_roots_count() == unknowns_before_backend
        {
            discovery.mark_unknown_roots(UnknownRoots::AlternativeBackends);
        }
    }

    discovery
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

    /// Returns the first-discovered template name for a file.
    ///
    /// This is the name Django binds the file to in template-directory discovery order, and it
    /// anchors relative-name resolution.
    pub fn primary_template_name(
        self,
        db: &'db dyn ProjectDb,
        file: File,
    ) -> Option<TemplateName<'db>> {
        self.template_names_for_file(db, file).first().copied()
    }

    #[must_use]
    pub fn resolve(
        self,
        db: &'db dyn ProjectDb,
        name: TemplateName<'db>,
    ) -> FindTemplateResult<'db> {
        self.resolve_excluding(db, name, &[])
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
        let index = template_directory_index(db, self);
        let excluded: FxHashSet<_> = excluded.iter().copied().collect();
        let mut certainty = SearchCertainty::Exhaustive;

        for evidence in index.search(db) {
            match evidence {
                TemplateSearchEvidence::Origin(origin)
                    if origin.template_name(db) == name && !excluded.contains(&origin.file(db)) =>
                {
                    match &mut certainty {
                        SearchCertainty::Exhaustive => {
                            return FindTemplateResult::Found(*origin);
                        }
                        SearchCertainty::Ordered { possible } => {
                            possible.push(*origin);
                            break;
                        }
                        SearchCertainty::Branching { possible } => possible.push(*origin),
                    }
                }
                TemplateSearchEvidence::UnknownRoots(kind) => {
                    certainty = certainty.weaken(*kind);
                }
                TemplateSearchEvidence::Issue(TemplateSearchIssue::Walk { .. }) => {
                    certainty = certainty.weaken_in_order();
                }
                TemplateSearchEvidence::Issue(TemplateSearchIssue::File {
                    name: issue_name,
                    ..
                }) if issue_name == name.name(db) => {
                    certainty = certainty.weaken_in_order();
                }
                TemplateSearchEvidence::Origin(_) | TemplateSearchEvidence::Issue(_) => {}
            }
        }

        match certainty {
            SearchCertainty::Exhaustive => {
                let tried = template_directories(db, self.project(db))
                    .known_roots()
                    .filter_map(|root| safe_join(root, name.name(db)).ok())
                    .collect();
                FindTemplateResult::DoesNotExist(TemplateDoesNotExist { name, tried })
            }
            SearchCertainty::Ordered { possible } | SearchCertainty::Branching { possible } => {
                FindTemplateResult::Inconclusive(InconclusiveTemplateSearch {
                    name,
                    possible_origins: possible,
                })
            }
        }
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
    #[tracked]
    #[returns(ref)]
    search: Vec<TemplateSearchEvidence<'db>>,
}

#[derive(Clone, Debug, PartialEq, Eq, salsa::Update)]
enum TemplateSearchEvidence<'db> {
    Origin(TemplateOrigin<'db>),
    UnknownRoots(UnknownRoots),
    Issue(TemplateSearchIssue),
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
    let mut search = Vec::new();

    for evidence in files.search() {
        match evidence {
            ProjectTemplateSearchEvidence::File(template) => {
                let template_name = TemplateName::new(db, template.name().to_string());
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
                search.push(TemplateSearchEvidence::Origin(origin));
            }
            ProjectTemplateSearchEvidence::UnknownRoots(kind) => {
                search.push(TemplateSearchEvidence::UnknownRoots(*kind));
            }
            ProjectTemplateSearchEvidence::Issue(issue) => {
                search.push(TemplateSearchEvidence::Issue(issue.clone()));
            }
        }
    }

    tracing::debug!("Discovered {} total template origins", ordered.len());

    TemplateDirectoryIndex::new(db, ordered, by_template_name, names_by_file, search)
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

/// How much an in-progress ordered search can still claim.
///
/// A match under [`Exhaustive`](Self::Exhaustive) wins definitively. [`Ordered`](Self::Ordered)
/// means an unenumerable gap precedes this point, so the first viable match is only the sole
/// possible known winner. [`Branching`](Self::Branching) means alternative backends may exist,
/// so every later branch can hold its own possible winner. Certainty only weakens: a later gap
/// never demotes an already-returned definite match.
enum SearchCertainty<'db> {
    Exhaustive,
    Ordered { possible: Vec<TemplateOrigin<'db>> },
    Branching { possible: Vec<TemplateOrigin<'db>> },
}

impl<'db> SearchCertainty<'db> {
    fn weaken(self, kind: UnknownRoots) -> Self {
        match kind {
            UnknownRoots::Positioned => self.weaken_in_order(),
            UnknownRoots::AlternativeBackends => Self::Branching {
                possible: self.into_possible(),
            },
        }
    }

    fn weaken_in_order(self) -> Self {
        match self {
            Self::Exhaustive => Self::Ordered {
                possible: Vec::new(),
            },
            state @ (Self::Ordered { .. } | Self::Branching { .. }) => state,
        }
    }

    fn into_possible(self) -> Vec<TemplateOrigin<'db>> {
        match self {
            Self::Exhaustive => Vec::new(),
            Self::Ordered { possible } | Self::Branching { possible } => possible,
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
    search: Vec<ProjectTemplateSearchEvidence>,
}

impl ProjectTemplateFiles {
    fn search(&self) -> &[ProjectTemplateSearchEvidence] {
        &self.search
    }
}

impl fmt::Debug for ProjectTemplateFiles {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProjectTemplateFiles")
            .field("search", &self.search)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
enum ProjectTemplateSearchEvidence {
    File(ProjectTemplateFile),
    UnknownRoots(UnknownRoots),
    Issue(TemplateSearchIssue),
}

impl fmt::Debug for ProjectTemplateSearchEvidence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::File(file) => file.fmt(f),
            Self::UnknownRoots(kind) => f.debug_tuple("UnknownRoots").field(kind).finish(),
            Self::Issue(issue) => issue.fmt(f),
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

    let mut search = Vec::new();
    let walk_options = WalkOptions::unrestricted();
    let directories = template_directories(db, project);

    for entry in directories.search() {
        let root = match entry {
            RootEntry::Unknown(kind) => {
                search.push(ProjectTemplateSearchEvidence::UnknownRoots(*kind));
                continue;
            }
            RootEntry::Known(root) => root,
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
            search.push(ProjectTemplateSearchEvidence::Issue(
                TemplateSearchIssue::Walk {
                    root: root.clone(),
                    kind,
                },
            ));
        }
        for entry in entries {
            if entry.kind != WalkEntryKind::File {
                continue;
            }
            let name = entry.relative.clean().to_string();
            match path_to_file(db, &entry.path) {
                Ok(file) => root_evidence.push(ProjectTemplateSearchEvidence::File(
                    ProjectTemplateFile::new(name, entry.path, file),
                )),
                Err(error) => {
                    tracing::warn!("Failed to index template file {}: {}", entry.path, error);
                    root_evidence.push(ProjectTemplateSearchEvidence::Issue(
                        TemplateSearchIssue::File {
                            name,
                            path: entry.path,
                            error,
                        },
                    ));
                }
            }
        }

        root_evidence.sort_by(|a, b| match (a, b) {
            (ProjectTemplateSearchEvidence::File(a), ProjectTemplateSearchEvidence::File(b)) => {
                a.name.cmp(&b.name).then_with(|| a.path.cmp(&b.path))
            }
            (
                ProjectTemplateSearchEvidence::Issue(TemplateSearchIssue::File { name: a, .. }),
                ProjectTemplateSearchEvidence::Issue(TemplateSearchIssue::File { name: b, .. }),
            ) => a.cmp(b),
            (ProjectTemplateSearchEvidence::File(_), _) => std::cmp::Ordering::Less,
            (_, ProjectTemplateSearchEvidence::File(_)) => std::cmp::Ordering::Greater,
            _ => std::cmp::Ordering::Equal,
        });
        search.extend(root_evidence);
    }

    ProjectTemplateFiles { search }
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
