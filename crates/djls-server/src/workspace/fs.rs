use std::path::Path;
use vfs::{MemoryFS, PhysicalFS, VfsPath, VfsResult, VfsMetadata, SeekAndRead, SeekAndWrite};
use std::time::SystemTime;

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
    
    /// Read content from the file system
    /// Checks memory layer first, then falls back to physical layer
    pub fn read(&self, path: &str) -> VfsResult<String> {
        let memory_path = self.memory.join(path)?;
        if memory_path.exists()? {
            return memory_path.read_to_string();
        }
        
        let physical_path = self.physical.join(path)?;
        physical_path.read_to_string()
    }
    
    /// Write content to memory layer only
    /// This preserves the original file on disk while allowing edits
    pub fn write_memory(&self, path: &str, content: &str) -> VfsResult<()> {
        let memory_path = self.memory.join(path)?;
        
        // Ensure parent directories exist in memory layer
        let parent = memory_path.parent();
        if !parent.is_root() && !parent.exists()? {
            parent.create_dir_all()?;
        }
        
        memory_path.create_file()?.write_all(content.as_bytes())?;
        Ok(())
    }
    
    /// Clear memory layer content for a specific path
    /// After clearing, reads will fall back to physical layer
    /// No whiteout markers are created - direct memory layer management
    pub fn clear_memory(&self, path: &str) -> VfsResult<()> {
        let memory_path = self.memory.join(path)?;
        
        // Only remove if it exists in memory layer
        if memory_path.exists()? {
            memory_path.remove_file()?;
        }
        
        Ok(())
    }
    
    /// Check if a path exists in either layer
    /// Checks memory layer first, then physical layer
    pub fn exists(&self, path: &str) -> VfsResult<bool> {
        let memory_path = self.memory.join(path)?;
        if memory_path.exists()? {
            return Ok(true);
        }
        
        let physical_path = self.physical.join(path)?;
        physical_path.exists()
    }
    
    /// Get memory layer root for advanced operations
    pub fn memory_root(&self) -> VfsPath {
        self.memory.clone()
    }
    
    /// Get physical layer root for advanced operations
    pub fn physical_root(&self) -> VfsPath {
        self.physical.clone()
    }
    
    /// Get root for backward compatibility (returns memory root)
    pub fn root(&self) -> VfsPath {
        self.memory.clone()
    }
}

// Implement vfs::FileSystem trait to make our FileSystem compatible with VfsPath
impl vfs::FileSystem for FileSystem {
    fn read_dir(&self, path: &str) -> VfsResult<Box<dyn Iterator<Item = String> + Send>> {
        // Check memory layer first, then physical
        let memory_path = self.memory.join(path)?;
        if memory_path.exists()? {
            return Ok(Box::new(memory_path.read_dir()?.map(|p| p.filename())));
        }
        
        let physical_path = self.physical.join(path)?;
        Ok(Box::new(physical_path.read_dir()?.map(|p| p.filename())))
    }
    
    fn create_dir(&self, path: &str) -> VfsResult<()> {
        // Create directory in memory layer only
        let memory_path = self.memory.join(path)?;
        memory_path.create_dir()
    }
    
