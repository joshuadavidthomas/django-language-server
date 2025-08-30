//! Integration tests for the LSP server's overlay → revision → invalidation flow
//!
//! These tests verify the complete two-layer architecture:
//! - Layer 1: LSP overlays (in-memory document state)
//! - Layer 2: Salsa database with revision tracking
//!
//! The tests ensure that document changes properly invalidate cached queries
//! and that overlays take precedence over disk content.

use std::path::PathBuf;
use std::sync::Arc;

use djls_server::DjangoLanguageServer;
use tempfile::TempDir;
use tower_lsp_server::lsp_types::{
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    InitializeParams, InitializedParams, TextDocumentContentChangeEvent, TextDocumentIdentifier,
    TextDocumentItem, VersionedTextDocumentIdentifier, WorkspaceFolder,
};
use tower_lsp_server::LanguageServer;
use url::Url;

/// Test helper that manages an LSP server instance for testing
struct TestServer {
    server: DjangoLanguageServer,
    _temp_dir: TempDir,
    workspace_root: PathBuf,
}

impl TestServer {
    /// Create a new test server with a temporary workspace
    async fn new() -> Self {
        // Create temporary directory for test workspace
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let workspace_root = temp_dir.path().to_path_buf();

        // Set up logging
        let (_non_blocking, guard) = tracing_appender::non_blocking(std::io::sink());

        // Create server (guard is moved into server, so we return it too)
        let server = DjangoLanguageServer::new(guard);

        // Initialize the server
        let workspace_folder = WorkspaceFolder {
            uri: format!("file://{}", workspace_root.display())
                .parse()
                .unwrap(),
            name: "test_workspace".to_string(),
        };

        let init_params = InitializeParams {
            workspace_folders: Some(vec![workspace_folder]),
            ..Default::default()
        };

        server
            .initialize(init_params)
            .await
            .expect("Failed to initialize");
        server.initialized(InitializedParams {}).await;

        Self {
            server,
            _temp_dir: temp_dir,
            workspace_root,
        }
    }

    /// Helper to create a file path in the test workspace
    fn workspace_file(&self, name: &str) -> PathBuf {
        self.workspace_root.join(name)
    }

    /// Helper to create a file URL in the test workspace
    fn workspace_url(&self, name: &str) -> Url {
        djls_workspace::paths::path_to_url(&self.workspace_file(name)).unwrap()
    }

    /// Open a document in the LSP server
    async fn open_document(&self, file_name: &str, content: &str, version: i32) {
        let params = DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: self.workspace_url(file_name).to_string().parse().unwrap(),
                language_id: if file_name.ends_with(".html") {
                    "html".to_string()
                } else if file_name.ends_with(".py") {
                    "python".to_string()
                } else {
                    "plaintext".to_string()
                },
                version,
                text: content.to_string(),
            },
        };

        self.server.did_open(params).await;
    }

    /// Change a document in the LSP server
    async fn change_document(&self, file_name: &str, new_content: &str, version: i32) {
        let params = DidChangeTextDocumentParams {
            text_document: VersionedTextDocumentIdentifier {
                uri: self.workspace_url(file_name).to_string().parse().unwrap(),
                version,
            },
            content_changes: vec![TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: new_content.to_string(),
            }],
        };

        self.server.did_change(params).await;
    }

    /// Close a document in the LSP server
    async fn close_document(&self, file_name: &str) {
        let params = DidCloseTextDocumentParams {
            text_document: TextDocumentIdentifier {
                uri: self.workspace_url(file_name).to_string().parse().unwrap(),
            },
        };

        self.server.did_close(params).await;
    }

    /// Get the content of a file through the session's query system
    async fn get_file_content(&self, file_name: &str) -> String {
        let path = self.workspace_file(file_name);
        self.server
            .with_session_mut(|session| session.file_content(path))
            .await
    }

    /// Write a file to disk in the test workspace
    fn write_file(&self, file_name: &str, content: &str) {
        let path = self.workspace_file(file_name);
        std::fs::write(path, content).expect("Failed to write test file");
    }

    /// Get the revision of a file
    async fn get_file_revision(&self, file_name: &str) -> Option<u64> {
        let path = self.workspace_file(file_name);
        self.server
            .with_session_mut(|session| session.file_revision(&path))
            .await
    }
}

