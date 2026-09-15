use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_source::FileSystem;
use djls_source::RootWalk;
use djls_source::WalkEntryKind;
use djls_source::WalkOptions;

struct PythonLayoutSelection {
    version: (u32, u32),
    directory: Option<String>,
}

/// Selection used to discover the Python environment's import roots.
///
/// A selected path may be a Python executable, virtual-environment root, or
/// system `sys.prefix`. DJLS inspects its filesystem layout but never executes
/// Python.
#[derive(Clone, Debug, PartialEq)]
pub enum PythonEnvironment {
    /// Discover a project-local environment, then active environment variables.
    Auto,
    /// Inspect a specific executable, environment root, or `sys.prefix`.
    Path(Utf8PathBuf),
}

impl PythonEnvironment {
    /// Use an explicitly selected Python environment or automatic discovery.
    #[must_use]
    pub fn discover(python: Option<&Utf8Path>) -> Self {
        python.map_or(Self::Auto, |path| Self::Path(path.to_path_buf()))
    }

    pub(crate) fn site_packages_paths(
        &self,
        fs: &dyn FileSystem,
        project_root: &Utf8Path,
    ) -> Vec<Utf8PathBuf> {
        match self {
            Self::Path(path) => Self::site_packages_paths_from_selection(fs, path),
            Self::Auto => {
                let virtual_env = std::env::var("VIRTUAL_ENV").ok().map(Utf8PathBuf::from);
                let conda_prefix = std::env::var("CONDA_PREFIX").ok().map(Utf8PathBuf::from);
                let path = std::env::var("PATH").ok();
                Self::auto_site_packages_paths(
                    fs,
                    project_root,
                    virtual_env.as_deref(),
                    conda_prefix.as_deref(),
                    path.as_deref(),
                )
            }
        }
    }

    fn auto_site_packages_paths(
        fs: &dyn FileSystem,
        project_root: &Utf8Path,
        virtual_env: Option<&Utf8Path>,
        conda_prefix: Option<&Utf8Path>,
        path: Option<&str>,
    ) -> Vec<Utf8PathBuf> {
        [".venv", "venv", "env", ".env"]
            .into_iter()
            .map(|dir| project_root.join(dir))
            .find_map(|environment| {
                let paths = Self::site_packages_paths_from_prefix(fs, &environment, None);
                (!paths.is_empty()).then_some(paths)
            })
            .or_else(|| virtual_env.map(|path| Self::site_packages_paths_from_selection(fs, path)))
            .filter(|paths| !paths.is_empty())
            .or_else(|| conda_prefix.map(|path| Self::site_packages_paths_from_selection(fs, path)))
            .filter(|paths| !paths.is_empty())
            .or_else(|| Self::site_packages_paths_on_path(fs, path?))
            .unwrap_or_default()
    }

    fn site_packages_paths_on_path(fs: &dyn FileSystem, path: &str) -> Option<Vec<Utf8PathBuf>> {
        let executable_names: &[&str] = if cfg!(windows) {
            &["python3.exe", "python.exe"]
        } else {
            &["python3", "python"]
        };
        executable_names.iter().find_map(|name| {
            std::env::split_paths(path).find_map(|directory| {
                let directory = Utf8PathBuf::from_path_buf(directory).ok()?;
                let candidate = directory.join(name);
                if !fs.is_file(&candidate) {
                    return None;
                }
                let paths = Self::site_packages_paths_from_selection(fs, &candidate);
                (!paths.is_empty()).then_some(paths)
            })
        })
    }

    fn site_packages_paths_from_selection(
        fs: &dyn FileSystem,
        selection: &Utf8Path,
    ) -> Vec<Utf8PathBuf> {
        let is_executable = fs.is_file(selection) && Self::looks_like_python_executable(selection);
        let Some(logical_prefix) = Self::prefix_from_selection(fs, selection) else {
            return Vec::new();
        };
        if !is_executable {
            return Self::site_packages_paths_from_prefix(fs, &logical_prefix, None);
        }

        let canonical_executable = fs.canonicalize(selection).ok();
        let layout = canonical_executable
            .as_deref()
            .and_then(Self::layout_from_executable)
            .or_else(|| Self::layout_from_executable(selection));
        let logical_paths =
            Self::site_packages_paths_from_prefix(fs, &logical_prefix, layout.as_ref());
        if !logical_paths.is_empty() || fs.is_file(&logical_prefix.join("pyvenv.cfg")) {
            return logical_paths;
        }

        canonical_executable
            .as_deref()
            .and_then(|path| Self::prefix_from_selection(fs, path))
            .filter(|prefix| prefix != &logical_prefix)
            .map_or(logical_paths, |prefix| {
                Self::site_packages_paths_from_prefix(fs, &prefix, layout.as_ref())
            })
    }

