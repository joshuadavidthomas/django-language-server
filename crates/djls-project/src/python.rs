use std::borrow::Borrow;
use std::fmt;
use std::sync::Arc;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_source::File;
use djls_source::FileSystem;
use djls_source::WalkEntryKind;
use djls_source::WalkOptions;
use serde::Deserialize;
use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum InvalidModuleName {
    #[error("python module name cannot be empty")]
    Empty,
    #[error("python module name cannot contain whitespace")]
    ContainsWhitespace,
    #[error("python module name cannot start with '.'")]
    StartsWithDot,
    #[error("python module name cannot end with '.'")]
    EndsWithDot,
    #[error("python module name cannot contain consecutive dots")]
    ContainsConsecutiveDots,
    #[error("python module name contains invalid segment: {0}")]
    InvalidSegment(String),
    #[error("python module source path must end with '.py'")]
    MustHavePyExtension,
    #[error("python module source path must be relative, got absolute: {0}")]
    SourcePathIsAbsolute(String),
}

/// A dotted Python module name, e.g. `"myapp.models"`.
#[derive(Clone, Debug, Eq, PartialEq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct PythonModuleName(Arc<str>);

impl PythonModuleName {
    pub fn parse(name: &str) -> Result<Self, InvalidModuleName> {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err(InvalidModuleName::Empty);
        }
        if trimmed.chars().any(char::is_whitespace) {
            return Err(InvalidModuleName::ContainsWhitespace);
        }
        validate_python_module_name(trimmed)?;
        Ok(Self(Arc::from(trimmed)))
    }

    pub(crate) fn from_relative_package_path(path: &Utf8Path) -> Result<Self, InvalidModuleName> {
        if path.is_absolute() {
            return Err(InvalidModuleName::SourcePathIsAbsolute(path.to_string()));
        }

        let dotted = path
            .components()
            .map(|component| component.as_str())
            .collect::<Vec<_>>()
            .join(".");

        Self::parse(&dotted)
    }

    pub fn from_relative_source_path(path: &Utf8Path) -> Result<Self, InvalidModuleName> {
        if path.is_absolute() {
            return Err(InvalidModuleName::SourcePathIsAbsolute(path.to_string()));
        }
        if path.extension() != Some("py") {
            return Err(InvalidModuleName::MustHavePyExtension);
        }

        Self::parse(&module_name_from_relative_source_path(path))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub(crate) fn into_string(self) -> String {
        self.0.to_string()
    }
}

fn module_name_from_relative_source_path(path: &Utf8Path) -> String {
    let without_ext = path.with_extension("");
    let parts: Vec<&str> = without_ext
        .components()
        .map(|component| component.as_str())
        .collect();
    if parts.last() == Some(&"__init__") {
        parts[..parts.len() - 1].join(".")
    } else {
        parts.join(".")
    }
}

fn validate_python_module_name(name: &str) -> Result<(), InvalidModuleName> {
    if name.starts_with('.') {
        return Err(InvalidModuleName::StartsWithDot);
    }

    if name.ends_with('.') {
        return Err(InvalidModuleName::EndsWithDot);
    }

    if name.contains("..") {
        return Err(InvalidModuleName::ContainsConsecutiveDots);
    }

    for segment in name.split('.') {
        if !is_python_identifier(segment) {
            return Err(InvalidModuleName::InvalidSegment(segment.to_string()));
        }
    }

    Ok(())
}

fn is_python_identifier(segment: &str) -> bool {
    let mut chars = segment.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if first != '_' && !first.is_alphabetic() {
        return false;
    }
    chars.all(|ch| ch == '_' || ch.is_alphanumeric())
}

impl Borrow<str> for PythonModuleName {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for PythonModuleName {
    type Error = InvalidModuleName;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<PythonModuleName> for String {
    fn from(value: PythonModuleName) -> Self {
        value.0.to_string()
    }
}

impl fmt::Display for PythonModuleName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_str().fmt(f)
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct PythonModule {
    name: PythonModuleName,
    path: Utf8PathBuf,
    file: File,
}

impl PythonModule {
    pub(crate) fn new(name: PythonModuleName, path: Utf8PathBuf, file: File) -> Self {
        Self { name, path, file }
    }

    #[must_use]
    pub fn name(&self) -> &PythonModuleName {
        &self.name
    }

    #[must_use]
    pub fn path(&self) -> &Utf8Path {
        &self.path
    }

    #[must_use]
    pub fn file(&self) -> File {
        self.file
    }
}

impl fmt::Debug for PythonModule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PythonModule")
            .field("name", &self.name)
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

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

    #[test]
    fn python_module_names_validate_and_normalize() {
        assert_eq!(
            PythonModuleName::from_relative_source_path(Utf8Path::new("pkg/__init__.py")),
            Ok(PythonModuleName::parse("pkg").unwrap())
        );
        assert_eq!(
            PythonModuleName::parse("django..template"),
            Err(InvalidModuleName::ContainsConsecutiveDots)
        );
        assert_eq!(
            PythonModuleName::parse(".django"),
            Err(InvalidModuleName::StartsWithDot)
        );
        assert_eq!(
            PythonModuleName::parse("django."),
            Err(InvalidModuleName::EndsWithDot)
        );
        assert_eq!(
            PythonModuleName::parse("pkg/models"),
            Err(InvalidModuleName::InvalidSegment("pkg/models".to_string()))
        );
        assert_eq!(
            PythonModuleName::parse("my-app.models"),
            Err(InvalidModuleName::InvalidSegment("my-app".to_string()))
        );
        assert_eq!(
            PythonModuleName::parse("123pkg.models"),
            Err(InvalidModuleName::InvalidSegment("123pkg".to_string()))
        );
        assert_eq!(
            PythonModuleName::from_relative_package_path(Utf8Path::new("pkg/bad-name")),
            Err(InvalidModuleName::InvalidSegment("bad-name".to_string()))
        );
        assert_eq!(
            PythonModuleName::from_relative_source_path(Utf8Path::new("pkg/bad-name.py")),
            Err(InvalidModuleName::InvalidSegment("bad-name".to_string()))
        );
        assert_eq!(
            PythonModuleName::from_relative_source_path(Utf8Path::new("pkg/module.txt")),
            Err(InvalidModuleName::MustHavePyExtension)
        );
    }

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
