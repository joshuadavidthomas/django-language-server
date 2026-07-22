//! Download and sync corpus repos from the lockfile.
//!
//! The lockfile contains fully-resolved refs, URLs, and commit SHAs.
//! This module downloads and extracts them without any network resolution —
//! all resolution happens in [`crate::corpus::lock`].

use std::collections::HashSet;
use std::io::Read;
use std::io::Write;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use serde::Deserialize;
use serde::Serialize;

use crate::corpus::archive::extract_tarball;
use crate::corpus::lock::LockedRepo;
use crate::corpus::lock::Lockfile;

const MAX_CONCURRENT_DOWNLOADS: usize = 8;

const COMPLETE_MARKER: &str = ".complete.json";
const EXTRACT_FORMAT_VERSION: u32 = 2;
const MAX_TARBALL_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct RepoMarker {
    name: String,
    url: String,
    git_ref: String,
    tag: String,
    extract_format_version: u32,
}

fn write_marker(out_dir: &Utf8Path, value: &impl Serialize) -> anyhow::Result<()> {
    let json = serde_json::to_string_pretty(value)?;
    let marker_path = out_dir.join(COMPLETE_MARKER);
    std::fs::write(marker_path.as_std_path(), format!("{json}\n"))?;
    Ok(())
}

impl From<&LockedRepo> for RepoMarker {
    fn from(repo: &LockedRepo) -> Self {
        Self {
            name: repo.name.clone(),
            url: repo.url.clone(),
            git_ref: repo.git_ref.clone(),
            tag: repo.tag.clone(),
            extract_format_version: EXTRACT_FORMAT_VERSION,
        }
    }
}

fn is_synced(repo: &LockedRepo, out_dir: &Utf8Path) -> bool {
    let marker_path = out_dir.join(COMPLETE_MARKER);
    std::fs::read_to_string(marker_path.as_std_path())
        .ok()
        .and_then(|content| serde_json::from_str::<RepoMarker>(&content).ok())
        .is_some_and(|marker| marker == RepoMarker::from(repo))
}