    fn layout_from_executable(executable: &Utf8Path) -> Option<PythonLayoutSelection> {
        let name = executable.file_name()?.to_ascii_lowercase();
        let name = name.strip_suffix(".exe").unwrap_or(&name);
        Some(PythonLayoutSelection {
            version: Self::python_version(name)?,
            directory: Some(name.to_string()),
        })
    }

    fn prefix_from_selection(fs: &dyn FileSystem, selection: &Utf8Path) -> Option<Utf8PathBuf> {
        if fs.is_dir(selection) {
            return Some(selection.to_path_buf());
        }
        if !fs.is_file(selection) || !Self::looks_like_python_executable(selection) {
            return None;
        }

        let parent = selection.parent()?;
        if parent.file_name().is_some_and(|name| {
            name.eq_ignore_ascii_case("bin") || name.eq_ignore_ascii_case("scripts")
        }) {
            parent.parent().map(Utf8Path::to_path_buf)
        } else {
            Some(parent.to_path_buf())
        }
    }

    fn looks_like_python_executable(path: &Utf8Path) -> bool {
        let Some(name) = path.file_stem() else {
            return false;
        };
        let name = name.to_ascii_lowercase();
        ["python", "pypy", "graalpy"]
            .iter()
            .any(|prefix| name.starts_with(prefix))
    }

    fn site_packages_paths_from_prefix(
        fs: &dyn FileSystem,
        prefix: &Utf8Path,
        preferred_layout: Option<&PythonLayoutSelection>,
    ) -> Vec<Utf8PathBuf> {
        let pyvenv_cfg = prefix.join("pyvenv.cfg");
        let mut include_system = false;
        let mut home = None;
        let mut configured_version = None;
        if let Ok(contents) = fs.read_to_string(&pyvenv_cfg) {
            for line in contents.lines() {
                let Some((key, value)) = line.split_once('=') else {
                    continue;
                };
                match key.trim().to_ascii_lowercase().as_str() {
                    "include-system-site-packages" => {
                        include_system = value.trim().eq_ignore_ascii_case("true");
                    }
                    "home" => home = Some(Utf8PathBuf::from(value.trim())),
                    "version" | "version_info" => {
                        configured_version = Self::parse_version(value.trim());
                    }
                    _ => {}
                }
            }
        }

        let configured_layout = configured_version.map(|version| PythonLayoutSelection {
            version,
            directory: None,
        });
        let selected_layout = preferred_layout.or(configured_layout.as_ref());
        let mut paths = Self::local_site_packages_paths(fs, prefix, selected_layout);
        if include_system
            && let Some(home) = home
            && let Some(base_prefix) =
                Self::prefix_from_home(fs.canonicalize(&home).as_deref().unwrap_or(&home))
        {
            for path in Self::local_site_packages_paths(fs, &base_prefix, selected_layout) {
                if !paths.contains(&path) {
                    paths.push(path);
                }
            }
        }

        paths
    }

    fn prefix_from_home(home: &Utf8Path) -> Option<Utf8PathBuf> {
        if home.file_name().is_some_and(|name| {
            name.eq_ignore_ascii_case("bin") || name.eq_ignore_ascii_case("scripts")
        }) {
            home.parent().map(Utf8Path::to_path_buf)
        } else {
            Some(home.to_path_buf())
        }
    }

