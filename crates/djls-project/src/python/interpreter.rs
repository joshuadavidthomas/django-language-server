use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_source::FileSystem;
use djls_source::RootWalk;
use djls_source::WalkEntryKind;
use djls_source::WalkOptions;

/// Interpreter specification for Python environment discovery.
///
/// This enum represents the different ways to specify which Python interpreter
/// to use for a project.
#[derive(Clone, Debug, PartialEq)]
pub enum Interpreter {
    /// Automatically discover a project-local interpreter, then fall back to
    /// `VIRTUAL_ENV`.
    Auto,
    /// Use a specific virtual environment path.
    VenvPath(Utf8PathBuf),
}

impl Interpreter {
    /// Use an explicitly configured environment or automatic discovery.
    #[must_use]
    pub fn discover(venv_path: Option<&Utf8Path>) -> Self {
        venv_path.map_or(Self::Auto, |path| Self::VenvPath(path.to_path_buf()))
    }

    pub(crate) fn site_packages_path(
        &self,
        fs: &dyn FileSystem,
        project_root: &Utf8Path,
    ) -> Option<Utf8PathBuf> {
        match self {
            Self::VenvPath(path) => Self::site_packages_path_in_venv(fs, path),
            Self::Auto => {
                let virtual_env = std::env::var("VIRTUAL_ENV").ok().map(Utf8PathBuf::from);
                Self::auto_site_packages_path(fs, project_root, virtual_env.as_deref())
            }
        }
    }

    fn auto_site_packages_path(
        fs: &dyn FileSystem,
        project_root: &Utf8Path,
        virtual_env: Option<&Utf8Path>,
    ) -> Option<Utf8PathBuf> {
        [".venv", "venv", "env", ".env"]
            .into_iter()
            .map(|dir| project_root.join(dir))
            .find_map(|venv| {
                fs.is_dir(&venv)
                    .then(|| Self::site_packages_path_in_venv(fs, &venv))
                    .flatten()
            })
            .or_else(|| virtual_env.and_then(|path| Self::site_packages_path_in_venv(fs, path)))
    }

    fn site_packages_path_in_venv(fs: &dyn FileSystem, venv: &Utf8Path) -> Option<Utf8PathBuf> {
        let windows_site_packages = venv.join("Lib").join("site-packages");
        if std::env::consts::OS == "windows" && fs.is_dir(&windows_site_packages) {
            return Some(windows_site_packages);
        }

        let lib_dir = venv.join("lib");
        let mut site_packages_directories = Vec::new();
        if let RootWalk::Directory { entries, .. } = fs.walk_root(&lib_dir, &WalkOptions::shallow())
        {
            for entry in entries {
                if entry.kind != WalkEntryKind::Directory {
                    continue;
                }

                let Some(name) = entry.path.file_name() else {
                    continue;
                };
                let Some(version_suffix) = name.strip_prefix("python") else {
                    continue;
                };

                let site_packages = entry.path.join("site-packages");
                if !fs.is_dir(&site_packages) {
                    continue;
                }

                let python_version = if let Some((major, minor_part)) =
                    version_suffix.split_once('.')
                {
                    let minor_digits: String = minor_part
                        .chars()
                        .take_while(char::is_ascii_digit)
                        .collect();
                    match (major.parse::<u32>(), minor_digits.parse::<u32>()) {
                        (Ok(major), Ok(minor)) if !minor_digits.is_empty() => Some((major, minor)),
                        _ => None,
                    }
                } else {
                    None
                };
                site_packages_directories.push((python_version, name.to_string(), site_packages));
            }
        }

        site_packages_directories.sort_by(
            |(left_version, left_name, _), (right_version, right_name, _)| {
                left_version
                    .cmp(right_version)
                    .then_with(|| left_name.cmp(right_name))
            },
        );
        if let Some((_version, _name, site_packages)) = site_packages_directories.pop() {
            return Some(site_packages);
        }

        fs.is_dir(&windows_site_packages)
            .then_some(windows_site_packages)
    }
}

#[cfg(test)]
mod tests {
    use camino::Utf8Path;

    use super::*;

    mod discovery {
        use super::*;

        #[test]
        fn test_discover_with_explicit_venv_path() {
            let interpreter = Interpreter::discover(Some(Utf8Path::new("/path/to/venv")));
            assert_eq!(
                interpreter,
                Interpreter::VenvPath(Utf8PathBuf::from("/path/to/venv"))
            );
        }

        #[test]
        fn test_discover_auto_without_explicit_path() {
            assert_eq!(Interpreter::discover(None), Interpreter::Auto);
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

            let site_packages = Interpreter::auto_site_packages_path(
                &fs,
                Utf8Path::new("/project"),
                Some(Utf8Path::new("/hook")),
            );

            assert_eq!(
                site_packages.as_deref(),
                Some(Utf8Path::new("/project/.venv/lib/python3.12/site-packages"))
            );
        }

        #[test]
        fn auto_falls_back_to_virtual_env_without_project_venv() {
            let mut fs = djls_source::InMemoryFileSystem::new();
            fs.add_file(
                "/hook/lib/python3.14/site-packages/django/__init__.py".into(),
                String::new(),
            );

            let site_packages = Interpreter::auto_site_packages_path(
                &fs,
                Utf8Path::new("/project"),
                Some(Utf8Path::new("/hook")),
            );

            assert_eq!(
                site_packages.as_deref(),
                Some(Utf8Path::new("/hook/lib/python3.14/site-packages"))
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

            let site_packages = Interpreter::auto_site_packages_path(
                &fs,
                Utf8Path::new("/project"),
                Some(Utf8Path::new("/hook")),
            );

            assert_eq!(
                site_packages.as_deref(),
                Some(Utf8Path::new("/hook/lib/python3.14/site-packages"))
            );
        }

        #[test]
        fn site_packages_path_finds_posix_venv_layout() {
            let mut fs = djls_source::InMemoryFileSystem::new();
            fs.add_file(
                "/venv/lib/python3.12/site-packages/django/__init__.py".into(),
                String::new(),
            );

            let site_packages =
                Interpreter::site_packages_path_in_venv(&fs, Utf8Path::new("/venv"));

            assert_eq!(
                site_packages.as_deref(),
                Some(Utf8Path::new("/venv/lib/python3.12/site-packages"))
            );
        }

        #[test]
        fn site_packages_path_finds_windows_venv_layout() {
            let mut fs = djls_source::InMemoryFileSystem::new();
            fs.add_file(
                "/venv/Lib/site-packages/django/__init__.py".into(),
                String::new(),
            );

            let site_packages =
                Interpreter::site_packages_path_in_venv(&fs, Utf8Path::new("/venv"));

            assert_eq!(
                site_packages.as_deref(),
                Some(Utf8Path::new("/venv/Lib/site-packages"))
            );
        }

        #[test]
        fn site_packages_path_uses_platform_layout_before_fallback() {
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
                Interpreter::site_packages_path_in_venv(&fs, Utf8Path::new("/venv"));
            let expected = if std::env::consts::OS == "windows" {
                Utf8Path::new("/venv/Lib/site-packages")
            } else {
                Utf8Path::new("/venv/lib/python3.12/site-packages")
            };

            assert_eq!(site_packages.as_deref(), Some(expected));
        }
    }
}