/// Validate that the local corpus checkout matches the lockfile.
pub(crate) fn validate_synced_corpus(
    lockfile: &Lockfile,
    corpus_root: &Utf8Path,
) -> anyhow::Result<()> {
    let repos_dir = corpus_root.join("repos");
    let stale: Vec<&str> = lockfile
        .repos
        .iter()
        .filter(|repo| !is_synced(repo, &repos_dir.join(&repo.name)))
        .map(|repo| repo.name.as_str())
        .collect();

    if stale.is_empty() {
        return Ok(());
    }

    anyhow::bail!(
        "Corpus is out of sync with manifest.lock for: {}. Run: just corpus sync",
        stale.join(", ")
    );
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
    let host = base_url
        .strip_prefix("https://")
        .or_else(|| base_url.strip_prefix("http://"))
        .and_then(|s| s.split('/').next())
        .unwrap_or_default();

    if host == "gitlab.com" {
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

    write_marker(out_dir, &RepoMarker::from(repo))?;

    Ok(())
}

pub fn sync_corpus(lockfile: &Lockfile, corpus_root: &Utf8Path, prune: bool) -> anyhow::Result<()> {
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_mins(5))
        .build()?;

    let repos_dir = corpus_root.join("repos");
    std::fs::create_dir_all(repos_dir.as_std_path())?;

    let mut work: Vec<SyncItem> = Vec::new();
    let mut skipped = 0usize;

    for repo in &lockfile.repos {
        let out_dir = repos_dir.join(&repo.name);
        let short_ref = repo.git_ref.get(..12).unwrap_or(&repo.git_ref);
        let label = format!("{} @ {} ({short_ref})", repo.name, repo.tag);
        if is_synced(repo, &out_dir) {
            skipped += 1;
        } else {
            if out_dir.as_std_path().exists() {
                tracing::info!(repo = repo.name, "removing stale corpus checkout");
                std::fs::remove_dir_all(out_dir.as_std_path())?;
            }
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
        sync_parallel(&client, &work)?
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

fn sync_parallel(
    client: &reqwest::blocking::Client,
    work: &[SyncItem],
) -> anyhow::Result<Vec<String>> {
    run_parallel(work, MAX_CONCURRENT_DOWNLOADS, |item| {
        sync_repo(client, item.repo, &item.out_dir, &item.label)
            .map_err(|error| format!("{}: {error}", item.label))
    })
}

fn run_parallel<T, F>(work: &[T], max_concurrent: usize, process: F) -> anyhow::Result<Vec<String>>
where
    T: Sync,
    F: Fn(&T) -> Result<(), String> + Sync,
{
    if work.is_empty() {
        return Ok(Vec::new());
    }
    if max_concurrent == 0 {
        anyhow::bail!("parallel worker limit must be greater than zero");
    }

    let next_index = AtomicUsize::new(0);
    let worker_count = work.len().min(max_concurrent);
    let mut errors = std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(worker_count);
        for _ in 0..worker_count {
            let next_index = &next_index;
            let process = &process;
            handles.push(scope.spawn(move || {
                let mut errors = Vec::new();
                loop {
                    let index = next_index.fetch_add(1, Ordering::Relaxed);
                    let Some(item) = work.get(index) else {
                        break;
                    };
                    if let Err(error) = process(item) {
                        errors.push((index, error));
                    }
                }
                errors
            }));
        }

        let mut errors = Vec::new();
        let mut worker_panicked = false;
        for handle in handles {
            match handle.join() {
                Ok(worker_errors) => errors.extend(worker_errors),
                Err(_panic) => worker_panicked = true,
            }
        }

        if worker_panicked {
            Err(anyhow::anyhow!("corpus download worker panicked"))
        } else {
            Ok(errors)
        }
    })?;

    errors.sort_unstable_by_key(|(index, _error)| *index);
    Ok(errors.into_iter().map(|(_index, error)| error).collect())
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
    use std::sync::atomic::AtomicBool;

    use super::*;

    fn locked_repo(url: &str) -> LockedRepo {
        LockedRepo {
            name: "test".to_string(),
            url: url.to_string(),
            tag: "main".to_string(),
            git_ref: "abc123def456".to_string(),
        }
    }

    fn temp_dir() -> (tempfile::TempDir, Utf8PathBuf) {
        let dir = tempfile::tempdir().expect("temporary sync test directory should be created");
        let path = Utf8PathBuf::from_path_buf(dir.path().to_path_buf())
            .expect("temporary sync test directory path should be UTF-8");
        (dir, path)
    }

    #[test]
    fn parallel_workers_take_more_work_while_one_item_is_blocked() {
        let work = [0, 1, 2, 3];
        let release_blocked_item = AtomicBool::new(false);
        let (progress_tx, progress_rx) = std::sync::mpsc::channel();

        std::thread::scope(|scope| {
            let handle = scope.spawn(|| {
                run_parallel(&work, 2, |item| {
                    if *item == 0 {
                        while !release_blocked_item.load(Ordering::Acquire) {
                            std::thread::yield_now();
                        }
                    } else {
                        progress_tx.send(*item).map_err(|error| error.to_string())?;
                    }
                    Ok(())
                })
            });

            let first = progress_rx.recv_timeout(Duration::from_secs(2));
            let second = progress_rx.recv_timeout(Duration::from_secs(2));
            release_blocked_item.store(true, Ordering::Release);
            let errors = handle
                .join()
                .expect("parallel test coordinator should not panic")
                .expect("parallel work should succeed");

            assert!(first.is_ok(), "one unblocked item should complete");
            assert!(
                second.is_ok(),
                "the available worker should take another item before the blocked item finishes"
            );
            assert!(errors.is_empty());
        });
    }

    #[test]
    fn parallel_workers_process_each_item_once_and_collect_errors_in_work_order() {
        let work = [0, 1, 2, 3, 4, 5, 6, 7];
        let attempts: [AtomicUsize; 8] = std::array::from_fn(|_| AtomicUsize::new(0));

        let errors = run_parallel(&work, 3, |item| {
            attempts[*item].fetch_add(1, Ordering::Relaxed);
            match *item {
                1 | 4 | 7 => Err(format!("item {item} failed")),
                0 | 2 | 3 | 5 | 6 => Ok(()),
                _ => Err(format!("unexpected item {item}")),
            }
        })
        .expect("parallel work should complete without a worker panic");

        assert_eq!(errors, ["item 1 failed", "item 4 failed", "item 7 failed"]);
        assert!(
            attempts
                .iter()
                .all(|attempts| attempts.load(Ordering::Relaxed) == 1)
        );
    }

    #[test]
    fn parallel_worker_panics_are_returned_as_errors() {
        let result = run_parallel(&[0], 1, |_item| -> Result<(), String> {
            panic!("intentional worker panic");
        });

        let error = result.expect_err("a worker panic should fail parallel work");
        assert_eq!(error.to_string(), "corpus download worker panicked");
    }

    #[test]
    fn synced_repo_requires_matching_marker() {
        let repo = locked_repo("https://github.com/owner/project.git");
        let (_dir, out) = temp_dir();
        std::fs::create_dir_all(out.as_std_path())
            .expect("synced-repo test directory should be created");
        write_marker(&out, &RepoMarker::from(&repo))
            .expect("matching repo marker should be written");

        assert!(is_synced(&repo, &out));
    }

    #[test]
    fn synced_repo_rejects_stale_marker_ref() {
        let repo = locked_repo("https://github.com/owner/project.git");
        let mut stale = locked_repo("https://github.com/owner/project.git");
        stale.git_ref = "old-ref".to_string();
        let (_dir, out) = temp_dir();
        std::fs::create_dir_all(out.as_std_path())
            .expect("stale-marker test directory should be created");
        write_marker(&out, &RepoMarker::from(&stale)).expect("stale repo marker should be written");

        assert!(!is_synced(&repo, &out));
    }

    #[test]
    fn synced_repo_rejects_missing_marker() {
        let repo = locked_repo("https://github.com/owner/project.git");
        let (_dir, out) = temp_dir();

        assert!(!is_synced(&repo, &out));
    }

    #[test]
    fn validate_synced_corpus_accepts_matching_markers() {
        let repo = locked_repo("https://github.com/owner/project.git");
        let lockfile = Lockfile { repos: vec![repo] };
        let (_dir, root) = temp_dir();
        let out = root.join("repos/test");
        std::fs::create_dir_all(out.as_std_path())
            .expect("matching-marker test directory should be created");
        write_marker(&out, &RepoMarker::from(&lockfile.repos[0]))
            .expect("matching repo marker should be written");

        validate_synced_corpus(&lockfile, &root).expect("matching repo marker should validate");
    }

    #[test]
    fn validate_synced_corpus_rejects_stale_markers() {
        let repo = locked_repo("https://github.com/owner/project.git");
        let lockfile = Lockfile { repos: vec![repo] };
        let mut stale = locked_repo("https://github.com/owner/project.git");
        stale.git_ref = "old-ref".to_string();
        let (_dir, root) = temp_dir();
        let out = root.join("repos/test");
        std::fs::create_dir_all(out.as_std_path())
            .expect("stale-validation test directory should be created");
        write_marker(&out, &RepoMarker::from(&stale)).expect("stale repo marker should be written");

        let error = validate_synced_corpus(&lockfile, &root)
            .expect_err("stale repo marker should fail corpus validation");
        assert!(error.to_string().contains("test"));
    }

    #[test]
    fn github_archive_url() {
        let repo = locked_repo("https://github.com/owner/project.git");
        let url = repo_archive_url(&repo).expect("GitHub archive URL should be built");
        assert_eq!(
            url,
            "https://github.com/owner/project/archive/abc123def456.tar.gz"
        );
    }

    #[test]
    fn github_archive_url_without_dot_git() {
        let repo = locked_repo("https://github.com/owner/project");
        let url = repo_archive_url(&repo).expect("GitHub archive URL without .git should be built");
        assert_eq!(
            url,
            "https://github.com/owner/project/archive/abc123def456.tar.gz"
        );
    }

    #[test]
    fn gitlab_com_archive_url() {
        let repo = locked_repo("https://gitlab.com/group/project.git");
        let url = repo_archive_url(&repo).expect("GitLab archive URL should be built");
        assert_eq!(
            url,
            "https://gitlab.com/group/project/-/archive/abc123def456/project-abc123def456.tar.gz"
        );
    }

    #[test]
    fn gitlab_com_nested_group() {
        let repo = locked_repo("https://gitlab.com/group/subgroup/project.git");
        let url = repo_archive_url(&repo).expect("nested-group GitLab archive URL should be built");
        assert_eq!(
            url,
            "https://gitlab.com/group/subgroup/project/-/archive/abc123def456/project-abc123def456.tar.gz"
        );
    }
}
