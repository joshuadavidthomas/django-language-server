use std::net::TcpListener;
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::Command;
use std::thread;
use std::time::Duration;

fn djls_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_djls"))
}

#[test]
fn serve_tcp_requires_an_address() {
    let output = Command::new(djls_binary())
        .args(["serve", "--connection-type", "tcp"])
        .output()
        .expect("djls serve process should run");

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--address <ADDRESS>"), "{stderr}");
    assert!(stderr.contains("required"), "{stderr}");
}

#[test]
fn serve_address_requires_tcp() {
    let output = Command::new(djls_binary())
        .args(["serve", "--address", "127.0.0.1:9257"])
        .output()
        .expect("djls serve process should run");

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("`--address` can only be used with `--connection-type tcp`"),
        "{stderr}"
    );
}

#[test]
fn serve_tcp_accepts_a_client() {
    let probe = TcpListener::bind("127.0.0.1:0").expect("test port should be allocated");
    let address = probe
        .local_addr()
        .expect("test address should be available");
    drop(probe);

    let mut child = Command::new(djls_binary())
        .args([
            "serve",
            "--connection-type",
            "tcp",
            "--address",
            &address.to_string(),
        ])
        .spawn()
        .expect("djls serve process should start");

    let stream = (0..100)
        .find_map(|_| {
            if let Ok(stream) = TcpStream::connect(address) {
                Some(stream)
            } else {
                thread::sleep(Duration::from_millis(10));
                None
            }
        })
        .unwrap_or_else(|| {
            child.kill().expect("TCP test server should be stopped");
            panic!("djls serve did not accept a TCP connection at {address}");
        });
    drop(stream);

    let status = (0..100)
        .find_map(|_| {
            let status = child.try_wait().expect("TCP test server should be polled");
            if status.is_none() {
                thread::sleep(Duration::from_millis(10));
            }
            status
        })
        .unwrap_or_else(|| {
            child.kill().expect("TCP test server should be stopped");
            panic!("djls serve did not exit after its TCP client disconnected");
        });

    assert!(status.success(), "TCP server exited with {status}");
}
