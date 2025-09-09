use std::fmt;
use std::path::Path;
use std::path::PathBuf;

use pyo3::prelude::*;

use crate::system;

#[derive(Clone, Debug, PartialEq)]
pub struct PythonEnvironment {
    python_path: PathBuf,
    sys_path: Vec<PathBuf>,
    sys_prefix: PathBuf,
}

impl PythonEnvironment {
    #[must_use]
    pub fn new(project_path: &Path, venv_path: Option<&str>) -> Option<Self> {
        if let Some(path) = venv_path {
            let prefix = PathBuf::from(path);
            if let Some(env) = Self::from_venv_prefix(&prefix) {
                return Some(env);
            }
            // Invalid explicit path, continue searching...
        }

        if let Ok(virtual_env) = system::env_var("VIRTUAL_ENV") {
            let prefix = PathBuf::from(virtual_env);
            if let Some(env) = Self::from_venv_prefix(&prefix) {
                return Some(env);
            }
        }

        for venv_dir in &[".venv", "venv", "env", ".env"] {
            let potential_venv = project_path.join(venv_dir);
            if potential_venv.is_dir() {
                if let Some(env) = Self::from_venv_prefix(&potential_venv) {
                    return Some(env);
                }
            }
        }

        Self::from_system_python()
    }

    fn from_venv_prefix(prefix: &Path) -> Option<Self> {
        #[cfg(unix)]
        let python_path = prefix.join("bin").join("python");
        #[cfg(windows)]
        let python_path = prefix.join("Scripts").join("python.exe");

        if !prefix.is_dir() || !python_path.exists() {
            return None;
        }

        #[cfg(unix)]
        let bin_dir = prefix.join("bin");
        #[cfg(windows)]
        let bin_dir = prefix.join("Scripts");

        let mut sys_path = Vec::new();
        sys_path.push(bin_dir);

        if let Some(site_packages) = Self::find_site_packages(prefix) {
            if site_packages.is_dir() {
                sys_path.push(site_packages);
            }
        }

        Some(Self {
            python_path: python_path.clone(),
            sys_path,
            sys_prefix: prefix.to_path_buf(),
        })
    }

    pub fn activate(&self, py: Python) -> PyResult<()> {
        let sys = py.import("sys")?;
        let py_path = sys.getattr("path")?;

        for path in &self.sys_path {
            if let Some(path_str) = path.to_str() {
                py_path.call_method1("append", (path_str,))?;
            }
        }

        Ok(())
    }

    fn from_system_python() -> Option<Self> {
        let Ok(python_path) = system::find_executable("python") else {
            return None;
        };
        let bin_dir = python_path.parent()?;
        let prefix = bin_dir.parent()?;

        let mut sys_path = Vec::new();
        sys_path.push(bin_dir.to_path_buf());

        if let Some(site_packages) = Self::find_site_packages(prefix) {
            if site_packages.is_dir() {
                sys_path.push(site_packages);
            }
        }

        Some(Self {
            python_path: python_path.clone(),
            sys_path,
            sys_prefix: prefix.to_path_buf(),
        })
    }

    #[cfg(unix)]
    fn find_site_packages(prefix: &Path) -> Option<PathBuf> {
        let lib_dir = prefix.join("lib");
        if !lib_dir.is_dir() {
            return None;
        }
        std::fs::read_dir(lib_dir)
            .ok()?
            .filter_map(Result::ok)
            .find(|e| {
                e.file_type().is_ok_and(|ft| ft.is_dir())
                    && e.file_name().to_string_lossy().starts_with("python")
            })
            .map(|e| e.path().join("site-packages"))
    }

    #[cfg(windows)]
    fn find_site_packages(prefix: &Path) -> Option<PathBuf> {
        Some(prefix.join("Lib").join("site-packages"))
    }
}

