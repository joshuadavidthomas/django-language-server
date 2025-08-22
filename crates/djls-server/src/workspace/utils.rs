use std::path::PathBuf;

use percent_encoding::percent_decode_str;
use tower_lsp_server::lsp_types::InitializeParams;
use tower_lsp_server::lsp_types::Uri;

/// Determines the project root path from initialization parameters.
///
/// Prioritizes workspace folders from the LSP client, then falls back
/// to the current directory if no workspace folders are provided.
pub fn get_project_path(params: &InitializeParams) -> Option<PathBuf> {
    // First try to get the workspace folder from LSP client
    let workspace_path = params
        .workspace_folders
        .as_ref()
        .and_then(|folders| {
            tracing::debug!("Found {} workspace folders", folders.len());
            folders.first()
        })
        .and_then(|folder| {
            tracing::debug!("Processing workspace folder URI: {}", folder.uri.as_str());
            uri_to_pathbuf(&folder.uri)
        });
    
    if let Some(path) = workspace_path {
        tracing::info!("Using workspace folder as project path: {}", path.display());
        return Some(path);
    }
    
    // Fall back to current directory if no workspace folders provided
    let current_dir = std::env::current_dir().ok();
    if let Some(ref dir) = current_dir {
        tracing::info!("No workspace folders provided, using current directory: {}", dir.display());
    } else {
        tracing::warn!("No workspace folders and current directory unavailable");
    }
    current_dir
}

/// Converts a `file:` URI into an absolute `PathBuf`.
fn uri_to_pathbuf(uri: &Uri) -> Option<PathBuf> {
    // Check if the scheme is "file"
    if uri.scheme().is_none_or(|s| s.as_str() != "file") {
        return None;
    }

    // Get the path part as a string
    let encoded_path_str = uri.path().as_str();

    // Decode the percent-encoded path string
    let decoded_path_cow = percent_decode_str(encoded_path_str).decode_utf8_lossy();
    let path_str = decoded_path_cow.as_ref();

    #[cfg(windows)]
    let path_str = {
        // Remove leading '/' for paths like /C:/...
        path_str.strip_prefix('/').unwrap_or(path_str)
    };

    Some(PathBuf::from(path_str))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower_lsp_server::lsp_types::WorkspaceFolder;

    #[test]
    fn test_get_project_path_prefers_workspace_folders() {
        // Create a mock InitializeParams with workspace folders
        let workspace_uri: Uri = "file:///workspace/project".parse().unwrap();
        let workspace_folder = WorkspaceFolder {
            uri: workspace_uri.clone(),
            name: "project".to_string(),
        };
        
        let mut params = InitializeParams::default();
        params.workspace_folders = Some(vec![workspace_folder]);
        
        // Get the project path
        let result = get_project_path(&params);
        
        // Should return the workspace folder path, not current_dir
        assert!(result.is_some());
        let path = result.unwrap();
        assert_eq!(path, PathBuf::from("/workspace/project"));
    }

    #[test]
    fn test_get_project_path_falls_back_to_current_dir() {
        // Create InitializeParams without workspace folders
        let params = InitializeParams::default();
        
        // Get the project path
        let result = get_project_path(&params);
        
        // Should return current directory as fallback
        assert!(result.is_some());
        let path = result.unwrap();
        let current = std::env::current_dir().unwrap();
        assert_eq!(path, current);
    }

    #[test]
    fn test_uri_to_pathbuf() {
        // Test valid file URI
        let uri: Uri = "file:///home/user/project".parse().unwrap();
        let result = uri_to_pathbuf(&uri);
        assert_eq!(result, Some(PathBuf::from("/home/user/project")));
        
        // Test non-file URI  
        let uri: Uri = "https://example.com".parse().unwrap();
        let result = uri_to_pathbuf(&uri);
        assert_eq!(result, None);
    }
}
