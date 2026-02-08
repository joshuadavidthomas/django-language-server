//! Download and sync corpus packages/repos.
//!
//! Every download is verified against the SHA256 checksum in the manifest
//! before extraction.

use std::time::Duration;

use camino::Utf8Path;
use camino::Utf8PathBuf;

use crate::archive::extract_tarball;
use crate::archive::verify_sha256;
use crate::manifest::Manifest;
use crate::manifest::Package;
use crate::manifest::Repo;

trait SyncEntry {
    fn out_dir(&self, base_dir: &Utf8Path) -> Utf8PathBuf;
    fn label(&self) -> String;
    fn sha256(&self) -> &str;
    fn tarball_url(&self, client: &reqwest::blocking::Client) -> anyhow::Result<String>;
}

impl SyncEntry for Package {
    fn out_dir(&self, base_dir: &Utf8Path) -> Utf8PathBuf {
        base_dir.join(&self.name).join(&self.version)
    }

    fn label(&self) -> String {
        format!("{}-{}", self.name, self.version)
    }

    fn sha256(&self) -> &str {
        &self.sha256
    }

    fn tarball_url(&self, client: &reqwest::blocking::Client) -> anyhow::Result<String> {
        let url = format!("https://pypi.org/pypi/{}/{}/json", self.name, self.version);
        let resp = client.get(&url).send()?;
        if !resp.status().is_success() {
            anyhow::bail!("PyPI returned {} for {}", resp.status(), self.label());
        }
        let json: serde_json::Value = resp.json()?;
        find_sdist_url(&json, &self.name, &self.version)
    }
}

impl SyncEntry for Repo {
    fn out_dir(&self, base_dir: &Utf8Path) -> Utf8PathBuf {
        base_dir.join(&self.name).join(&self.git_ref)
    }

    fn label(&self) -> String {
        let short_ref = self.git_ref.get(..12).unwrap_or(&self.git_ref);
        format!("{} @ {short_ref}", self.name)
    }

    fn sha256(&self) -> &str {
        &self.sha256
    }

    fn tarball_url(&self, _client: &reqwest::blocking::Client) -> anyhow::Result<String> {
        Ok(format!(
            "{}/archive/{}.tar.gz",
            self.url.trim_end_matches(".git"),
            self.git_ref
        ))
    }
}

fn http_client() -> anyhow::Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(300))
        .build()?)
}

/// Find the sdist `.tar.gz` URL from a `PyPI` JSON API response.
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

fn sync_entry(
    client: &reqwest::blocking::Client,
    entry: &dyn SyncEntry,
    base_dir: &Utf8Path,
) -> anyhow::Result<()> {
    let out_dir = entry.out_dir(base_dir);
    let label = entry.label();

    if out_dir.join(".complete").as_std_path().exists() {
        eprintln!("  [skip] {label} (already synced)");
        return Ok(());
    }

    eprintln!("  [sync] {label}");

    let tarball_url = entry.tarball_url(client)?;

    let resp = client.get(&tarball_url).send()?;
    if !resp.status().is_success() {
        anyhow::bail!("HTTP {} fetching tarball from {tarball_url}", resp.status());
    }
    let bytes = resp.bytes()?.to_vec();
    verify_sha256(&bytes, entry.sha256(), &tarball_url)?;

    extract_tarball(&bytes, &out_dir)?;
    std::fs::write(out_dir.join(".complete").as_std_path(), "")?;

    Ok(())
}

pub fn sync_corpus(manifest: &Manifest, corpus_root: &Utf8Path) -> anyhow::Result<()> {
    let client = http_client()?;

    let packages_dir = corpus_root.join("packages");
    let repos_dir = corpus_root.join("repos");

    std::fs::create_dir_all(packages_dir.as_std_path())?;
    std::fs::create_dir_all(repos_dir.as_std_path())?;

    let mut errors = Vec::new();

    for package in &manifest.packages {
        if let Err(e) = sync_entry(&client, package, &packages_dir) {
            eprintln!("Warning: Failed to sync {}: {e}", package.label());
            errors.push(package.label());
        }
    }

    for repo in &manifest.repos {
        if let Err(e) = sync_entry(&client, repo, &repos_dir) {
            eprintln!("Warning: Failed to sync {}: {e}", repo.label());
            errors.push(repo.label());
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
