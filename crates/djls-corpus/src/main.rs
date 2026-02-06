//! CLI entry point for corpus management.

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

fn corpus_sync() {
    let manifest_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("manifest.toml");
    let manifest =
        djls_corpus::manifest::Manifest::load(&manifest_path).expect("Failed to load manifest");
    let corpus_root = Path::new(env!("CARGO_MANIFEST_DIR")).join(&manifest.corpus.root_dir);

    eprintln!("Syncing corpus to {}...", corpus_root.display());
    djls_corpus::sync::sync_corpus(&manifest, &corpus_root).expect("Failed to sync corpus");
    eprintln!("Corpus synced to {}", corpus_root.display());
}

fn corpus_clean() {
    let manifest_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("manifest.toml");
    let manifest =
        djls_corpus::manifest::Manifest::load(&manifest_path).expect("Failed to load manifest");
    let corpus_root = Path::new(env!("CARGO_MANIFEST_DIR")).join(&manifest.corpus.root_dir);
    if corpus_root.exists() {
        std::fs::remove_dir_all(&corpus_root).expect("Failed to remove corpus");
        eprintln!("Corpus cleaned");
    } else {
        eprintln!("No corpus to clean");
    }
}
