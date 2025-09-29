#[cfg(test)]
use std::env::VarError;

use camino::Utf8PathBuf;
use which::Error as WhichError;

pub fn find_executable(name: &str) -> Result<Utf8PathBuf, WhichError> {
    #[cfg(not(test))]
    {
        which::which(name).and_then(|path| {
            Utf8PathBuf::from_path_buf(path).map_err(|_| WhichError::CannotFindBinaryPath)
        })
    }
    #[cfg(test)]
    {
        mock::find_executable_mocked(name)
    }
}

#[cfg(test)]
pub fn env_var(key: &str) -> Result<String, VarError> {
    #[cfg(not(test))]
    {
        std::env::var(key)
    }
    #[cfg(test)]
    {
        mock::env_var_mocked(key)
    }
}

#[cfg(test)]
pub mod mock {
    use std::cell::RefCell;
    use std::thread_local;

    use rustc_hash::FxHashMap;

    use super::*;

    thread_local! {
        static MOCK_EXEC_RESULTS: RefCell<FxHashMap<String, Result<Utf8PathBuf, WhichError>>> = RefCell::new(FxHashMap::default());
        static MOCK_ENV_RESULTS: RefCell<FxHashMap<String, Result<String, VarError>>> = RefCell::new(FxHashMap::default());
    }

    pub(super) fn find_executable_mocked(name: &str) -> Result<Utf8PathBuf, WhichError> {
        MOCK_EXEC_RESULTS.with(|mocks| {
            mocks
                .borrow()
                .get(name)
                .cloned()
                .unwrap_or(Err(WhichError::CannotFindBinaryPath))
        })
    }

    pub(super) fn env_var_mocked(key: &str) -> Result<String, VarError> {
        MOCK_ENV_RESULTS.with(|mocks| {
            mocks
                .borrow()
                .get(key)
                .cloned()
                .unwrap_or(Err(VarError::NotPresent))
        })
    }

    // RAII guard to clear all mocks automatically after each test.
    pub struct MockGuard;
    impl Drop for MockGuard {
        fn drop(&mut self) {
            MOCK_EXEC_RESULTS.with(|mocks| mocks.borrow_mut().clear());
            MOCK_ENV_RESULTS.with(|mocks| mocks.borrow_mut().clear());
        }
    }

    pub fn set_exec_path(name: &str, path: Utf8PathBuf) {
        MOCK_EXEC_RESULTS.with(|mocks| {
            mocks.borrow_mut().insert(name.to_string(), Ok(path));
        });
    }

    pub fn set_exec_error(name: &str, error: WhichError) {
        MOCK_EXEC_RESULTS.with(|mocks| {
            mocks.borrow_mut().insert(name.to_string(), Err(error));
        });
    }

    pub fn set_env_var(key: &str, value: String) {
        MOCK_ENV_RESULTS.with(|mocks| {
            mocks.borrow_mut().insert(key.to_string(), Ok(value));
        });
    }

    // Simulates VarError::NotPresent
    pub fn remove_env_var(key: &str) {
        MOCK_ENV_RESULTS.with(|mocks| {
            mocks
                .borrow_mut()
                .insert(key.to_string(), Err(VarError::NotPresent));
        });
    }
}

#[cfg(test)]
mod tests {
    use std::env::VarError;

    use super::mock::MockGuard;
    use super::mock::{
        self as sys_mock,
    };
    use super::*;

    #[test]
    fn test_exec_mock_path_retrieval() {
        let _guard = MockGuard;
        let expected_path = Utf8PathBuf::from("/mock/path/to/python");
        sys_mock::set_exec_path("python", expected_path.clone());
        let result = find_executable("python");
        assert_eq!(result.unwrap(), expected_path);
    }

    #[test]
    fn test_exec_mock_error_retrieval() {
        let _guard = MockGuard;
        sys_mock::set_exec_error("cargo", WhichError::CannotFindBinaryPath);
        let result = find_executable("cargo");
        assert!(matches!(result, Err(WhichError::CannotFindBinaryPath)));
    }

    #[test]
    fn test_exec_mock_default_error_if_unmocked() {
        let _guard = MockGuard;
        let result = find_executable("git"); // Not mocked
        assert!(matches!(result, Err(WhichError::CannotFindBinaryPath)));
    }

    #[test]
    fn test_env_mock_set_var_retrieval() {
        let _guard = MockGuard;
        sys_mock::set_env_var("MY_VAR", "my_value".to_string());
        let result = env_var("MY_VAR");
        assert_eq!(result.unwrap(), "my_value");
    }

    #[test]
    fn test_env_mock_remove_var_retrieval() {
        let _guard = MockGuard;
        // Set it first, then remove it via mock
        sys_mock::set_env_var("TEMP_VAR", "temp_value".to_string());
        sys_mock::remove_env_var("TEMP_VAR");
        let result = env_var("TEMP_VAR");
        assert!(matches!(result, Err(VarError::NotPresent)));
    }

    #[test]
    fn test_env_mock_default_error_if_unmocked() {
        let _guard = MockGuard;
        let result = env_var("UNMOCKED_VAR"); // Not mocked
        assert!(matches!(result, Err(VarError::NotPresent)));
    }

    #[test]
    fn test_mock_guard_clears_all_mocks() {
        let expected_exec_path = Utf8PathBuf::from("/tmp/myprog");
        let expected_env_val = "test_value".to_string();

        {
            let _guard = MockGuard;
            sys_mock::set_exec_path("myprog", expected_exec_path.clone());
            sys_mock::set_env_var("MY_TEST_ENV", expected_env_val.clone());
            // Guard drops here, clearing both mocks
        }

        // Verify mocks were cleared
        let _guard = MockGuard;
        let result_exec = find_executable("myprog");
        assert!(matches!(result_exec, Err(WhichError::CannotFindBinaryPath)));
        let result_env = env_var("MY_TEST_ENV");
        assert!(matches!(result_env, Err(VarError::NotPresent)));
    }

    #[test]
    fn test_mocks_are_separate_between_tests() {
        let _guard = MockGuard; // Ensures clean state

        // Check state from previous tests (should be cleared)
        let result_python = find_executable("python");
        assert!(matches!(
            result_python,
            Err(WhichError::CannotFindBinaryPath)
        ));
        let result_myvar = env_var("MY_VAR");
        assert!(matches!(result_myvar, Err(VarError::NotPresent)));

        // Set mocks specific to this test
        let expected_path_node = Utf8PathBuf::from("/usr/bin/node");
        sys_mock::set_exec_path("node", expected_path_node.clone());
        sys_mock::set_env_var("NODE_ENV", "production".to_string());

        let result_node = find_executable("node");
        assert_eq!(result_node.unwrap(), expected_path_node);
        let result_node_env = env_var("NODE_ENV");
        assert_eq!(result_node_env.unwrap(), "production");
    }
}