#[tokio::test]
async fn test_full_lsp_lifecycle() {
    let server = TestServer::new().await;
    let file_name = "test.html";

    // Write initial content to disk
    server.write_file(file_name, "<h1>Disk Content</h1>");

    // 1. Test did_open creates overlay and file
    server
        .open_document(file_name, "<h1>Overlay Content</h1>", 1)
        .await;

    // Verify overlay content is returned (not disk content)
    let content = server.get_file_content(file_name).await;
    assert_eq!(content, "<h1>Overlay Content</h1>");

    // Verify file was created with revision 0
    let revision = server.get_file_revision(file_name).await;
    assert_eq!(revision, Some(0));

    // 2. Test did_change updates overlay and bumps revision
    server
        .change_document(file_name, "<h1>Updated Content</h1>", 2)
        .await;

    // Verify content changed
    let content = server.get_file_content(file_name).await;
    assert_eq!(content, "<h1>Updated Content</h1>");

    // Verify revision was bumped
    let revision = server.get_file_revision(file_name).await;
    assert_eq!(revision, Some(1));

    // 3. Test did_close removes overlay and bumps revision
    server.close_document(file_name).await;

    // Verify content now comes from disk (empty since file doesn't exist)
    let content = server.get_file_content(file_name).await;
    assert_eq!(content, "<h1>Disk Content</h1>");

    // Verify revision was bumped again
    let revision = server.get_file_revision(file_name).await;
    assert_eq!(revision, Some(2));
}

#[tokio::test]
async fn test_overlay_precedence() {
    let server = TestServer::new().await;
    let file_name = "template.html";

    // Write content to disk
    server.write_file(file_name, "{% block content %}Disk{% endblock %}");

    // Read content before overlay - should get disk content
    let content = server.get_file_content(file_name).await;
    assert_eq!(content, "{% block content %}Disk{% endblock %}");

    // Open document with different content
    server
        .open_document(file_name, "{% block content %}Overlay{% endblock %}", 1)
        .await;

    // Verify overlay content takes precedence
    let content = server.get_file_content(file_name).await;
    assert_eq!(content, "{% block content %}Overlay{% endblock %}");

    // Close document
    server.close_document(file_name).await;

    // Verify we're back to disk content
    let content = server.get_file_content(file_name).await;
    assert_eq!(content, "{% block content %}Disk{% endblock %}");
}

#[tokio::test]
async fn test_template_parsing_with_overlays() {
    let server = TestServer::new().await;
    let file_name = "template.html";

    // Write initial template to disk
    server.write_file(file_name, "{% if true %}Original{% endif %}");

    // Open with different template content
    server
        .open_document(
            file_name,
            "{% for item in items %}{{ item }}{% endfor %}",
            1,
        )
        .await;
    use djls_workspace::db::parse_template;

    // Parse template through the session  
    let workspace_path = server.workspace_file(file_name);
    let ast = server
        .server
        .with_session_mut(|session| {
            session.with_db_mut(|db| {
                let file = db.get_or_create_file(workspace_path);
                parse_template(db, file)
            })
        })
        .await;

    // Verify we parsed the overlay content (for loop), not disk content (if statement)
    assert!(ast.is_some());
    let ast = ast.unwrap();
    let ast_str = format!("{:?}", ast.ast);
    assert!(ast_str.contains("for") || ast_str.contains("For"));
    assert!(!ast_str.contains("if") && !ast_str.contains("If"));
}

#[tokio::test]
async fn test_multiple_documents_independent() {
    let server = TestServer::new().await;

    // Open multiple documents
    server.open_document("file1.html", "Content 1", 1).await;
    server.open_document("file2.html", "Content 2", 1).await;
    server.open_document("file3.html", "Content 3", 1).await;

    // Change one document
    server.change_document("file2.html", "Updated 2", 2).await;

    // Verify only file2 was updated
    assert_eq!(server.get_file_content("file1.html").await, "Content 1");
    assert_eq!(server.get_file_content("file2.html").await, "Updated 2");
    assert_eq!(server.get_file_content("file3.html").await, "Content 3");

    // Verify revision changes
    assert_eq!(server.get_file_revision("file1.html").await, Some(0));
    assert_eq!(server.get_file_revision("file2.html").await, Some(1));
    assert_eq!(server.get_file_revision("file3.html").await, Some(0));
}

