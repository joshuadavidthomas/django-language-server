use std::env;
use std::path::Path;

fn main() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let corpus_dir = Path::new(&manifest_dir).join(".corpus");

    println!("cargo:rerun-if-changed=manifest.toml");
    println!("cargo:rerun-if-changed=manifest.lock");
    println!("cargo:rerun-if-changed=.corpus");

    if corpus_dir.is_dir() {
        println!("cargo:rustc-cfg=corpus_available");
    } else {
        println!("cargo:warning=Corpus not synced. Run: cargo run --bin djls-corpus -- sync");
    }
}
