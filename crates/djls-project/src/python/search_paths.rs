use std::cmp::Ordering;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_source::FileRootKind;
use djls_source::FileSystem;
use djls_source::RootWalk;
use djls_source::Utf8PathClean;
use djls_source::WalkEntryKind;
use djls_source::WalkOptions;

use crate::db::Db as ProjectDb;
use crate::python::PythonEnvironment;
use crate::python::evaluation::StructuralOrd;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SearchPath {
    FirstParty(Utf8PathBuf),
    Extra(Utf8PathBuf),
    SitePackages(Utf8PathBuf),
    Editable(Utf8PathBuf),
}

impl StructuralOrd for SearchPath {
    fn structural_cmp(&self, other: &Self) -> Ordering {
        self.structural_rank()
            .cmp(&other.structural_rank())
            .then_with(|| self.path().cmp(other.path()))
    }
}

impl SearchPath {
    /// Structural precedence preserves the former diagnostic-name order while
    /// remaining separate from resolver precedence in [`SearchPaths`].
    fn structural_rank(&self) -> u8 {
        match self {
            Self::Editable(_) => 0,
            Self::Extra(_) => 1,
            Self::FirstParty(_) => 2,
            Self::SitePackages(_) => 3,
        }
    }

    fn from_pythonpath(
        root: &Utf8Path,
        discovered_site_packages: &[Utf8PathBuf],
        path: Utf8PathBuf,
    ) -> Self {
        if discovered_site_packages.contains(&path)
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

    pub(crate) fn is_project_code(&self) -> bool {
        matches!(self, Self::FirstParty(_) | Self::Extra(_))
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

    /// Build search paths from a layout the caller already knows. No discovery,
    /// no `.pth` processing. Test scaffolding uses this to describe a project
    /// whose dependencies were placed by hand rather than installed.
    #[must_use]
    pub fn from_paths(paths: Vec<SearchPath>) -> Self {
        Self { paths }
    }

    #[must_use]
    pub fn from_project_settings(
        fs: &dyn FileSystem,
        root: &Utf8Path,
        python_environment: &PythonEnvironment,
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

        let discovered_site_packages = python_environment.site_packages_paths(fs, root);
        if discovered_site_packages.is_empty() {
            match python_environment {
                PythonEnvironment::Path(venv_path) => {
                    tracing::warn!(
                        "Could not discover site-packages under configured venv_path \
                         '{venv_path}'; expected a conventional Python environment layout; \
                         continuing with project and configured pythonpath roots"
                    );
                }
                PythonEnvironment::Auto => {
                    tracing::debug!(
                        "No virtual-environment site-packages discovered for project {root}; \
                         continuing with project and configured pythonpath roots"
                    );
                }
            }
        } else {
            for site_packages in &discovered_site_packages {
                tracing::debug!("Using discovered site-packages search path: {site_packages}");
            }
        }

        let mut processed_site_packages = Vec::new();
        for configured_path in pythonpath {
            let resolved_path = if configured_path.is_relative() {
                root.join(configured_path)
            } else {
                configured_path.clone()
            };
            if !fs.is_dir(&resolved_path) {
                continue;
            }

            let search_path =
                SearchPath::from_pythonpath(root, &discovered_site_packages, resolved_path.clone());
            if let Some(existing) = search_paths
                .paths
                .iter_mut()
                .find(|existing| existing.path() == resolved_path)
            {
                if matches!(existing, SearchPath::Editable(_)) {
                    *existing = search_path;
                }
                continue;
            }
            let site_packages = match &search_path {
                SearchPath::SitePackages(path) => Some(path.clone()),
                SearchPath::FirstParty(_) | SearchPath::Extra(_) | SearchPath::Editable(_) => None,
            };
            search_paths.paths.push(search_path);
            if let Some(site_packages) = site_packages {
                search_paths.add_pth_editable_roots(fs, &site_packages);
                processed_site_packages.push(site_packages);
            }
        }

        for site_packages in discovered_site_packages {
            if !search_paths.contains_path(&site_packages) {
                search_paths
                    .paths
                    .push(SearchPath::SitePackages(site_packages.clone()));
            }
            if !processed_site_packages.contains(&site_packages) {
                search_paths.add_pth_editable_roots(fs, &site_packages);
                processed_site_packages.push(site_packages);
            }
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
        let RootWalk::Directory { entries, .. } =
            fs.walk_root(site_packages, &WalkOptions::shallow())
        else {
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

#[cfg(test)]
mod tests {
    use std::cmp::Ordering;

    use camino::Utf8Path;
    use camino::Utf8PathBuf;
    use djls_source::InMemoryFileSystem;

    use super::PythonEnvironment;
    use super::SearchPath;
    use super::SearchPaths;
    use super::StructuralOrd;

    #[test]
    fn every_discovered_site_directory_processes_pth_files() {
        let mut fs = InMemoryFileSystem::new();
        fs.add_file(
            "/env/lib/python3.12/site-packages/next.pth".into(),
            "/env/lib64/python3.12/site-packages\n".into(),
        );
        fs.add_file(
            "/env/lib64/python3.12/site-packages/editable.pth".into(),
            "/editable\n".into(),
        );
        fs.add_file("/editable/package.py".into(), String::new());

        let paths = SearchPaths::from_project_settings(
            &fs,
            Utf8Path::new("/project"),
            &PythonEnvironment::Path(Utf8PathBuf::from("/env")),
            &[],
        )
        .iter()
        .cloned()
        .collect::<Vec<_>>();

        assert!(paths.contains(&SearchPath::Editable(Utf8PathBuf::from("/editable"))));
    }

    #[test]
    fn typed_module_order_search_path_variants_are_distinct_and_total() {
        let path = Utf8PathBuf::from("/shared");
        let paths = [
            SearchPath::Editable(path.clone()),
            SearchPath::Extra(path.clone()),
            SearchPath::FirstParty(path.clone()),
            SearchPath::SitePackages(path),
        ];

        for (left_index, left) in paths.iter().enumerate() {
            for (right_index, right) in paths.iter().enumerate() {
                let ordering = left.structural_cmp(right);
                assert_eq!(ordering, right.structural_cmp(left).reverse());
                assert_eq!(ordering == Ordering::Equal, left == right);
                assert_eq!(ordering, left_index.cmp(&right_index));
            }
        }
    }

    #[test]
    fn typed_module_order_search_paths_compare_variant_before_path() {
        let later_editable = SearchPath::Editable(Utf8PathBuf::from("/z"));
        let earlier_extra = SearchPath::Extra(Utf8PathBuf::from("/a"));
        let earlier_editable = SearchPath::Editable(Utf8PathBuf::from("/a"));

        assert_eq!(
            later_editable.structural_cmp(&earlier_extra),
            Ordering::Less
        );
        assert_eq!(
            earlier_editable.structural_cmp(&later_editable),
            Ordering::Less
        );
    }
}
