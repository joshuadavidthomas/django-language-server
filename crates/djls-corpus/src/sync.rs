//! Download and sync corpus packages/repos.
//!
//! Every download is verified against the SHA256 checksum in the manifest
//! before extraction.

use std::io::Read;
use std::io::Write;
use std::time::Duration;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use sha2::Digest;
use sha2::Sha256;

use crate::archive::extract_tarball;
use crate::manifest::Manifest;
use crate::manifest::Package;
use crate::manifest::Repo;

const MAX_TARBALL_BYTES: u64 = 200 * 1024 * 1024;

trait SyncEntry {
    fn out_dir(&self, base_dir: &Utf8Path) -> Utf8PathBuf;
    fn label(&self) -> String;
    fn sha256(&self) -> &str;
    fn tarball_url(&self, client: &reqwest::blocking::Client) -> anyhow::Result<String>;

    fn sync(&self, client: &reqwest::blocking::Client, base_dir: &Utf8Path) -> anyhow::Result<()> {
        let out_dir = self.out_dir(base_dir);
        let label = self.label();

        if out_dir.join(".complete").as_std_path().exists() {
            eprintln!("  [skip] {label} (already synced)");
            return Ok(());
        }

        eprintln!("  [sync] {label}");

        let tarball_url = self.tarball_url(client)?;

        let mut resp = client.get(&tarball_url).send()?;
        if !resp.status().is_success() {
            anyhow::bail!("HTTP {} fetching tarball from {tarball_url}", resp.status());
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

        let actual_sha256 = format!("{:x}", hasher.finalize());
        if !actual_sha256.eq_ignore_ascii_case(self.sha256()) {
            anyhow::bail!(
                "SHA256 mismatch for {tarball_url}\n  expected: {}\n  actual:   {actual_sha256}",
                self.sha256()
            );
        }

        let file = tmp.reopen()?;
        extract_tarball(file, &out_dir)?;
        std::fs::write(out_dir.join(".complete").as_std_path(), "")?;

        Ok(())
    }
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

        // Find the sdist `.tar.gz` URL from a `PyPI` JSON API response.
        let json: &serde_json::Value = &resp.json()?;
        let name: &str = &self.name;
        let version: &str = &self.version;
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
        if let Err(e) = package.sync(&client, &packages_dir) {
            eprintln!("Warning: Failed to sync {}: {e}", package.label());
            errors.push(package.label());
        }
    }

    for repo in &manifest.repos {
        if let Err(e) = repo.sync(&client, &repos_dir) {
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
