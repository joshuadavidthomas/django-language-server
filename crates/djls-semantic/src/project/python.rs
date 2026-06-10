use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_source::FileSystem;
use djls_source::WalkEntryKind;
use djls_source::WalkOptions;

use crate::project::system;

/// Interpreter specification for Python environment discovery.
///
/// This enum represents the different ways to specify which Python interpreter
/// to use for a project.
#[derive(Clone, Debug, PartialEq)]
pub enum Interpreter {
    /// Automatically discover interpreter (`VIRTUAL_ENV`, project venv dirs, system)
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
        venv_path
            .map(|path| Self::VenvPath(path.to_string()))
            .or_else(|| {
                #[cfg(not(test))]
                {
                    std::env::var("VIRTUAL_ENV").ok().map(Self::VenvPath)
                }
                #[cfg(test)]
                {
                    system::env_var("VIRTUAL_ENV").ok().map(Self::VenvPath)
                }
            })
            .unwrap_or(Self::Auto)
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

    fn python_executable_in_venv(venv: &Utf8Path) -> Utf8PathBuf {
        #[cfg(unix)]
        {
            venv.join("bin").join("python")
        }
        #[cfg(windows)]
        {
            venv.join("Scripts").join("python.exe")
        }
    }

    /// Resolve to the actual Python executable path
    #[must_use]
    pub fn python_path(&self, fs: &dyn FileSystem, project_root: &Utf8Path) -> Option<Utf8PathBuf> {
        match self {
            Self::InterpreterPath(path) => {
                let path_buf = Utf8PathBuf::from(path);
                if fs.exists(&path_buf) {
                    Some(path_buf)
                } else {
                    None
                }
            }
            Self::VenvPath(venv_path) => {
                let python = Self::python_executable_in_venv(Utf8Path::new(venv_path));
                fs.exists(&python).then_some(python)
            }
            Self::Auto => Self::auto_venv_paths(project_root)
                .find_map(|venv| {
                    fs.is_dir(&venv)
                        .then(|| Self::python_executable_in_venv(&venv))
                        .filter(|python| fs.exists(python))
                })
                .or_else(|| system::find_executable("python").ok()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    use tempfile::tempdir;

    use super::*;

    fn create_mock_venv(dir: &Utf8Path) -> Utf8PathBuf {
        let prefix = dir.to_path_buf();

        #[cfg(unix)]
        {
            let bin_dir = prefix.join("bin");
            fs::create_dir_all(&bin_dir).unwrap();
            let python_path = bin_dir.join("python");
            fs::write(&python_path, "").unwrap();
            let mut perms = fs::metadata(&python_path).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&python_path, perms).unwrap();
        }
        #[cfg(windows)]
        {
            let bin_dir = prefix.join("Scripts");
            fs::create_dir_all(&bin_dir).unwrap();
            fs::write(bin_dir.join("python.exe"), "").unwrap();
        }

        prefix
    }

    mod interpreter_discovery {
        use system::mock::MockGuard;
        use system::mock::{
            self as sys_mock,
        };

        use super::*;

        #[test]
        fn test_discover_with_explicit_venv_path() {
            let interpreter = Interpreter::discover(Some("/path/to/venv"));
            assert_eq!(
                interpreter,
                Interpreter::VenvPath("/path/to/venv".to_string())
            );
        }

        #[test]
        fn test_discover_with_virtual_env_var() {
            let _guard = MockGuard;
            sys_mock::set_env_var("VIRTUAL_ENV", "/env/path".to_string());

            let interpreter = Interpreter::discover(None);
            assert_eq!(interpreter, Interpreter::VenvPath("/env/path".to_string()));
        }

        #[test]
        fn test_discover_explicit_overrides_env_var() {
            let _guard = MockGuard;
            sys_mock::set_env_var("VIRTUAL_ENV", "/env/path".to_string());

            let interpreter = Interpreter::discover(Some("/explicit/path"));
            assert_eq!(
                interpreter,
                Interpreter::VenvPath("/explicit/path".to_string())
            );
        }

        #[test]
        fn test_discover_auto_when_no_hints() {
            let _guard = MockGuard;
            sys_mock::remove_env_var("VIRTUAL_ENV");

            let interpreter = Interpreter::discover(None);
            assert_eq!(interpreter, Interpreter::Auto);
        }
    }

    mod interpreter_resolution {
        use system::mock::MockGuard;
        use system::mock::{
            self as sys_mock,
        };
        use which::Error as WhichError;

        use super::*;

        #[test]
        fn test_interpreter_path_resolution() {
            let temp_dir = tempdir().unwrap();
            let temp_path = Utf8Path::from_path(temp_dir.path()).unwrap();
            let python_path = temp_path.join("python");
            fs::write(&python_path, "").unwrap();
            #[cfg(unix)]
            {
                let mut perms = fs::metadata(&python_path).unwrap().permissions();
                perms.set_mode(0o755);
                fs::set_permissions(&python_path, perms).unwrap();
            }

            let interpreter = Interpreter::InterpreterPath(python_path.to_string());
            let resolved = interpreter.python_path(&djls_source::OsFileSystem, temp_path);
            assert_eq!(resolved, Some(python_path));
        }

        #[test]
        fn test_interpreter_path_not_found() {
            let interpreter = Interpreter::InterpreterPath("/non/existent/python".to_string());
            let resolved =
                interpreter.python_path(&djls_source::OsFileSystem, Utf8Path::new("/project"));
            assert_eq!(resolved, None);
        }

        #[test]
        fn test_venv_path_resolution() {
            let venv_dir = tempdir().unwrap();
            let venv_path = create_mock_venv(Utf8Path::from_path(venv_dir.path()).unwrap());

            let interpreter = Interpreter::VenvPath(venv_path.to_string());
            let resolved =
                interpreter.python_path(&djls_source::OsFileSystem, Utf8Path::new("/project"));

            assert!(resolved.is_some());
            #[cfg(unix)]
            assert!(resolved.unwrap().ends_with("bin/python"));
            #[cfg(windows)]
            assert!(resolved.unwrap().ends_with("Scripts\\python.exe"));
        }

        #[test]
        fn test_venv_path_not_found() {
            let interpreter = Interpreter::VenvPath("/non/existent/venv".to_string());
            let resolved =
                interpreter.python_path(&djls_source::OsFileSystem, Utf8Path::new("/project"));
            assert_eq!(resolved, None);
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

        #[test]
        fn test_auto_finds_project_venv() {
            let project_dir = tempdir().unwrap();
            let project_path = Utf8Path::from_path(project_dir.path()).unwrap();
            let venv_path = project_path.join(".venv");
            create_mock_venv(&venv_path);

            let _guard = MockGuard;
            sys_mock::remove_env_var("VIRTUAL_ENV");

            let interpreter = Interpreter::Auto;
            let resolved = interpreter.python_path(&djls_source::OsFileSystem, project_path);

            assert!(resolved.is_some());
            #[cfg(unix)]
            assert!(resolved.unwrap().ends_with(".venv/bin/python"));
            #[cfg(windows)]
            assert!(resolved.unwrap().ends_with(".venv\\Scripts\\python.exe"));
        }

        #[test]
        fn test_auto_priority_order() {
            let project_dir = tempdir().unwrap();
            let project_path = Utf8Path::from_path(project_dir.path()).unwrap();

            // Create both .venv and venv
            create_mock_venv(&project_path.join(".venv"));
            create_mock_venv(&project_path.join("venv"));

            let _guard = MockGuard;
            sys_mock::remove_env_var("VIRTUAL_ENV");

            let interpreter = Interpreter::Auto;
            let resolved = interpreter.python_path(&djls_source::OsFileSystem, project_path);

            // Should find .venv first due to order
            assert!(resolved.is_some());
            assert!(resolved.unwrap().as_str().contains(".venv"));
        }

        #[test]
        fn test_auto_falls_back_to_system() {
            let project_dir = tempdir().unwrap();
            let project_path = Utf8Path::from_path(project_dir.path()).unwrap();

            let _guard = MockGuard;
            sys_mock::remove_env_var("VIRTUAL_ENV");

            // Mock system python
            let mock_python = tempdir().unwrap();
            let mock_python_path = Utf8Path::from_path(mock_python.path())
                .unwrap()
                .join("python");
            fs::write(&mock_python_path, "").unwrap();
            #[cfg(unix)]
            {
                let mut perms = fs::metadata(&mock_python_path).unwrap().permissions();
                perms.set_mode(0o755);
                fs::set_permissions(&mock_python_path, perms).unwrap();
            }

            sys_mock::set_exec_path("python", mock_python_path.clone());

            let interpreter = Interpreter::Auto;
            let resolved = interpreter.python_path(&djls_source::OsFileSystem, project_path);

            assert_eq!(resolved, Some(mock_python_path));
        }

        #[test]
        fn test_auto_no_python_found() {
            let project_dir = tempdir().unwrap();
            let project_path = Utf8Path::from_path(project_dir.path()).unwrap();

            let _guard = MockGuard;
            sys_mock::remove_env_var("VIRTUAL_ENV");
            sys_mock::set_exec_error("python", WhichError::CannotFindBinaryPath);

            let interpreter = Interpreter::Auto;
            let resolved = interpreter.python_path(&djls_source::OsFileSystem, project_path);

            assert_eq!(resolved, None);
        }
    }
}
