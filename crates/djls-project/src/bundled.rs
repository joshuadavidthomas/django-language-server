//! Compressed Django sources, materialized for ordinary file-based navigation.

use std::fmt::Write as _;
use std::io;
use std::io::Cursor;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use directories::ProjectDirs;
use djls_conf::DjangoVersion;
use sha2::Digest;
use sha2::Sha256;
use zip::ZipArchive;

fn archive(version: DjangoVersion) -> &'static [u8] {
    match version {
        DjangoVersion::Django52 => include_bytes!("../vendor/django-5.2.zip"),
        DjangoVersion::Django60 => include_bytes!("../vendor/django-6.0.zip"),
        DjangoVersion::Django61 => include_bytes!("../vendor/django-6.1.zip"),
    }
}

pub(crate) fn source_root(version: DjangoVersion) -> io::Result<Utf8PathBuf> {
    let directories = ProjectDirs::from("", "", "djls")
        .ok_or_else(|| io::Error::other("could not locate the DJLS cache directory"))?;
    let cache =
        Utf8PathBuf::from_path_buf(directories.cache_dir().join("django")).map_err(|path| {
            io::Error::other(format!("DJLS cache path is not UTF-8: {}", path.display()))
        })?;
    materialize(&cache, version)
}

fn materialize(cache: &Utf8Path, version: DjangoVersion) -> io::Result<Utf8PathBuf> {
    let bytes = archive(version);
    // Content addressing isolates patch bumps and changes to the archive recipe.
    let mut digest = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        write!(&mut digest, "{byte:02x}").map_err(io::Error::other)?;
    }
    let pins: serde_json::Value = serde_json::from_str(include_str!("../vendor/django.json"))?;
    let pin = pins[version.as_str()]["version"]
        .as_str()
        .ok_or_else(|| io::Error::other("missing bundled Django pin"))?;
    let destination = cache.join(format!("{pin}-{digest}"));
    if destination.join(".complete").is_file() {
        return Ok(destination);
    }
    std::fs::create_dir_all(cache)?;
    let staging = tempfile::tempdir_in(cache)?;
    let mut archive = ZipArchive::new(Cursor::new(bytes))?;
    archive.extract(staging.path())?;
    std::fs::write(staging.path().join(".complete"), b"")?;
    // Publish the whole tree atomically. A concurrent process may publish first.
    if let Err(error) = std::fs::rename(staging.path(), &destination)
        && !destination.join(".complete").is_file()
    {
        return Err(error);
    }
    Ok(destination)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundles_match_pins_and_include_templates_and_licenses() {
        let temp = tempfile::tempdir().expect("temporary cache");
        let cache = Utf8Path::from_path(temp.path()).expect("UTF-8 cache");
        let pins: serde_json::Value =
            serde_json::from_str(include_str!("../vendor/django.json")).expect("valid pins");
        for version in [
            DjangoVersion::Django52,
            DjangoVersion::Django60,
            DjangoVersion::Django61,
        ] {
            let root = materialize(cache, version).expect("bundle should extract");
            let source = std::fs::read_to_string(root.join("django/__init__.py"))
                .expect("Django version source");
            let pin = pins[version.as_str()]["version"].as_str().expect("pin");
            let tuple = pin.split('.').collect::<Vec<_>>().join(", ");
            assert!(source.contains(&format!("VERSION = ({tuple},")));
            assert!(root.join("django/template/defaulttags.py").is_file());
            assert!(
                root.join("django/contrib/admin/templates/admin/base.html")
                    .is_file()
            );
            assert!(
                root.join("django/forms/templates/django/forms/widgets/text.html")
                    .is_file()
            );
            assert!(
                root.join(format!("django-{pin}.dist-info/licenses/LICENSE"))
                    .is_file()
            );
            assert!(!root.join("django/contrib/admin/static").exists());
            assert_eq!(materialize(cache, version).expect("cached bundle"), root);
        }
        assert_eq!(std::fs::read_dir(cache).expect("cache entries").count(), 3);
    }

    #[test]
    fn concurrent_materialization_publishes_one_complete_tree() {
        let temp = tempfile::tempdir().expect("temporary cache");
        let cache = Utf8Path::from_path(temp.path()).expect("UTF-8 cache");
        let barrier = std::sync::Barrier::new(2);
        std::thread::scope(|scope| {
            let handles: Vec<_> = (0..2)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        materialize(cache, DjangoVersion::Django52).expect("concurrent extraction")
                    })
                })
                .collect();
            let roots: Vec<_> = handles
                .into_iter()
                .map(|handle| handle.join().expect("worker"))
                .collect();
            assert_eq!(roots[0], roots[1]);
            assert!(roots[0].join(".complete").is_file());
            assert!(roots[0].join("django/template/defaulttags.py").is_file());
        });
        assert_eq!(std::fs::read_dir(cache).expect("cache entries").count(), 1);
    }
}
