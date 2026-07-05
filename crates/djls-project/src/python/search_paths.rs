use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_source::FileRootKind;
use djls_source::FileSystem;
use djls_source::Utf8PathClean;
use djls_source::WalkEntryKind;
use djls_source::WalkOptions;

use crate::db::Db as ProjectDb;
use crate::python::Interpreter;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SearchPath {
    FirstParty(Utf8PathBuf),
    Extra(Utf8PathBuf),
    SitePackages(Utf8PathBuf),
    Editable(Utf8PathBuf),
}

impl SearchPath {
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
            Self::SitePackages(path)
        } else if path.starts_with(root) {
            Self::FirstParty(path)
        } else {
            Self::Extra(path)
        }
    }

    #[must_use]
    pub fn path(&self) -> &Utf8Path {
        match self {
            Self::FirstParty(path)
            | Self::Extra(path)
            | Self::SitePackages(path)
            | Self::Editable(path) => path,
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
            Self::SitePackages(_) | Self::Editable(_) => FileRootKind::SearchPath,
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
            .push(SearchPath::FirstParty(root.to_path_buf()));
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

        let src_root = root.join("src");
        if fs.is_dir(&src_root) && !fs.is_file(&src_root.join("__init__.py")) {
            search_paths.paths.push(SearchPath::FirstParty(src_root));
        }

        search_paths
            .paths
            .push(SearchPath::FirstParty(root.to_path_buf()));

        let discovered_site_packages = interpreter.site_packages_path(fs, root);

        for path in pythonpath {
            if !fs.is_dir(path) || search_paths.contains_path(path) {
                continue;
            }

            let search_path = SearchPath::from_pythonpath(
                root,
                discovered_site_packages.as_deref(),
                path.clone(),
            );
            let site_packages = match &search_path {
                SearchPath::SitePackages(path) => Some(path.clone()),
                _ => None,
            };
            search_paths.paths.push(search_path);
            if let Some(site_packages) = site_packages {
                search_paths.add_pth_editable_roots(fs, &site_packages);
            }
        }

        if let Some(site_packages) = discovered_site_packages
            && !search_paths.contains_path(&site_packages)
        {
            search_paths
                .paths
                .push(SearchPath::SitePackages(site_packages.clone()));
            search_paths.add_pth_editable_roots(fs, &site_packages);
        }

        search_paths
    }

    pub fn register_roots(&self, db: &dyn ProjectDb) {
        let first_party_paths = self
            .iter()
            .filter(|search_path| search_path.is_first_party())
            .map(SearchPath::path)
            .collect::<Vec<_>>();

        let mut roots = Vec::new();
        for search_path in self.iter() {
            if search_path.is_first_party()
                && first_party_paths
                    .iter()
                    .any(|path| *path != search_path.path() && search_path.path().starts_with(path))
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

    fn add_pth_editable_roots(&mut self, fs: &dyn FileSystem, site_packages: &Utf8Path) {
        let Ok(entries) = fs.walk_entries(site_packages, &WalkOptions::shallow()) else {
            return;
        };

        let mut pth_files: Vec<_> = entries
            .into_iter()
            .filter(|entry| entry.kind == WalkEntryKind::File)
            .filter(|entry| entry.path.extension() == Some("pth"))
            .collect();
        pth_files.sort_by(|left, right| left.path.cmp(&right.path));

        for pth_file in pth_files {
            let Ok(contents) = fs.read_to_string(&pth_file.path) else {
                continue;
            };

            for line in contents.lines() {
                let line = line.trim_end();
                if line.is_empty()
                    || line.starts_with('#')
                    || line.starts_with("import ")
                    || line.starts_with("import\t")
                {
                    continue;
                }

                let path = Utf8Path::new(line);
                let path = if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    site_packages.join(path).clean()
                };
                if fs.is_dir(&path) && !self.contains_path(&path) {
                    self.paths.push(SearchPath::Editable(path));
                }
            }
        }
    }
}
