//! Download and sync corpus packages/repos.
//!
//! SHA256 checksums are resolved at sync time (from `PyPI` for packages,
//! computed on download for repos) and recorded in `.complete.json`
//! markers for auditability and idempotent re-runs.
//!
//! Package versions in the manifest can be minor (`5.2`) or exact (`5.2.11`).
//! Minor versions are resolved to the latest stable patch release on `PyPI`.

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

/// Parse a version string like `"5.2.11"` into numeric segments `[5, 2, 11]`.
///
/// Returns `None` if any segment is non-numeric (pre-release like `5.2a1`).
fn parse_version(s: &str) -> Option<Vec<u32>> {
    s.split('.')
        .map(|part| part.parse::<u32>().ok())
        .collect()
}

/// Check whether a candidate version matches a version spec.
///
/// The spec is treated as a prefix: `[5, 2]` matches `[5, 2]`, `[5, 2, 11]`,
/// etc. but not `[5, 20]` or `[5, 1, 2]`.
fn version_matches(spec: &[u32], candidate: &[u32]) -> bool {
    candidate.len() >= spec.len() && candidate[..spec.len()] == *spec
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

/// Resolved package info from `PyPI`.
struct ResolvedPackage {
    version: String,
    url: String,
    expected_sha256: String,
}

/// Query `PyPI` to resolve a version spec and find the sdist.
///
/// If `version_spec` is an exact version (e.g. `"5.2.11"`), resolves to that
/// version. If it's a minor version (e.g. `"5.2"`), resolves to the latest
/// stable patch release matching that prefix.
fn resolve_pypi_package(
    client: &reqwest::blocking::Client,
    name: &str,
    version_spec: &str,
) -> anyhow::Result<ResolvedPackage> {
    let api_url = format!("https://pypi.org/pypi/{name}/json");
    let resp = client.get(&api_url).send()?;
    if !resp.status().is_success() {
        anyhow::bail!("PyPI returned {} for {name}", resp.status());
    }

    let json: serde_json::Value = resp.json()?;

    let spec_parts = parse_version(version_spec)
        .ok_or_else(|| anyhow::anyhow!("Invalid version spec: {version_spec}"))?;

    let releases = json["releases"]
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("No releases found for {name}"))?;

    // Find the latest stable version matching the spec prefix.
    let resolved_version = releases
        .keys()
        .filter_map(|v| parse_version(v).map(|parts| (parts, v.as_str())))
        .filter(|(parts, _)| version_matches(&spec_parts, parts))
        .max_by(|(a, _), (b, _)| a.cmp(b))
        .map(|(_, v)| v)
        .ok_or_else(|| anyhow::anyhow!("No release matching {version_spec} for {name}"))?;

    // Find the sdist in the resolved version's files.
    let files = releases
        .get(resolved_version)
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow::anyhow!("No files for {name}-{resolved_version}"))?;

    let sdist = files
        .iter()
        .find(|f| {
            f["packagetype"].as_str() == Some("sdist")
                && f["filename"]
                    .as_str()
                    .is_some_and(|name| name.ends_with(".tar.gz"))
        })
        .ok_or_else(|| anyhow::anyhow!("No sdist found for {name}-{resolved_version}"))?;

    let url = sdist["url"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("No URL in sdist for {name}-{resolved_version}"))?
        .to_string();

    let expected_sha256 = sdist["digests"]["sha256"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("No SHA256 in sdist for {name}-{resolved_version}"))?
        .to_string();

    Ok(ResolvedPackage {
        version: resolved_version.to_string(),
        url,
        expected_sha256,
    })
}

