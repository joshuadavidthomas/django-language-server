//! Download and sync corpus packages/repos from the lockfile.
//!
//! The lockfile contains fully-resolved versions, URLs, and checksums.
//! This module downloads and extracts them without any network resolution —
//! all resolution happens in [`crate::lock`].

use std::collections::HashSet;
use std::io::Read;
use std::io::Write;
use std::sync::Mutex;
use std::time::Duration;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use serde::Serialize;
use sha2::Digest;
use sha2::Sha256;

use crate::archive::extract_tarball;
use crate::lock::LockedPackage;
use crate::lock::LockedRepo;
use crate::lock::Lockfile;

const MAX_CONCURRENT_DOWNLOADS: usize = 8;

const COMPLETE_MARKER: &str = ".complete.json";
const MAX_TARBALL_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Serialize)]
struct PackageMarker {
    name: String,
    version: String,
    sha256: String,
    url: String,
}

#[derive(Serialize)]
struct RepoMarker {
    name: String,
    url: String,
    git_ref: String,
    tag: String,
}

fn write_marker(out_dir: &Utf8Path, value: &impl Serialize) -> anyhow::Result<()> {
    let json = serde_json::to_string_pretty(value)?;
    let marker_path = out_dir.join(COMPLETE_MARKER);
    std::fs::write(marker_path.as_std_path(), format!("{json}\n"))?;
    Ok(())
}

fn is_synced(out_dir: &Utf8Path) -> bool {
    out_dir.join(COMPLETE_MARKER).as_std_path().exists()
}

/// Download a tarball, streaming through a SHA256 hasher to a temp file.
///
/// Returns `(temp_file, computed_sha256_hex)`.
fn download_tarball(
    client: &reqwest::blocking::Client,
    url: &str,
    label: &str,
) -> anyhow::Result<(tempfile::NamedTempFile, String)> {
    let mut resp = client.get(url).send()?;
    if !resp.status().is_success() {
        anyhow::bail!("HTTP {} fetching tarball from {url}", resp.status());
    }

    let mut tmp = tempfile::NamedTempFile::new()?;
    let mut hasher = Sha256::new();
    let mut total_bytes: u64 = 0;

    let mut buf = [0u8; 16 * 1024];
    loop {
        let n = resp.read(&mut buf)?;
        if n == 0 {
            break;
        }

        total_bytes += n as u64;
        if total_bytes > MAX_TARBALL_BYTES {
            anyhow::bail!(
                "Tarball too large ({total_bytes} bytes) for {label} (max {MAX_TARBALL_BYTES} bytes)"
            );
        }

        hasher.update(&buf[..n]);
        tmp.write_all(&buf[..n])?;
    }

    tmp.flush()?;
    let sha256 = format!("{:x}", hasher.finalize());
    Ok((tmp, sha256))
}

fn sync_package(
    client: &reqwest::blocking::Client,
    package: &LockedPackage,
    out_dir: &Utf8Path,
    label: &str,
) -> anyhow::Result<()> {
    tracing::info!("{label}: downloading");
    let (tmp, actual_sha256) = download_tarball(client, &package.url, label)?;

    if !actual_sha256.eq_ignore_ascii_case(&package.sha256) {
        anyhow::bail!(
            "SHA256 mismatch for {}\n  expected: {}\n  actual:   {actual_sha256}",
            package.url,
            package.sha256
        );
    }

    tracing::info!("{label}: extracting");
    let file = tmp.reopen()?;
    let warnings = extract_tarball(file, out_dir)?;
    for w in &warnings {
        tracing::warn!("{w}");
    }

    write_marker(
        out_dir,
        &PackageMarker {
            name: package.name.clone(),
            version: package.resolved.clone(),
            sha256: actual_sha256,
            url: package.url.clone(),
        },
    )?;

    Ok(())
}

fn sync_repo(
    client: &reqwest::blocking::Client,
    repo: &LockedRepo,
    out_dir: &Utf8Path,
    label: &str,
) -> anyhow::Result<()> {
    tracing::info!("{label}: downloading");
    let base_url = repo.url.trim_end_matches(".git");
    let url = format!("{base_url}/archive/{}.tar.gz", repo.git_ref);
    let (tmp, _sha256) = download_tarball(client, &url, label)?;

    tracing::info!("{label}: extracting");
    let file = tmp.reopen()?;
    let warnings = extract_tarball(file, out_dir)?;
    for w in &warnings {
        tracing::warn!("{w}");
    }

    write_marker(
        out_dir,
        &RepoMarker {
            name: repo.name.clone(),
            url: repo.url.clone(),
            git_ref: repo.git_ref.clone(),
            tag: repo.tag.clone(),
        },
    )?;

    Ok(())
}

