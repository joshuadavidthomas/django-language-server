use std::fs;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::Stdio;

use tempfile::tempdir;

fn djls_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_djls"))
}

fn setup_project(dir: &Path) {
    std::fs::write(
        dir.join("djls.toml"),
        r#"
[tagspecs]
version = "0.6.0"
engine = "django"

[[tagspecs.libraries]]
module = "django.template.defaulttags"

[[tagspecs.libraries.tags]]
name = "block"
type = "block"

[tagspecs.libraries.tags.end]
name = "endblock"

[[tagspecs.libraries.tags]]
name = "if"
type = "block"

[tagspecs.libraries.tags.end]
name = "endif"

[[tagspecs.libraries.tags]]
name = "for"
type = "block"

[tagspecs.libraries.tags.end]
name = "endfor"
"#,
    )
    .unwrap();
}

fn configure_template_directories(dir: &Path, directories: &[&Path]) {
    let config = std::fs::read_to_string(dir.join("djls.toml")).unwrap();
    std::fs::write(
        dir.join("djls.toml"),
        format!("django_settings_module = \"settings\"\n{config}"),
    )
    .unwrap();

    let directories = directories
        .iter()
        .map(|path| format!("'{path}'", path = path.display()))
        .collect::<Vec<_>>()
        .join(", ");
    std::fs::write(
        dir.join("settings.py"),
        format!(
            "INSTALLED_APPS = []\nTEMPLATES = [{{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [{directories}], 'APP_DIRS': False}}]\n"
        ),
    )
    .unwrap();
}

fn setup_multi_backend_project(dir: &Path) {
    let alpha_templates = dir.join("alpha-templates");
    let beta_templates = dir.join("beta-templates");
    fs::create_dir_all(&alpha_templates).unwrap();
    fs::create_dir_all(&beta_templates).unwrap();
    fs::write(
        dir.join("djls.toml"),
        "django_settings_module = \"settings\"\n",
    )
    .unwrap();
    fs::write(
        dir.join("settings.py"),
        format!(
            "INSTALLED_APPS = []\nTEMPLATES = [\n    {{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['{}'], 'APP_DIRS': False, 'OPTIONS': {{'libraries': {{'shared': 'alpha_tags'}}}}}},\n    {{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['{}'], 'APP_DIRS': False, 'OPTIONS': {{'libraries': {{'shared': 'beta_tags'}}}}}},\n]\n",
            alpha_templates.display(),
            beta_templates.display()
        ),
    )
    .unwrap();
    fs::write(
        dir.join("alpha_tags.py"),
        "from django import template\nregister = template.Library()\n@register.simple_tag(name='shared_tag')\ndef alpha_tag(value): pass\n",
    )
    .unwrap();
    fs::write(
        dir.join("beta_tags.py"),
        "from django import template\nregister = template.Library()\n@register.simple_tag(name='shared_tag')\ndef beta_tag(): pass\n",
    )
    .unwrap();
}

