//! Download and extract corpus packages/repos.
//!
//! Extracts only files relevant to extraction testing:
//! - `**/templatetags/**/*.py`
//! - `**/template/defaulttags.py`, `defaultfilters.py`, `loader_tags.py`

use std::io::Read;
use std::path::Path;

use crate::manifest::Manifest;
use crate::manifest::Package;
use crate::manifest::Repo;

/// Whether a file path is relevant for extraction testing.
fn is_extraction_relevant(path: &str) -> bool {
    if !Path::new(path)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("py"))
    {
        return false;
    }
    if path.contains("__pycache__") {
        return false;
    }

    // templatetags directories
    if path.contains("/templatetags/") {
        return true;
    }

    // Django core template modules
    if path.contains("/template/") {
        let file_name = path.rsplit('/').next().unwrap_or("");
        return matches!(
            file_name,
            "defaulttags.py" | "defaultfilters.py" | "loader_tags.py"
        );
    }

    false
}

pub fn sync_corpus(manifest: &Manifest, corpus_root: &Path) -> anyhow::Result<()> {
    let packages_dir = corpus_root.join("packages");
    let repos_dir = corpus_root.join("repos");

    std::fs::create_dir_all(&packages_dir)?;
    std::fs::create_dir_all(&repos_dir)?;

    for package in &manifest.packages {
        if let Err(e) = sync_package(package, &packages_dir) {
            eprintln!(
                "Warning: Failed to sync {}-{}: {e}",
                package.name, package.version
            );
        }
    }

    for repo in &manifest.repos {
        if let Err(e) = sync_repo(repo, &repos_dir) {
            eprintln!("Warning: Failed to sync {}: {e}", repo.name);
        }
    }

    Ok(())
}

fn sync_package(package: &Package, packages_dir: &Path) -> anyhow::Result<()> {
    let out_dir = packages_dir
        .join(&package.name)
        .join(&package.version);
    let marker = out_dir.join(".complete");

    if marker.exists() {
        eprintln!(
            "  [skip] {}-{} (already synced)",
            package.name, package.version
        );
        return Ok(());
    }

    eprintln!("  [sync] {}-{}", package.name, package.version);

    // Query PyPI for download URL
    let url = format!(
        "https://pypi.org/pypi/{}/{}/json",
        package.name, package.version
    );
    let resp = reqwest::blocking::get(&url)?;
    if !resp.status().is_success() {
        anyhow::bail!(
            "PyPI returned {} for {}-{}",
            resp.status(),
            package.name,
            package.version
        );
    }

    let json: serde_json::Value = resp.json()?;

    // Find sdist (.tar.gz) URL
    let sdist_url = find_sdist_url(&json, &package.name, &package.version)?;

    // Download and extract relevant files
    extract_tarball(&sdist_url, &out_dir)?;

    std::fs::write(marker, "")?;
    Ok(())
}

fn find_sdist_url(
    json: &serde_json::Value,
    name: &str,
    version: &str,
) -> anyhow::Result<String> {
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

fn extract_tarball(url: &str, out_dir: &Path) -> anyhow::Result<()> {
    let resp = reqwest::blocking::get(url)?;
    let gz = flate2::read::GzDecoder::new(resp);
    let mut archive = tar::Archive::new(gz);

    std::fs::create_dir_all(out_dir)?;

    for entry in archive.entries()? {
        let mut entry = entry?;
        let entry_path = entry.path()?.to_string_lossy().to_string();

        // Strip the top-level directory (e.g., "Django-5.2.11/")
        let relative = entry_path
            .split_once('/')
            .map_or(entry_path.as_str(), |x| x.1);

        if !is_extraction_relevant(relative) {
            continue;
        }

        let dest = out_dir.join(relative);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let mut content = Vec::new();
        entry.read_to_end(&mut content)?;
        std::fs::write(&dest, &content)?;
    }

    Ok(())
}

fn sync_repo(repo: &Repo, repos_dir: &Path) -> anyhow::Result<()> {
    let out_dir = repos_dir.join(&repo.name).join(&repo.git_ref);
    let marker = out_dir.join(".complete");

    if marker.exists() {
        eprintln!("  [skip] {} (already synced)", repo.name);
        return Ok(());
    }

    eprintln!("  [sync] {} @ {}", repo.name, &repo.git_ref[..12]);

    // Download tarball from GitHub
    let tarball_url = format!(
        "{}/archive/{}.tar.gz",
        repo.url.trim_end_matches(".git"),
        repo.git_ref
    );

    extract_tarball(&tarball_url, &out_dir)?;

    std::fs::write(marker, "")?;
    Ok(())
}
