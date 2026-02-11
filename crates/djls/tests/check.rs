use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

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
        stdout.contains("Unclosed tag"),
        "Expected 'Unclosed tag' in output:\n{stdout}"
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
        "Expected exit 0 with --ignore S100, got {:?}\nstdout: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
    );
}

#[test]
fn check_stdin_detects_errors() {
    let dir = tempfile::tempdir().unwrap();
    setup_project(dir.path());

    let mut child = Command::new(djls_binary())
        .args(["check"])
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
