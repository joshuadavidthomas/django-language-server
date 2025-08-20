use std::path::Path;
use vfs::{MemoryFS, PhysicalFS, VfsPath, VfsResult, VfsMetadata, SeekAndRead, SeekAndWrite};
use std::time::SystemTime;
use std::collections::BTreeSet;

/// A custom VFS implementation optimized for Language Server Protocol operations.
/// 
/// This FileSystem provides a dual-layer architecture specifically designed for LSP needs:
/// - **Memory layer**: Tracks unsaved editor changes and temporary content
/// - **Physical layer**: Provides access to the actual files on disk
/// 
/// ## Design Rationale
/// 
/// This custom implementation was chosen over existing overlay filesystems because it provides:
/// - Proper deletion semantics without whiteout markers
/// - LSP-specific behavior for handling editor lifecycle events
/// - Predictable exists() behavior that aligns with LSP client expectations
/// - Full control over layer management for optimal language server performance
/// 
/// ## Layer Management
/// 
/// - **Read operations**: Check memory layer first, then fall back to physical layer
/// - **Write operations**: Always go to memory layer only, preserving original disk files
/// - **Existence checks**: Return true if file exists in either layer
/// - **Deletions**: Remove from memory layer only (no whiteout markers)
/// 
/// ## LSP Integration
/// 
/// The FileSystem is designed around the LSP document lifecycle:
/// - `didOpen`: File tracking begins (no immediate memory allocation)
/// - `didChange`: Content changes stored in memory layer via [`write_string`]
/// - `didSave`: Editor saves to disk; memory layer can be cleared via [`discard_changes`]
/// - `didClose`: Memory layer cleaned up via [`discard_changes`]
/// 
/// This ensures language analysis always uses the current editor state while
/// preserving the original files until the editor explicitly saves them.
/// 
/// [`write_string`]: FileSystem::write_string
/// [`discard_changes`]: FileSystem::discard_changes
#[derive(Debug)]
pub struct FileSystem {
    memory: VfsPath,
    physical: VfsPath,
}

impl FileSystem {
    /// Creates a new FileSystem rooted at the specified path.
    /// 
    /// The FileSystem will provide access to files within the root path through both
    /// the physical layer (disk) and memory layer (unsaved changes). All file paths
    /// used with this FileSystem should be relative to this root.
    /// 
    /// # Arguments
    /// 
    /// * `root_path` - The workspace root directory (typically from LSP initialization)
    /// 
    /// # Example
    /// 
    /// ```rust,ignore
    /// let fs = FileSystem::new("/path/to/django/project")?;
    /// ```
    pub fn new<P: AsRef<Path>>(root_path: P) -> VfsResult<Self> {
        let memory = VfsPath::new(MemoryFS::new());
        let physical = VfsPath::new(PhysicalFS::new(root_path.as_ref()));
        
        Ok(FileSystem { memory, physical })
    }
    
    /// Reads file content as a UTF-8 string, prioritizing unsaved editor changes.
    /// 
    /// This method implements the core LSP behavior of always providing the most current
    /// view of a file. It checks the memory layer first (which contains any unsaved
    /// changes from `textDocument/didChange` events), then falls back to reading from
    /// the physical disk if no memory version exists.
    /// 
    /// # Arguments
    /// 
    /// * `path` - Relative path from the workspace root (e.g., "myapp/models.py")
    /// 
    /// # Returns
    /// 
    /// The current content of the file as a string, or an error if the file doesn't
    /// exist in either layer or cannot be read.
    /// 
    /// # Example
    /// 
    /// ```rust,ignore
    /// // This will return unsaved changes if the file was modified in the editor
    /// let content = fs.read_to_string("templates/base.html")?;
    /// ```
    pub fn read_to_string(&self, path: &str) -> VfsResult<String> {
        let memory_path = self.memory.join(path)?;
        if memory_path.exists().unwrap_or(false) {
            return memory_path.read_to_string();
        }
        
        let physical_path = self.physical.join(path)?;
        physical_path.read_to_string()
    }
    
    /// Writes content to the memory layer to track unsaved editor changes.
    /// 
    /// This method is typically called in response to `textDocument/didChange` events
    /// from the LSP client. It stores the content in the memory layer only, ensuring
    /// that subsequent reads via [`read_to_string`] will return this updated content
    /// while preserving the original file on disk.
    /// 
    /// The editor client is responsible for actual disk writes when the user saves
    /// the file (`textDocument/didSave`). The language server only tracks the
    /// in-memory changes for analysis purposes.
    /// 
    /// # Arguments
    /// 
    /// * `path` - Relative path from the workspace root
    /// * `content` - The new file content as provided by the editor
    /// 
    /// # Example
    /// 
    /// ```rust,ignore
    /// // Store unsaved changes from textDocument/didChange
    /// fs.write_string("models.py", "class User(models.Model):\n    pass")?;
    /// ```
    /// 
    /// [`read_to_string`]: FileSystem::read_to_string
    pub fn write_string(&self, path: &str, content: &str) -> VfsResult<()> {
        let memory_path = self.memory.join(path)?;
        
        // Ensure parent directories exist in memory layer
        let parent = memory_path.parent();
        if !parent.is_root() && !parent.exists().unwrap_or(false) {
            parent.create_dir_all()?;
        }
        
        memory_path.create_file()?.write_all(content.as_bytes())?;
        Ok(())
    }
    
