use std::path::PathBuf;
use std::process::Command;

fn djls_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_djls"))
}

#[test]
fn serve_tcp_reports_unsupported_connection_type() {
    let output = Command::new(djls_binary())
        .args(["serve", "--connection-type", "tcp"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("`djls serve --connection-type tcp` is not supported yet"),
        "Expected unsupported connection-type message, got:\n{stderr}"
    );
}
