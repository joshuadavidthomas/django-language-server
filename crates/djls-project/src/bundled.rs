//! Immutable archive-backed Django sources; disk copies are navigation targets only.

mod facts;

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io;
use std::io::Cursor;
use std::io::Read;
use std::io::Write;
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::OnceLock;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use directories::ProjectDirs;
use djls_conf::DjangoVersion;
use djls_source::CaseSensitivity;
use djls_source::FileSystem;
use djls_source::RootWalk;
use djls_source::WalkEntry;
use djls_source::WalkEntryKind;
use djls_source::WalkOptions;
use ignore::overrides::OverrideBuilder;
use sha2::Digest;
use sha2::Sha256;
use zip::ZipArchive;

pub use self::facts::BundledDjangoLibrary;
pub use self::facts::bundled_django_libraries;
pub use self::facts::prime_bundled_django_libraries;

struct Bundle {
    root: Utf8PathBuf,
    archive: ZipArchive<Cursor<&'static [u8]>>,
    // Only names are indexed. File bodies stay compressed until read.
    index: BTreeMap<Utf8PathBuf, WalkEntryKind>,
}

const VERSIONS: [DjangoVersion; 3] = [
    DjangoVersion::Django52,
    DjangoVersion::Django60,
    DjangoVersion::Django61,
];

// Embedded releases fill their slot on first use; the others only when a host registers one.
static BUNDLES: [OnceLock<Bundle>; 3] = [const { OnceLock::new() }; 3];

// Size-sensitive builds (wasm) leave releases out and register them at runtime instead.
#[cfg(feature = "django-5-2")]
const DJANGO_5_2: Option<&[u8]> = Some(include_bytes!("../vendor/django-5.2.zip"));
#[cfg(not(feature = "django-5-2"))]
const DJANGO_5_2: Option<&[u8]> = None;
#[cfg(feature = "django-6-0")]
const DJANGO_6_0: Option<&[u8]> = Some(include_bytes!("../vendor/django-6.0.zip"));
#[cfg(not(feature = "django-6-0"))]
const DJANGO_6_0: Option<&[u8]> = None;
#[cfg(feature = "django-6-1")]
const DJANGO_6_1: Option<&[u8]> = Some(include_bytes!("../vendor/django-6.1.zip"));
#[cfg(not(feature = "django-6-1"))]
const DJANGO_6_1: Option<&[u8]> = None;

static CACHE: LazyLock<io::Result<Utf8PathBuf>> = LazyLock::new(|| {
    let cache = ProjectDirs::from("", "", "djls")
        .map_or_else(fallback_cache_dir, |dirs| dirs.cache_dir().to_path_buf());
    let cache = Utf8PathBuf::from_path_buf(cache).map_err(|path| {
        io::Error::other(format!(
            "bundle cache path is not UTF-8: {}",
            path.display()
        ))
    })?;
    Ok(cache.join("django"))
});

const fn slot(version: DjangoVersion) -> usize {
    match version {
        DjangoVersion::Django52 => 0,
        DjangoVersion::Django60 => 1,
        DjangoVersion::Django61 => 2,
    }
}

fn cache_root() -> io::Result<&'static Utf8Path> {
    CACHE
        .as_deref()
        .map_err(|error| io::Error::other(error.to_string()))
}

/// `None` when the release is neither embedded nor registered.
fn bundle(version: DjangoVersion) -> Option<io::Result<&'static Bundle>> {
    let slot = &BUNDLES[slot(version)];
    if let Some(bundle) = slot.get() {
        return Some(Ok(bundle));
    }
    let bytes = match version {
        DjangoVersion::Django52 => DJANGO_5_2,
        DjangoVersion::Django60 => DJANGO_6_0,
        DjangoVersion::Django61 => DJANGO_6_1,
    }?;
    Some(
        cache_root()
            .and_then(|cache| Bundle::new(cache, bytes))
            .map(|bundle| slot.get_or_init(|| bundle)),
    )
}

