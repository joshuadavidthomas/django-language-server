use std::fs;
use std::path::{Path, PathBuf};

struct Version(&'static str);

impl Version {
    fn collect_protos(&self, proto_root: &Path) -> Vec<PathBuf> {
        fs::read_dir(proto_root.join(self.0))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().and_then(|s| s.to_str()) == Some("proto"))
            .map(|entry| entry.path())
            .collect()
    }
}

const VERSIONS: &[Version] = &[Version("v1")];

fn main() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir.parent().unwrap().parent().unwrap();
    let proto_dir = workspace_root.join("proto");

    let mut protos = Vec::new();
    for version in VERSIONS {
        protos.extend(version.collect_protos(&proto_dir));
    }

    prost_build::Config::new()
        .compile_protos(
            &protos
                .iter()
                .map(|p| p.to_str().unwrap())
                .collect::<Vec<_>>(),
            &[proto_dir],
        )
        .unwrap();
}