    fn open_file(&self, path: &str) -> VfsResult<Box<dyn SeekAndRead + Send>> {
        // Check memory layer first, then physical
        let memory_path = self.memory.join(path)?;
        if memory_path.exists()? {
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
        if !parent.is_root() && !parent.exists()? {
            parent.create_dir_all()?;
        }
        
        memory_path.create_file()
    }
    
    fn append_file(&self, path: &str) -> VfsResult<Box<dyn SeekAndWrite + Send>> {
        // For append, we need to check if file exists and copy to memory if needed
        let memory_path = self.memory.join(path)?;
        
        if !memory_path.exists()? {
            // Copy from physical to memory first if it exists
            let physical_path = self.physical.join(path)?;
            if physical_path.exists()? {
                let content = physical_path.read_to_string()?;
                self.write_memory(path, &content)?;
            }
        }
        
        memory_path.append_file()
    }
    
    fn metadata(&self, path: &str) -> VfsResult<VfsMetadata> {
        // Check memory layer first, then physical
        let memory_path = self.memory.join(path)?;
        if memory_path.exists()? {
            return memory_path.metadata();
        }
        
        let physical_path = self.physical.join(path)?;
        physical_path.metadata()
    }
    
    fn set_creation_time(&self, path: &str, time: SystemTime) -> VfsResult<()> {
        // Set on memory layer if exists, otherwise physical
        let memory_path = self.memory.join(path)?;
        if memory_path.exists()? {
            return memory_path.set_creation_time(time);
        }
        
        let physical_path = self.physical.join(path)?;
        physical_path.set_creation_time(time)
    }
    
    fn set_modification_time(&self, path: &str, time: SystemTime) -> VfsResult<()> {
        // Set on memory layer if exists, otherwise physical
        let memory_path = self.memory.join(path)?;
        if memory_path.exists()? {
            return memory_path.set_modification_time(time);
        }
        
        let physical_path = self.physical.join(path)?;
        physical_path.set_modification_time(time)
    }
    
    fn set_access_time(&self, path: &str, time: SystemTime) -> VfsResult<()> {
        // Set on memory layer if exists, otherwise physical
        let memory_path = self.memory.join(path)?;
        if memory_path.exists()? {
            return memory_path.set_access_time(time);
        }
        
        let physical_path = self.physical.join(path)?;
        physical_path.set_access_time(time)
    }
    
    fn exists(&self, path: &str) -> VfsResult<bool> {
        // Use our existing method which already handles layer checking
        self.exists(path)
    }
    
    fn remove_file(&self, path: &str) -> VfsResult<()> {
        // Only remove from memory layer - this aligns with our LSP semantics
        let memory_path = self.memory.join(path)?;
        if memory_path.exists()? {
            memory_path.remove_file()?;
        }
        Ok(())
    }
    
    fn remove_dir(&self, path: &str) -> VfsResult<()> {
        // Only remove from memory layer - this aligns with our LSP semantics
        let memory_path = self.memory.join(path)?;
        if memory_path.exists()? {
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
        let vfs = FileSystem::new(temp_dir.path()).unwrap();
        
        // Should be able to get roots
        let _memory_root = vfs.memory_root();
        let _physical_root = vfs.physical_root();
        let _root = vfs.root();
    }
    
    #[test]
    fn test_read_physical_file() {
        let temp_dir = TempDir::new().unwrap();
        let test_file = temp_dir.path().join("test.html");
        fs::write(&test_file, "physical content").unwrap();
        
        let vfs = FileSystem::new(temp_dir.path()).unwrap();
        let content = vfs.read("test.html").unwrap();
        
        assert_eq!(content, "physical content");
    }
    
    #[test]
    fn test_write_memory_and_read() {
        let temp_dir = TempDir::new().unwrap();
        let vfs = FileSystem::new(temp_dir.path()).unwrap();
        
        vfs.write_memory("test.html", "memory content").unwrap();
        let content = vfs.read("test.html").unwrap();
        
        assert_eq!(content, "memory content");
    }
    
    #[test]
    fn test_memory_layer_priority() {
        let temp_dir = TempDir::new().unwrap();
        let test_file = temp_dir.path().join("test.html");
        fs::write(&test_file, "physical content").unwrap();
        
        let vfs = FileSystem::new(temp_dir.path()).unwrap();
        
        // First read should get physical content
        assert_eq!(vfs.read("test.html").unwrap(), "physical content");
        
        // Write to memory layer
        vfs.write_memory("test.html", "memory content").unwrap();
        
        // Now read should get memory content (higher priority)
        assert_eq!(vfs.read("test.html").unwrap(), "memory content");
        
        // Physical file should remain unchanged
        let physical_content = fs::read_to_string(&test_file).unwrap();
        assert_eq!(physical_content, "physical content");
    }
    
    #[test]
    fn test_clear_memory_fallback() {
        let temp_dir = TempDir::new().unwrap();
        let test_file = temp_dir.path().join("test.html");
        fs::write(&test_file, "physical content").unwrap();
        
        let vfs = FileSystem::new(temp_dir.path()).unwrap();
        
        // Write to memory
        vfs.write_memory("test.html", "memory content").unwrap();
        assert_eq!(vfs.read("test.html").unwrap(), "memory content");
        
        // Clear memory
        vfs.clear_memory("test.html").unwrap();
        
        // Should now read from physical layer
        assert_eq!(vfs.read("test.html").unwrap(), "physical content");
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
        vfs.write_memory("memory.html", "content").unwrap();
        assert!(vfs.exists("memory.html").unwrap());
        
        // Non-existent file
        assert!(!vfs.exists("nonexistent.html").unwrap());
    }
    
    #[test]
    fn test_clear_memory_nonexistent() {
        let temp_dir = TempDir::new().unwrap();
        let vfs = FileSystem::new(temp_dir.path()).unwrap();
        
        // Should not error when clearing non-existent memory file
        vfs.clear_memory("nonexistent.html").unwrap();
    }

    #[test]
    fn test_vfs_trait_implementation() {
        let temp_dir = TempDir::new().unwrap();
        let test_file = temp_dir.path().join("trait_test.html");
        fs::write(&test_file, "trait physical content").unwrap();
        
        let filesystem = FileSystem::new(temp_dir.path()).unwrap();
        let vfs_path = VfsPath::new(filesystem);
        
        // Test that our FileSystem works as a vfs::FileSystem trait
        assert!(vfs_path.join("trait_test.html").unwrap().exists().unwrap());
        
        // Test reading through trait
        let content = vfs_path.join("trait_test.html").unwrap().read_to_string().unwrap();
        assert_eq!(content, "trait physical content");
        
        // Test creating file through trait
        let new_file = vfs_path.join("new_trait_file.html").unwrap();
        new_file.create_file().unwrap().write_all(b"trait memory content").unwrap();
        
        // Should read from memory layer
        let memory_content = new_file.read_to_string().unwrap();
        assert_eq!(memory_content, "trait memory content");
        
        // Physical file should not exist (memory layer only)
        let physical_new_file = temp_dir.path().join("new_trait_file.html");
        assert!(!physical_new_file.exists());
    }
}