    /// Discards unsaved changes by removing the file from the memory layer.
    /// 
    /// This method is typically called in response to `textDocument/didSave` (after
    /// the editor has written changes to disk) or `textDocument/didClose` (when the
    /// user closes a file without saving). After calling this method, subsequent
    /// reads will return the physical file content from disk.
    /// 
    /// This operation is safe to call even if the file doesn't exist in the memory
    /// layer - it will simply have no effect.
    /// 
    /// # Arguments
    /// 
    /// * `path` - Relative path from the workspace root
    /// 
    /// # Example
    /// 
    /// ```rust,ignore
    /// // After textDocument/didSave, discard the memory copy since it's now on disk
    /// fs.discard_changes("models.py")?;
    /// ```
    pub fn discard_changes(&self, path: &str) -> VfsResult<()> {
        let memory_path = self.memory.join(path)?;
        
        // Only remove if it exists in memory layer
        if memory_path.exists().unwrap_or(false) {
            memory_path.remove_file()?;
        }
        
        Ok(())
    }
    
    /// Checks if a file exists in either the memory or physical layer.
    /// 
    /// Returns `true` if the file exists in the memory layer (unsaved changes)
    /// or in the physical layer (on disk). This provides the LSP client's
    /// expected view of file existence, including files that exist only as
    /// unsaved editor content.
    /// 
    /// # Arguments
    /// 
    /// * `path` - Relative path from the workspace root
    /// 
    /// # Example
    /// 
    /// ```rust,ignore
    /// if fs.exists("settings.py")? {
    ///     let content = fs.read_to_string("settings.py")?;
    ///     // Process file content...
    /// }
    /// ```
    pub fn exists(&self, path: &str) -> VfsResult<bool> {
        let memory_path = self.memory.join(path)?;
        if memory_path.exists().unwrap_or(false) {
            return Ok(true);
        }
        
        let physical_path = self.physical.join(path)?;
        Ok(physical_path.exists().unwrap_or(false))
    }
    

}

/// Implementation of the `vfs::FileSystem` trait for VfsPath compatibility.
/// 
/// This trait implementation allows our custom FileSystem to be used with VfsPath
/// while maintaining the dual-layer architecture. All operations respect the
/// memory-over-physical priority, ensuring LSP semantics are preserved even when
/// accessed through the generic VFS interface.
/// 
/// Most LSP code should prefer the inherent methods ([`read_to_string`], [`write_string`], 
/// [`discard_changes`]) as they provide more explicit semantics for language server operations.
/// 
/// [`read_to_string`]: FileSystem::read_to_string
/// [`write_string`]: FileSystem::write_string  
/// [`discard_changes`]: FileSystem::discard_changes
impl vfs::FileSystem for FileSystem {
    fn read_dir(&self, path: &str) -> VfsResult<Box<dyn Iterator<Item = String> + Send>> {
        // Collect entries from both layers and merge them
        let mut entries = BTreeSet::new();
        
        // Add memory layer entries
        let memory_path = self.memory.join(path)?;
        if memory_path.exists().unwrap_or(false) {
            for entry in memory_path.read_dir()? {
                entries.insert(entry.filename());
            }
        }
        
        // Add physical layer entries
        let physical_path = self.physical.join(path)?;
        if physical_path.exists().unwrap_or(false) {
            for entry in physical_path.read_dir()? {
                entries.insert(entry.filename());
            }
        }
        
        // Return merged, deduplicated entries
        Ok(Box::new(entries.into_iter()))
    }
    
    fn create_dir(&self, path: &str) -> VfsResult<()> {
        // Create directory in memory layer only
        let memory_path = self.memory.join(path)?;
        memory_path.create_dir()
    }
    
    fn open_file(&self, path: &str) -> VfsResult<Box<dyn SeekAndRead + Send>> {
        // Check memory layer first, then physical
        let memory_path = self.memory.join(path)?;
        if memory_path.exists().unwrap_or(false) {
            return memory_path.open_file();
        }
        
        let physical_path = self.physical.join(path)?;
        physical_path.open_file()
    }
    