/// Supply a Django release at runtime to a build that does not embed it.
///
/// `bytes` is a Django wheel or vendored bundle; its `.dist-info` name selects the release.
/// Register before the release is first used, because Django Discovery keeps a missing-bundle
/// result. On wasm this also extracts the release's tag facts, so call it outside any query. The archive stays in memory for the rest of the process.
pub fn register_django_bundle(bytes: Vec<u8>) -> io::Result<DjangoVersion> {
    let version = ZipArchive::new(Cursor::new(bytes.as_slice()))?
        .file_names()
        .find_map(|name| dist_info_release(&name.ok()?))
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "archive is not a supported Django release",
            )
        })?;
    let already_loaded = || {
        io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("Django {} sources are already loaded", version.as_str()),
        )
    };
    let slot = &BUNDLES[slot(version)];
    if slot.get().is_some() {
        return Err(already_loaded());
    }
    let bundle = Bundle::new(cache_root()?, Box::leak(bytes.into_boxed_slice()))?;
    slot.set(bundle).map_err(|_rejected| already_loaded())?;
    // Registration happens before any database exists, the only safe place to extract without
    // a second thread.
    #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
    prime_bundled_django_libraries();
    Ok(version)
}

/// The supported release named by a `django-<version>.dist-info/` archive entry.
fn dist_info_release(name: &str) -> Option<DjangoVersion> {
    let (directory, _) = name.split_once('/')?;
    let (project, release) = directory.strip_suffix(".dist-info")?.split_once('-')?;
    if !project.eq_ignore_ascii_case("django") {
        return None;
    }
    crate::django_version::feature_line(&release.parse().ok()?)
}

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
fn fallback_cache_dir() -> std::path::PathBuf {
    std::env::temp_dir()
}

// `std::env::temp_dir` panics without a host filesystem. Bundle paths only need a
// stable identity; `materialize_bundled_path` writes nothing on this target.
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
fn fallback_cache_dir() -> std::path::PathBuf {
    std::path::PathBuf::from("/djls-cache")
}

impl Bundle {
    fn new(cache: &Utf8Path, bytes: &'static [u8]) -> io::Result<Self> {
        let mut digest = String::with_capacity(64);
        for byte in Sha256::digest(bytes) {
            write!(&mut digest, "{byte:02x}").map_err(io::Error::other)?;
        }
        let root = cache.join(digest);
        let archive = ZipArchive::new(Cursor::new(bytes))?;
        let mut index = BTreeMap::new();
        for name in archive.file_names() {
            let name = name?;
            let path = root.join(&*name);
            index.insert(
                path.clone(),
                if name.ends_with('/') {
                    WalkEntryKind::Directory
                } else {
                    WalkEntryKind::File
                },
            );
            for parent in path
                .ancestors()
                .skip(1)
                .take_while(|parent| parent.starts_with(&root))
            {
                index
                    .entry(parent.to_path_buf())
                    .or_insert(WalkEntryKind::Directory);
            }
        }
        Ok(Self {
            root,
            archive,
            index,
        })
    }

    fn read(&self, path: &Utf8Path) -> io::Result<String> {
        let relative = path.strip_prefix(&self.root).map_err(io::Error::other)?;
        let name = relative
            .components()
            .map(|part| part.as_str())
            .collect::<Vec<_>>()
            .join("/");
        let mut archive = self.archive.clone();
        let mut text = String::new();
        archive.by_name(&name)?.read_to_string(&mut text)?;
        Ok(text)
    }

