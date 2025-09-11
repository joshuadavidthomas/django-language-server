use std::fmt;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use crate::db::Db as ProjectDb;
use crate::system;
use crate::Project;

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

/// Resolve the Python interpreter path for the current project.
///
/// This tracked function determines the interpreter path based on the project's
/// interpreter specification.
#[salsa::tracked]
pub fn resolve_interpreter(db: &dyn ProjectDb, project: Project) -> Option<PathBuf> {
    match &project.interpreter(db) {
        Interpreter::InterpreterPath(path) => {
            let path_buf = PathBuf::from(path.as_str());
            if path_buf.exists() {
                Some(path_buf)
            } else {
                None
            }
        }
        Interpreter::VenvPath(venv_path) => {
            // Derive interpreter path from venv
            #[cfg(unix)]
            let interpreter_path = PathBuf::from(venv_path.as_str()).join("bin").join("python");
            #[cfg(windows)]
            let interpreter_path = PathBuf::from(venv_path.as_str())
                .join("Scripts")
                .join("python.exe");

            if interpreter_path.exists() {
                Some(interpreter_path)
            } else {
                None
            }
        }
        Interpreter::Auto => {
            // Try common venv directories
            for venv_dir in &[".venv", "venv", "env", ".env"] {
                let potential_venv = project.root(db).join(venv_dir);
                if potential_venv.is_dir() {
                    #[cfg(unix)]
                    let interpreter_path = potential_venv.join("bin").join("python");
                    #[cfg(windows)]
                    let interpreter_path = potential_venv.join("Scripts").join("python.exe");

                    if interpreter_path.exists() {
                        return Some(interpreter_path);
                    }
                }
            }

            // Fall back to system python
            crate::system::find_executable("python").ok()
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PythonEnvironment {
    pub python_path: PathBuf,
    pub sys_path: Vec<PathBuf>,
    pub sys_prefix: PathBuf,
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
///
/// Find the Python environment for the current Django project.
///
/// This Salsa tracked function discovers the Python environment based on:
/// 1. Explicit venv path from project config
/// 2. VIRTUAL_ENV environment variable
/// 3. Common venv directories in project root (.venv, venv, env, .env)
/// 4. System Python as fallback
#[salsa::tracked]
pub fn python_environment(db: &dyn ProjectDb, project: Project) -> Option<Arc<PythonEnvironment>> {
    let interpreter_path = resolve_interpreter(db, project)?;
    let project_path = project.root(db);

    // For venv paths, we need to determine the venv root
    let interpreter_spec = project.interpreter(db);
    let venv_path = match &interpreter_spec {
        Interpreter::InterpreterPath(_) => {
            // Try to determine venv from interpreter path
            interpreter_path
                .parent()
                .and_then(|bin_dir| bin_dir.parent())
                .and_then(|venv_root| venv_root.to_str())
        }
        Interpreter::VenvPath(path) => Some(path.as_str()),
        Interpreter::Auto => {
            // For auto-discovery, let PythonEnvironment::new handle it
            None
        }
    };

    PythonEnvironment::new(project_path, venv_path).map(Arc::new)
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

    mod env_discovery {
        use which::Error as WhichError;

        use super::*;
        use crate::system::mock::MockGuard;
        use crate::system::mock::{self as sys_mock};

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

    mod salsa_integration {
        use std::sync::Arc;
        use std::sync::Mutex;

        use djls_workspace::FileSystem;
        use djls_workspace::InMemoryFileSystem;

        use super::*;

        /// Test implementation of ProjectDb for unit tests
        #[salsa::db]
        #[derive(Clone)]
        struct TestDatabase {
            storage: salsa::Storage<TestDatabase>,
            project_root: PathBuf,
            project: Arc<Mutex<Option<crate::project::Project>>>,
            fs: Arc<dyn FileSystem>,
        }

        impl TestDatabase {
            fn new(project_root: PathBuf) -> Self {
                Self {
                    storage: salsa::Storage::new(None),
                    project_root,
                    project: Arc::new(Mutex::new(None)),
                    fs: Arc::new(InMemoryFileSystem::new()),
                }
            }

            fn set_project(&self, project: crate::project::Project) {
                *self.project.lock().unwrap() = Some(project);
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
            fn project(&self) -> Option<crate::project::Project> {
                // Return existing project or create a new one
                let mut project_lock = self.project.lock().unwrap();
                if project_lock.is_none() {
                    let root = &self.project_root;
                    let interpreter_spec = crate::python::Interpreter::Auto;
                    let django_settings = std::env::var("DJANGO_SETTINGS_MODULE").ok();

                    *project_lock = Some(crate::project::Project::new(
                        self,
                        root.clone(),
                        interpreter_spec,
                        django_settings,
                    ));
                }
                *project_lock
            }

            fn inspector_pool(&self) -> Arc<crate::inspector::pool::InspectorPool> {
                Arc::new(crate::inspector::pool::InspectorPool::new())
            }
        }

        #[test]
        fn test_python_environment_with_salsa_db() {
            let project_dir = tempdir().unwrap();
            let venv_dir = tempdir().unwrap();

            // Create a mock venv
            let venv_prefix = create_mock_venv(venv_dir.path(), None);

            // Create a TestDatabase with the project root
            let db = TestDatabase::new(project_dir.path().to_path_buf());

            // Create and configure the project with the venv path
            let project = crate::project::Project::new(
                &db,
                project_dir.path().to_path_buf(),
                crate::python::Interpreter::VenvPath(venv_prefix.to_string_lossy().to_string()),
                None,
            );
            db.set_project(project);

            // Call the tracked function
            let env = crate::python_environment(&db, project);

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
        fn test_python_environment_with_project_venv() {
            let project_dir = tempdir().unwrap();

            // Create a .venv in the project directory
            let venv_prefix = create_mock_venv(&project_dir.path().join(".venv"), None);

            // Create a TestDatabase with the project root
            let db = TestDatabase::new(project_dir.path().to_path_buf());

            // Mock to ensure VIRTUAL_ENV is not set
            let _guard = system::mock::MockGuard;
            system::mock::remove_env_var("VIRTUAL_ENV");

            // Call the tracked function (should find .venv)
            let project = db.project().unwrap();
            let env = crate::python_environment(&db, project);

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
