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
        let out_dir = packages_dir.join(&package.name).join(&package.resolved);
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
        let out_dir = repos_dir.join(&repo.name).join(&repo.git_ref);
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
pub fn clean_packages(corpus_root: &Utf8Path, names: &[String]) -> anyhow::Result<()> {
    let packages_dir = corpus_root.join("packages");
    let repos_dir = corpus_root.join("repos");

    for name in names {
        for base in [&packages_dir, &repos_dir] {
            let dir = base.join(name);
            if dir.as_std_path().exists() {
                std::fs::remove_dir_all(dir.as_std_path())?;
                tracing::info!(name, "cleaned");
            }
        }
    }

    Ok(())
}

/// Remove synced versions not present in the lockfile.
fn prune_corpus(lockfile: &Lockfile, corpus_root: &Utf8Path) -> anyhow::Result<()> {
    let packages_dir = corpus_root.join("packages");
    let repos_dir = corpus_root.join("repos");

    let locked_packages: HashSet<(&str, &str)> = lockfile
        .packages
        .iter()
        .map(|p| (p.name.as_str(), p.resolved.as_str()))
        .collect();

    let locked_repos: HashSet<(&str, &str)> = lockfile
        .repos
        .iter()
        .map(|r| (r.name.as_str(), r.git_ref.as_str()))
        .collect();

    prune_dir(&packages_dir, &locked_packages)?;
    prune_dir(&repos_dir, &locked_repos)?;

    Ok(())
}

/// Walk `{base}/{name}/{version_or_ref}/` and remove entries not in `keep`.
///
/// Also removes empty `{name}/` parent directories after pruning.
fn prune_dir(base: &Utf8Path, keep: &HashSet<(&str, &str)>) -> anyhow::Result<()> {
    let Ok(names) = std::fs::read_dir(base.as_std_path()) else {
        return Ok(());
    };

    for name_entry in names.filter_map(Result::ok) {
        if !name_entry.file_type().ok().is_some_and(|ft| ft.is_dir()) {
            continue;
        }
        let Some(name) = name_entry.file_name().to_str().map(String::from) else {
            continue;
        };

        let name_dir = base.join(&name);
        let Ok(versions) = std::fs::read_dir(name_dir.as_std_path()) else {
            continue;
        };

        for version_entry in versions.filter_map(Result::ok) {
            if !version_entry.file_type().ok().is_some_and(|ft| ft.is_dir()) {
                continue;
            }
            let Some(version) = version_entry.file_name().to_str().map(String::from) else {
                continue;
            };

            if !keep.contains(&(name.as_str(), version.as_str())) {
                let stale = name_dir.join(&version);
                tracing::info!(name, version, "pruned");
                std::fs::remove_dir_all(stale.as_std_path())?;
            }
        }

        if name_dir
            .as_std_path()
            .read_dir()
            .ok()
            .is_some_and(|mut d| d.next().is_none())
        {
            tracing::info!(name, "pruned empty directory");
            std::fs::remove_dir(name_dir.as_std_path())?;
        }
    }

    Ok(())
}