    fn materialize(&self, path: &Utf8Path) -> io::Result<()> {
        let text = self.read(path)?;
        if std::fs::read_to_string(path).is_ok_and(|cached| cached == text) {
            return Ok(());
        }
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("missing parent"))?;
        std::fs::create_dir_all(parent)?;
        let mut staging = tempfile::NamedTempFile::new_in(parent)?;
        staging.write_all(text.as_bytes())?;
        staging.persist(path).map_err(|error| error.error)?;
        Ok(())
    }

    fn walk(&self, root: &Utf8Path, options: &WalkOptions) -> RootWalk {
        match self.index.get(root) {
            Some(WalkEntryKind::File) => return RootWalk::File(WalkEntry::file_root(root)),
            Some(WalkEntryKind::Directory) => {}
            _ => return RootWalk::Missing,
        }
        let mut builder = OverrideBuilder::new(root);
        for glob in &options.globs {
            drop(builder.add(glob));
        }
        let overrides = builder.build().ok();
        let mut entries = Vec::new();
        let mut excluded = Vec::<Utf8PathBuf>::new();
        for (path, kind) in self.index.range(root.to_path_buf()..) {
            if path == root {
                continue;
            }
            let Ok(relative) = path.strip_prefix(root) else {
                break;
            };
            let directory = *kind == WalkEntryKind::Directory;
            if excluded.iter().any(|prefix| path.starts_with(prefix)) {
                continue;
            }
            if (!options.hidden
                && relative
                    .components()
                    .any(|part| part.as_str().starts_with('.')))
                || options
                    .max_depth
                    .is_some_and(|depth| relative.components().count() > depth)
                || overrides
                    .as_ref()
                    .is_some_and(|overrides| overrides.matched(path, directory).is_ignore())
            {
                if directory {
                    excluded.push(path.clone());
                }
                continue;
            }
            entries.push(WalkEntry {
                root: root.to_path_buf(),
                path: path.clone(),
                relative: relative.to_path_buf(),
                kind: *kind,
            });
        }
        RootWalk::Directory {
            entries,
            issues: Vec::new(),
        }
    }
}

fn bundle_for(path: &Utf8Path) -> Option<&'static Bundle> {
    VERSIONS
        .into_iter()
        .filter_map(|version| bundle(version)?.ok())
        .find(|bundle| path.starts_with(&bundle.root))
}

/// The source root for `version`, or for the nearest supported release this build has when
/// `version` is neither embedded nor registered (as in a wasm host that registered only one).
///
/// Mounting a neighbor keeps Django's builtins known instead of leaving the project without
/// any Django source. Returns the release actually mounted.
pub(crate) fn nearest_source_root(
    version: DjangoVersion,
) -> io::Result<(DjangoVersion, Utf8PathBuf)> {
    let available =
        nearest_release(version, |candidate| bundle(candidate).is_some()).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("no Django sources are bundled for {}", version.as_str()),
            )
        })?;
    Ok((available, source_root(available)?))
}

/// `version` itself, else the closest newer release, else the closest older one. Newer releases
/// rarely remove Tags but often add them, so they are less likely to report a used Tag unknown.
fn nearest_release(
    version: DjangoVersion,
    available: impl Fn(DjangoVersion) -> bool,
) -> Option<DjangoVersion> {
    let index = slot(version);
    std::iter::once(version)
        .chain(VERSIONS[index + 1..].iter().copied())
        .chain(VERSIONS[..index].iter().rev().copied())
        .find(|&candidate| available(candidate))
}

pub(crate) fn source_root(version: DjangoVersion) -> io::Result<Utf8PathBuf> {
    let bundle = bundle(version).unwrap_or_else(|| {
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("Django {} sources are not bundled", version.as_str()),
        ))
    })?;
    Ok(bundle.root.clone())
}

/// Prepare an ordinary file URI target without changing its source identity.
pub fn materialize_bundled_path(path: &Utf8Path) -> io::Result<()> {
    // Without a host filesystem, navigation targets stay virtual bundle paths that callers
    // read back through `BundledFileSystem`.
    if cfg!(all(target_arch = "wasm32", target_os = "unknown")) {
        return Ok(());
    }
    if let Some(bundle) = bundle_for(path) {
        bundle.materialize(path)?;
    }
    Ok(())
}

/// Mount immutable bundles above another filesystem, including editor overlays.
pub struct BundledFileSystem {
    inner: Arc<dyn FileSystem>,
}

impl BundledFileSystem {
    pub fn new(inner: Arc<dyn FileSystem>) -> Self {
        Self { inner }
    }
}

impl FileSystem for BundledFileSystem {
    fn read_to_string(&self, path: &Utf8Path) -> io::Result<String> {
        match bundle_for(path) {
            Some(bundle) => bundle.read(path),
            None => self.inner.read_to_string(path),
        }
    }

