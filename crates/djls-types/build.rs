use std::fs;
use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir.parent().unwrap().parent().unwrap();
    let proto_dir = workspace_root.join("proto");

    let protos: Vec<_> = fs::read_dir(&proto_dir)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().and_then(|s| s.to_str()) == Some("proto"))
        .map(|entry| entry.path())
        .collect();

    prost_build::compile_protos(
        &protos
            .iter()
            .map(|p| p.to_str().unwrap())
            .collect::<Vec<_>>(),
        &[proto_dir],
    )
    .unwrap();
}