/// Check if any version matching this spec is already synced locally.
fn find_synced_match(packages_dir: &Utf8Path, name: &str, spec: &[u32]) -> bool {
    let name_dir = packages_dir.join(name);
    let Ok(entries) = std::fs::read_dir(name_dir.as_std_path()) else {
        return false;
    };

    entries.filter_map(Result::ok).any(|entry| {
        let file_name = entry.file_name();
        let dir_name = file_name.to_string_lossy();
        if let Some(parts) = parse_version(&dir_name) {
            if version_matches(spec, &parts) {
                let path = entry.path();
                let dir_path = Utf8Path::from_path(&path).expect("non-UTF8 corpus path");
                return is_synced(dir_path);
            }
        }
        false
    })
}

fn sync_package(
    client: &reqwest::blocking::Client,
    package: &Package,
    packages_dir: &Utf8Path,
) -> anyhow::Result<()> {
    let label = format!("{}-{}", package.name, package.version);

    // Fast path: check if a matching version is already synced locally
    // without hitting PyPI.
    let spec_parts = parse_version(&package.version)
        .ok_or_else(|| anyhow::anyhow!("Invalid version spec: {}", package.version))?;

    if find_synced_match(packages_dir, &package.name, &spec_parts) {
        eprintln!("  [skip] {label} (already synced)");
        return Ok(());
    }

    eprintln!("  [sync] {label}");

    let resolved = resolve_pypi_package(client, &package.name, &package.version)?;
    let resolved_label = format!("{}-{}", package.name, resolved.version);

    // Check again with the resolved version (another manifest entry may have
    // already synced this exact version).
    let out_dir = packages_dir.join(&package.name).join(&resolved.version);
    if is_synced(&out_dir) {
        eprintln!("  [skip] {resolved_label} (already synced)");
        return Ok(());
    }

    if resolved.version != package.version {
        eprintln!("  [resolve] {} → {resolved_label}", package.version);
    }

    let (tmp, actual_sha256) = download_tarball(client, &resolved.url, &resolved_label)?;

    if !actual_sha256.eq_ignore_ascii_case(&resolved.expected_sha256) {
        anyhow::bail!(
            "SHA256 mismatch for {}\n  expected: {}\n  actual:   {actual_sha256}",
            resolved.url,
            resolved.expected_sha256
        );
    }

    let file = tmp.reopen()?;
    extract_tarball(file, &out_dir)?;

    write_marker(
        &out_dir,
        &PackageMarker {
            name: package.name.clone(),
            version: resolved.version,
            sha256: actual_sha256,
            url: resolved.url,
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
    let (tmp, _sha256) = download_tarball(client, &url, &label)?;

    let file = tmp.reopen()?;
    extract_tarball(file, &out_dir)?;

    write_marker(
        &out_dir,
        &RepoMarker {
            name: repo.name.clone(),
            url: repo.url.clone(),
            git_ref: repo.git_ref.clone(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_version_stable() {
        assert_eq!(parse_version("5.2.11"), Some(vec![5, 2, 11]));
        assert_eq!(parse_version("5.2"), Some(vec![5, 2]));
        assert_eq!(parse_version("2.2"), Some(vec![2, 2]));
    }

    #[test]
    fn parse_version_prerelease() {
        assert_eq!(parse_version("5.2a1"), None);
        assert_eq!(parse_version("5.2rc1"), None);
        assert_eq!(parse_version("5.2b1"), None);
    }

    #[test]
    fn version_matches_exact() {
        assert!(version_matches(&[5, 2, 11], &[5, 2, 11]));
        assert!(!version_matches(&[5, 2, 11], &[5, 2, 10]));
    }

    #[test]
    fn version_matches_minor_prefix() {
        assert!(version_matches(&[5, 2], &[5, 2]));
        assert!(version_matches(&[5, 2], &[5, 2, 11]));
        assert!(!version_matches(&[5, 2], &[5, 1, 2]));
        assert!(!version_matches(&[5, 2], &[5, 20]));
    }

    #[test]
    fn version_matches_major_prefix() {
        assert!(version_matches(&[5], &[5, 2, 11]));
        assert!(!version_matches(&[5], &[6, 0]));
    }
}
