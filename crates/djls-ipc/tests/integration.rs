use std::{
    path::Path,
    time::{Duration, Instant},
};

use anyhow::Result;
use djls_ipc::{Client, Server};
use serde::{Deserialize, Serialize};

const FIXTURES_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

async fn setup_echo_server() -> Result<(Server, Client)> {
    let path = format!("{}/echo_server.py", FIXTURES_PATH);
    let server = Server::start_script(&path, &[])?;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let client = Client::connect(server.get_path()).await?;
    Ok((server, client))
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
struct TestMessage {
    field1: String,
    field2: i32,
    vec_field: Vec<String>,
}

#[tokio::test]
async fn test_full_communication_cycle() -> Result<()> {
    let (_server, mut client) = setup_echo_server().await?;

    let test_msg = TestMessage {
        field1: "hello".to_string(),
        field2: 42,
        vec_field: vec!["a".to_string(), "b".to_string()],
    };

    let response: TestMessage = client.send(test_msg.clone()).await?;
    assert_eq!(response, test_msg);

    Ok(())
}

#[tokio::test]
async fn test_long_running_session() -> Result<()> {
    let (_server, mut client) = setup_echo_server().await?;

    for i in 0..10 {
        let string_msg = format!("test message {}", i);
        let response: String = client.send(string_msg.clone()).await?;
        assert_eq!(response, string_msg);

        let complex_msg = TestMessage {
            field1: format!("message {}", i),
            field2: i,
            vec_field: vec![format!("item {}", i)],
        };
        let response: TestMessage = client.send(complex_msg.clone()).await?;
        assert_eq!(response, complex_msg);
    }

    Ok(())
}

#[tokio::test]
async fn test_multiple_clients_single_server() -> Result<()> {
    let (server, mut client1) = setup_echo_server().await?;
    let mut client2 = Client::connect(server.get_path()).await?;
    let mut client3 = Client::connect(server.get_path()).await?;

    let msg1 = TestMessage {
        field1: "client1".to_string(),
        field2: 1,
        vec_field: vec!["a".to_string()],
    };
    let msg2 = TestMessage {
        field1: "client2".to_string(),
        field2: 2,
        vec_field: vec!["b".to_string()],
    };
    let msg3 = TestMessage {
        field1: "client3".to_string(),
        field2: 3,
        vec_field: vec!["c".to_string()],
    };

    let response1: TestMessage = client1.send(msg1.clone()).await?;
    let response2: TestMessage = client2.send(msg2.clone()).await?;
    let response3: TestMessage = client3.send(msg3.clone()).await?;

    assert_eq!(response1, msg1);
    assert_eq!(response2, msg2);
    assert_eq!(response3, msg3);

    Ok(())
}

#[tokio::test]
async fn test_server_restart() -> Result<()> {
    let (server1, mut client1) = setup_echo_server().await?;

    let msg = "test".to_string();
    let response: String = client1.send(msg.clone()).await?;
    assert_eq!(response, msg);

    drop(client1);
    drop(server1);

    let (_server2, mut client2) = setup_echo_server().await?;

    let msg = "test after restart".to_string();
    let response: String = client2.send(msg.clone()).await?;
    assert_eq!(response, msg);

    Ok(())
}

#[tokio::test]
async fn test_large_messages() -> Result<()> {
    let (_server, mut client) = setup_echo_server().await?;

    let large_vec: Vec<String> = (0..1000).map(|i| format!("item {}", i)).collect();

    let large_msg = TestMessage {
        field1: "x".repeat(10000),
        field2: 42,
        vec_field: large_vec.clone(),
    };

    let response: TestMessage = client.send(large_msg.clone()).await?;
    assert_eq!(response, large_msg);

    Ok(())
}

#[tokio::test]
async fn test_rapid_messages() -> Result<()> {
    let (_server, mut client) = setup_echo_server().await?;

    for i in 0..100 {
        let msg = format!("rapid message {}", i);
        let response: String = client.send(msg.clone()).await?;
        assert_eq!(response, msg);
    }

    Ok(())
}

#[tokio::test]
async fn test_connect_with_delayed_server() -> Result<()> {
    let path = format!("{}/echo_server.py", FIXTURES_PATH);
    let server_path = Path::new(&path).to_owned();

    // Start server with a shorter delay
    let server_handle = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        Server::start_script(server_path.to_str().unwrap(), &[])
    });

    // Wait for server to start
    let server = server_handle.await??;
    let mut client = Client::connect(server.get_path()).await?;

    // Test the connection works
    let msg = "test".to_string();
    let response: String = client.send(msg.clone()).await?;
    assert_eq!(response, msg);

    Ok(())
}

#[tokio::test]
async fn test_connect_with_server_restart() -> Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let socket_path = temp_dir.path().join("ipc.sock");

    // Start first server
    let path = format!("{}/echo_server.py", FIXTURES_PATH);
    let server = Server::start_script(&path, &["--ipc-path", socket_path.to_str().unwrap()])?;

    let mut client = Client::connect(&socket_path).await?;

    let msg = "test".to_string();
    let response: String = client.send(msg.clone()).await?;
    assert_eq!(response, msg);

    // Drop old server and client
    drop(server);
    drop(client);
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Start new server
    let new_server = Server::start_script(&path, &["--ipc-path", socket_path.to_str().unwrap()])?;
    println!(
        "Second server started, socket path: {:?}",
        new_server.get_path()
    );

    // Create new client
    let mut new_client = Client::connect(&socket_path).await?;

    // Try to send a message
    let msg = "test after restart".to_string();
    let response: String = new_client.send(msg.clone()).await?;
    assert_eq!(response, msg);

    Ok(())
}
