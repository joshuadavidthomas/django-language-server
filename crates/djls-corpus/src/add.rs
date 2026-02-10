use std::time::Duration;

use camino::Utf8Path;
use clap::ValueEnum;

#[derive(Clone, Copy, ValueEnum)]
pub enum Bounds {
    Major,
    Minor,
    Exact,
}

pub fn add_packages(
    manifest_path: &Utf8Path,
    names: &[String],
    bounds: Bounds,
) -> anyhow::Result<()> {
    if names.is_empty() {
        anyhow::bail!("Specify one or more package names");
    }
    for name in names {
        add_package(manifest_path, name, bounds)?;
    }
    Ok(())
}

fn add_package(manifest_path: &Utf8Path, name: &str, bounds: Bounds) -> anyhow::Result<()> {
    let (_, latest) = resolve_pypi_latest(name)?;

    let parts: Vec<&str> = latest.split('.').collect();
    let version_spec = match bounds {
        Bounds::Major => parts[..1].join("."),
        Bounds::Minor if parts.len() >= 2 => parts[..2].join("."),
        Bounds::Minor | Bounds::Exact => latest.clone(),
    };

    let content = std::fs::read_to_string(manifest_path.as_std_path())?;
    let mut doc = content
        .parse::<toml_edit::DocumentMut>()
        .map_err(|e| anyhow::anyhow!("Failed to parse manifest: {e}"))?;

    let packages = doc["package"]
        .as_array_of_tables_mut()
        .ok_or_else(|| anyhow::anyhow!("No [[package]] array in manifest"))?;

    // Remove existing entry if present
    let mut i = 0;
    while i < packages.len() {
        let is_match = packages
            .get(i)
            .and_then(|t| t.get("name"))
            .and_then(|v| v.as_str())
            .is_some_and(|n| n == name);
        if is_match {
            packages.remove(i);
        } else {
            i += 1;
        }
    }

    // Find sorted insertion point
    let mut insert_at = packages.len();
    for (i, table) in packages.iter().enumerate() {
        let Some(existing_name) = table.get("name").and_then(|v| v.as_str()) else {
            continue;
        };
        if existing_name > name {
            insert_at = i;
            break;
        }
    }

    let mut entry = toml_edit::Table::new();
    entry.insert("name", toml_edit::value(name));
    entry.insert("version", toml_edit::value(&version_spec));

    // toml_edit only has push(); rebuild with insertion at the right position
    let mut tables: Vec<toml_edit::Table> = Vec::new();
    for (i, table) in packages.iter().enumerate() {
        if i == insert_at {
            tables.push(entry.clone());
        }
        tables.push(table.clone());
    }
    if insert_at >= packages.len() {
        tables.push(entry);
    }

    while !packages.is_empty() {
        packages.remove(0);
    }
    for t in tables {
        packages.push(t);
    }

    let output = doc.to_string();
    let trimmed = output.trim_end().to_string() + "\n";
    std::fs::write(manifest_path.as_std_path(), trimmed)?;

    eprintln!("Added {name} {version_spec} (latest: {latest})");
    Ok(())
}

/// Query `PyPI` for the latest stable version of a package.
///
/// Returns `(minor_spec, full_version)` — e.g. `("5.2", "5.2.11")`.
fn resolve_pypi_latest(name: &str) -> anyhow::Result<(String, String)> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent("djls-corpus")
        .build()?;

    let api_url = format!("https://pypi.org/pypi/{name}/json");
    let resp = client.get(&api_url).send()?;
    if !resp.status().is_success() {
        anyhow::bail!("PyPI returned {} for {name}", resp.status());
    }

    let json: serde_json::Value = resp.json()?;

    let releases = json["releases"]
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("No releases found for {name}"))?;

    let parse_version = |s: &str| -> Option<Vec<u32>> {
        s.split('.').map(|part| part.parse::<u32>().ok()).collect()
    };

    let (_, latest) = releases
        .keys()
        .filter_map(|v| parse_version(v).map(|parts| (parts, v.as_str())))
        .max_by(|(a, _), (b, _)| a.cmp(b))
        .ok_or_else(|| anyhow::anyhow!("No stable releases found for {name}"))?;

    let parts: Vec<&str> = latest.split('.').collect();
    let minor_spec = if parts.len() >= 2 {
        format!("{}.{}", parts[0], parts[1])
    } else {
        latest.to_string()
    };

    Ok((minor_spec, latest.to_string()))
}
