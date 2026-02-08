//! Download and extract corpus packages/repos.
//!
//! Extracts files relevant to extraction testing and template validation:
//! - `**/templatetags/**/*.py`
//! - `**/template/defaulttags.py`, `defaultfilters.py`, `loader_tags.py`
//! - `**/templates/**/*.html`, `**/templates/**/*.txt` (Django templates)

use std::io::Read;
use std::time::Duration;

use camino::Utf8Path;

use crate::manifest::Manifest;
use crate::manifest::Package;
use crate::manifest::Repo;

fn http_client() -> anyhow::Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(300))
        .build()?)
}

/// Whether a file path is relevant for corpus download.
///
/// This is the union of all extraction-target and template predicates — it
/// decides what to extract from tarballs during sync. The `Corpus` methods
/// apply stricter filtering (e.g. excluding `__init__.py`, `docs/`, `tests/`).
fn is_download_relevant(path: &str) -> bool {
    if path.contains("__pycache__") {
        return false;
    }

    let utf8 = Utf8Path::new(path);

    if utf8.extension().is_some_and(|ext| ext == "py") {
        return path.contains("/templatetags/")
            || (path.contains("/template/")
                && matches!(
                    utf8.file_name(),
                    Some("defaulttags.py" | "defaultfilters.py" | "loader_tags.py")
                ));
    }

    if utf8
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("html") || ext.eq_ignore_ascii_case("txt"))
    {
        return path.contains("/templates/");
    }

    false
}

pub fn sync_corpus(manifest: &Manifest, corpus_root: &Utf8Path) -> anyhow::Result<()> {
    let client = http_client()?;

    let packages_dir = corpus_root.join("packages");
    let repos_dir = corpus_root.join("repos");

    std::fs::create_dir_all(packages_dir.as_std_path())?;
    std::fs::create_dir_all(repos_dir.as_std_path())?;

    for package in &manifest.packages {
        if let Err(e) = sync_package(&client, package, &packages_dir) {
            eprintln!(
                "Warning: Failed to sync {}-{}: {e}",
                package.name, package.version
            );
        }
    }

    for repo in &manifest.repos {
        if let Err(e) = sync_repo(&client, repo, &repos_dir) {
            eprintln!("Warning: Failed to sync {}: {e}", repo.name);
        }
    }

    Ok(())
}

fn sync_package(
    client: &reqwest::blocking::Client,
    package: &Package,
    packages_dir: &Utf8Path,
) -> anyhow::Result<()> {
    let out_dir = packages_dir.join(&package.name).join(&package.version);
    let marker = out_dir.join(".complete");

    if marker.as_std_path().exists() {
        eprintln!(
            "  [skip] {}-{} (already synced)",
            package.name, package.version
        );
        return Ok(());
    }

    eprintln!("  [sync] {}-{}", package.name, package.version);

    let url = format!(
        "https://pypi.org/pypi/{}/{}/json",
        package.name, package.version
    );
    let resp = client.get(&url).send()?;
    if !resp.status().is_success() {
        anyhow::bail!(
            "PyPI returned {} for {}-{}",
            resp.status(),
            package.name,
            package.version
        );
    }

    let json: serde_json::Value = resp.json()?;
    let sdist_url = find_sdist_url(&json, &package.name, &package.version)?;

    extract_tarball(client, &sdist_url, &out_dir)?;

    std::fs::write(marker.as_std_path(), "")?;
    Ok(())
}

fn find_sdist_url(json: &serde_json::Value, name: &str, version: &str) -> anyhow::Result<String> {
    json["urls"]
        .as_array()
        .and_then(|urls| {
            urls.iter().find(|u| {
                u["packagetype"].as_str() == Some("sdist")
                    && u["filename"]
                        .as_str()
                        .is_some_and(|f| f.ends_with(".tar.gz"))
            })
        })
        .and_then(|u| u["url"].as_str())
        .map(String::from)
        .ok_or_else(|| anyhow::anyhow!("No sdist found for {name}-{version}"))
}

fn extract_tarball(
    client: &reqwest::blocking::Client,
    url: &str,
    out_dir: &Utf8Path,
) -> anyhow::Result<()> {
    let resp = client.get(url).send()?;
    if !resp.status().is_success() {
        anyhow::bail!("HTTP {} fetching tarball from {}", resp.status(), url);
    }
    let gz = flate2::read::GzDecoder::new(resp);
    let mut archive = tar::Archive::new(gz);

    std::fs::create_dir_all(out_dir.as_std_path())?;

    for entry in archive.entries()? {
        let mut entry = entry?;
        let entry_path = entry.path()?.to_string_lossy().to_string();

        // Strip the top-level directory (e.g., "Django-5.2.11/")
        let relative = entry_path
            .split_once('/')
            .map_or(entry_path.as_str(), |x| x.1);

        if !is_download_relevant(relative) {
            continue;
        }

        // Reject paths containing ".." to prevent directory traversal
        if std::path::Path::new(relative)
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            anyhow::bail!("Path traversal detected in tarball entry: {entry_path}");
        }

        let dest = out_dir.join(relative);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent.as_std_path())?;
        }

        let mut content = Vec::new();
        entry.read_to_end(&mut content)?;
        std::fs::write(dest.as_std_path(), &content)?;
    }

    Ok(())
}

fn sync_repo(
    client: &reqwest::blocking::Client,
    repo: &Repo,
    repos_dir: &Utf8Path,
) -> anyhow::Result<()> {
    let out_dir = repos_dir.join(&repo.name).join(&repo.git_ref);
    let marker = out_dir.join(".complete");

    if marker.as_std_path().exists() {
        eprintln!("  [skip] {} (already synced)", repo.name);
        return Ok(());
    }

    eprintln!(
        "  [sync] {} @ {}",
        repo.name,
        repo.git_ref.get(..12).unwrap_or(&repo.git_ref)
    );

    let tarball_url = format!(
        "{}/archive/{}.tar.gz",
        repo.url.trim_end_matches(".git"),
        repo.git_ref
    );

    extract_tarball(client, &tarball_url, &out_dir)?;

    std::fs::write(marker.as_std_path(), "")?;
    Ok(())
}