    fn local_site_packages_paths(
        fs: &dyn FileSystem,
        prefix: &Utf8Path,
        preferred_layout: Option<&PythonLayoutSelection>,
    ) -> Vec<Utf8PathBuf> {
        let windows = prefix.join("Lib").join("site-packages");
        if std::env::consts::OS == "windows" && fs.is_dir(&windows) {
            return vec![windows];
        }

        let is_usr = prefix == Utf8Path::new("/usr");
        let mut layouts = Vec::new();
        let mut library_dirs = vec![(prefix.join("lib"), true), (prefix.join("lib64"), true)];
        if is_usr {
            library_dirs.insert(0, (Utf8PathBuf::from("/usr/local/lib"), false));
        }
        for (library_dir, include_site_packages) in library_dirs {
            let RootWalk::Directory { entries, .. } =
                fs.walk_root(&library_dir, &WalkOptions::shallow())
            else {
                continue;
            };
            for entry in entries {
                if entry.kind != WalkEntryKind::Directory {
                    continue;
                }
                let Some(name) = entry.path.file_name() else {
                    continue;
                };
                let Some(version) = Self::python_version(name) else {
                    continue;
                };
                if (include_site_packages && fs.is_dir(&entry.path.join("site-packages")))
                    || fs.is_dir(&entry.path.join("dist-packages"))
                {
                    layouts.push((version, name.to_string()));
                }
            }
        }

        layouts.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
        layouts.dedup();
        let selected_directory = Self::select_layout_directory(&layouts, preferred_layout);

        let mut paths = Vec::new();
        if let Some(version_dir) = selected_directory {
            if is_usr {
                let local_dist_packages = Utf8Path::new("/usr/local/lib")
                    .join(&version_dir)
                    .join("dist-packages");
                if fs.is_dir(&local_dist_packages) {
                    paths.push(local_dist_packages);
                }
            }

            for library_dir in ["lib", "lib64"] {
                for packages_dir in ["site-packages", "dist-packages"] {
                    let path = prefix
                        .join(library_dir)
                        .join(&version_dir)
                        .join(packages_dir);
                    if fs.is_dir(&path) && !paths.contains(&path) {
                        paths.push(path);
                    }
                }
            }
        }

        if is_usr {
            let shared_dist_packages = prefix.join("lib/python3/dist-packages");
            if fs.is_dir(&shared_dist_packages) {
                paths.push(shared_dist_packages);
            }
        }
        if paths.is_empty() && fs.is_dir(&windows) {
            paths.push(windows);
        }
        paths
    }

    fn select_layout_directory(
        layouts: &[((u32, u32), String)],
        preferred_layout: Option<&PythonLayoutSelection>,
    ) -> Option<String> {
        if let Some(preferred) = preferred_layout {
            preferred.directory.as_ref().map_or_else(
                || {
                    let regular = format!("python{}.{}", preferred.version.0, preferred.version.1);
                    layouts
                        .iter()
                        .find(|(version, directory)| {
                            *version == preferred.version && directory == &regular
                        })
                        .or_else(|| {
                            layouts
                                .iter()
                                .find(|(version, _)| *version == preferred.version)
                        })
                        .map(|(_, directory)| directory.clone())
                },
                |directory| {
                    layouts
                        .iter()
                        .any(|(version, candidate)| {
                            *version == preferred.version && candidate == directory
                        })
                        .then(|| directory.clone())
                },
            )
        } else {
            layouts.last().map(|(version, directory)| {
                let regular = format!("python{}.{}", version.0, version.1);
                layouts
                    .iter()
                    .find(|(candidate_version, candidate)| {
                        candidate_version == version && candidate == &regular
                    })
                    .map_or_else(|| directory.clone(), |(_, candidate)| candidate.clone())
            })
        }
    }

    fn python_version(name: &str) -> Option<(u32, u32)> {
        let suffix = ["python", "pypy"]
            .iter()
            .find_map(|prefix| name.strip_prefix(prefix))?;
        let (major, minor) = suffix.split_once('.')?;
        let minor: String = minor.chars().take_while(char::is_ascii_digit).collect();
        (!minor.is_empty()).then(|| Some((major.parse().ok()?, minor.parse().ok()?)))?
    }

    fn parse_version(value: &str) -> Option<(u32, u32)> {
        let mut components = value.split('.');
        Some((
            components.next()?.parse().ok()?,
            components.next()?.parse().ok()?,
        ))
    }
}

#[cfg(test)]
mod tests {
    use camino::Utf8Path;

    use super::*;

    mod discovery {
        use super::*;

