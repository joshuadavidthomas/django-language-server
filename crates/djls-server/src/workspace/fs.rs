use std::path::Path;
use vfs::{MemoryFS, PhysicalFS, VfsPath, VfsResult, VfsMetadata, SeekAndRead, SeekAndWrite};
use std::time::SystemTime;
use std::collections::BTreeSet;

/// A file system for managing workspace file content with manual layer management.
/// 
/// The file system uses two separate layers:
/// - Memory layer: for unsaved edits and temporary content
/// - Physical layer: for disk-based files
/// 
/// When reading, the memory layer is checked first, falling back to physical layer.
/// Write operations go to memory layer only, preserving original files on disk.
/// Clearing memory layer allows immediate fallback to physical layer without whiteout markers.
#[derive(Debug)]
pub struct FileSystem {
    memory: VfsPath,
    physical: VfsPath,
}

impl FileSystem {
    /// Create a new FileSystem with separate memory and physical layers
    pub fn new<P: AsRef<Path>>(root_path: P) -> VfsResult<Self> {
        let memory = VfsPath::new(MemoryFS::new());
        let physical = VfsPath::new(PhysicalFS::new(root_path.as_ref()));
        
        Ok(FileSystem { memory, physical })
    }
    
    /// Read file content as string (checks unsaved edits first, then disk)
    /// 
    /// This is a high-level convenience method for LSP operations.
    /// Checks memory layer (unsaved edits) first, then falls back to physical layer (disk).
    pub fn read_to_string(&self, path: &str) -> VfsResult<String> {
        let memory_path = self.memory.join(path)?;
        if memory_path.exists().unwrap_or(false) {
            return memory_path.read_to_string();
        }
        
        let physical_path = self.physical.join(path)?;
        physical_path.read_to_string()
    }
    
    /// Write string content to memory layer (tracks unsaved edits from editor)
    /// 
    /// This is a high-level convenience method for LSP operations.
    /// Writes to memory layer only, preserving the original file on disk.
    /// The editor is responsible for actual disk writes via didSave.
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
    
    /// Discard unsaved changes for a file (removes from memory layer)
    /// 
    /// This is a high-level convenience method for LSP operations.
    /// After discarding, reads will fall back to the physical layer (disk state).
    /// Typically called when editor sends didClose without saving.
    pub fn discard_changes(&self, path: &str) -> VfsResult<()> {
        let memory_path = self.memory.join(path)?;
        
        // Only remove if it exists in memory layer
        if memory_path.exists().unwrap_or(false) {
            memory_path.remove_file()?;
        }
        
        Ok(())
    }
    
    /// Check if a path exists in either layer
    /// Checks memory layer first, then physical layer
    pub fn exists(&self, path: &str) -> VfsResult<bool> {
        let memory_path = self.memory.join(path)?;
        if memory_path.exists().unwrap_or(false) {
            return Ok(true);
        }
        
        let physical_path = self.physical.join(path)?;
        Ok(physical_path.exists().unwrap_or(false))
    }
    

}

// Implement vfs::FileSystem trait to make our FileSystem compatible with VfsPath
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