impl fmt::Display for PythonEnvironment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Python path: {}", self.python_path.display())?;
        writeln!(f, "Sys prefix: {}", self.sys_prefix.display())?;
        writeln!(f, "Sys paths:")?;
        for path in &self.sys_path {
            writeln!(f, "  {}", path.display())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    use tempfile::tempdir;

    use super::*;

    fn create_mock_venv(dir: &Path, version: Option<&str>) -> PathBuf {
        let prefix = dir.to_path_buf();

        #[cfg(unix)]
        {
            let bin_dir = prefix.join("bin");
            fs::create_dir_all(&bin_dir).unwrap();
            fs::write(bin_dir.join("python"), "").unwrap();
            let lib_dir = prefix.join("lib");
            fs::create_dir_all(&lib_dir).unwrap();
            let py_version_dir = lib_dir.join(version.unwrap_or("python3.9"));
            fs::create_dir_all(&py_version_dir).unwrap();
            fs::create_dir_all(py_version_dir.join("site-packages")).unwrap();
        }
        #[cfg(windows)]
        {
            let bin_dir = prefix.join("Scripts");
            fs::create_dir_all(&bin_dir).unwrap();
            fs::write(bin_dir.join("python.exe"), "").unwrap();
            let lib_dir = prefix.join("Lib");
            fs::create_dir_all(&lib_dir).unwrap();
            fs::create_dir_all(lib_dir.join("site-packages")).unwrap();
        }

        prefix
    }

    fn get_sys_path(py: Python) -> PyResult<Vec<String>> {
        let sys = py.import("sys")?;
        let py_path = sys.getattr("path")?;
        py_path.extract::<Vec<String>>()
    }

    fn create_test_env(sys_paths: Vec<PathBuf>) -> PythonEnvironment {
        PythonEnvironment {
            python_path: PathBuf::from("dummy/bin/python"),
            sys_prefix: PathBuf::from("dummy"),
            sys_path: sys_paths,
        }
    }

    mod env_discovery {
        use which::Error as WhichError;

        use super::*;
        use crate::system::mock::MockGuard;
        use crate::system::mock::{
            self as sys_mock,
        };

        #[test]
        fn test_explicit_venv_path_found() {
            let project_dir = tempdir().unwrap();
            let venv_dir = tempdir().unwrap();
            let venv_prefix = create_mock_venv(venv_dir.path(), None);

            let env =
                PythonEnvironment::new(project_dir.path(), Some(venv_prefix.to_str().unwrap()))
                    .expect("Should find environment with explicit path");

            assert_eq!(env.sys_prefix, venv_prefix);

            #[cfg(unix)]
            {
                assert!(env.python_path.ends_with("bin/python"));
                assert!(env.sys_path.contains(&venv_prefix.join("bin")));
                assert!(env
                    .sys_path
                    .contains(&venv_prefix.join("lib/python3.9/site-packages")));
            }
            #[cfg(windows)]
            {
                assert!(env.python_path.ends_with("Scripts\\python.exe"));
                assert!(env.sys_path.contains(&venv_prefix.join("Scripts")));
                assert!(env
                    .sys_path
                    .contains(&venv_prefix.join("Lib").join("site-packages")));
            }
        }

        #[test]
        fn test_explicit_venv_path_invalid_falls_through_to_project_venv() {
            let project_dir = tempdir().unwrap();
            let project_venv_prefix = create_mock_venv(&project_dir.path().join(".venv"), None);

            let _guard = MockGuard;
            // Ensure VIRTUAL_ENV is not set (returns VarError::NotPresent)
            sys_mock::remove_env_var("VIRTUAL_ENV");

            // Provide an invalid explicit path
            let invalid_path = project_dir.path().join("non_existent_venv");
            let env =
                PythonEnvironment::new(project_dir.path(), Some(invalid_path.to_str().unwrap()))
                    .expect("Should fall through to project .venv");

            // Should have found the one in the project dir
            assert_eq!(env.sys_prefix, project_venv_prefix);
        }

        #[test]
        fn test_virtual_env_variable_found() {
            let project_dir = tempdir().unwrap();
            let venv_dir = tempdir().unwrap();
            let venv_prefix = create_mock_venv(venv_dir.path(), None);

            let _guard = MockGuard;
            // Mock VIRTUAL_ENV to point to the mock venv
            sys_mock::set_env_var("VIRTUAL_ENV", venv_prefix.to_str().unwrap().to_string());

            let env = PythonEnvironment::new(project_dir.path(), None)
                .expect("Should find environment via VIRTUAL_ENV");

            assert_eq!(env.sys_prefix, venv_prefix);

            #[cfg(unix)]
            assert!(env.python_path.ends_with("bin/python"));
            #[cfg(windows)]
            assert!(env.python_path.ends_with("Scripts\\python.exe"));
        }

        #[test]
        fn test_explicit_path_overrides_virtual_env() {
            let project_dir = tempdir().unwrap();
            let venv1_dir = tempdir().unwrap();
            let venv1_prefix = create_mock_venv(venv1_dir.path(), None); // Mocked by VIRTUAL_ENV
            let venv2_dir = tempdir().unwrap();
            let venv2_prefix = create_mock_venv(venv2_dir.path(), None); // Provided explicitly

            let _guard = MockGuard;
            // Mock VIRTUAL_ENV to point to venv1
            sys_mock::set_env_var("VIRTUAL_ENV", venv1_prefix.to_str().unwrap().to_string());

            // Call with explicit path to venv2
            let env =
                PythonEnvironment::new(project_dir.path(), Some(venv2_prefix.to_str().unwrap()))
                    .expect("Should find environment via explicit path");

            // Explicit path (venv2) should take precedence
            assert_eq!(
                env.sys_prefix, venv2_prefix,
                "Explicit path should take precedence"
            );
        }

        #[test]
        fn test_project_venv_found() {
            let project_dir = tempdir().unwrap();
            let venv_prefix = create_mock_venv(&project_dir.path().join(".venv"), None);

            let _guard = MockGuard;
            // Ensure VIRTUAL_ENV is not set
            sys_mock::remove_env_var("VIRTUAL_ENV");

            let env = PythonEnvironment::new(project_dir.path(), None)
                .expect("Should find environment in project .venv");

            assert_eq!(env.sys_prefix, venv_prefix);
        }

        #[test]
        fn test_project_venv_priority() {
            let project_dir = tempdir().unwrap();
            let dot_venv_prefix = create_mock_venv(&project_dir.path().join(".venv"), None);
            let _venv_prefix = create_mock_venv(&project_dir.path().join("venv"), None);

            let _guard = MockGuard;
            // Ensure VIRTUAL_ENV is not set
            sys_mock::remove_env_var("VIRTUAL_ENV");

            let env =
                PythonEnvironment::new(project_dir.path(), None).expect("Should find environment");

            // Should find .venv because it's checked first in the loop
            assert_eq!(env.sys_prefix, dot_venv_prefix);
        }

        #[test]
        fn test_system_python_fallback() {
            let project_dir = tempdir().unwrap();

            let _guard = MockGuard;
            // Ensure VIRTUAL_ENV is not set
            sys_mock::remove_env_var("VIRTUAL_ENV");

            let mock_sys_python_dir = tempdir().unwrap();
            let mock_sys_python_prefix = mock_sys_python_dir.path();

            #[cfg(unix)]
            let (bin_subdir, python_exe, site_packages_rel_path) = (
                "bin",
                "python",
                Path::new("lib").join("python3.9").join("site-packages"),
            );
            #[cfg(windows)]
            let (bin_subdir, python_exe, site_packages_rel_path) = (
                "Scripts",
                "python.exe",
                Path::new("Lib").join("site-packages"),
            );

            let bin_dir = mock_sys_python_prefix.join(bin_subdir);
            fs::create_dir_all(&bin_dir).unwrap();
            let python_path = bin_dir.join(python_exe);
            fs::write(&python_path, "").unwrap();

            #[cfg(unix)]
            {
                let mut perms = fs::metadata(&python_path).unwrap().permissions();
                perms.set_mode(0o755);
                fs::set_permissions(&python_path, perms).unwrap();
            }

            let site_packages_path = mock_sys_python_prefix.join(site_packages_rel_path);
            fs::create_dir_all(&site_packages_path).unwrap();

            sys_mock::set_exec_path("python", python_path.clone());

            let system_env = PythonEnvironment::new(project_dir.path(), None);

            // Assert it found the mock system python via the mocked finder
            assert!(
                system_env.is_some(),
                "Should fall back to the mock system python"
            );

            if let Some(env) = system_env {
                assert_eq!(
                    env.python_path, python_path,
                    "Python path should match mock"
                );
                assert_eq!(
                    env.sys_prefix, mock_sys_python_prefix,
                    "Sys prefix should match mock prefix"
                );
                assert!(
                    env.sys_path.contains(&bin_dir),
                    "Sys path should contain mock bin dir"
                );
                assert!(
                    env.sys_path.contains(&site_packages_path),
                    "Sys path should contain mock site-packages"
                );
            } else {
                panic!("Expected to find environment, but got None");
            }
        }

        #[test]
        fn test_no_python_found() {
            let project_dir = tempdir().unwrap();

            let _guard = MockGuard; // Setup guard to clear mocks

            // Ensure VIRTUAL_ENV is not set
            sys_mock::remove_env_var("VIRTUAL_ENV");

            // Ensure find_executable returns an error
            sys_mock::set_exec_error("python", WhichError::CannotFindBinaryPath);

            let env = PythonEnvironment::new(project_dir.path(), None);

            assert!(
                env.is_none(),
                "Expected no environment to be found when all discovery methods fail"
            );
        }

        #[test]
        #[cfg(unix)]
        fn test_unix_site_packages_discovery() {
            let venv_dir = tempdir().unwrap();
            let prefix = venv_dir.path();
            let bin_dir = prefix.join("bin");
            fs::create_dir_all(&bin_dir).unwrap();
            fs::write(bin_dir.join("python"), "").unwrap();
            let lib_dir = prefix.join("lib");
            fs::create_dir_all(&lib_dir).unwrap();
            let py_version_dir1 = lib_dir.join("python3.8");
            fs::create_dir_all(&py_version_dir1).unwrap();
            fs::create_dir_all(py_version_dir1.join("site-packages")).unwrap();
            let py_version_dir2 = lib_dir.join("python3.10");
            fs::create_dir_all(&py_version_dir2).unwrap();
            fs::create_dir_all(py_version_dir2.join("site-packages")).unwrap();

            let env = PythonEnvironment::from_venv_prefix(prefix).unwrap();

            let found_site_packages = env.sys_path.iter().any(|p| p.ends_with("site-packages"));
            assert!(
                found_site_packages,
                "Should have found a site-packages directory"
            );
            assert!(env.sys_path.contains(&prefix.join("bin")));
        }

        #[test]
        #[cfg(windows)]
        fn test_windows_site_packages_discovery() {
            let venv_dir = tempdir().unwrap();
            let prefix = venv_dir.path();
            let bin_dir = prefix.join("Scripts");
            fs::create_dir_all(&bin_dir).unwrap();
            fs::write(bin_dir.join("python.exe"), "").unwrap();
            let lib_dir = prefix.join("Lib");
            fs::create_dir_all(&lib_dir).unwrap();
            let site_packages = lib_dir.join("site-packages");
            fs::create_dir_all(&site_packages).unwrap();

            let env = PythonEnvironment::from_venv_prefix(prefix).unwrap();

            assert!(env.sys_path.contains(&prefix.join("Scripts")));
            assert!(
                env.sys_path.contains(&site_packages),
                "Should have found Lib/site-packages"
            );
        }

        #[test]
        fn test_from_venv_prefix_returns_none_if_dir_missing() {
            let dir = tempdir().unwrap();
            let result = PythonEnvironment::from_venv_prefix(dir.path());
            assert!(result.is_none());
        }

        #[test]
        fn test_from_venv_prefix_returns_none_if_binary_missing() {
            let dir = tempdir().unwrap();
            let prefix = dir.path();
            fs::create_dir_all(prefix).unwrap();

            #[cfg(unix)]
            fs::create_dir_all(prefix.join("bin")).unwrap();
            #[cfg(windows)]
            fs::create_dir_all(prefix.join("Scripts")).unwrap();

            let result = PythonEnvironment::from_venv_prefix(prefix);
            assert!(result.is_none());
        }
    }

    mod env_activation {
        use super::*;

        #[test]
        #[ignore = "Requires Python runtime - run with --ignored flag"]
        fn test_activate_appends_paths() -> PyResult<()> {
            let temp_dir = tempdir().unwrap();
            let path1 = temp_dir.path().join("scripts");
            let path2 = temp_dir.path().join("libs");
            fs::create_dir_all(&path1).unwrap();
            fs::create_dir_all(&path2).unwrap();

            let test_env = create_test_env(vec![path1.clone(), path2.clone()]);

            pyo3::prepare_freethreaded_python();

            Python::with_gil(|py| {
                let initial_sys_path = get_sys_path(py)?;
                let initial_len = initial_sys_path.len();

                test_env.activate(py)?;

                let final_sys_path = get_sys_path(py)?;
                assert_eq!(
                    final_sys_path.len(),
                    initial_len + 2,
                    "Should have added 2 paths"
                );
                assert_eq!(
                    final_sys_path.get(initial_len).unwrap(),
                    path1.to_str().expect("Path 1 should be valid UTF-8")
                );
                assert_eq!(
                    final_sys_path.get(initial_len + 1).unwrap(),
                    path2.to_str().expect("Path 2 should be valid UTF-8")
                );

                Ok(())
            })
        }

        #[test]
        #[ignore = "Requires Python runtime - run with --ignored flag"]
        fn test_activate_empty_sys_path() -> PyResult<()> {
            let test_env = create_test_env(vec![]);

            pyo3::prepare_freethreaded_python();

            Python::with_gil(|py| {
                let initial_sys_path = get_sys_path(py)?;

                test_env.activate(py)?;

                let final_sys_path = get_sys_path(py)?;
                assert_eq!(
                    final_sys_path, initial_sys_path,
                    "sys.path should remain unchanged for empty env.sys_path"
                );

                Ok(())
            })
        }

        #[test]
        #[ignore = "Requires Python runtime - run with --ignored flag"]
        fn test_activate_with_non_existent_paths() -> PyResult<()> {
            let temp_dir = tempdir().unwrap();
            let path1 = temp_dir.path().join("non_existent_dir");
            let path2 = temp_dir.path().join("another_missing/path");

            let test_env = create_test_env(vec![path1.clone(), path2.clone()]);

            pyo3::prepare_freethreaded_python();

            Python::with_gil(|py| {
                let initial_sys_path = get_sys_path(py)?;
                let initial_len = initial_sys_path.len();

                test_env.activate(py)?;

                let final_sys_path = get_sys_path(py)?;
                assert_eq!(
                    final_sys_path.len(),
                    initial_len + 2,
                    "Should still add 2 paths even if they don't exist"
                );
                assert_eq!(
                    final_sys_path.get(initial_len).unwrap(),
                    path1.to_str().unwrap()
                );
                assert_eq!(
                    final_sys_path.get(initial_len + 1).unwrap(),
                    path2.to_str().unwrap()
                );

                Ok(())
            })
        }

        #[test]
        #[cfg(unix)]
        #[ignore = "Requires Python runtime - run with --ignored flag"]
        fn test_activate_skips_non_utf8_paths_unix() -> PyResult<()> {
            use std::ffi::OsStr;
            use std::os::unix::ffi::OsStrExt;

            let temp_dir = tempdir().unwrap();
            let valid_path = temp_dir.path().join("valid_dir");
            fs::create_dir(&valid_path).unwrap();

            let invalid_bytes = b"invalid_\xff_utf8";
            let os_str = OsStr::from_bytes(invalid_bytes);
            let non_utf8_path = PathBuf::from(os_str);
            assert!(
                non_utf8_path.to_str().is_none(),
                "Path should not be convertible to UTF-8 str"
            );

            let test_env = create_test_env(vec![valid_path.clone(), non_utf8_path.clone()]);

            pyo3::prepare_freethreaded_python();

            Python::with_gil(|py| {
                let initial_sys_path = get_sys_path(py)?;
                let initial_len = initial_sys_path.len();

                test_env.activate(py)?;

                let final_sys_path = get_sys_path(py)?;
                assert_eq!(
                    final_sys_path.len(),
                    initial_len + 1,
                    "Should only add valid UTF-8 paths"
                );
                assert_eq!(
                    final_sys_path.get(initial_len).unwrap(),
                    valid_path.to_str().unwrap()
                );

                let invalid_path_lossy = non_utf8_path.to_string_lossy();
                assert!(
                    !final_sys_path
                        .iter()
                        .any(|p| p.contains(&*invalid_path_lossy)),
                    "Non-UTF8 path should not be present in sys.path"
                );

                Ok(())
            })
        }

        #[test]
        #[cfg(windows)]
        #[ignore = "Requires Python runtime - run with --ignored flag"]
        fn test_activate_skips_non_utf8_paths_windows() -> PyResult<()> {
            use std::ffi::OsString;
            use std::os::windows::ffi::OsStringExt;

            let temp_dir = tempdir().unwrap();
            let valid_path = temp_dir.path().join("valid_dir");

            let invalid_wide: Vec<u16> = vec![
                'i' as u16, 'n' as u16, 'v' as u16, 'a' as u16, 'l' as u16, 'i' as u16, 'd' as u16,
                '_' as u16, 0xD800, '_' as u16, 'w' as u16, 'i' as u16, 'd' as u16, 'e' as u16,
            ];
            let os_string = OsString::from_wide(&invalid_wide);
            let non_utf8_path = PathBuf::from(os_string);

            assert!(
                non_utf8_path.to_str().is_none(),
                "Path with lone surrogate should not be convertible to UTF-8 str"
            );

            let test_env = create_test_env(vec![valid_path.clone(), non_utf8_path.clone()]);

            pyo3::prepare_freethreaded_python();

            Python::with_gil(|py| {
                let initial_sys_path = get_sys_path(py)?;
                let initial_len = initial_sys_path.len();

                test_env.activate(py)?;

                let final_sys_path = get_sys_path(py)?;
                assert_eq!(
                    final_sys_path.len(),
                    initial_len + 1,
                    "Should only add paths convertible to valid UTF-8"
                );
                assert_eq!(
                    final_sys_path.get(initial_len).unwrap(),
                    valid_path.to_str().unwrap()
                );

                let invalid_path_lossy = non_utf8_path.to_string_lossy();
                assert!(
                    !final_sys_path
                        .iter()
                        .any(|p| p.contains(&*invalid_path_lossy)),
                    "Non-UTF8 path (from invalid wide chars) should not be present in sys.path"
                );

                Ok(())
            })
        }
    }

    mod salsa_integration {
        use std::sync::Arc;

        use djls_workspace::FileSystem;
        use djls_workspace::InMemoryFileSystem;

        use super::*;
        use crate::db::find_python_environment;
        use crate::db::Db as ProjectDb;
        use crate::meta::ProjectMetadata;

        /// Test implementation of ProjectDb for unit tests
        #[salsa::db]
        #[derive(Clone)]
        struct TestDatabase {
            storage: salsa::Storage<TestDatabase>,
            metadata: ProjectMetadata,
            fs: Arc<dyn FileSystem>,
        }

        impl TestDatabase {
            fn new(metadata: ProjectMetadata) -> Self {
                Self {
                    storage: salsa::Storage::new(None),
                    metadata,
                    fs: Arc::new(InMemoryFileSystem::new()),
                }
            }
        }

        #[salsa::db]
        impl salsa::Database for TestDatabase {}

        #[salsa::db]
        impl djls_workspace::Db for TestDatabase {
            fn fs(&self) -> Arc<dyn FileSystem> {
                self.fs.clone()
            }

            fn read_file_content(&self, path: &std::path::Path) -> std::io::Result<String> {
                self.fs.read_to_string(path)
            }
        }

        #[salsa::db]
        impl ProjectDb for TestDatabase {
            fn metadata(&self) -> &ProjectMetadata {
                &self.metadata
            }
        }

        #[test]
        fn test_find_python_environment_with_salsa_db() {
            let project_dir = tempdir().unwrap();
            let venv_dir = tempdir().unwrap();

            // Create a mock venv
            let venv_prefix = create_mock_venv(venv_dir.path(), None);

            // Create a metadata instance with project path and explicit venv path
            let metadata =
                ProjectMetadata::new(project_dir.path().to_path_buf(), Some(venv_prefix.clone()));

            // Create a TestDatabase with the metadata
            let db = TestDatabase::new(metadata);

            // Call the tracked function
            let env = find_python_environment(&db);

            // Verify we found the environment
            assert!(env.is_some(), "Should find environment via salsa db");

            if let Some(env) = env {
                assert_eq!(env.sys_prefix, venv_prefix);

                #[cfg(unix)]
                {
                    assert!(env.python_path.ends_with("bin/python"));
                    assert!(env.sys_path.contains(&venv_prefix.join("bin")));
                }
                #[cfg(windows)]
                {
                    assert!(env.python_path.ends_with("Scripts\\python.exe"));
                    assert!(env.sys_path.contains(&venv_prefix.join("Scripts")));
                }
            }
        }

        #[test]
        fn test_find_python_environment_with_project_venv() {
            let project_dir = tempdir().unwrap();

            // Create a .venv in the project directory
            let venv_prefix = create_mock_venv(&project_dir.path().join(".venv"), None);

            // Create a metadata instance with project path but no explicit venv path
            let metadata = ProjectMetadata::new(project_dir.path().to_path_buf(), None);

            // Create a TestDatabase with the metadata
            let db = TestDatabase::new(metadata);

            // Mock to ensure VIRTUAL_ENV is not set
            let _guard = system::mock::MockGuard;
            system::mock::remove_env_var("VIRTUAL_ENV");

            // Call the tracked function
            let env = find_python_environment(&db);

            // Verify we found the environment
            assert!(
                env.is_some(),
                "Should find environment in project .venv via salsa db"
            );

            if let Some(env) = env {
                assert_eq!(env.sys_prefix, venv_prefix);
            }
        }
    }
}