        #[test]
        fn explicit_python_path() {
            let environment = PythonEnvironment::discover(Some(Utf8Path::new("/path/to/python")));
            assert_eq!(
                environment,
                PythonEnvironment::Path(Utf8PathBuf::from("/path/to/python"))
            );
        }

        #[test]
        fn auto_without_explicit_path() {
            assert_eq!(PythonEnvironment::discover(None), PythonEnvironment::Auto);
        }
    }

    mod resolution {
        use super::*;

        #[test]
        fn auto_prefers_project_venv_over_virtual_env() {
            let mut fs = djls_source::InMemoryFileSystem::new();
            fs.add_file(
                "/project/.venv/lib/python3.12/site-packages/django/__init__.py".into(),
                String::new(),
            );
            fs.add_file(
                "/hook/lib/python3.14/site-packages/django_language_server/__init__.py".into(),
                String::new(),
            );

            let site_packages = PythonEnvironment::auto_site_packages_paths(
                &fs,
                Utf8Path::new("/project"),
                Some(Utf8Path::new("/hook")),
                None,
                None,
            );

            assert_eq!(
                site_packages,
                vec![Utf8PathBuf::from(
                    "/project/.venv/lib/python3.12/site-packages"
                )]
            );
        }

        #[test]
        fn auto_falls_back_to_virtual_env_without_project_venv() {
            let mut fs = djls_source::InMemoryFileSystem::new();
            fs.add_file(
                "/hook/lib/python3.14/site-packages/django/__init__.py".into(),
                String::new(),
            );

            let site_packages = PythonEnvironment::auto_site_packages_paths(
                &fs,
                Utf8Path::new("/project"),
                Some(Utf8Path::new("/hook")),
                None,
                None,
            );

            assert_eq!(
                site_packages,
                vec![Utf8PathBuf::from("/hook/lib/python3.14/site-packages")]
            );
        }

        #[test]
        fn auto_skips_unusable_project_venv_before_virtual_env() {
            let mut fs = djls_source::InMemoryFileSystem::new();
            fs.add_file("/project/.venv/pyvenv.cfg".into(), String::new());
            fs.add_file(
                "/hook/lib/python3.14/site-packages/django/__init__.py".into(),
                String::new(),
            );

            let site_packages = PythonEnvironment::auto_site_packages_paths(
                &fs,
                Utf8Path::new("/project"),
                Some(Utf8Path::new("/hook")),
                None,
                None,
            );

            assert_eq!(
                site_packages,
                vec![Utf8PathBuf::from("/hook/lib/python3.14/site-packages")]
            );
        }

        #[test]
        fn environment_root_finds_posix_layout() {
            let mut fs = djls_source::InMemoryFileSystem::new();
            fs.add_file(
                "/venv/lib/python3.12/site-packages/django/__init__.py".into(),
                String::new(),
            );

            let site_packages =
                PythonEnvironment::site_packages_paths_from_selection(&fs, Utf8Path::new("/venv"));

            assert_eq!(
                site_packages,
                vec![Utf8PathBuf::from("/venv/lib/python3.12/site-packages")]
            );
        }

        #[test]
        fn environment_root_finds_windows_layout() {
            let mut fs = djls_source::InMemoryFileSystem::new();
            fs.add_file(
                "/venv/Lib/site-packages/django/__init__.py".into(),
                String::new(),
            );

            let site_packages =
                PythonEnvironment::site_packages_paths_from_selection(&fs, Utf8Path::new("/venv"));

            assert_eq!(
                site_packages,
                vec![Utf8PathBuf::from("/venv/Lib/site-packages")]
            );
        }

        #[test]
        fn environment_root_uses_platform_layout_before_fallback() {
            let mut fs = djls_source::InMemoryFileSystem::new();
            fs.add_file(
                "/venv/lib/python3.12/site-packages/posix/__init__.py".into(),
                String::new(),
            );
            fs.add_file(
                "/venv/Lib/site-packages/windows/__init__.py".into(),
                String::new(),
            );

            let site_packages =
                PythonEnvironment::site_packages_paths_from_selection(&fs, Utf8Path::new("/venv"));
            let expected = if std::env::consts::OS == "windows" {
                Utf8PathBuf::from("/venv/Lib/site-packages")
            } else {
                Utf8PathBuf::from("/venv/lib/python3.12/site-packages")
            };

            assert_eq!(site_packages, vec![expected]);
        }