#[tokio::test]
async fn test_concurrent_overlay_updates() {
    let server = Arc::new(TestServer::new().await);

    // Open initial documents
    for i in 0..5 {
        server
            .open_document(&format!("file{}.html", i), &format!("Initial {}", i), 1)
            .await;
    }

    // Spawn concurrent tasks to update different documents
    let mut handles = vec![];

    for i in 0..5 {
        let server_clone = Arc::clone(&server);
        let handle = tokio::spawn(async move {
            // Each task updates its document multiple times
            for version in 2..10 {
                server_clone
                    .change_document(
                        &format!("file{}.html", i),
                        &format!("Updated {} v{}", i, version),
                        version,
                    )
                    .await;

                // Small delay to encourage interleaving
                tokio::time::sleep(tokio::time::Duration::from_millis(1)).await;
            }
        });
        handles.push(handle);
    }

    // Wait for all tasks to complete
    for handle in handles {
        handle.await.expect("Task failed");
    }

    // Verify final state of all documents
    for i in 0..5 {
        let content = server.get_file_content(&format!("file{}.html", i)).await;
        assert_eq!(content, format!("Updated {} v9", i));

        // Each document should have had 8 changes (versions 2-9)
        let revision = server.get_file_revision(&format!("file{}.html", i)).await;
        assert_eq!(revision, Some(8));
    }
}

#[tokio::test]
async fn test_caching_behavior() {
    let server = TestServer::new().await;

    // Open three template files
    server
        .open_document("template1.html", "{% block a %}1{% endblock %}", 1)
        .await;
    server
        .open_document("template2.html", "{% block b %}2{% endblock %}", 1)
        .await;
    server
        .open_document("template3.html", "{% block c %}3{% endblock %}", 1)
        .await;

    // Parse all templates once to populate cache
    for i in 1..=3 {
        let _ = server
            .get_file_content(&format!("template{}.html", i))
            .await;
    }

    // Store initial revisions
    let rev1_before = server.get_file_revision("template1.html").await.unwrap();
    let rev2_before = server.get_file_revision("template2.html").await.unwrap();
    let rev3_before = server.get_file_revision("template3.html").await.unwrap();

    // Change only template2
    server
        .change_document("template2.html", "{% block b %}CHANGED{% endblock %}", 2)
        .await;

    // Verify only template2's revision changed
    let rev1_after = server.get_file_revision("template1.html").await.unwrap();
    let rev2_after = server.get_file_revision("template2.html").await.unwrap();
    let rev3_after = server.get_file_revision("template3.html").await.unwrap();

    assert_eq!(
        rev1_before, rev1_after,
        "template1 revision should not change"
    );
    assert_eq!(
        rev2_before + 1,
        rev2_after,
        "template2 revision should increment"
    );
    assert_eq!(
        rev3_before, rev3_after,
        "template3 revision should not change"
    );

    // Verify content
    assert_eq!(
        server.get_file_content("template1.html").await,
        "{% block a %}1{% endblock %}"
    );
    assert_eq!(
        server.get_file_content("template2.html").await,
        "{% block b %}CHANGED{% endblock %}"
    );
    assert_eq!(
        server.get_file_content("template3.html").await,
        "{% block c %}3{% endblock %}"
    );
}

#[tokio::test]
async fn test_revision_tracking_across_lifecycle() {
    let server = TestServer::new().await;
    let file_name = "tracked.html";

    // Create file on disk
    server.write_file(file_name, "Initial");

    // Open document - should create file with revision 0
    server.open_document(file_name, "Opened", 1).await;
    assert_eq!(server.get_file_revision(file_name).await, Some(0));

    // Change document multiple times
    for i in 2..=5 {
        server
            .change_document(file_name, &format!("Change {}", i), i)
            .await;
        assert_eq!(
            server.get_file_revision(file_name).await,
            Some((i - 1) as u64),
            "Revision should be {} after change {}",
            i - 1,
            i
        );
    }

    // Close document - should bump revision one more time
    server.close_document(file_name).await;
    assert_eq!(server.get_file_revision(file_name).await, Some(5));

    // Re-open document - file already exists, should bump revision to invalidate cache
    server.open_document(file_name, "Reopened", 10).await;
    assert_eq!(
        server.get_file_revision(file_name).await,
        Some(6),
        "Revision should bump on re-open to invalidate cache"
    );

    // Change again
    server.change_document(file_name, "Final", 11).await;
    assert_eq!(server.get_file_revision(file_name).await, Some(7));
}