pub fn sync_corpus(lockfile: &Lockfile, corpus_root: &Utf8Path, prune: bool) -> anyhow::Result<()> {
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .build()?;

    let packages_dir = corpus_root.join("packages");
    let repos_dir = corpus_root.join("repos");

    std::fs::create_dir_all(packages_dir.as_std_path())?;
    std::fs::create_dir_all(repos_dir.as_std_path())?;

    let mut work: Vec<SyncItem> = Vec::new();
    let mut skipped = 0usize;

    for package in &lockfile.packages {
        let dir_name = lockfile.package_dir_name(package);
        let out_dir = packages_dir.join(&dir_name);
        let label = format!("{}-{}", package.name, package.resolved);
        if is_synced(&out_dir) {
            skipped += 1;
        } else {
            work.push(SyncItem::Package {
                package,
                out_dir,
                label,
            });
        }
    }

    for repo in &lockfile.repos {
        let out_dir = repos_dir.join(&repo.name);
        let short_ref = repo.git_ref.get(..12).unwrap_or(&repo.git_ref);
        let label = format!("{} @ {} ({short_ref})", repo.name, repo.tag);
        if is_synced(&out_dir) {
            skipped += 1;
        } else {
            work.push(SyncItem::Repo {
                repo,
                out_dir,
                label,
            });
        }
    }

    if skipped > 0 {
        tracing::info!(skipped, "already synced");
    }

    if work.is_empty() {
        return Ok(());
    }

    tracing::info!(count = work.len(), "downloading");
    let errors = sync_parallel(&client, &work);

    if prune {
        prune_corpus(lockfile, corpus_root)?;
    }

    if !errors.is_empty() {
        for e in &errors {
            tracing::error!("{e}");
        }
        anyhow::bail!("failed to sync {} entries", errors.len());
    }

    Ok(())
}

enum SyncItem<'a> {
    Package {
        package: &'a LockedPackage,
        out_dir: Utf8PathBuf,
        label: String,
    },
    Repo {
        repo: &'a LockedRepo,
        out_dir: Utf8PathBuf,
        label: String,
    },
}

fn sync_parallel(client: &reqwest::blocking::Client, work: &[SyncItem]) -> Vec<String> {
    let (permit_tx, permit_rx) = std::sync::mpsc::sync_channel(MAX_CONCURRENT_DOWNLOADS);
    for _ in 0..MAX_CONCURRENT_DOWNLOADS {
        permit_tx.send(()).unwrap();
    }

    let errors: Mutex<Vec<String>> = Mutex::new(Vec::new());

    std::thread::scope(|s| {
        for item in work {
            permit_rx.recv().unwrap();
            let permit_tx = permit_tx.clone();
            let errors = &errors;

            s.spawn(move || {
                let result = match item {
                    SyncItem::Package {
                        package,
                        out_dir,
                        label,
                    } => sync_package(client, package, out_dir, label),
                    SyncItem::Repo {
                        repo,
                        out_dir,
                        label,
                    } => sync_repo(client, repo, out_dir, label),
                };

                if let Err(e) = result {
                    let label = match item {
                        SyncItem::Package { label, .. } | SyncItem::Repo { label, .. } => label,
                    };
                    errors.lock().unwrap().push(format!("{label}: {e}"));
                }

                let _ = permit_tx.send(());
            });
        }
    });

    errors.into_inner().unwrap()
}

