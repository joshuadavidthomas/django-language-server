//! Select a bundled feature line without executing a project's Python tools.

use camino::Utf8Path;
use djls_conf::DjangoVersion;
use djls_source::FileSystem;
use djls_source::Utf8PathClean;
use pep440_rs::Version;
use pep440_rs::VersionSpecifiers;
use pep508_rs::MarkerExpression;
use pep508_rs::MarkerTree;
use pep508_rs::MarkerValueVersion;
use pep508_rs::Requirement;
use pep508_rs::VersionOrUrl;
use rustc_hash::FxHashSet;

#[derive(Clone)]
struct LockedDjango {
    version: Option<Version>,
    marker: MarkerTree,
}

/// Called during imperative environment discovery, so edits are reread on reload.
pub fn bundled_django_version(
    fs: &dyn FileSystem,
    root: &Utf8Path,
    configured: Option<DjangoVersion>,
) -> Option<DjangoVersion> {
    if let Some(version) = configured {
        return Some(version);
    }

    let mut requirements = DjangoRequirements::default();
    if let Some(project) = read_toml(fs, &root.join("pyproject.toml")) {
        if let Some(source) = project
            .get("project")
            .and_then(|project| project.get("requires-python"))
            .and_then(toml::Value::as_str)
            && let Ok(range) = source.parse::<VersionSpecifiers>()
        {
            requirements.environment.and(python_marker(&[range]));
        }
        if let Some(dependencies) = project
            .get("project")
            .and_then(|project| project.get("dependencies"))
            .and_then(toml::Value::as_array)
        {
            for dependency in dependencies.iter().filter_map(toml::Value::as_str) {
                requirements.add(dependency);
            }
        }
        if let Some(dependencies) = project
            .get("tool")
            .and_then(|tool| tool.get("poetry"))
            .and_then(|poetry| poetry.get("dependencies"))
            .and_then(toml::Value::as_table)
        {
            for (name, value) in dependencies {
                if name.eq_ignore_ascii_case("django") {
                    requirements.add_poetry(value);
                } else if name == "python"
                    && let Some(source) = value.as_str()
                    && let Some(ranges) = poetry_specifiers(source)
                {
                    requirements.environment.and(python_marker(&ranges));
                }
            }
        }
    }
    requirements.read_setup_cfg(fs, root);
    requirements.read_files(fs, root);
    for entry in &mut requirements.entries {
        entry.marker.and(requirements.environment.clone());
    }
    requirements
        .entries
        .retain(|entry| !entry.marker.is_false());

    // Prefer a resolved version, but do not trust a lock that contradicts the
    // current declarations. Universal locks may contain several feasible versions.
    let mut locked_requirement = MarkerTree::FALSE;
    for filename in [
        "uv.lock",
        "poetry.lock",
        "pdm.lock",
        "Pipfile.lock",
        "pylock.toml",
    ] {
        let mut versions = locked_django_versions(fs, &root.join(filename));
        for candidate in &mut versions {
            candidate.marker.and(requirements.environment.clone());
            locked_requirement.or(candidate.marker.clone());
        }
        let version = versions
            .iter()
            .filter_map(|candidate| {
                candidate
                    .version
                    .as_ref()
                    .filter(|version| requirements.admits(version, &candidate.marker))
            })
            .min();
        if let Some(version) = version {
            let selected = feature_line(version);
            if selected.is_none() {
                tracing::warn!("Django {version} in {filename} has no supported bundle");
            }
            return selected;
        }
    }

    if !requirements.has_requirement() {
        if locked_requirement.is_false() {
            return Some(DjangoVersion::default());
        }
        // A stale runtime lock still establishes a dependency: constraints must
        // limit the inferred replacement even without a direct declaration.
        requirements.entries.push(DjangoRequirement {
            alternatives: vec![VersionSpecifiers::default()],
            marker: locked_requirement,
            required: true,
        });
    }
    let selected = requirements.feature_line();
    if selected.is_none() {
        tracing::warn!(
            "Project Django requirements have no supported bundle; set django_version to override"
        );
    }
    selected
}