#[test]
fn check_clean_template_exits_zero() {
    let dir = tempfile::tempdir().unwrap();
    setup_project(dir.path());

    let templates = dir.path().join("templates");
    std::fs::create_dir_all(&templates).unwrap();
    std::fs::write(
        templates.join("good.html"),
        "{% block content %}<p>Hello</p>{% endblock %}\n",
    )
    .unwrap();

    let output = Command::new(djls_binary())
        .args(["check", "templates/"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "Expected exit 0, got {:?}\nstdout: {}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[test]
fn check_quiet_clean_template_exits_zero_without_output() {
    let dir = tempdir().unwrap();
    setup_project(dir.path());

    let templates = dir.path().join("templates");
    fs::create_dir_all(&templates).unwrap();
    fs::write(
        templates.join("good.html"),
        "{% block content %}<p>Hello</p>{% endblock %}\n",
    )
    .unwrap();

    let output = Command::new(djls_binary())
        .args(["check", "--quiet", "templates/"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[test]
fn check_without_django_source_keeps_builtin_grammar_and_source_less_loads() {
    let dir = tempdir().unwrap();
    setup_project(dir.path());

    let config = fs::read_to_string(dir.path().join("djls.toml")).unwrap();
    fs::write(
        dir.path().join("djls.toml"),
        format!(
            "django_settings_module = \"settings\"\n{config}\n[[tagspecs.libraries]]\nmodule = \"missing.panel_tags\"\n\n[[tagspecs.libraries.tags]]\nname = \"panel\"\ntype = \"standalone\"\n"
        ),
    )
    .unwrap();
    fs::write(
        dir.path().join("settings.py"),
        "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['templates'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'panels': 'missing.panel_tags'}}}]\n",
    )
    .unwrap();
    let templates = dir.path().join("templates");
    fs::create_dir_all(&templates).unwrap();
    fs::write(
        templates.join("source-less.html"),
        "{% load panels %}{% panel %}{% if condition %}{% for item in items %}{% comment %}{% endfor %}{% endif %}{% endcomment %}{% empty %}empty{% endfor %}{% else %}fallback{% endif %}\n",
    )
    .unwrap();

    let output = Command::new(djls_binary())
        .args(["check", "templates/source-less.html"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "Expected source-less grammar to validate\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn check_pythonpath_custom_tag_is_discovered_before_parallel_validation() {
    let dir = tempdir().unwrap();
    let vendor = dir.path().join("vendor");
    let package = vendor.join("extras");
    let templates = dir.path().join("templates");
    fs::create_dir_all(&package).unwrap();
    fs::create_dir_all(&templates).unwrap();
    fs::write(package.join("__init__.py"), "").unwrap();
    fs::write(
        package.join("tags.py"),
        "from django import template\nregister = template.Library()\n@register.simple_tag\ndef custom_tag(): pass\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("settings.py"),
        "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['templates'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'custom': 'extras.tags'}}}]\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("djls.toml"),
        format!(
            "django_settings_module = \"settings\"\npythonpath = [\"{}\"]\n",
            vendor.display()
        ),
    )
    .unwrap();
    fs::write(
        templates.join("custom.html"),
        "{% load custom %}{% custom_tag %}\n",
    )
    .unwrap();

    let output = Command::new(djls_binary())
        .args(["check", "templates/custom.html"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "pythonpath Template Library must be primed before validation\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn check_broken_template_exits_one() {
    let dir = tempfile::tempdir().unwrap();
    setup_project(dir.path());

    let templates = dir.path().join("templates");
    std::fs::create_dir_all(&templates).unwrap();
    std::fs::write(
        templates.join("broken.html"),
        "{% block content %}\n<p>Hello</p>\n",
    )
    .unwrap();

    let output = Command::new(djls_binary())
        .args(["check", "templates/"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("S100"),
        "Expected S100 error code in output:\n{stdout}"
    );
    assert!(
        stdout.contains("Unclosed 'block' tag"),
        "Expected unclosed block tag message in output:\n{stdout}"
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "Found 1 error in 1 file.\n"
    );
}

#[test]
fn check_plural_path_summary_reports_errors_and_files_exactly() {
    let dir = tempdir().unwrap();
    setup_project(dir.path());

    let templates = dir.path().join("templates");
    fs::create_dir_all(&templates).unwrap();
    fs::write(templates.join("first.html"), "{% block first %}\n").unwrap();
    fs::write(templates.join("second.html"), "{% block second %}\n").unwrap();

    let output = Command::new(djls_binary())
        .args(["check", "templates/"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "Found 2 errors in 2 files.\n"
    );
}

#[test]
fn check_plural_errors_with_singular_file_summary_exactly() {
    let dir = tempdir().unwrap();
    setup_project(dir.path());

    let templates = dir.path().join("templates");
    fs::create_dir_all(&templates).unwrap();
    fs::write(
        templates.join("broken.html"),
        "{% first_unknown %}\n{% second_unknown %}\n",
    )
    .unwrap();

    let output = Command::new(djls_binary())
        .args(["check", "templates/"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "Found 2 errors in 1 file.\n"
    );
}

#[test]
fn check_quiet_counts_enabled_diagnostics_without_rendering() {
    let dir = tempdir().unwrap();
    setup_project(dir.path());

    let templates = dir.path().join("templates");
    fs::create_dir_all(&templates).unwrap();
    fs::write(templates.join("broken.html"), "{% block content %}\n").unwrap();

    let output = Command::new(djls_binary())
        .args(["check", "--quiet", "templates/"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(
        output.stdout.is_empty(),
        "quiet check must not render diagnostics or a summary: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        output.stderr.is_empty(),
        "quiet check must not write stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn check_ignore_suppresses_errors() {
    let dir = tempfile::tempdir().unwrap();
    setup_project(dir.path());

    let templates = dir.path().join("templates");
    std::fs::create_dir_all(&templates).unwrap();
    std::fs::write(
        templates.join("broken.html"),
        "{% block content %}\n<p>Hello</p>\n",
    )
    .unwrap();

    let output = Command::new(djls_binary())
        .args(["check", "--ignore", "S100", "templates/"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "Expected exit 0 with --ignore S100, got {:?}\nstdout: {}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        output.stdout.is_empty(),
        "disabled diagnostics must not be rendered: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        output.stderr.is_empty(),
        "disabled diagnostics must not be summarized: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn check_stdin_detects_errors() {
    let dir = tempfile::tempdir().unwrap();
    setup_project(dir.path());

    let mut child = Command::new(djls_binary())
        .args(["check", "-"])
        .current_dir(dir.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();

    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"{% block content %}<p>Hello</p>\n")
        .unwrap();

    let output = child.wait_with_output().unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("S100"),
        "Expected S100 in stdin output:\n{stdout}"
    );
    assert_eq!(String::from_utf8_lossy(&output.stderr), "Found 1 error.\n");
}

#[test]
fn check_plural_stdin_summary_reports_errors_exactly() {
    let dir = tempdir().unwrap();
    setup_project(dir.path());

    let mut child = Command::new(djls_binary())
        .args(["check", "-"])
        .current_dir(dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"{% first_unknown %}\n{% second_unknown %}\n")
        .unwrap();

    let output = child.wait_with_output().unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(String::from_utf8_lossy(&output.stderr), "Found 2 errors.\n");
}

#[test]
fn check_multi_backend_stdin_uses_inventory_while_concrete_path_uses_backend() {
    let dir = tempdir().unwrap();
    setup_multi_backend_project(dir.path());
    let alpha_template = dir.path().join("alpha-templates/page.html");
    let source = "{% load shared %}{% shared_tag %}\n";
    fs::write(&alpha_template, source).unwrap();

    let concrete = Command::new(djls_binary())
        .args(["check", "alpha-templates/page.html"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(
        concrete.status.code(),
        Some(1),
        "a concrete file must use only its resolving backend\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&concrete.stdout),
        String::from_utf8_lossy(&concrete.stderr)
    );
    let concrete_stdout = String::from_utf8_lossy(&concrete.stdout);
    assert!(concrete_stdout.contains("error[S117]"), "{concrete_stdout}");
    assert!(
        concrete_stdout.contains("Tag 'shared_tag' requires at least 1 argument"),
        "{concrete_stdout}"
    );

    let mut child = Command::new(djls_binary())
        .args(["check", "-"])
        .current_dir(dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(source.as_bytes())
        .unwrap();
    let stdin = child.wait_with_output().unwrap();

    assert!(
        stdin.status.success(),
        "synthetic stdin must use the outside-root Project Inventory\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&stdin.stdout),
        String::from_utf8_lossy(&stdin.stderr)
    );
    assert!(stdin.stdout.is_empty());
    assert!(stdin.stderr.is_empty());
}

#[test]
fn check_help_describes_generic_stdin_template() {
    let output = Command::new(djls_binary())
        .args(["check", "--help"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let normalized = stdout.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        normalized.contains("stdin is analyzed as a generic Template in the current Project"),
        "{stdout}"
    );
    assert!(
        normalized.contains("stdin cannot be combined with paths"),
        "{stdout}"
    );
}

#[test]
fn check_rejects_mixed_stdin_and_paths() {
    let dir = tempfile::tempdir().unwrap();
    setup_project(dir.path());

    let output = Command::new(djls_binary())
        .args(["check", "-", "templates/"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Cannot mix `-` (stdin) with file or directory paths"),
        "Expected mixed-stdin error message, got:\n{stderr}"
    );
}

#[test]
fn check_invalid_settings_error_precedes_mixed_stdin_and_paths_error() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("djls.toml"), "debug = not_a_boolean\n").unwrap();

    let output = Command::new(djls_binary())
        .args(["check", "-", "template.html"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Failed to load settings"),
        "Expected settings error to win, got:\n{stderr}"
    );
    assert!(
        !stderr.contains("Cannot mix `-` (stdin) with file or directory paths"),
        "Mixed-input classification must happen after settings loading:\n{stderr}"
    );
}

#[test]
fn check_without_paths_scans_known_template_directories() {
    let dir = tempfile::tempdir().unwrap();
    setup_project(dir.path());

    let templates = dir.path().join("configured");
    std::fs::create_dir_all(&templates).unwrap();
    std::fs::write(
        templates.join("configured-broken.html"),
        "{% block content %}\n",
    )
    .unwrap();
    std::fs::write(dir.path().join("root-broken.html"), "{% block content %}\n").unwrap();
    configure_template_directories(dir.path(), &[&templates]);

    let output = Command::new(djls_binary())
        .arg("check")
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("configured-broken.html"), "{stdout}");
    assert!(!stdout.contains("root-broken.html"), "{stdout}");
}

#[test]
fn check_without_paths_falls_back_when_roots_may_be_omitted() {
    let dir = tempfile::tempdir().unwrap();
    setup_project(dir.path());
    std::fs::write(dir.path().join("broken.html"), "{% block content %}\n").unwrap();

    let output = Command::new(djls_binary())
        .arg("check")
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert_eq!(
        output.status.code(),
        Some(1),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("broken.html"),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn check_without_paths_does_not_fallback_for_closed_known_empty_roots() {
    let dir = tempfile::tempdir().unwrap();
    setup_project(dir.path());
    std::fs::write(dir.path().join("broken.html"), "{% block content %}\n").unwrap();
    configure_template_directories(dir.path(), &[]);

    let output = Command::new(djls_binary())
        .arg("check")
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "Expected closed empty roots not to fall back\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn check_explicit_paths_take_precedence_over_known_roots() {
    let dir = tempfile::tempdir().unwrap();
    setup_project(dir.path());

    let configured = dir.path().join("configured");
    std::fs::create_dir_all(&configured).unwrap();
    std::fs::write(configured.join("broken.html"), "{% block content %}\n").unwrap();
    let explicit = dir.path().join("explicit.html");
    std::fs::write(&explicit, "{% block content %}{% endblock %}\n").unwrap();
    configure_template_directories(dir.path(), &[&configured]);

    let output = Command::new(djls_binary())
        .args(["check", "explicit.html"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "Expected explicit path to override discovered roots\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn check_no_templates_exits_zero() {
    let dir = tempfile::tempdir().unwrap();
    setup_project(dir.path());

    let empty_dir = dir.path().join("templates");
    std::fs::create_dir_all(&empty_dir).unwrap();

    let output = Command::new(djls_binary())
        .args(["check", "templates/"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "Expected exit 0 for empty dir, got {:?}",
        output.status.code(),
    );
}
