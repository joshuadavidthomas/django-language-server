use percent_encoding::percent_decode_str;
use std::path::PathBuf;
use tower_lsp_server::lsp_types::{InitializeParams, Uri};

/// Determines the project root path from initialization parameters.
///
/// Tries the current directory first, then falls back to the first workspace folder.
pub fn get_project_path(params: &InitializeParams) -> Option<PathBuf> {
    // Try current directory first
    std::env::current_dir().ok().or_else(|| {
        // Fall back to the first workspace folder URI
        params
            .workspace_folders
            .as_ref()
            .and_then(|folders| folders.first())
            .and_then(|folder| uri_to_pathbuf(&folder.uri))
    })
}

/// Converts a `file:` URI into an absolute `PathBuf`.
fn uri_to_pathbuf(uri: &Uri) -> Option<PathBuf> {
    // Check if the scheme is "file"
    if uri.scheme().map_or(true, |s| s.as_str() != "file") {
        return None;
    }

    // Get the path part as a string
    let encoded_path_str = uri.path().as_str();

    // Decode the percent-encoded path string
    let decoded_path_cow = percent_decode_str(encoded_path_str).decode_utf8_lossy();

    #[cfg(unix)]
    let path_str = decoded_path_cow.as_ref();
    #[cfg(windows)]
    // Remove leading '/' for paths like /C:/...
    let path_str = path_str.strip_prefix('/').unwrap_or(path_str);

    Some(PathBuf::from(path_str))
}