fn locked_django_versions(fs: &dyn FileSystem, path: &Utf8Path) -> Vec<LockedDjango> {
    if path.file_name() == Some("Pipfile.lock") {
        let Ok(source) = fs.read_to_string(path) else {
            return Vec::new();
        };
        let lock: serde_json::Value = match serde_json::from_str(&source) {
            Ok(lock) => lock,
            Err(error) => {
                tracing::warn!("Could not read Django dependency metadata from {path}: {error}");
                return Vec::new();
            }
        };
        return lock
            .get("default")
            .and_then(serde_json::Value::as_object)
            .into_iter()
            .flatten()
            .filter(|(name, _)| name.eq_ignore_ascii_case("django"))
            .filter_map(|(_, package)| {
                package
                    .get("version")?
                    .as_str()?
                    .strip_prefix("==")?
                    .parse()
                    .ok()
                    .map(|version| LockedDjango {
                        version: Some(version),
                        marker: match package.get("markers").and_then(serde_json::Value::as_str) {
                            Some(source) => runtime_marker(source),
                            None => MarkerTree::TRUE,
                        },
                    })
            })
            .collect();
    }
    let Some(lock) = read_toml(fs, path) else {
        return Vec::new();
    };
    if path.file_name() == Some("uv.lock") {
        return uv_locked_django_versions(&lock);
    }
    let mut scope = lock_scope(&lock);
    if let Some(metadata) = lock.get("metadata") {
        scope.and(lock_scope(metadata));
    }
    let key = if path.file_name() == Some("pylock.toml") {
        "packages"
    } else {
        "package"
    };
    lock.get(key)
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|package| {
            if !package
                .get("name")?
                .as_str()?
                .eq_ignore_ascii_case("django")
            {
                return None;
            }
            if path.file_name() == Some("pdm.lock")
                && !package
                    .get("groups")
                    .and_then(toml::Value::as_array)
                    .is_some_and(|groups| {
                        groups.iter().any(|group| group.as_str() == Some("default"))
                    })
            {
                return None;
            }
            if path.file_name() == Some("poetry.lock") {
                let runtime = package
                    .get("groups")
                    .and_then(toml::Value::as_array)
                    .is_some_and(|groups| {
                        groups.iter().any(|group| group.as_str() == Some("main"))
                    })
                    || package.get("category").and_then(toml::Value::as_str) == Some("main");
                if !runtime || package.get("optional").and_then(toml::Value::as_bool) == Some(true)
                {
                    return None;
                }
            }
            let version = match package.get("version") {
                Some(value) => Some(value.as_str()?.parse().ok()?),
                None if ["vcs", "directory", "archive"]
                    .iter()
                    .any(|key| package.get(key).is_some_and(toml::Value::is_table)) =>
                {
                    None
                }
                None => return None,
            };
            let mut marker = scope.clone();
            marker.and(lock_scope(package));
            marker.and(package_marker(package));
            Some(LockedDjango { version, marker })
        })
        .collect()
}

fn lock_scope(value: &toml::Value) -> MarkerTree {
    let mut scope = MarkerTree::TRUE;
    for key in ["requires-python", "python-versions"] {
        if let Some(value) = value.get(key) {
            let marker = value
                .as_str()
                .and_then(poetry_specifiers)
                .map(|ranges| python_marker(&ranges));
            scope.and(marker.unwrap_or(MarkerTree::FALSE));
        }
    }
    if let Some(environments) = value.get("environments") {
        let mut alternatives = MarkerTree::FALSE;
        if let Some(environments) = environments.as_array() {
            for source in environments.iter().filter_map(toml::Value::as_str) {
                alternatives.or(runtime_marker(source));
            }
        }
        scope.and(alternatives);
    }
    scope
}

fn package_marker(package: &toml::Value) -> MarkerTree {
    let Some(value) = package.get("marker").or_else(|| package.get("markers")) else {
        return MarkerTree::TRUE;
    };
    // Poetry 2 can store a different marker for each group. Only main is active.
    let source = value.as_str().or_else(|| value.get("main")?.as_str());
    source.map_or(MarkerTree::FALSE, runtime_marker)
}