    fn canonicalize(&self, path: &Utf8Path) -> io::Result<Utf8PathBuf> {
        match bundle_for(path) {
            Some(bundle) => bundle
                .index
                .contains_key(path)
                .then(|| path.to_path_buf())
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "bundle entry not found")),
            None => self.inner.canonicalize(path),
        }
    }

    fn exists(&self, path: &Utf8Path) -> bool {
        bundle_for(path).map_or_else(
            || self.inner.exists(path),
            |bundle| bundle.index.contains_key(path),
        )
    }

    fn is_file(&self, path: &Utf8Path) -> bool {
        bundle_for(path).map_or_else(
            || self.inner.is_file(path),
            |bundle| bundle.index.get(path) == Some(&WalkEntryKind::File),
        )
    }

    fn is_dir(&self, path: &Utf8Path) -> bool {
        bundle_for(path).map_or_else(
            || self.inner.is_dir(path),
            |bundle| bundle.index.get(path) == Some(&WalkEntryKind::Directory),
        )
    }

    fn case_sensitivity(&self) -> CaseSensitivity {
        // The host and archive may have different semantics.
        match self.inner.case_sensitivity() {
            CaseSensitivity::CaseSensitive => CaseSensitivity::CaseSensitive,
            CaseSensitivity::CaseInsensitive | CaseSensitivity::Unknown => CaseSensitivity::Unknown,
        }
    }

    fn path_exists_case_sensitive(&self, path: &Utf8Path, prefix: &Utf8Path) -> bool {
        bundle_for(path).map_or_else(
            || self.inner.path_exists_case_sensitive(path, prefix),
            |bundle| bundle.index.contains_key(path),
        )
    }

    fn walk_root(&self, root: &Utf8Path, options: &WalkOptions) -> RootWalk {
        bundle_for(root).map_or_else(
            || self.inner.walk_root(root, options),
            |bundle| bundle.walk(root, options),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unavailable_release_falls_back_to_nearest_available_release() {
        use DjangoVersion::Django52;
        use DjangoVersion::Django60;
        use DjangoVersion::Django61;

        let only = |available: &'static [DjangoVersion]| {
            move |candidate: DjangoVersion| available.contains(&candidate)
        };
        for (requested, available, expected) in [
            (
                Django61,
                &[Django52, Django60, Django61][..],
                Some(Django61),
            ),
            (Django61, &[Django52][..], Some(Django52)),
            (Django61, &[Django52, Django60][..], Some(Django60)),
            (Django60, &[Django52, Django61][..], Some(Django61)),
            (Django52, &[Django60, Django61][..], Some(Django60)),
            (Django60, &[][..], None),
        ] {
            assert_eq!(
                nearest_release(requested, only(available)),
                expected,
                "{requested:?} with {available:?}"
            );
        }
    }

    #[test]
    fn bundles_match_pins_and_include_templates_and_licenses() {
        let fs =
            BundledFileSystem::new(Arc::new(djls_source::InMemoryFileSystem::case_insensitive()));
        assert_eq!(fs.case_sensitivity(), CaseSensitivity::Unknown);
        let pins: serde_json::Value =
            serde_json::from_str(include_str!("../vendor/django.json")).expect("pins");
        for version in [
            DjangoVersion::Django52,
            DjangoVersion::Django60,
            DjangoVersion::Django61,
        ] {
            let root = source_root(version).expect("source root");
            let pin = pins[version.as_str()]["version"].as_str().expect("pin");
            let tuple = pin.split('.').collect::<Vec<_>>().join(", ");
            let init = root.join("django/__init__.py");
            assert_eq!(
                fs.canonicalize(&init).expect("canonical archive identity"),
                init
            );
            assert!(
                fs.read_to_string(&root.join("django/__init__.py"))
                    .expect("version source")
                    .contains(&format!("VERSION = ({tuple},"))
            );
            for name in [
                "django/contrib/admin/templates/admin/base.html",
                "django/forms/templates/django/forms/widgets/text.html",
            ] {
                assert!(
                    !fs.read_to_string(&root.join(name))
                        .expect("template source")
                        .is_empty()
                );
            }
            assert!(
                fs.read_to_string(&root.join(format!("django-{pin}.dist-info/licenses/LICENSE")))
                    .expect("license")
                    .contains("Redistribution")
            );
            assert!(!fs.exists(&root.join("django/contrib/admin/static")));
            assert!(!fs.exists(&root.join("django/Template")));
            assert!(
                !fs.path_exists_case_sensitive(&root.join("django/Template/defaulttags.py"), &root)
            );
            assert!(
                fs.read_to_string(&root.join("django/Template/defaulttags.py"))
                    .is_err()
            );
            let template = root.join("django/template");
            let RootWalk::Directory { entries, issues } = fs.walk_root(
                &template,
                &WalkOptions {
                    globs: vec!["default*.py".into()],
                    max_depth: Some(1),
                    ..WalkOptions::unrestricted()
                },
            ) else {
                panic!("directory");
            };
            assert!(issues.is_empty());
            let names: Vec<_> = entries
                .iter()
                .filter(|entry| entry.kind == WalkEntryKind::File)
                .map(|entry| entry.relative.as_str())
                .collect();
            assert_eq!(names, ["defaultfilters.py", "defaulttags.py"]);
        }
    }

    #[test]
    fn runtime_bundles_are_identified_by_dist_info() {
        for (name, expected) in [
            (
                "django-5.2.18.dist-info/licenses/LICENSE",
                Some(DjangoVersion::Django52),
            ),
            ("Django-6.0.dist-info/RECORD", Some(DjangoVersion::Django60)),
            (
                "django-6.1.2.dist-info/METADATA",
                Some(DjangoVersion::Django61),
            ),
            ("django-4.2.20.dist-info/METADATA", None),
            ("django_crispy_forms-2.4.dist-info/METADATA", None),
            ("django/__init__.py", None),
        ] {
            assert_eq!(dist_info_release(name), expected, "{name}");
        }
    }

    #[test]
    fn archive_reads_and_walks_do_not_create_cache() {
        let temp = tempfile::tempdir().expect("temporary cache");
        let cache = Utf8Path::from_path(temp.path())
            .expect("UTF-8 cache")
            .join("uncreated");
        let bundle =
            Bundle::new(&cache, include_bytes!("../vendor/django-5.2.zip")).expect("bundle");
        let path = bundle.root.join("django/template/defaulttags.py");
        assert!(
            bundle
                .read(&path)
                .expect("archive source")
                .contains("def do_if(")
        );
        assert_eq!(
            bundle
                .index
                .get(&bundle.root.join("django/contrib/admin/templates")),
            Some(&WalkEntryKind::Directory)
        );
        assert!(matches!(
            bundle.walk(&bundle.root, &WalkOptions::shallow()),
            RootWalk::Directory { .. }
        ));
        assert!(!cache.exists());
        // A cache path that cannot be a directory still permits source reads.
        std::fs::write(&cache, "not a directory").expect("block cache writes");
        assert!(
            bundle
                .read(&path)
                .expect("archive source without cache")
                .contains("def do_if(")
        );
        assert!(bundle.materialize(&path).is_err());
    }

    #[test]
    fn concurrent_navigation_materializes_only_target_and_repairs_modified_copy() {
        let temp = tempfile::tempdir().expect("temporary cache");
        let cache = Utf8Path::from_path(temp.path()).expect("UTF-8 cache");
        let bundle =
            Bundle::new(cache, include_bytes!("../vendor/django-5.2.zip")).expect("bundle");
        let path = bundle.root.join("django/template/defaulttags.py");
        std::thread::scope(|scope| {
            for _ in 0..4 {
                scope.spawn(|| bundle.materialize(&path).expect("navigation copy"));
            }
        });
        assert_eq!(
            std::fs::read_to_string(&path).expect("disk copy"),
            bundle.read(&path).expect("archive source")
        );
        assert!(!bundle.root.join("django/__init__.py").exists());
        assert_eq!(
            std::fs::read_dir(path.parent().expect("parent"))
                .expect("cache entries")
                .count(),
            1
        );
        std::fs::write(&path, "modified").expect("modify copy");
        assert!(
            bundle
                .read(&path)
                .expect("immutable source")
                .contains("def do_if(")
        );
        bundle.materialize(&path).expect("repair copy");
        assert_eq!(
            std::fs::read_to_string(&path).expect("repaired copy"),
            bundle.read(&path).expect("archive source")
        );
    }
}
