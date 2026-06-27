mod modules;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_source::FileSystem;
use djls_source::WalkEntryKind;
use djls_source::WalkOptions;
pub use modules::InvalidModulePath;
pub use modules::PythonModule;
pub use modules::PythonModulePath;

/// Interpreter specification for Python environment discovery.
///
/// This enum represents the different ways to specify which Python interpreter
/// to use for a project.
#[derive(Clone, Debug, PartialEq)]
pub enum Interpreter {
    /// Automatically discover interpreter (`VIRTUAL_ENV`, project venv dirs)
    Auto,
    /// Use specific virtual environment path
    VenvPath(String),
    /// Use specific interpreter executable path
    InterpreterPath(String),
}

impl Interpreter {
    /// Discover interpreter based on explicit path, `VIRTUAL_ENV`, or auto
    #[must_use]
    pub fn discover(venv_path: Option<&str>) -> Self {
        let virtual_env = std::env::var("VIRTUAL_ENV").ok();
        Self::discover_from_sources(venv_path, virtual_env.as_deref())
    }

    fn discover_from_sources(venv_path: Option<&str>, virtual_env: Option<&str>) -> Self {
        venv_path
            .or(virtual_env)
            .map_or(Self::Auto, |path| Self::VenvPath(path.to_string()))
    }

    pub(crate) fn site_packages_path(
        &self,
        fs: &dyn FileSystem,
        project_root: &Utf8Path,
    ) -> Option<Utf8PathBuf> {
        match self {
            Self::VenvPath(path) => Self::site_packages_path_in_venv(fs, Utf8Path::new(path)),
            Self::Auto => Self::auto_venv_paths(project_root).find_map(|venv| {
                fs.is_dir(&venv)
                    .then(|| Self::site_packages_path_in_venv(fs, &venv))
                    .flatten()
            }),
            Self::InterpreterPath(_) => None,
        }
    }

    fn site_packages_path_in_venv(fs: &dyn FileSystem, venv: &Utf8Path) -> Option<Utf8PathBuf> {
        let windows_site_packages = venv.join("Lib").join("site-packages");
        if std::env::consts::OS == "windows" && fs.is_dir(&windows_site_packages) {
            return Some(windows_site_packages);
        }

        let lib_dir = venv.join("lib");
        let mut site_packages_directories = Vec::new();
        if fs.is_dir(&lib_dir)
            && let Ok(entries) = fs.walk_entries(&lib_dir, &WalkOptions::shallow())
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

    fn auto_venv_paths(project_root: &Utf8Path) -> impl Iterator<Item = Utf8PathBuf> + '_ {
        [".venv", "venv", "env", ".env"]
            .into_iter()
            .map(|dir| project_root.join(dir))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod interpreter_discovery {
        use super::*;

        #[test]
        fn test_discover_with_explicit_venv_path() {
            let interpreter = Interpreter::discover_from_sources(Some("/path/to/venv"), None);
            assert_eq!(
                interpreter,
                Interpreter::VenvPath("/path/to/venv".to_string())
            );
        }

        #[test]
        fn test_discover_with_virtual_env_var() {
            let interpreter = Interpreter::discover_from_sources(None, Some("/env/path"));
            assert_eq!(interpreter, Interpreter::VenvPath("/env/path".to_string()));
        }

        #[test]
        fn test_discover_explicit_overrides_env_var() {
            let interpreter =
                Interpreter::discover_from_sources(Some("/explicit/path"), Some("/env/path"));
            assert_eq!(
                interpreter,
                Interpreter::VenvPath("/explicit/path".to_string())
            );
        }

        #[test]
        fn test_discover_auto_when_no_hints() {
            let interpreter = Interpreter::discover_from_sources(None, None);
            assert_eq!(interpreter, Interpreter::Auto);
        }
    }

    mod interpreter_resolution {
        use super::*;

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