/// Remove synced data for specific packages or repos by name.
///
/// For packages, removes all directories matching the name (including
/// versioned variants like `django-6.0.2` for the name `django`).
pub fn clean_packages(corpus_root: &Utf8Path, names: &[String]) -> anyhow::Result<()> {
    let packages_dir = corpus_root.join("packages");
    let repos_dir = corpus_root.join("repos");

    for name in names {
        // Repos: flat directory
        let dir = repos_dir.join(name);
        if dir.as_std_path().exists() {
            std::fs::remove_dir_all(dir.as_std_path())?;
            tracing::info!(name, "cleaned repo");
        }

        // Packages: could be exact name or name-version
        if let Ok(entries) = std::fs::read_dir(packages_dir.as_std_path()) {
            for entry in entries.filter_map(Result::ok) {
                let Some(dir_name) = entry.file_name().to_str().map(String::from) else {
                    continue;
                };
                // Match exact name or name-{version} prefix
                if dir_name == *name || dir_name.starts_with(&format!("{name}-")) {
                    let dir = packages_dir.join(&dir_name);
                    std::fs::remove_dir_all(dir.as_std_path())?;
                    tracing::info!(dir_name, "cleaned package");
                }
            }
        }
    }

    Ok(())
}

/// Remove synced data not present in the lockfile.
fn prune_corpus(lockfile: &Lockfile, corpus_root: &Utf8Path) -> anyhow::Result<()> {
    let packages_dir = corpus_root.join("packages");
    let repos_dir = corpus_root.join("repos");

    let locked_package_dirs: HashSet<String> = lockfile
        .packages
        .iter()
        .map(|p| lockfile.package_dir_name(p))
        .collect();

    let locked_repo_dirs: HashSet<&str> = lockfile.repos.iter().map(|r| r.name.as_str()).collect();

    prune_flat_dir(&packages_dir, &locked_package_dirs)?;
    prune_flat_dir(&repos_dir, &locked_repo_dirs)?;

    // Also clean up old two-level layout directories (packages/{name}/{version}/)
    prune_old_nested_dirs(&packages_dir, &locked_package_dirs)?;
    prune_old_nested_dirs(&repos_dir, &locked_repo_dirs)?;

    Ok(())
}

/// Remove directories under `base/` whose names are not in `keep`.
fn prune_flat_dir(base: &Utf8Path, keep: &HashSet<impl AsRef<str>>) -> anyhow::Result<()> {
    let Ok(entries) = std::fs::read_dir(base.as_std_path()) else {
        return Ok(());
    };

    for entry in entries.filter_map(Result::ok) {
        if !entry.file_type().ok().is_some_and(|ft| ft.is_dir()) {
            continue;
        }
        let Some(dir_name) = entry.file_name().to_str().map(String::from) else {
            continue;
        };

        if !keep.iter().any(|k| k.as_ref() == dir_name) {
            let dir = base.join(&dir_name);
            // Only prune if it has a .complete.json (it's a synced dir, not a
            // leftover nested parent from the old layout — those are handled by
            // prune_old_nested_dirs).
            if dir.join(".complete.json").as_std_path().exists() {
                tracing::info!(dir_name, "pruned");
                std::fs::remove_dir_all(dir.as_std_path())?;
            }
        }
    }

    Ok(())
}

/// Remove old two-level layout directories (`{base}/{name}/{version}/`).
///
/// These are leftovers from the previous `packages/{name}/{version}/` layout.
/// Directories that contain subdirectories with `.complete.json` markers (but
/// don't have their own marker) are old nested parents.
fn prune_old_nested_dirs(
    base: &Utf8Path,
    _keep: &HashSet<impl AsRef<str>>,
) -> anyhow::Result<()> {
    let Ok(entries) = std::fs::read_dir(base.as_std_path()) else {
        return Ok(());
    };

    for entry in entries.filter_map(Result::ok) {
        if !entry.file_type().ok().is_some_and(|ft| ft.is_dir()) {
            continue;
        }
        let Some(dir_name) = entry.file_name().to_str().map(String::from) else {
            continue;
        };

        let dir = base.join(&dir_name);

        // Old layout: directory has no .complete.json itself but contains
        // subdirectories that do (e.g. packages/django/4.2.28/.complete.json)
        if dir.join(".complete.json").as_std_path().exists() {
            continue;
        }

        // Check if any child is a synced directory
        let Ok(children) = std::fs::read_dir(dir.as_std_path()) else {
            continue;
        };

        let has_synced_children = children.filter_map(Result::ok).any(|child| {
            child.file_type().ok().is_some_and(|ft| ft.is_dir())
                && child.path().join(".complete.json").exists()
        });

        if has_synced_children {
            tracing::info!(dir_name, "pruned old nested layout");
            std::fs::remove_dir_all(dir.as_std_path())?;
        }
    }

    Ok(())
}
