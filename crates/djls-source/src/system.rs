use camino::Utf8Path;
use camino::Utf8PathBuf;
use rustc_hash::FxHashMap;
use std::io;

pub trait FileSystem: Send + Sync {
    fn read_to_string(&self, path: &Utf8Path) -> io::Result<String>;
    fn exists(&self, path: &Utf8Path) -> bool;
}

pub struct InMemoryFileSystem {
    files: FxHashMap<Utf8PathBuf, String>,
}

impl InMemoryFileSystem {
    #[must_use]
    pub fn new() -> Self {
        Self {
            files: FxHashMap::default(),
        }
    }

    pub fn add_file(&mut self, path: Utf8PathBuf, content: String) {
        self.files.insert(path, content);
    }
}

impl Default for InMemoryFileSystem {
    fn default() -> Self {
        Self::new()
    }
}

impl FileSystem for InMemoryFileSystem {
    fn read_to_string(&self, path: &Utf8Path) -> io::Result<String> {
        self.files
            .get(path)
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "File not found"))
    }

    fn exists(&self, path: &Utf8Path) -> bool {
        self.files.contains_key(path)
    }
}

/// Standard file system implementation that uses [`std::fs`].
pub struct OsFileSystem;

impl FileSystem for OsFileSystem {
    fn read_to_string(&self, path: &Utf8Path) -> io::Result<String> {
        std::fs::read_to_string(path)
    }

    fn exists(&self, path: &Utf8Path) -> bool {
        path.exists()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod in_memory {
        use super::*;

        #[test]
        fn test_read_existing_file() {
            let mut fs = InMemoryFileSystem::new();
            fs.add_file("/test.py".into(), "file content".to_string());

            assert_eq!(
                fs.read_to_string(Utf8Path::new("/test.py")).unwrap(),
                "file content"
            );
        }

        #[test]
        fn test_read_nonexistent_file() {
            let fs = InMemoryFileSystem::new();

            let result = fs.read_to_string(Utf8Path::new("/missing.py"));
            assert!(result.is_err());
            assert_eq!(result.unwrap_err().kind(), io::ErrorKind::NotFound);
        }

        #[test]
        fn test_exists_returns_true_for_existing() {
            let mut fs = InMemoryFileSystem::new();
            fs.add_file("/exists.py".into(), "content".to_string());

            assert!(fs.exists(Utf8Path::new("/exists.py")));
        }

        #[test]
        fn test_exists_returns_false_for_nonexistent() {
            let fs = InMemoryFileSystem::new();

            assert!(!fs.exists(Utf8Path::new("/missing.py")));
        }
    }
}