        #[test]
        fn executable_path_identifies_environment_without_execution() {
            let mut fs = djls_source::InMemoryFileSystem::new();
            fs.add_file("/venv/bin/python".into(), String::new());
            fs.add_file(
                "/venv/lib/python3.12/site-packages/django/__init__.py".into(),
                String::new(),
            );

            assert_eq!(
                PythonEnvironment::site_packages_paths_from_selection(
                    &fs,
                    Utf8Path::new("/venv/bin/python"),
                ),
                vec![Utf8PathBuf::from("/venv/lib/python3.12/site-packages")]
            );
        }

        #[test]
        fn executable_version_selects_matching_prefix_layout() {
            let mut fs = djls_source::InMemoryFileSystem::new();
            fs.add_file("/usr/bin/python3.11".into(), String::new());
            fs.add_file(
                "/usr/lib/python3.11/site-packages/selected.py".into(),
                String::new(),
            );
            fs.add_file(
                "/usr/lib/python3.12/site-packages/newer.py".into(),
                String::new(),
            );

            assert_eq!(
                PythonEnvironment::site_packages_paths_from_selection(
                    &fs,
                    Utf8Path::new("/usr/bin/python3.11"),
                ),
                vec![Utf8PathBuf::from("/usr/lib/python3.11/site-packages")]
            );
        }

        #[test]
        fn executable_version_does_not_fall_back_to_another_minor() {
            let mut fs = djls_source::InMemoryFileSystem::new();
            fs.add_file("/prefix/bin/python3.11".into(), String::new());
            fs.add_file(
                "/prefix/lib/python3.12/site-packages/newer.py".into(),
                String::new(),
            );

            assert!(
                PythonEnvironment::site_packages_paths_from_selection(
                    &fs,
                    Utf8Path::new("/prefix/bin/python3.11"),
                )
                .is_empty()
            );
        }

        #[test]
        fn executable_version_takes_precedence_over_pyvenv_metadata() {
            let mut fs = djls_source::InMemoryFileSystem::new();
            fs.add_file("/venv/bin/python3.11".into(), String::new());
            fs.add_file("/venv/pyvenv.cfg".into(), "version = 3.12.1\n".into());
            fs.add_file(
                "/venv/lib/python3.11/site-packages/selected.py".into(),
                String::new(),
            );
            fs.add_file(
                "/venv/lib/python3.12/site-packages/metadata.py".into(),
                String::new(),
            );

            assert_eq!(
                PythonEnvironment::site_packages_paths_from_selection(
                    &fs,
                    Utf8Path::new("/venv/bin/python3.11"),
                ),
                vec![Utf8PathBuf::from("/venv/lib/python3.11/site-packages")]
            );
        }

        #[test]
        fn unknown_version_selects_highest_package_bearing_layout() {
            let mut fs = djls_source::InMemoryFileSystem::new();
            fs.add_file(
                "/prefix/lib/python3.12/site-packages/package.py".into(),
                String::new(),
            );
            fs.add_file("/prefix/lib/python3.13/os.py".into(), String::new());

            assert_eq!(
                PythonEnvironment::site_packages_paths_from_selection(
                    &fs,
                    Utf8Path::new("/prefix"),
                ),
                vec![Utf8PathBuf::from("/prefix/lib/python3.12/site-packages")]
            );
        }

        #[test]
        fn unknown_version_preserves_free_threaded_layout() {
            let mut fs = djls_source::InMemoryFileSystem::new();
            fs.add_file(
                "/prefix/lib/python3.13t/site-packages/package.py".into(),
                String::new(),
            );

            assert_eq!(
                PythonEnvironment::site_packages_paths_from_selection(
                    &fs,
                    Utf8Path::new("/prefix"),
                ),
                vec![Utf8PathBuf::from("/prefix/lib/python3.13t/site-packages")]
            );
        }

        #[test]
        fn pyvenv_version_uses_free_threaded_layout_when_regular_is_absent() {
            let mut fs = djls_source::InMemoryFileSystem::new();
            fs.add_file("/venv/pyvenv.cfg".into(), "version = 3.13.1\n".into());
            fs.add_file(
                "/venv/lib/python3.13t/site-packages/package.py".into(),
                String::new(),
            );

            assert_eq!(
                PythonEnvironment::site_packages_paths_from_selection(&fs, Utf8Path::new("/venv")),
                vec![Utf8PathBuf::from("/venv/lib/python3.13t/site-packages")]
            );
        }

