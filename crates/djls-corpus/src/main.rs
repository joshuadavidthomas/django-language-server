//! CLI entry point for corpus management.

use std::path::Component;
use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().collect();

    match args.get(1).map(String::as_str) {
        Some("sync") => corpus_sync(),
        Some("clean") => corpus_clean(),
        _ => {
            eprintln!("Usage: cargo run -p djls-corpus -- [sync|clean]");
            eprintln!("   or: just corpus-sync | just corpus-clean");
            std::process::exit(1);
        }
    }
}

/// Build and validate the corpus root path from the manifest.
///
/// Rejects absolute paths and paths containing `..` components to prevent
/// `remove_dir_all` or file writes outside the crate directory.
fn validated_corpus_root(manifest: &djls_corpus::manifest::Manifest) -> std::path::PathBuf {
    let root_dir = &manifest.corpus.root_dir;
    let root_path = Path::new(root_dir);

    assert!(
        !root_path.is_absolute(),
        "corpus root_dir must be a relative path, got: {root_dir}"
    );

    assert!(
        !root_path
            .components()
            .any(|c| matches!(c, Component::ParentDir)),
        "corpus root_dir must not contain '..' components, got: {root_dir}"
    );

    Path::new(env!("CARGO_MANIFEST_DIR")).join(root_dir)
}

fn corpus_sync() {
    let manifest_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("manifest.toml");
    let manifest =
        djls_corpus::manifest::Manifest::load(&manifest_path).expect("Failed to load manifest");
    let corpus_root = validated_corpus_root(&manifest);

    eprintln!("Syncing corpus to {}...", corpus_root.display());
    djls_corpus::sync::sync_corpus(&manifest, &corpus_root).expect("Failed to sync corpus");
    eprintln!("Corpus synced to {}", corpus_root.display());
}

fn corpus_clean() {
    let manifest_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("manifest.toml");
    let manifest =
        djls_corpus::manifest::Manifest::load(&manifest_path).expect("Failed to load manifest");
    let corpus_root = validated_corpus_root(&manifest);
    if corpus_root.exists() {
        std::fs::remove_dir_all(&corpus_root).expect("Failed to remove corpus");
        eprintln!("Corpus cleaned");
    } else {
        eprintln!("No corpus to clean");
    }
}