fn runtime_marker(source: &str) -> MarkerTree {
    let mut warned = false;
    let Ok(marker) = MarkerTree::parse_reporter(source, &mut |_, _| warned = true) else {
        // This parser does not support PEP 751's set-valued extras/groups yet.
        // Such entries are not evidence for a runtime dependency.
        return MarkerTree::FALSE;
    };
    if warned {
        return MarkerTree::FALSE;
    }
    without_extras(marker)
}

fn without_extras(marker: MarkerTree) -> MarkerTree {
    if marker.is_false() || marker.is_true() {
        return marker;
    }
    let mut result = MarkerTree::FALSE;
    for expressions in marker.to_dnf() {
        let mut branch = MarkerTree::TRUE;
        for expression in expressions {
            let is_extra = matches!(expression, MarkerExpression::Extra { .. });
            let predicate = MarkerTree::expression(expression);
            if is_extra {
                if !predicate.evaluate_extras(&[]) {
                    branch = MarkerTree::FALSE;
                    break;
                }
            } else {
                branch.and(predicate);
            }
        }
        result.or(branch);
    }
    result
}

fn uv_locked_django_versions(lock: &toml::Value) -> Vec<LockedDjango> {
    let Some(packages) = lock.get("package").and_then(toml::Value::as_array) else {
        return Vec::new();
    };
    let roots = packages
        .iter()
        .enumerate()
        .filter(|(_, package)| {
            package.get("source").is_some_and(|source| {
                source.get("virtual").and_then(toml::Value::as_str) == Some(".")
                    || source.get("editable").and_then(toml::Value::as_str) == Some(".")
            })
        })
        .map(|(index, _)| index);

    let mut pending: Vec<_> = roots
        .map(|index| (index, lock_scope(lock), Vec::<String>::new()))
        .collect();
    let mut seen = FxHashSet::default();
    let mut found = Vec::new();
    while let Some((index, mut marker, extras)) = pending.pop() {
        let package = &packages[index];
        marker.and(package_marker(package));
        marker.and(lock_scope(package));
        if let Some(markers) = package
            .get("resolution-markers")
            .and_then(toml::Value::as_array)
        {
            let mut resolution = MarkerTree::FALSE;
            for source in markers.iter().filter_map(toml::Value::as_str) {
                resolution.or(runtime_marker(source));
            }
            marker.and(resolution);
        }
        if marker.is_false() {
            continue;
        }
        let name = package
            .get("name")
            .and_then(toml::Value::as_str)
            .unwrap_or("");
        let version = package
            .get("version")
            .and_then(toml::Value::as_str)
            .unwrap_or("");
        let key = (index, extras.clone(), marker.clone());
        if !seen.insert(key) {
            continue;
        }
        if name.eq_ignore_ascii_case("django")
            && let Ok(version) = version.parse()
        {
            found.push(LockedDjango {
                version: Some(version),
                marker: marker.clone(),
            });
        }
        let normal = package
            .get("dependencies")
            .and_then(toml::Value::as_array)
            .into_iter()
            .flatten();
        let optional = extras.iter().flat_map(|extra| {
            package
                .get("optional-dependencies")
                .and_then(|groups| groups.get(extra))
                .and_then(toml::Value::as_array)
                .into_iter()
                .flatten()
        });
        for edge in normal.chain(optional) {
            let Some(edge_name) = edge.get("name").and_then(toml::Value::as_str) else {
                continue;
            };
            let mut edge_marker = marker.clone();
            edge_marker.and(package_marker(edge));
            if edge_marker.is_false() {
                continue;
            }
            let edge_extras = edge
                .get("extra")
                .and_then(toml::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(toml::Value::as_str)
                .map(str::to_owned)
                .collect();
            let mut targets = packages.iter().enumerate().filter(|(_, candidate)| {
                candidate.get("name").and_then(toml::Value::as_str) == Some(edge_name)
                    && ["version", "source"].into_iter().all(|field| {
                        edge.get(field).is_none() || edge.get(field) == candidate.get(field)
                    })
            });
            if let Some((target, _)) = targets.next()
                && targets.next().is_none()
            {
                pending.push((target, edge_marker, edge_extras));
            }
        }
    }
    found
}

fn read_toml(fs: &dyn FileSystem, path: &Utf8Path) -> Option<toml::Value> {
    let source = fs.read_to_string(path).ok()?;
    match toml::from_str(&source) {
        Ok(value) => Some(value),
        Err(error) => {
            tracing::warn!("Could not read Django dependency metadata from {path}: {error}");
            None
        }
    }
}

fn feature_line(version: &Version) -> Option<DjangoVersion> {
    match version.release() {
        [5, 2, ..] => Some(DjangoVersion::Django52),
        [6, 0, ..] | [6] => Some(DjangoVersion::Django60),
        [6, 1, ..] => Some(DjangoVersion::Django61),
        _ => None,
    }
}

#[derive(Default)]
struct DjangoRequirements {
    entries: Vec<DjangoRequirement>,
    environment: MarkerTree,
}

struct DjangoRequirement {
    alternatives: Vec<VersionSpecifiers>,
    marker: MarkerTree,
    required: bool,
}

impl DjangoRequirements {
    fn add(&mut self, source: &str) {
        self.add_with_provenance(source, true);
    }

    fn add_with_provenance(&mut self, source: &str, required: bool) {
        let Ok(requirement) = source.parse::<Requirement>() else {
            return;
        };
        if requirement.name.as_ref() != "django" {
            return;
        }
        let specifiers = match requirement.version_or_url {
            Some(VersionOrUrl::VersionSpecifier(specifiers)) => specifiers,
            Some(VersionOrUrl::Url(_)) | None => VersionSpecifiers::default(),
        };
        let marker = without_extras(requirement.marker);
        if !marker.is_false() {
            self.entries.push(DjangoRequirement {
                alternatives: vec![specifiers],
                marker,
                required,
            });
        }
    }

    fn add_poetry(&mut self, value: &toml::Value) {
        if let Some(values) = value.as_array() {
            for value in values {
                self.add_poetry(value);
            }
            return;
        }
        if value.get("optional").and_then(toml::Value::as_bool) == Some(true) {
            return;
        }
        let source = value
            .as_str()
            .or_else(|| value.get("version")?.as_str())
            .or_else(|| {
                ["git", "url", "path", "file"]
                    .iter()
                    .any(|key| value.get(key).and_then(toml::Value::as_str).is_some())
                    .then_some("*")
            });
        let Some(source) = source else {
            return;
        };
        let Some(specifiers) = poetry_specifiers(source) else {
            tracing::warn!("Could not read Poetry Django version constraint: {source}");
            return;
        };
        let mut marker = MarkerTree::TRUE;
        if let Some(source) = value.get("markers").and_then(toml::Value::as_str) {
            marker.and(runtime_marker(source));
        }
        if let Some(source) = value.get("python").and_then(toml::Value::as_str) {
            let Some(ranges) = poetry_specifiers(source) else {
                return;
            };
            marker.and(python_marker(&ranges));
        }
        if let Some(platform) = value.get("platform").and_then(toml::Value::as_str) {
            let Ok(parsed) = format!("sys_platform == '{platform}'").parse::<MarkerTree>() else {
                return;
            };
            marker.and(parsed);
        }
        if !marker.is_false() {
            self.entries.push(DjangoRequirement {
                alternatives: specifiers,
                marker,
                required: true,
            });
        }
    }

    fn read_setup_cfg(&mut self, fs: &dyn FileSystem, root: &Utf8Path) {
        let path = root.join("setup.cfg");
        let Ok(source) = fs.read_to_string(&path) else {
            return;
        };
        let mut config = configparser::ini::Ini::new();
        config.set_multiline(true);
        // Semicolons belong to PEP 508 environment markers, not INI comments.
        config.set_inline_comment_symbols(Some(&['#']));
        if let Err(error) = config.read(source) {
            tracing::warn!("Could not read Django dependency metadata from {path}: {error}");
            return;
        }
        if let Some(source) = config.get("options", "python_requires")
            && let Ok(range) = source.parse::<VersionSpecifiers>()
        {
            self.environment.and(python_marker(&[range]));
        }
        if let Some(requirements) = config.get("options", "install_requires") {
            for requirement in requirements.lines() {
                self.add(requirement.trim());
            }
        }
    }

    fn read_files(&mut self, fs: &dyn FileSystem, root: &Utf8Path) {
        let mut pending = vec![
            (root.join("requirements.txt"), true),
            (root.join("requirements.in"), true),
        ];
        let mut seen = FxHashSet::default();
        while let Some((path, required)) = pending.pop() {
            let path = fs.canonicalize(&path).unwrap_or(path).clean();
            if !seen.insert((path.clone(), required)) {
                continue;
            }
            let Ok(source) = fs.read_to_string(&path) else {
                continue;
            };
            for line in source.replace("\\\r\n", "").replace("\\\n", "").lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                let line = line
                    .split_once(" #")
                    .map_or(line, |(requirement, _)| requirement)
                    .trim();
                let include = [
                    ("--requirement", true),
                    ("-r", true),
                    ("--constraint", false),
                    ("-c", false),
                ]
                .iter()
                .find_map(|(prefix, mode)| line.strip_prefix(prefix).map(|value| (value, *mode)));
                if let Some((include, include_required)) = include {
                    let include = include.trim_start_matches('=').trim();
                    if !include.is_empty()
                        && let Some(parent) = path.parent()
                    {
                        pending.push((parent.join(include), include_required));
                    }
                } else {
                    self.add_with_provenance(
                        line.split_once(" --hash=")
                            .map_or(line, |(requirement, _)| requirement),
                        required,
                    );
                }
            }
        }
    }

    fn has_requirement(&self) -> bool {
        self.entries.iter().any(|entry| entry.required)
    }

    fn admits(&self, version: &Version, lock_marker: &MarkerTree) -> bool {
        // Look for a feasible environment in which Django is required and all
        // active constraints admit this version. Never use the host interpreter
        // to choose between a project's conditional dependency branches.
        let mut allowed = self.environment.clone();
        // A reachable runtime lock entry is itself requirement evidence when
        // declarations contain constraints only.
        let mut required = if self.has_requirement() {
            MarkerTree::FALSE
        } else {
            MarkerTree::TRUE
        };
        for entry in &self.entries {
            let contains = entry
                .alternatives
                .iter()
                .any(|range| range.contains(version));
            if contains && entry.required {
                required.or(entry.marker.clone());
            }
            if !contains {
                allowed.and(entry.marker.negate());
            }
        }
        allowed.and(required);
        allowed.and(lock_marker.clone());
        !allowed.is_false()
    }

    fn feature_line(&self) -> Option<DjangoVersion> {
        for (major, minor, line) in [
            (5, 2, DjangoVersion::Django52),
            (6, 0, DjangoVersion::Django60),
            (6, 1, DjangoVersion::Django61),
        ] {
            let mut candidates = vec![Version::new([major, minor, 0])];
            for specifier in self
                .entries
                .iter()
                .flat_map(|entry| entry.alternatives.iter())
                .flat_map(|range| range.iter())
            {
                let boundary = specifier.version();
                if feature_line(boundary) != Some(line) {
                    continue;
                }
                candidates.push(boundary.clone());
                // Check the next patch as well as exact boundaries: !=6.0.0 and
                // >6.0.3 still admit this feature line. Exact patch pins select
                // their line even when the bundled patch is newer.
                if let Some(patch) = boundary
                    .release()
                    .get(2)
                    .copied()
                    .unwrap_or(0)
                    .checked_add(1)
                {
                    candidates.push(Version::new([major, minor, patch]));
                }
            }
            if candidates
                .iter()
                .any(|version| self.admits(version, &MarkerTree::TRUE))
            {
                return Some(line);
            }
        }
        None
    }
}