        #[test]
        fn free_threaded_executable_selects_matching_layout() {
            let mut fs = djls_source::InMemoryFileSystem::new();
            fs.add_file("/prefix/bin/python3.13t".into(), String::new());
            fs.add_file(
                "/prefix/lib/python3.13/site-packages/regular.py".into(),
                String::new(),
            );
            fs.add_file(
                "/prefix/lib/python3.13t/site-packages/threaded.py".into(),
                String::new(),
            );

            assert_eq!(
                PythonEnvironment::site_packages_paths_from_selection(
                    &fs,
                    Utf8Path::new("/prefix/bin/python3.13t"),
                ),
                vec![Utf8PathBuf::from("/prefix/lib/python3.13t/site-packages")]
            );
        }

        #[test]
        fn usr_prefix_includes_debian_package_roots_in_import_order() {
            let mut fs = djls_source::InMemoryFileSystem::new();
            fs.add_file("/usr/bin/python3.12".into(), String::new());
            fs.add_file(
                "/usr/local/lib/python3.12/dist-packages/local.py".into(),
                String::new(),
            );
            fs.add_file(
                "/usr/lib/python3.12/site-packages/upstream.py".into(),
                String::new(),
            );
            fs.add_file(
                "/usr/lib/python3/dist-packages/debian.py".into(),
                String::new(),
            );

            assert_eq!(
                PythonEnvironment::site_packages_paths_from_selection(
                    &fs,
                    Utf8Path::new("/usr/bin/python3.12"),
                ),
                vec![
                    Utf8PathBuf::from("/usr/local/lib/python3.12/dist-packages"),
                    Utf8PathBuf::from("/usr/lib/python3.12/site-packages"),
                    Utf8PathBuf::from("/usr/lib/python3/dist-packages"),
                ]
            );
        }

        #[test]
        fn usr_unknown_version_ignores_ineligible_local_site_packages() {
            let mut fs = djls_source::InMemoryFileSystem::new();
            fs.add_file(
                "/usr/local/lib/python3.12/dist-packages/selected.py".into(),
                String::new(),
            );
            fs.add_file(
                "/usr/local/lib/python3.13/site-packages/ineligible.py".into(),
                String::new(),
            );

            assert_eq!(
                PythonEnvironment::site_packages_paths_from_selection(&fs, Utf8Path::new("/usr")),
                vec![Utf8PathBuf::from("/usr/local/lib/python3.12/dist-packages")]
            );
        }

        #[cfg(unix)]
        #[test]
        fn executable_symlink_uses_canonical_installation_when_anchor_is_unusable() {
            use std::os::unix::fs::symlink;

            let temporary = tempfile::tempdir().expect("temporary directory should be created");
            let root = Utf8Path::from_path(temporary.path())
                .expect("temporary directory should have a UTF-8 path");
            let installation = root.join("installation");
            let executable = installation.join("bin/python3.11");
            let packages = installation.join("lib/python3.11/site-packages");
            let link = root.join("links/python3");
            std::fs::create_dir_all(packages.as_std_path())
                .expect("site-packages directory should be created");
            std::fs::create_dir_all(installation.join("bin").as_std_path())
                .expect("installation bin directory should be created");
            std::fs::create_dir_all(root.join("links").as_std_path())
                .expect("link directory should be created");
            std::fs::write(executable.as_std_path(), "")
                .expect("Python executable fixture should be created");
            symlink(executable.as_std_path(), link.as_std_path())
                .expect("Python executable symlink should be created");

            assert_eq!(
                PythonEnvironment::site_packages_paths_from_selection(
                    &djls_source::OsFileSystem::default(),
                    &link,
                ),
                vec![packages]
            );
        }