    fn create_file(&self, path: &str) -> VfsResult<Box<dyn SeekAndWrite + Send>> {
        // Create file in memory layer only
        let memory_path = self.memory.join(path)?;
        
        // Ensure parent directories exist in memory layer
        let parent = memory_path.parent();
        if !parent.is_root() && !parent.exists().unwrap_or(false) {
            parent.create_dir_all()?;
        }
        
        memory_path.create_file()
    }
    
    fn append_file(&self, path: &str) -> VfsResult<Box<dyn SeekAndWrite + Send>> {
        // For append, we need to check if file exists and copy to memory if needed
        let memory_path = self.memory.join(path)?;
        
        if !memory_path.exists().unwrap_or(false) {
            // Copy from physical to memory first if it exists
            let physical_path = self.physical.join(path)?;
            if physical_path.exists().unwrap_or(false) {
                let content = physical_path.read_to_string()?;
                self.write_string(path, &content)?;
            }
        }
        
        memory_path.append_file()
    }
    
    fn metadata(&self, path: &str) -> VfsResult<VfsMetadata> {
        // Check memory layer first, then physical
        let memory_path = self.memory.join(path)?;
        if memory_path.exists().unwrap_or(false) {
            return memory_path.metadata();
        }
        
        let physical_path = self.physical.join(path)?;
        physical_path.metadata()
    }
    
    fn set_creation_time(&self, path: &str, time: SystemTime) -> VfsResult<()> {
        // Set on memory layer if exists, otherwise physical
        let memory_path = self.memory.join(path)?;
        if memory_path.exists().unwrap_or(false) {
            return memory_path.set_creation_time(time);
        }
        
        let physical_path = self.physical.join(path)?;
        physical_path.set_creation_time(time)
    }
    
    fn set_modification_time(&self, path: &str, time: SystemTime) -> VfsResult<()> {
        // Set on memory layer if exists, otherwise physical
        let memory_path = self.memory.join(path)?;
        if memory_path.exists().unwrap_or(false) {
            return memory_path.set_modification_time(time);
        }
        
        let physical_path = self.physical.join(path)?;
        physical_path.set_modification_time(time)
    }
    
    fn set_access_time(&self, path: &str, time: SystemTime) -> VfsResult<()> {
        // Set on memory layer if exists, otherwise physical
        let memory_path = self.memory.join(path)?;
        if memory_path.exists().unwrap_or(false) {
            return memory_path.set_access_time(time);
        }
        
        let physical_path = self.physical.join(path)?;
        physical_path.set_access_time(time)
    }
    
    fn exists(&self, path: &str) -> VfsResult<bool> {
        // Call our inherent method which handles layer checking properly
        FileSystem::exists(self, path)
    }
    
    fn remove_file(&self, path: &str) -> VfsResult<()> {
        // Only remove from memory layer - this aligns with our LSP semantics
        let memory_path = self.memory.join(path)?;
        if memory_path.exists().unwrap_or(false) {
            memory_path.remove_file()?;
        }
        Ok(())
    }
    
