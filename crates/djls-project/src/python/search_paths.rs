use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_source::FileRootKind;
use djls_source::FileSystem;

use crate::db::Db as ProjectDb;
use crate::python::Interpreter;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SearchPath {
    FirstParty(Utf8PathBuf),
    Extra(Utf8PathBuf),
    SitePackages(Utf8PathBuf),
}

impl SearchPath {
    fn first_party(path: Utf8PathBuf) -> Self {
        Self::FirstParty(path)
    }

    fn extra(path: Utf8PathBuf) -> Self {
        Self::Extra(path)
    }

    fn site_packages(path: Utf8PathBuf) -> Self {
        Self::SitePackages(path)
    }

    fn from_pythonpath(
        root: &Utf8Path,
        discovered_site_packages: Option<&Utf8Path>,
        path: Utf8PathBuf,
    ) -> Self {
        if discovered_site_packages.is_some_and(|site_packages| site_packages == path)
            || path
                .components()
                .any(|component| matches!(component.as_str(), "site-packages" | "dist-packages"))
        {
            Self::site_packages(path)
        } else if path.starts_with(root) {
            Self::first_party(path)
        } else {
            Self::extra(path)
        }
    }

    #[must_use]
    pub fn path(&self) -> &Utf8Path {
        match self {
            Self::FirstParty(path) | Self::Extra(path) | Self::SitePackages(path) => path,
        }
    }

    #[must_use]
    pub(crate) fn is_first_party(&self) -> bool {
        matches!(self, Self::FirstParty(_))
    }

    pub(crate) fn root_kind(&self) -> FileRootKind {
        match self {
            // Extra pythonpath entries are user-edited code, so they get the
            // same low-durability treatment as project files.
            Self::FirstParty(_) | Self::Extra(_) => FileRootKind::Project,
            Self::SitePackages(_) => FileRootKind::SearchPath,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchPaths {
    paths: Vec<SearchPath>,
}

impl SearchPaths {
    #[must_use]
    pub(crate) fn root_only(root: &Utf8Path) -> Self {
        let mut search_paths = Self::default();
        search_paths
            .paths
            .push(SearchPath::first_party(root.to_path_buf()));
        search_paths
    }

    #[must_use]
    pub fn from_project_settings(
        fs: &dyn FileSystem,
        root: &Utf8Path,
        interpreter: &Interpreter,
        pythonpath: &[Utf8PathBuf],
    ) -> Self {
        let mut search_paths = Self::default();
        search_paths
            .paths
            .push(SearchPath::first_party(root.to_path_buf()));
        let discovered_site_packages = interpreter.site_packages_path(fs, root);

        for path in pythonpath {
            if !fs.is_dir(path) || search_paths.contains_path(path) {
                continue;
            }

            search_paths.paths.push(SearchPath::from_pythonpath(
                root,
                discovered_site_packages.as_deref(),
                path.clone(),
            ));
        }

        if let Some(site_packages) = discovered_site_packages
            && !search_paths.contains_path(&site_packages)
        {
            search_paths
                .paths
                .push(SearchPath::site_packages(site_packages));
        }

        search_paths
    }

    pub fn register_roots(&self, db: &dyn ProjectDb) {
        let mut roots = Vec::new();
        for search_path in self.iter() {
            if search_path.is_first_party()
                && roots.iter().any(|(path, kind)| {
                    *kind == FileRootKind::Project && search_path.path().starts_with(path)
                })
            {
                continue;
            }

            roots.push((search_path.path().to_path_buf(), search_path.root_kind()));
        }

        db.files().replace_roots(db, roots);
    }

    pub fn iter(&self) -> impl Iterator<Item = &SearchPath> {
        self.paths.iter()
    }

    fn contains_path(&self, path: &Utf8Path) -> bool {
        self.iter().any(|search_path| search_path.path() == path)
    }
}
