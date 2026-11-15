//! The Django side of the oracle: run `tools/django_oracle.py` and parse its answers.

use std::collections::BTreeMap;
use std::io::Read as _;
use std::io::Write as _;
use std::process::Command;
use std::process::Stdio;
use std::time::Duration;
use std::time::Instant;

use anyhow::Context as _;
use serde::Deserialize;
use serde::Serialize;

use crate::oracle::OracleTarget;

const ORACLE_SCRIPT: &str = include_str!("../../../../tools/django_oracle.py");
const ORACLE_CONFIG: &str = include_str!("../../oracle.toml");

#[derive(Debug, Default, Serialize)]
pub(super) struct Request {
    pub(super) settings_module: String,
    pub(super) sys_path: Vec<String>,
    pub(super) library_modules: BTreeMap<String, String>,
    pub(super) template_names: Vec<String>,
    pub(super) include_django: bool,
    pub(super) backend: Option<String>,
    pub(super) replay_templates: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize)]
pub(super) struct Answers {
    pub(super) setup_error: Option<String>,
    pub(super) django_version: Option<String>,
    pub(super) python_version: Option<String>,
    pub(super) backend: Option<String>,
    #[serde(default)]
    pub(super) native_probes: BTreeMap<String, Probe>,
    #[serde(default)]
    pub(super) library_errors: BTreeMap<String, LibraryError>,
    #[serde(default)]
    pub(super) libraries: Vec<Library>,
    pub(super) templates: Option<Templates>,
}

#[derive(Debug, Deserialize)]
pub(super) struct LibraryError {
    pub(super) kind: LibraryErrorKind,
    pub(super) message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum LibraryErrorKind {
    /// The module imports but has no module-level `register`: a fact about the source.
    NotALibrary,
    /// The module could not be imported in this environment: a gap in the oracle.
    ImportFailed,
}

#[derive(Debug, Deserialize)]
pub(super) struct Library {
    pub(super) module: String,
    pub(super) load_name: Option<String>,
    pub(super) probed: bool,
    pub(super) tags: BTreeMap<String, Tag>,
    pub(super) filters: BTreeMap<String, Filter>,
}

#[derive(Debug, Deserialize)]
pub(super) struct Tag {
    pub(super) kind: String,
    #[serde(default)]
    pub(super) probes: Vec<Probe>,
    pub(super) block: Option<Block>,
    pub(super) min_compiled_bits: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(super) struct Block {
    pub(super) end_tags: Vec<String>,
    pub(super) intermediates: Vec<String>,
    #[serde(default)]
    pub(super) unexplored: BTreeMap<String, String>,
    pub(super) opening_arguments: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct Filter {
    pub(super) arity: Option<String>,
    #[serde(default)]
    pub(super) probes: Vec<Probe>,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct Probe {
    pub(super) template: String,
    pub(super) django: ProbeOutcome,
    pub(super) error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ProbeOutcome {
    Compiled,
    Rejected,
    Unchecked,
}

#[derive(Debug, Deserialize)]
pub(super) struct Templates {
    pub(super) resolved: BTreeMap<String, serde_json::Value>,
    pub(super) truncated: bool,
}

pub(super) fn run(
    target: &OracleTarget,
    request: &Request,
    timeout: Duration,
) -> anyhow::Result<Answers> {
    let script_dir = tempfile::tempdir()?;
    let script = script_dir.path().join("django_oracle.py");
    std::fs::write(&script, ORACLE_SCRIPT)?;

    // Django resolves relative `DIRS` against its working directory; run where a user would.
    let mut command = Command::new(&target.python);
    command.envs(target.env.iter().map(|(key, value)| (key, value)));
    let mut child = command
        .arg("-I")
        .arg(&script)
        .current_dir(&target.project_root)
        .env_remove("DJANGO_SETTINGS_MODULE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to start `{}`", target.python))?;
    child
        .stdin
        .take()
        .context("oracle stdin")?
        .write_all(&serde_json::to_vec(request)?)?;
    let mut stdout = child.stdout.take().context("oracle stdout")?;
    let mut stderr = child.stderr.take().context("oracle stderr")?;
    let reader = std::thread::spawn(move || {
        let mut out = Vec::new();
        stdout.read_to_end(&mut out).map(|_| out)
    });
    let error_reader = std::thread::spawn(move || {
        let mut out = String::new();
        stderr.read_to_string(&mut out).map(|_| out)
    });
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() > timeout {
            child.kill()?;
            anyhow::bail!(
                "Django oracle for {} timed out after {timeout:?}",
                target.name
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let stdout = reader
        .join()
        .map_err(|_panic| anyhow::anyhow!("oracle stdout reader panicked"))??;
    let stderr = error_reader
        .join()
        .map_err(|_panic| anyhow::anyhow!("oracle stderr reader panicked"))??;
    anyhow::ensure!(
        status.success(),
        "Django oracle for {} exited with {status}: {}",
        target.name,
        stderr
            .lines()
            .skip(stderr.lines().count().saturating_sub(40))
            .collect::<Vec<_>>()
            .join("\n")
    );
    serde_json::from_slice(&stdout)
        .with_context(|| format!("Django oracle for {} wrote invalid JSON", target.name))
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct OracleConfig {
    #[serde(default)]
    apps: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    env: BTreeMap<String, BTreeMap<String, String>>,
}

/// Apps and Django-process environment `oracle.toml` configures for a corpus repository.
type RepoConfig = (Vec<String>, Vec<(String, String)>);

pub(super) fn oracle_repo_config(name: &str) -> anyhow::Result<RepoConfig> {
    let config: OracleConfig = toml::from_str(ORACLE_CONFIG).context("invalid oracle.toml")?;
    let apps = config.apps.get(name).cloned().unwrap_or_default();
    let env = config
        .env
        .get(name)
        .map(|env| env.clone().into_iter().collect())
        .unwrap_or_default();
    Ok((apps, env))
}
