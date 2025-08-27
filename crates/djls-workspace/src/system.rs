//! File system abstraction following Ruff's pattern
//!
//! This module provides the FileSystem trait that abstracts file I/O operations.
//! This allows the LSP to work with both real files and in-memory overlays.

use std::io;
use std::path::Path;

/// Trait for file system operations
///
/// This follows Ruff's pattern of abstracting file system operations behind a trait,
/// allowing different implementations for testing, in-memory operation, and real file access.
pub trait FileSystem: Send + Sync {
    /// Read the entire contents of a file
    fn read_to_string(&self, path: &Path) -> io::Result<String>;
    
    /// Check if a path exists
    fn exists(&self, path: &Path) -> bool;
    
    /// Check if a path is a file
    fn is_file(&self, path: &Path) -> bool;
    
    /// Check if a path is a directory
    fn is_directory(&self, path: &Path) -> bool;
    
    /// List directory contents
    fn read_directory(&self, path: &Path) -> io::Result<Vec<std::path::PathBuf>>;
    
    /// Get file metadata (size, modified time, etc.)
    fn metadata(&self, path: &Path) -> io::Result<std::fs::Metadata>;
}

/// Standard file system implementation that uses std::fs
pub struct StdFileSystem;

impl FileSystem for StdFileSystem {
    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        std::fs::read_to_string(path)
    }
    
    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }
    
    fn is_file(&self, path: &Path) -> bool {
        path.is_file()
    }
    
    fn is_directory(&self, path: &Path) -> bool {
        path.is_dir()
    }
    
    fn read_directory(&self, path: &Path) -> io::Result<Vec<std::path::PathBuf>> {
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(path)? {
            entries.push(entry?.path());
        }
        Ok(entries)
    }
    
    fn metadata(&self, path: &Path) -> io::Result<std::fs::Metadata> {
        std::fs::metadata(path)
    }
}

/// In-memory file system for testing
#[cfg(test)]
pub struct MemoryFileSystem {
    files: std::collections::HashMap<std::path::PathBuf, String>,
}

#[cfg(test)]
impl MemoryFileSystem {
    pub fn new() -> Self {
        Self {
            files: std::collections::HashMap::new(),
        }
    }
    
    pub fn add_file(&mut self, path: std::path::PathBuf, content: String) {
        self.files.insert(path, content);
    }
}

#[cfg(test)]
impl FileSystem for MemoryFileSystem {
    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        self.files
            .get(path)
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "File not found"))
    }
    
    fn exists(&self, path: &Path) -> bool {
        self.files.contains_key(path)
    }
    
    fn is_file(&self, path: &Path) -> bool {
        self.files.contains_key(path)
    }
    
    fn is_directory(&self, _path: &Path) -> bool {
        // Simplified for testing - no directories in memory filesystem
        false
    }
    
    fn read_directory(&self, _path: &Path) -> io::Result<Vec<std::path::PathBuf>> {
        // Simplified for testing
        Ok(Vec::new())
    }
    
    fn metadata(&self, _path: &Path) -> io::Result<std::fs::Metadata> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Metadata not supported in memory filesystem",
        ))
    }
}