    fn remove_dir(&self, path: &str) -> VfsResult<()> {
        // Only remove from memory layer - this aligns with our LSP semantics
        let memory_path = self.memory.join(path)?;
        if memory_path.exists().unwrap_or(false) {
            memory_path.remove_dir()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;
    
    #[test]
    fn test_new_vfs() {
        let temp_dir = TempDir::new().unwrap();
        let _vfs = FileSystem::new(temp_dir.path()).unwrap();
        
        // FileSystem should be created successfully
        // (removed testing of internal-exposing methods)
    }
    
    #[test]
    fn test_read_physical_file() {
        let temp_dir = TempDir::new().unwrap();
        let test_file = temp_dir.path().join("test.html");
        fs::write(&test_file, "physical content").unwrap();
        
        let vfs = FileSystem::new(temp_dir.path()).unwrap();
        let content = vfs.read_to_string("test.html").unwrap();
        
        assert_eq!(content, "physical content");
    }
    
    #[test]
    fn test_write_string_and_read_to_string() {
        let temp_dir = TempDir::new().unwrap();
        let vfs = FileSystem::new(temp_dir.path()).unwrap();
        
        vfs.write_string("test.html", "memory content").unwrap();
        let content = vfs.read_to_string("test.html").unwrap();
        
        assert_eq!(content, "memory content");
    }
    
    #[test]
    fn test_memory_layer_priority() {
        let temp_dir = TempDir::new().unwrap();
        let test_file = temp_dir.path().join("test.html");
        fs::write(&test_file, "physical content").unwrap();
        
        let vfs = FileSystem::new(temp_dir.path()).unwrap();
        
        // First read should get physical content
        assert_eq!(vfs.read_to_string("test.html").unwrap(), "physical content");
        
        // Write to memory layer
        vfs.write_string("test.html", "memory content").unwrap();
        
        // Now read should get memory content (higher priority)
        assert_eq!(vfs.read_to_string("test.html").unwrap(), "memory content");
        
        // Physical file should remain unchanged
        let physical_content = fs::read_to_string(&test_file).unwrap();
        assert_eq!(physical_content, "physical content");
    }
    
    #[test]
    fn test_discard_changes_fallback() {
        let temp_dir = TempDir::new().unwrap();
        let test_file = temp_dir.path().join("test.html");
        fs::write(&test_file, "physical content").unwrap();
        
        let vfs = FileSystem::new(temp_dir.path()).unwrap();
        
        // Write to memory
        vfs.write_string("test.html", "memory content").unwrap();
        assert_eq!(vfs.read_to_string("test.html").unwrap(), "memory content");
        
        // Clear memory
        vfs.discard_changes("test.html").unwrap();
        
        // Should now read from physical layer
        assert_eq!(vfs.read_to_string("test.html").unwrap(), "physical content");
    }
    
    #[test]
    fn test_exists() {
        let temp_dir = TempDir::new().unwrap();
        let test_file = temp_dir.path().join("physical.html");
        fs::write(&test_file, "content").unwrap();
        
        let vfs = FileSystem::new(temp_dir.path()).unwrap();
        
        // Physical file exists
        assert!(vfs.exists("physical.html").unwrap());
        
        // Memory file exists after writing
        vfs.write_string("memory.html", "content").unwrap();
        assert!(vfs.exists("memory.html").unwrap());
        
        // Non-existent file
        assert!(!vfs.exists("nonexistent.html").unwrap());
    }
    
    #[test]
    fn test_discard_changes_nonexistent() {
        let temp_dir = TempDir::new().unwrap();
        let vfs = FileSystem::new(temp_dir.path()).unwrap();
        
        // Should not error when clearing non-existent memory file
        vfs.discard_changes("nonexistent.html").unwrap();
    }

    #[test]
    fn test_vfs_trait_implementation() {
        let temp_dir = TempDir::new().unwrap();
        let test_file = temp_dir.path().join("trait_test.html");
        fs::write(&test_file, "trait physical content").unwrap();
        
        let filesystem = FileSystem::new(temp_dir.path()).unwrap();
        
        // Test our trait implementation directly instead of through VfsPath
        // VfsPath would create absolute paths which our security validation rejects
        use vfs::FileSystem as VfsFileSystemTrait;
        
        // Test exists through trait
        assert!(filesystem.exists("trait_test.html").unwrap());
        
        // Test reading through trait
        let mut file = filesystem.open_file("trait_test.html").unwrap();
        let mut content = String::new();
        use std::io::Read;
        file.read_to_string(&mut content).unwrap();
        assert_eq!(content, "trait physical content");
        
        // Test creating file through trait
        let mut new_file = filesystem.create_file("new_trait_file.html").unwrap();
        use std::io::Write;
        new_file.write_all(b"trait memory content").unwrap();
        drop(new_file); // Close the file to flush writes
        
        // Should read from memory layer
        let mut memory_file = filesystem.open_file("new_trait_file.html").unwrap();
        let mut memory_content = String::new();
        memory_file.read_to_string(&mut memory_content).unwrap();
        assert_eq!(memory_content, "trait memory content");
        
        // Physical file should not exist (memory layer only)
        let physical_new_file = temp_dir.path().join("new_trait_file.html");
        assert!(!physical_new_file.exists());
    }
    
    #[test]
    fn test_directory_merge() {
        let temp_dir = TempDir::new().unwrap();
        let test_dir = temp_dir.path().join("testdir");
        fs::create_dir(&test_dir).unwrap();
        fs::write(test_dir.join("physical1.txt"), "content").unwrap();
        fs::write(test_dir.join("physical2.txt"), "content").unwrap();
        
        let filesystem = FileSystem::new(temp_dir.path()).unwrap();
        
        // Create memory layer files using the trait methods
        use vfs::FileSystem as VfsFileSystemTrait;
        filesystem.create_dir("testdir").ok(); // May already exist
        
        let mut mem1 = filesystem.create_file("testdir/memory1.txt").unwrap();
        use std::io::Write;
        mem1.write_all(b"memory content").unwrap();
        drop(mem1);
        
        let mut mem2 = filesystem.create_file("testdir/memory2.txt").unwrap();
        mem2.write_all(b"memory content").unwrap();
        drop(mem2);
        
        // Read directory should show merged content
        let entries: Vec<String> = filesystem.read_dir("testdir").unwrap().collect();
        
        // Should contain both physical and memory files
        assert!(entries.contains(&"physical1.txt".to_string()));
        assert!(entries.contains(&"physical2.txt".to_string()));
        assert!(entries.contains(&"memory1.txt".to_string()));
        assert!(entries.contains(&"memory2.txt".to_string()));
        assert_eq!(entries.len(), 4);
    }
}