fn python_marker(ranges: &[VersionSpecifiers]) -> MarkerTree {
    let mut python = MarkerTree::FALSE;
    for range in ranges {
        let mut branch = MarkerTree::TRUE;
        for specifier in range.iter() {
            branch.and(MarkerTree::expression(MarkerExpression::Version {
                key: MarkerValueVersion::PythonFullVersion,
                specifier: specifier.clone(),
            }));
        }
        python.or(branch);
    }
    python
}

// Poetry's legacy operators are not PEP 440 syntax. Translate each union branch
// separately so alternatives remain alternatives when combined with other inputs.
fn poetry_specifiers(source: &str) -> Option<Vec<VersionSpecifiers>> {
    source
        .split("||")
        .map(|branch| {
            let mut terms = Vec::new();
            for term in poetry_terms(branch)? {
                if term == "*" {
                    continue;
                }
                if term.starts_with('^') || (term.starts_with('~') && !term.starts_with("~=")) {
                    let version: Version = term[1..].trim().parse().ok()?;
                    let mut upper = version.release().to_vec();
                    let index = if term.starts_with('^') {
                        upper
                            .iter()
                            .position(|part| *part != 0)
                            .unwrap_or(upper.len() - 1)
                    } else {
                        usize::from(upper.len() > 1)
                    };
                    upper[index] = upper[index].checked_add(1)?;
                    upper.truncate(index + 1);
                    terms.push(format!(">={version},<{}", Version::new(upper)));
                } else if term.starts_with(|c: char| c.is_ascii_digit()) {
                    terms.push(format!("=={term}"));
                } else {
                    terms.push(term.clone());
                }
            }
            terms.join(",").parse().ok()
        })
        .collect()
}