        #[cfg(unix)]
        #[test]
        fn venv_executable_symlink_preserves_logical_environment() {
            use std::os::unix::fs::symlink;

            let temporary = tempfile::tempdir().expect("temporary directory should be created");
            let root = Utf8Path::from_path(temporary.path())
                .expect("temporary directory should have a UTF-8 path");
            let base_executable = root.join("base/bin/python3.11");
            let base_packages = root.join("base/lib/python3.11/site-packages");
            let venv_executable = root.join("venv/bin/python");
            let venv_packages = root.join("venv/lib/python3.11/site-packages");
            std::fs::create_dir_all(base_packages.as_std_path())
                .expect("base site-packages directory should be created");
            std::fs::create_dir_all(venv_packages.as_std_path())
                .expect("venv site-packages directory should be created");
            std::fs::create_dir_all(root.join("base/bin").as_std_path())
                .expect("base bin directory should be created");
            std::fs::create_dir_all(root.join("venv/bin").as_std_path())
                .expect("venv bin directory should be created");
            std::fs::write(base_executable.as_std_path(), "")
                .expect("base Python executable fixture should be created");
            std::fs::write(root.join("venv/pyvenv.cfg"), "version = 3.11.1\n")
                .expect("pyvenv.cfg fixture should be created");
            symlink(base_executable.as_std_path(), venv_executable.as_std_path())
                .expect("venv Python executable symlink should be created");

            assert_eq!(
                PythonEnvironment::site_packages_paths_from_selection(
                    &djls_source::OsFileSystem::default(),
                    &venv_executable,
                ),
                vec![venv_packages]
            );
        }

        #[test]
        fn auto_falls_back_to_python_on_path() {
            let mut fs = djls_source::InMemoryFileSystem::new();
            let unusable_executable = if cfg!(windows) {
                "/shim/python3.exe"
            } else {
                "/shim/python3"
            };
            let executable = if cfg!(windows) {
                "/tools/python3.exe"
            } else {
                "/tools/python3"
            };
            fs.add_file(unusable_executable.into(), String::new());
            fs.add_file(executable.into(), String::new());
            fs.add_file("/tools/Lib/site-packages/windows.py".into(), String::new());
            fs.add_file(
                "/tools/lib/python3.12/site-packages/posix.py".into(),
                String::new(),
            );

            let expected = if cfg!(windows) {
                "/tools/Lib/site-packages"
            } else {
                "/tools/lib/python3.12/site-packages"
            };
            let path = if cfg!(windows) {
                "/missing;/shim;/tools"
            } else {
                "/missing:/shim:/tools"
            };
            assert_eq!(
                PythonEnvironment::auto_site_packages_paths(
                    &fs,
                    Utf8Path::new("/project"),
                    None,
                    None,
                    Some(path),
                ),
                vec![Utf8PathBuf::from(expected)]
            );
        }

        #[test]
        fn prefix_includes_lib64_and_dist_packages_for_selected_version() {
            let mut fs = djls_source::InMemoryFileSystem::new();
            fs.add_file(
                "/prefix/lib/python3.11/site-packages/old.py".into(),
                String::new(),
            );
            fs.add_file(
                "/prefix/lib/python3.12/dist-packages/debian.py".into(),
                String::new(),
            );
            fs.add_file(
                "/prefix/lib64/python3.12/site-packages/package.py".into(),
                String::new(),
            );

            assert_eq!(
                PythonEnvironment::site_packages_paths_from_selection(
                    &fs,
                    Utf8Path::new("/prefix"),
                ),
                vec![
                    Utf8PathBuf::from("/prefix/lib/python3.12/dist-packages"),
                    Utf8PathBuf::from("/prefix/lib64/python3.12/site-packages"),
                ]
            );
        }

        #[test]
        fn pyvenv_cfg_can_include_base_environment_packages() {
            let mut fs = djls_source::InMemoryFileSystem::new();
            fs.add_file(
                "/venv/pyvenv.cfg".into(),
                "home = /usr/bin\ninclude-system-site-packages = true\nversion = 3.12.1\n".into(),
            );
            fs.add_file(
                "/venv/lib/python3.12/site-packages/local.py".into(),
                String::new(),
            );
            fs.add_file(
                "/usr/lib/python3/dist-packages/system.py".into(),
                String::new(),
            );

            assert_eq!(
                PythonEnvironment::site_packages_paths_from_selection(&fs, Utf8Path::new("/venv"),),
                vec![
                    Utf8PathBuf::from("/venv/lib/python3.12/site-packages"),
                    Utf8PathBuf::from("/usr/lib/python3/dist-packages"),
                ]
            );
        }
    }
}
