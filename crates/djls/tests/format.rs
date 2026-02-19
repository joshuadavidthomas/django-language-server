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
fn format_passthrough_exits_zero_and_keeps_content() {
    let dir = tempfile::tempdir().unwrap();
    setup_project(dir.path());

    let templates = dir.path().join("templates");
    std::fs::create_dir_all(&templates).unwrap();

    let template_path = templates.join("page.html");
    let source = "{% if user %}<p>{{ user.name }}</p>{% endif %}\n";
    std::fs::write(&template_path, source).unwrap();

    let output = Command::new(djls_binary())
        .args(["format", "templates/"])
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

    let formatted = std::fs::read_to_string(template_path).unwrap();
    assert_eq!(formatted, source);
}

#[test]
fn format_check_exits_zero_for_passthrough_formatter() {
    let dir = tempfile::tempdir().unwrap();
    setup_project(dir.path());

    let templates = dir.path().join("templates");
    std::fs::create_dir_all(&templates).unwrap();
    std::fs::write(
        templates.join("page.djhtml"),
        "{%if user%}{{user.name}}{%endif%}\n",
    )
    .unwrap();

    let output = Command::new(djls_binary())
        .args(["format", "--check", "templates/"])
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
fn format_stdin_passthrough() {
    let dir = tempfile::tempdir().unwrap();
    setup_project(dir.path());

    let mut child = Command::new(djls_binary())
        .args(["format", "-"])
        .current_dir(dir.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();

    let source = "{%if user%}{{user.name}}{%endif%}\n";

    child
        .stdin
        .take()
        .unwrap()
        .write_all(source.as_bytes())
        .unwrap();

    let output = child.wait_with_output().unwrap();

    assert!(
        output.status.success(),
        "Expected exit 0, got {:?}\nstdout: {}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout, source);
}

#[test]
fn format_rejects_mixed_stdin_and_paths() {
    let dir = tempfile::tempdir().unwrap();
    setup_project(dir.path());

    let output = Command::new(djls_binary())
        .args(["format", "-", "templates/"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Cannot mix `-` (stdin) with file or directory paths"),
        "Expected mixed-stdin error message, got:\n{stdout}"
    );
}

#[test]
fn format_help_shows_expected_flags() {
    let output = Command::new(djls_binary())
        .args(["format", "--help"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "Expected --help to exit 0, got {:?}",
        output.status.code(),
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--check"));
    assert!(stdout.contains("--diff"));
    assert!(stdout.contains("--glob"));
    assert!(stdout.contains("--no-ignore"));
    assert!(stdout.contains("--follow"));
    assert!(stdout.contains("--max-depth"));
    assert!(stdout.contains("--color"));
}