fn poetry_terms(branch: &str) -> Option<Vec<String>> {
    let mut terms = Vec::new();
    let mut rest = branch.trim();
    while !rest.is_empty() {
        rest = rest
            .trim_start_matches(|character: char| character.is_whitespace() || character == ',');
        if rest.is_empty() {
            break;
        }
        let operator_len = ["===", "==", "!=", "<=", ">=", "~=", "<", ">", "^", "~"]
            .into_iter()
            .find(|operator| rest.starts_with(operator))
            .map_or(0, str::len);
        let (operator, after_operator) = rest.split_at(operator_len);
        let after_operator = after_operator.trim_start();
        let end = after_operator
            .find(|character: char| character.is_whitespace() || character == ',')
            .unwrap_or(after_operator.len());
        if end == 0 {
            return None;
        }
        terms.push(format!("{operator}{}", &after_operator[..end]));
        rest = &after_operator[end..];
    }
    Some(terms)
}

#[cfg(test)]
mod tests {
    use djls_conf::DjangoVersion;

    use super::DjangoRequirements;

    #[test]
    fn dependency_ranges_select_the_lowest_compatible_feature_line() {
        for (requirement, expected) in [
            ("Django>=5.2", Some(DjangoVersion::Django52)),
            ("Django>=6.0,<6.1", Some(DjangoVersion::Django60)),
            ("Django==6.0.3", Some(DjangoVersion::Django60)),
            ("Django~=6.1.2", Some(DjangoVersion::Django61)),
            ("Django>=5.2,!=5.2.*", Some(DjangoVersion::Django60)),
            ("Django>6.0.3,<6.1", Some(DjangoVersion::Django60)),
            ("Django>=6.0,!=6.0.0", Some(DjangoVersion::Django60)),
            ("Django==6.1.*", Some(DjangoVersion::Django61)),
            ("Django==5.1.9", None),
            ("Django>=7", None),
            ("Django>=6.1,<6.0", None),
        ] {
            let mut requirements = DjangoRequirements::default();
            requirements.add(requirement);
            assert_eq!(requirements.feature_line(), expected, "{requirement}");
        }
    }

    #[test]
    fn repeated_constraints_intersect_but_markers_preserve_alternatives() {
        let mut requirements = DjangoRequirements::default();
        requirements.add("Django>=5.2");
        requirements.add("Django<6.1");
        requirements.add("Django[argon2]==5.2.3; python_version < '3.12'");
        requirements.add("Django==6.0.1; python_version >= '3.12'");
        assert_eq!(requirements.feature_line(), Some(DjangoVersion::Django52));
        requirements.add("Django>=6.0");
        assert_eq!(requirements.feature_line(), Some(DjangoVersion::Django60));
    }

    #[test]
    fn conditional_constraints_must_be_simultaneously_satisfiable() {
        let mut requirements = DjangoRequirements::default();
        requirements.add("Django>=6.0");
        requirements.add("Django<6.0; python_version < '3.12'");
        assert_eq!(requirements.feature_line(), Some(DjangoVersion::Django60));
        requirements.add("Django>=7; python_version >= '3.12'");
        assert_eq!(requirements.feature_line(), None);
    }
}
