// Included only by the broker's unit-test module. Uses the production server,
// owned loopback listeners and an owned SQLite file; no OS process or signal.

async fn fixture_http(
    address: std::net::SocketAddr,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Value {
    let method = method.to_owned();
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || {
        use std::io::{Read, Write};
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let remaining = || {
            deadline.checked_duration_since(std::time::Instant::now())
                .filter(|value| !value.is_zero())
                .expect("owned HTTP absolute deadline exceeded")
        };
        let mut stream = std::net::TcpStream::connect_timeout(&address, remaining())
            .expect("owned broker connection");
        let body = body.map(|value| value.to_string()).unwrap_or_default();
        let request = format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        let mut pending = request.as_bytes();
        while !pending.is_empty() {
            stream.set_write_timeout(Some(remaining())).unwrap();
            let count = stream.write(pending).unwrap();
            assert!(count > 0, "owned HTTP request write stalled");
            pending = &pending[count..];
        }
        let mut response = Vec::new();
        loop {
            stream.set_read_timeout(Some(remaining())).unwrap();
            let mut chunk = [0u8; 4096];
            let count = stream.read(&mut chunk).unwrap();
            if count == 0 {
                break;
            }
            assert!(response.len() + count <= 65536, "owned HTTP response exceeds 64 KiB");
            response.extend_from_slice(&chunk[..count]);
        }
        let split = response.windows(4).position(|value| value == b"\r\n\r\n").unwrap();
        let header = std::str::from_utf8(&response[..split]).unwrap();
        assert!(header.starts_with("HTTP/1.1 200"), "{header}");
        serde_json::from_slice(&response[split + 4..]).unwrap()
    })
    .await
    .unwrap()
}

async fn fixture_service(
    path: &std::path::Path,
) -> (
    std::net::SocketAddr,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<std::io::Result<()>>,
) {
    let store = Arc::new(Store::open(path).unwrap());
    let main = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = main.local_addr().unwrap();
    let metrics = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let (done, shutdown) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(serve_bound(
        store,
        main,
        metrics,
        Duration::from_secs(3600),
        Duration::from_secs(2),
        async move {
            let _ = shutdown.await;
        },
    ));
    (address, done, server)
}

#[tokio::test]
async fn owned_http_round_trip_survives_graceful_service_reopen() {
    let path = std::env::temp_dir().join(format!(
        "swb-owned-service-{}-{}.sqlite3",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let from = "claude:macbook-neo:1001:owned-source";
    let to = "codex:sting:1002:owned-target";
    let pi = "pi:sting:1003:owned-pi";
    let thread = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
    let first_id = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
    let second_id = "01ARZ3NDEKTSV4RRFFQ69G5FAX";
    let (address, done, server) = fixture_service(&path).await;
    for (harness, host, pid, session, expected) in [
        ("claude", "macbook-neo", 1001, "owned-source", from),
        ("codex", "sting", 1002, "owned-target", to),
        ("pi", "sting", 1003, "owned-pi", pi),
    ] {
        let result = fixture_http(
            address,
            "POST",
            "/v1/register",
            Some(json!({
                "harness":harness,"host":host,"pid":pid,
                "session_id":session,"proc_start":"owned-fixture-start"
            })),
        )
        .await;
        assert_eq!(result["agent_id"], expected);
    }
    let input = json!({"from":from,"to":to,"ticket":"TIN-4655",
        "body":"owned fixture","thread_id":thread,"msg_id":first_id});
    let sent = fixture_http(address, "POST", "/v1/send", Some(input.clone())).await;
    assert_eq!(sent["authority"], "peer");
    assert_eq!(sent["seq"], 1);
    let retry = fixture_http(address, "POST", "/v1/send", Some(input)).await;
    assert_eq!(retry["seq"], 1);
    let inbox_path = format!("/v1/inbox?me={to}");
    let delivered = fixture_http(address, "GET", &inbox_path, None).await;
    assert_eq!(delivered["messages"][0]["msg_id"], first_id);
    assert_eq!(delivered["messages"][0]["delivery_count"], 1);
    done.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(3), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    // All server/store references are dropped. Reopen the same SQLite file
    // through the same production service entrypoint, never a Store-only call.
    let (address, done, server) = fixture_service(&path).await;
    let peers = fixture_http(address, "GET", "/v1/peers", None).await;
    assert_eq!(peers["peers"].as_array().unwrap().len(), 3);
    let redelivered = fixture_http(address, "GET", &inbox_path, None).await;
    assert_eq!(redelivered["messages"][0]["msg_id"], first_id);
    assert_eq!(redelivered["messages"][0]["delivery_count"], 2);
    let acknowledged = fixture_http(
        address,
        "POST",
        "/v1/ack",
        Some(json!({"me":to,"msg_id":first_id})),
    )
    .await;
    assert_eq!(acknowledged["state"], "acked");
    let empty = fixture_http(address, "GET", &inbox_path, None).await;
    assert!(empty["messages"].as_array().unwrap().is_empty());
    let next = fixture_http(
        address,
        "POST",
        "/v1/send",
        Some(json!({
            "from":to,"to":pi,"ticket":"none","body":"reply",
            "thread_id":thread,"msg_id":second_id,"in_reply_to":first_id
        })),
    )
    .await;
    assert_eq!(next["seq"], 2);
    assert_eq!(next["in_reply_to"], first_id);
    done.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(3), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    // Prove ack state and sequence remain durable through another reopen.
    let (address, done, server) = fixture_service(&path).await;
    let codex_empty = fixture_http(address, "GET", &inbox_path, None).await;
    assert!(codex_empty["messages"].as_array().unwrap().is_empty());
    let final_inbox = fixture_http(address, "GET", &format!("/v1/inbox?me={pi}"), None).await;
    let messages = final_inbox["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["msg_id"], second_id);
    assert_eq!(messages[0]["seq"], 2);
    assert_eq!(messages[0]["in_reply_to"], first_id);
    assert_eq!(messages[0]["from"], to);
    assert_eq!(messages[0]["to"], pi);
    done.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(3), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    for candidate in [
        path.clone(),
        std::path::PathBuf::from(format!("{}-wal", path.display())),
        std::path::PathBuf::from(format!("{}-shm", path.display())),
    ] {
        if candidate.exists() {
            std::fs::remove_file(candidate).unwrap();
        }
    }
}
