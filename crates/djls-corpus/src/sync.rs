//! Download and sync corpus repos from the lockfile.
//!
//! The lockfile contains fully-resolved refs, URLs, and commit SHAs.
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

use crate::archive::extract_tarball;
use crate::lock::LockedRepo;
use crate::lock::Lockfile;

const MAX_CONCURRENT_DOWNLOADS: usize = 8;

const COMPLETE_MARKER: &str = ".complete.json";
const MAX_TARBALL_BYTES: u64 = 512 * 1024 * 1024;

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

/// Download a tarball to a temp file.
fn download_tarball(
    client: &reqwest::blocking::Client,
    url: &str,
    label: &str,
) -> anyhow::Result<tempfile::NamedTempFile> {
    let mut resp = client.get(url).send()?;
    if !resp.status().is_success() {
        anyhow::bail!("HTTP {} fetching tarball from {url}", resp.status());
    }

    let mut tmp = tempfile::NamedTempFile::new()?;
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

        tmp.write_all(&buf[..n])?;
    }

    tmp.flush()?;
    Ok(tmp)
}

fn repo_archive_url(repo: &LockedRepo) -> anyhow::Result<String> {
    let base_url = repo.url.trim_end_matches(".git");

    if is_gitlab_url(base_url) {
        let project = base_url
            .rsplit('/')
            .next()
            .ok_or_else(|| anyhow::anyhow!("cannot extract project name from {base_url}"))?;
        Ok(format!(
            "{base_url}/-/archive/{ref}/{project}-{ref}.tar.gz",
            ref = repo.git_ref,
            project = project,
        ))
    } else {
        Ok(format!("{base_url}/archive/{}.tar.gz", repo.git_ref))
    }
}

fn is_gitlab_url(url: &str) -> bool {
    let host = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .and_then(|s| s.split('/').next())
        .unwrap_or_default();

    host == "gitlab.com" || host.starts_with("gitlab.")
}

fn sync_repo(
    client: &reqwest::blocking::Client,
    repo: &LockedRepo,
    out_dir: &Utf8Path,
    label: &str,
) -> anyhow::Result<()> {
    tracing::info!("{label}: downloading");
    let url = repo_archive_url(repo)?;
    let tmp = download_tarball(client, &url, label)?;

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
        .timeout(Duration::from_secs(300))
        .build()?;

    let repos_dir = corpus_root.join("repos");
    std::fs::create_dir_all(repos_dir.as_std_path())?;

    let mut work: Vec<SyncItem> = Vec::new();
    let mut skipped = 0usize;

    for repo in &lockfile.repos {
        let out_dir = repos_dir.join(&repo.name);
        let short_ref = repo.git_ref.get(..12).unwrap_or(&repo.git_ref);
        let label = format!("{} @ {} ({short_ref})", repo.name, repo.tag);
        if is_synced(&out_dir) {
            skipped += 1;
        } else {
            work.push(SyncItem {
                repo,
                out_dir,
                label,
            });
        }
    }

    if skipped > 0 {
        tracing::info!(skipped, "already synced");
    }

    let errors = if work.is_empty() {
        Vec::new()
    } else {
        tracing::info!(count = work.len(), "downloading");
        sync_parallel(&client, &work)
    };

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

struct SyncItem<'a> {
    repo: &'a LockedRepo,
    out_dir: Utf8PathBuf,
    label: String,
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
                if let Err(e) = sync_repo(client, item.repo, &item.out_dir, &item.label) {
                    errors.lock().unwrap().push(format!("{}: {e}", item.label));
                }

                let _ = permit_tx.send(());
            });
        }
    });

    errors.into_inner().unwrap()
}

/// Remove synced data for specific entries by name.
pub fn clean_entries(corpus_root: &Utf8Path, names: &[String]) -> anyhow::Result<()> {
    let repos_dir = corpus_root.join("repos");

    for name in names {
        let dir = repos_dir.join(name);
        if dir.as_std_path().exists() {
            std::fs::remove_dir_all(dir.as_std_path())?;
            tracing::info!(name, "cleaned");
        }
    }

    Ok(())
}

/// Remove synced data not present in the lockfile.
fn prune_corpus(lockfile: &Lockfile, corpus_root: &Utf8Path) -> anyhow::Result<()> {
    let repos_dir = corpus_root.join("repos");

    let locked_repo_dirs: HashSet<&str> = lockfile.repos.iter().map(|r| r.name.as_str()).collect();

    prune_dir(&repos_dir, &locked_repo_dirs)?;

    // Also clean up leftover packages/ directory from old layout
    let packages_dir = corpus_root.join("packages");
    if packages_dir.as_std_path().exists() {
        tracing::info!("pruned old packages/ directory");
        std::fs::remove_dir_all(packages_dir.as_std_path())?;
    }

    Ok(())
}

/// Remove directories under `base/` whose names are not in `keep`.
fn prune_dir(base: &Utf8Path, keep: &HashSet<impl AsRef<str>>) -> anyhow::Result<()> {
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
            if dir.join(".complete.json").as_std_path().exists() {
                tracing::info!(dir_name, "pruned");
                std::fs::remove_dir_all(dir.as_std_path())?;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn locked_repo(url: &str) -> LockedRepo {
        LockedRepo {
            name: "test".to_string(),
            url: url.to_string(),
            tag: "main".to_string(),
            git_ref: "abc123def456".to_string(),
        }
    }

    #[test]
    fn github_archive_url() {
        let repo = locked_repo("https://github.com/owner/project.git");
        let url = repo_archive_url(&repo).unwrap();
        assert_eq!(
            url,
            "https://github.com/owner/project/archive/abc123def456.tar.gz"
        );
    }

    #[test]
    fn github_archive_url_without_dot_git() {
        let repo = locked_repo("https://github.com/owner/project");
        let url = repo_archive_url(&repo).unwrap();
        assert_eq!(
            url,
            "https://github.com/owner/project/archive/abc123def456.tar.gz"
        );
    }

    #[test]
    fn gitlab_com_archive_url() {
        let repo = locked_repo("https://gitlab.com/group/project.git");
        let url = repo_archive_url(&repo).unwrap();
        assert_eq!(
            url,
            "https://gitlab.com/group/project/-/archive/abc123def456/project-abc123def456.tar.gz"
        );
    }

    #[test]
    fn gitlab_com_nested_group() {
        let repo = locked_repo("https://gitlab.com/group/subgroup/project.git");
        let url = repo_archive_url(&repo).unwrap();
        assert_eq!(
            url,
            "https://gitlab.com/group/subgroup/project/-/archive/abc123def456/project-abc123def456.tar.gz"
        );
    }

    #[test]
    fn gitlab_self_hosted() {
        let repo = locked_repo("https://gitlab.example.com/team/project.git");
        let url = repo_archive_url(&repo).unwrap();
        assert_eq!(
            url,
            "https://gitlab.example.com/team/project/-/archive/abc123def456/project-abc123def456.tar.gz"
        );
    }

    #[test]
    fn is_gitlab_detects_gitlab_com() {
        assert!(is_gitlab_url("https://gitlab.com/group/project"));
    }

    #[test]
    fn is_gitlab_detects_self_hosted() {
        assert!(is_gitlab_url("https://gitlab.example.com/team/project"));
    }

    #[test]
    fn is_gitlab_rejects_github() {
        assert!(!is_gitlab_url("https://github.com/owner/project"));
    }

    #[test]
    fn is_gitlab_rejects_other() {
        assert!(!is_gitlab_url("https://codeberg.org/user/project"));
    }
}
