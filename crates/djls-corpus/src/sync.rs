//! Download and sync corpus packages/repos.
//!
//! SHA256 checksums are resolved at sync time (from `PyPI` for packages,
//! computed on download for repos) and recorded in `.complete.json`
//! markers for auditability and idempotent re-runs.

use std::io::Read;
use std::io::Write;
use std::time::Duration;

use camino::Utf8Path;
use serde::Serialize;
use sha2::Digest;
use sha2::Sha256;

use crate::archive::extract_tarball;
use crate::manifest::Manifest;
use crate::manifest::Package;
use crate::manifest::Repo;

const COMPLETE_MARKER: &str = ".complete.json";
const MAX_TARBALL_BYTES: u64 = 200 * 1024 * 1024;

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
    sha256: String,
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

/// Query `PyPI` for the sdist URL and its expected SHA256.
fn resolve_pypi_sdist(
    client: &reqwest::blocking::Client,
    name: &str,
    version: &str,
) -> anyhow::Result<(String, String)> {
    let api_url = format!("https://pypi.org/pypi/{name}/{version}/json");
    let resp = client.get(&api_url).send()?;
    if !resp.status().is_success() {
        anyhow::bail!("PyPI returned {} for {name}-{version}", resp.status());
    }

    let json: serde_json::Value = resp.json()?;
    let sdist = json["urls"]
        .as_array()
        .and_then(|urls| {
            urls.iter().find(|u| {
                u["packagetype"].as_str() == Some("sdist")
                    && u["filename"]
                        .as_str()
                        .is_some_and(|f| f.ends_with(".tar.gz"))
            })
        })
        .ok_or_else(|| anyhow::anyhow!("No sdist found for {name}-{version}"))?;

    let url = sdist["url"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("No URL in sdist entry for {name}-{version}"))?
        .to_string();

    let expected_sha256 = sdist["digests"]["sha256"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("No SHA256 digest in sdist entry for {name}-{version}"))?
        .to_string();

    Ok((url, expected_sha256))
}

fn sync_package(
    client: &reqwest::blocking::Client,
    package: &Package,
    packages_dir: &Utf8Path,
) -> anyhow::Result<()> {
    let out_dir = packages_dir.join(&package.name).join(&package.version);
    let label = format!("{}-{}", package.name, package.version);

    if is_synced(&out_dir) {
        eprintln!("  [skip] {label} (already synced)");
        return Ok(());
    }

    eprintln!("  [sync] {label}");

    let (url, expected_sha256) = resolve_pypi_sdist(client, &package.name, &package.version)?;
    let (tmp, actual_sha256) = download_tarball(client, &url, &label)?;

    if !actual_sha256.eq_ignore_ascii_case(&expected_sha256) {
        anyhow::bail!(
            "SHA256 mismatch for {url}\n  expected: {expected_sha256}\n  actual:   {actual_sha256}"
        );
    }

    let file = tmp.reopen()?;
    extract_tarball(file, &out_dir)?;

    write_marker(
        &out_dir,
        &PackageMarker {
            name: package.name.clone(),
            version: package.version.clone(),
            sha256: actual_sha256,
            url,
        },
    )?;

    Ok(())
}

fn sync_repo(
    client: &reqwest::blocking::Client,
    repo: &Repo,
    repos_dir: &Utf8Path,
) -> anyhow::Result<()> {
    let out_dir = repos_dir.join(&repo.name).join(&repo.git_ref);
    let short_ref = repo.git_ref.get(..12).unwrap_or(&repo.git_ref);
    let label = format!("{} @ {short_ref}", repo.name);

    if is_synced(&out_dir) {
        eprintln!("  [skip] {label} (already synced)");
        return Ok(());
    }

    eprintln!("  [sync] {label}");

    let url = format!(
        "{}/archive/{}.tar.gz",
        repo.url.trim_end_matches(".git"),
        repo.git_ref
    );
    let (tmp, sha256) = download_tarball(client, &url, &label)?;

    let file = tmp.reopen()?;
    extract_tarball(file, &out_dir)?;

    write_marker(
        &out_dir,
        &RepoMarker {
            name: repo.name.clone(),
            url: repo.url.clone(),
            git_ref: repo.git_ref.clone(),
            sha256,
        },
    )?;

    Ok(())
}

pub fn sync_corpus(manifest: &Manifest, corpus_root: &Utf8Path) -> anyhow::Result<()> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(300))
        .build()?;

    let packages_dir = corpus_root.join("packages");
    let repos_dir = corpus_root.join("repos");

    std::fs::create_dir_all(packages_dir.as_std_path())?;
    std::fs::create_dir_all(repos_dir.as_std_path())?;

    let mut errors = Vec::new();

    for package in &manifest.packages {
        if let Err(e) = sync_package(&client, package, &packages_dir) {
            let label = format!("{}-{}", package.name, package.version);
            eprintln!("Warning: Failed to sync {label}: {e}");
            errors.push(label);
        }
    }

    for repo in &manifest.repos {
        if let Err(e) = sync_repo(&client, repo, &repos_dir) {
            let short_ref = repo.git_ref.get(..12).unwrap_or(&repo.git_ref);
            let label = format!("{} @ {short_ref}", repo.name);
            eprintln!("Warning: Failed to sync {label}: {e}");
            errors.push(label);
        }
    }

    if !errors.is_empty() {
        anyhow::bail!(
            "Failed to sync {} entries: {}",
            errors.len(),
            errors.join(", ")
        );
    }

    Ok(())
}
