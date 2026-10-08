//! Tailnet broker REST and rmcp Streamable HTTP server (SWB-R02, SWB-R14,
//! SWB-R33, SWB-R46). Audit output omits bodies until SWB-R33/34 proof.

use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
        ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
    },
    service::RequestContext,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use serde_json::{Value, json};
use std::{collections::HashMap, future::Future, io, sync::Arc, time::Duration};
use swb_proto::Authority;
use swb_store::Store;

pub const HOOK_TIMEOUT_MS: u64 = 2_000;
pub fn stamped_authority() -> Authority {
    Authority::Peer
}

fn operation(store: &Store, name: &str, args: &Value) -> Result<Value, String> {
    let mut created = true;
    let result = match name {
        "register" => store.register(args),
        "end" => store.end(
            args.get("me").and_then(Value::as_str).ok_or("missing me")?,
            args.get("proc_start")
                .and_then(Value::as_str)
                .ok_or("missing proc_start")?,
        ),
        "peers" => store.peers(
            args.get("me")
                .map(|v| v.as_str().ok_or("invalid me"))
                .transpose()?,
        ),
        "send" => {
            let outcome = store.send(args)?;
            created = outcome.created;
            Ok(outcome.envelope)
        }
        "inbox" => {
            let me = args.get("me").and_then(Value::as_str).ok_or("missing me")?;
            let limit = args
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(20)
                .min(100) as u32;
            let wait = args
                .get("wait_seconds")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                .min(25);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(wait);
            loop {
                let batch = store.inbox(me, limit)?;
                if batch["messages"].as_array().is_some_and(|v| !v.is_empty())
                    || std::time::Instant::now() >= deadline
                {
                    break Ok(batch);
                }
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
        }
        "ack" => store.ack(
            args.get("me").and_then(Value::as_str).ok_or("missing me")?,
            args.get("msg_id")
                .and_then(Value::as_str)
                .ok_or("missing msg_id")?,
        ),
        _ => Err("unknown operation".into()),
    }?;
    if created && matches!(name, "register" | "send" | "ack") {
        let body = result.get("body").and_then(Value::as_str).unwrap_or("");
        let body_hmac = if name == "send" {
            Some(store.body_hmac(body)?)
        } else {
            None
        };
        println!(
            "{}",
            audit_record(name, args, &result, body_hmac.as_deref())
        );
    }
    Ok(result)
}

fn audit_record(name: &str, args: &Value, result: &Value, body_hmac: Option<&str>) -> Value {
    let body_size = result
        .get("body")
        .and_then(Value::as_str)
        .map_or(0, str::len);
    json!({
        "ts":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0,|d|d.as_secs()),
        "op":name,"me":args.get("me").or_else(||args.get("from")),"to":result.get("to"),
        "ticket":result.get("ticket"),"ruling":result.get("ruling"),
        "msg_id":result.get("msg_id"),"size":body_size,"body_hmac":body_hmac,
        "agent_id":result.get("agent_id"),"state":result.get("state")
    })
}

fn tools() -> Vec<Tool> {
    [
        ("register", "Register a self-asserted agent session", json!({"type":"object","required":["harness","host","pid","session_id","proc_start"],"properties":{"harness":{"type":"string"},"host":{"type":"string"},"pid":{"type":"integer"},"session_id":{"type":"string"},"proc_start":{"type":"string"}}})),
        ("peers", "List session leases; optional registered me renews only that caller's lease; omission is passive discovery", json!({"type":"object","properties":{"me":{"type":"string"}}})),
        ("send", "Send a peer-authority message", json!({"type":"object","required":["from","to","ticket","body"],"properties":{"from":{"type":"string"},"to":{"type":"string"},"ticket":{"type":"string"},"body":{"type":"string"},"msg_id":{"type":"string"},"thread_id":{"type":"string"},"in_reply_to":{"type":"string"},"operator_directed":{"type":"boolean"},"ruling":{"type":"string","minLength":1,"maxLength":512},"reply_expires":{"type":"string","format":"date-time"},"reply_format":{"type":"string","maxLength":512},"artifacts":{"type":"array","maxItems":20,"uniqueItems":true,"items":{"type":"string","minLength":1,"maxLength":2048}},"ttl_hours":{"type":"integer"}}})),
        ("inbox", "Fetch unacked messages", json!({"type":"object","required":["me"],"properties":{"me":{"type":"string"},"limit":{"type":"integer"},"wait_seconds":{"type":"integer"}}})),
        ("ack", "Acknowledge a received message", json!({"type":"object","required":["me","msg_id"],"properties":{"me":{"type":"string"},"msg_id":{"type":"string"}}})),
    ].into_iter().map(|(name,description,schema)| Tool::new(name,description,Arc::new(schema.as_object().expect("static object schema").clone()))).collect()
}

#[derive(Clone)]
struct BrokerMcp {
    store: Arc<Store>,
}
impl ServerHandler for BrokerMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("agent-switchboard", "0.2.1"))
    }
    async fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult {
            tools: tools(),
            ..Default::default()
        })
    }
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let name = request.name.into_owned();
        if !matches!(
            name.as_str(),
            "register" | "peers" | "send" | "inbox" | "ack"
        ) {
            return Err(McpError::invalid_params("unknown tool", None));
        }
        let args = Value::Object(request.arguments.unwrap_or_default());
        let store = Arc::clone(&self.store);
        match tokio::task::spawn_blocking(move || operation(&store, &name, &args)).await {
            Ok(Ok(value)) => Ok(CallToolResult::structured(value).into()),
            Ok(Err(error)) => Ok(CallToolResult::error(vec![ContentBlock::text(error)]).into()),
            Err(error) => Err(McpError::internal_error(error.to_string(), None)),
        }
    }
}

async fn run_operation(store: Arc<Store>, name: &'static str, args: Value) -> Response {
    match tokio::task::spawn_blocking(move || operation(&store, name, &args)).await {
        Ok(Ok(value)) => (StatusCode::OK, Json(value)).into_response(),
        Ok(Err(error)) => {
            let status = if error == "msg_id conflict" {
                StatusCode::CONFLICT
            } else {
                StatusCode::BAD_REQUEST
            };
            (status, Json(json!({"error":error}))).into_response()
        }
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error":"broker unavailable"})),
        )
            .into_response(),
    }
}
async fn register(State(store): State<Arc<Store>>, Json(args): Json<Value>) -> Response {
    run_operation(store, "register", args).await
}
async fn end(State(store): State<Arc<Store>>, Json(args): Json<Value>) -> Response {
    run_operation(store, "end", args).await
}
async fn send(State(store): State<Arc<Store>>, Json(args): Json<Value>) -> Response {
    run_operation(store, "send", args).await
}
async fn ack(State(store): State<Arc<Store>>, Json(args): Json<Value>) -> Response {
    run_operation(store, "ack", args).await
}
async fn peers(
    State(store): State<Arc<Store>>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let args = query
        .get("me")
        .map_or_else(|| json!({}), |me| json!({"me":me}));
    run_operation(store, "peers", args).await
}
async fn inbox(
    State(store): State<Arc<Store>>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let args = json!({"me":query.get("me"),"limit":query.get("limit").and_then(|v|v.parse::<u64>().ok()),"wait_seconds":query.get("wait_seconds").and_then(|v|v.parse::<u64>().ok())});
    run_operation(store, "inbox", args).await
}
async fn metrics(State(store): State<Arc<Store>>) -> Response {
    match tokio::task::spawn_blocking(move || store.metrics()).await {
        Ok(Ok(value)) => (
            StatusCode::OK,
            [(
                axum::http::header::CONTENT_TYPE,
                "text/plain; version=0.0.4",
            )],
            value,
        )
            .into_response(),
        _ => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
#[cfg(test)]
fn router(store: Arc<Store>, allowed_hosts: Vec<String>) -> Router {
    router_with_config(
        store,
        StreamableHttpServerConfig::default().with_allowed_hosts(allowed_hosts),
    )
}

fn router_with_config(store: Arc<Store>, config: StreamableHttpServerConfig) -> Router {
    let factory_store = Arc::clone(&store);
    let service = StreamableHttpService::new(
        move || {
            Ok(BrokerMcp {
                store: Arc::clone(&factory_store),
            })
        },
        LocalSessionManager::default().into(),
        config,
    );
    Router::new()
        .nest_service("/mcp", service)
        .route("/v1/register", post(register))
        .route("/v1/end", post(end))
        .route("/v1/peers", get(peers))
        .route("/v1/send", post(send))
        .route("/v1/inbox", get(inbox))
        .route("/v1/ack", post(ack))
        .with_state(store)
}
pub async fn serve(store: Arc<Store>, listen: &str, metrics_listen: &str) -> io::Result<()> {
    serve_until(
        store,
        listen,
        metrics_listen,
        Duration::from_secs(3600),
        Duration::from_secs(30),
        shutdown_signal(),
    )
    .await
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("install SIGTERM handler");
        tokio::select! {
            _ = terminate.recv() => {},
            _ = tokio::signal::ctrl_c() => {},
        }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c()
        .await
        .expect("install Ctrl-C handler");
}

async fn serve_until<F>(
    store: Arc<Store>,
    listen: &str,
    metrics_listen: &str,
    prune_interval: Duration,
    drain_timeout: Duration,
    shutdown: F,
) -> io::Result<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    serve_until_with(
        store,
        listen,
        metrics_listen,
        prune_interval,
        drain_timeout,
        shutdown,
        Router::new(),
    )
    .await
}

/// `serve_until` plus `extra` routes merged into the main listener. Production
/// passes an empty router; only the `test-clock` feature passes anything else.
async fn serve_until_with<F>(
    store: Arc<Store>,
    listen: &str,
    metrics_listen: &str,
    prune_interval: Duration,
    drain_timeout: Duration,
    shutdown: F,
    extra: Router,
) -> io::Result<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    let main_listener = tokio::net::TcpListener::bind(listen).await?;
    let metrics_listener = tokio::net::TcpListener::bind(metrics_listen).await?;
    serve_bound_with(
        store,
        main_listener,
        metrics_listener,
        prune_interval,
        drain_timeout,
        shutdown,
        extra,
    )
    .await
}

#[cfg(test)]
async fn serve_bound<F>(
    store: Arc<Store>,
    main_listener: tokio::net::TcpListener,
    metrics_listener: tokio::net::TcpListener,
    prune_interval: Duration,
    drain_timeout: Duration,
    shutdown: F,
) -> io::Result<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    serve_bound_with(
        store,
        main_listener,
        metrics_listener,
        prune_interval,
        drain_timeout,
        shutdown,
        Router::new(),
    )
    .await
}

async fn serve_bound_with<F>(
    store: Arc<Store>,
    main_listener: tokio::net::TcpListener,
    metrics_listener: tokio::net::TcpListener,
    prune_interval: Duration,
    drain_timeout: Duration,
    shutdown: F,
    extra: Router,
) -> io::Result<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    let allowed_hosts = std::env::var("SWB_MCP_ALLOWED_HOSTS")
        .ok()
        .map(|v| {
            v.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_else(|| vec!["localhost".into(), "127.0.0.1".into(), "::1".into()]);
    store.prune().map_err(io::Error::other)?;
    let config = StreamableHttpServerConfig::default().with_allowed_hosts(allowed_hosts);
    let mcp_cancel = config.cancellation_token.clone();
    let app = router_with_config(Arc::clone(&store), config).merge(extra);
    let metrics_app = Router::new()
        .route("/metrics", get(metrics))
        .with_state(Arc::clone(&store));
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        shutdown.await;
        let _ = shutdown_tx.send(true);
    });
    let mut main_shutdown = shutdown_rx.clone();
    let mut metrics_shutdown = shutdown_rx.clone();
    let mut maintenance_shutdown = shutdown_rx;
    let mut drain_start = maintenance_shutdown.clone();
    let maintenance = async move {
        let mut interval =
            tokio::time::interval_at(tokio::time::Instant::now() + prune_interval, prune_interval);
        loop {
            tokio::select! {
                _ = maintenance_shutdown.changed() => break,
                _ = interval.tick() => {
                    let store = Arc::clone(&store);
                    match tokio::task::spawn_blocking(move || store.prune()).await {
                        Ok(Ok(())) => {},
                        Ok(Err(error)) => eprintln!("retention prune: {error}"),
                        Err(error) => eprintln!("retention prune task: {error}"),
                    }
                }
            }
        }
        Ok::<(), io::Error>(())
    };
    let listeners = async {
        tokio::try_join!(
            axum::serve(main_listener, app).with_graceful_shutdown(async move {
                let _ = main_shutdown.changed().await;
            }),
            axum::serve(metrics_listener, metrics_app).with_graceful_shutdown(async move {
                let _ = metrics_shutdown.changed().await;
            }),
            maintenance,
        )?;
        Ok::<(), io::Error>(())
    };
    tokio::pin!(listeners);
    tokio::select! {
        result = &mut listeners => result?,
        _ = drain_start.changed() => {
            match tokio::time::timeout(drain_timeout, &mut listeners).await {
                Ok(result) => result?,
                Err(_) => {
                    mcp_cancel.cancel();
                    tokio::time::timeout(Duration::from_secs(5), &mut listeners).await
                        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "broker drain exceeded deadline"))??;
                }
            }
        }
    }
    Ok(())
}

/// Test-only time control for the spec live adapter (R-C262). Compiled only
/// with the `test-clock` Cargo feature, which no production target enables:
/// the `//crates/swb:swb` binary and the `//deploy:image` it ships are built
/// without it. A broker built with it still runs on the wall clock unless
/// `swb serve` is started with `SWB_TEST_CLOCK=1` on loopback listeners.
#[cfg(feature = "test-clock")]
pub mod test_clock {
    use super::*;
    use std::net::SocketAddr;
    use swb_store::ManualClock;

    /// Largest single advance: 400 days, past every retention bound.
    pub const MAX_ADVANCE_SECONDS: u64 = 400 * 86_400;

    /// `POST /v1/test/clock {"advance_seconds": n}` moves the clock forward
    /// by n seconds (0 reads it) and returns `{"now": <unix seconds>}`.
    pub fn router(clock: Arc<ManualClock>) -> Router {
        Router::new()
            .route("/v1/test/clock", post(advance))
            .with_state(clock)
    }

    async fn advance(State(clock): State<Arc<ManualClock>>, Json(args): Json<Value>) -> Response {
        match args
            .get("advance_seconds")
            .and_then(Value::as_u64)
            .filter(|n| *n <= MAX_ADVANCE_SECONDS)
        {
            Some(n) => {
                let now = clock.advance(n as u32);
                (StatusCode::OK, Json(json!({"now":now}))).into_response()
            }
            None => (
                StatusCode::BAD_REQUEST,
                Json(json!({"error":"invalid advance_seconds"})),
            )
                .into_response(),
        }
    }

    /// Refuse any listener that is not a loopback socket address.
    pub fn require_loopback(listen: &str) -> io::Result<()> {
        match listen.parse::<SocketAddr>() {
            Ok(address) if address.ip().is_loopback() => Ok(()),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("test clock refuses non-loopback listener {listen:?}"),
            )),
        }
    }

    /// `serve` with the clock route on the main listener. Both listeners must
    /// be loopback addresses.
    pub async fn serve(
        store: Arc<Store>,
        clock: Arc<ManualClock>,
        listen: &str,
        metrics_listen: &str,
    ) -> io::Result<()> {
        require_loopback(listen)?;
        require_loopback(metrics_listen)?;
        serve_until_with(
            store,
            listen,
            metrics_listen,
            Duration::from_secs(3600),
            Duration::from_secs(30),
            shutdown_signal(),
            router(clock),
        )
        .await
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use axum::body::{Body, to_bytes};
        use axum::http::Request;
        use tower::ServiceExt;

        #[tokio::test]
        async fn clock_route_drives_store_time() {
            let clock = Arc::new(ManualClock::new(1_700_000_000));
            let store = Arc::new(Store::memory_with_clock(clock.clone()).unwrap());
            let app = router(clock).merge(super::super::router(
                Arc::clone(&store),
                vec!["localhost".into()],
            ));
            let post = |body: &'static str| {
                Request::post("/v1/test/clock")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap()
            };
            let response = app
                .clone()
                .oneshot(post(r#"{"advance_seconds":901}"#))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let body: Value =
                serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap())
                    .unwrap();
            assert_eq!(body["now"], 1_700_000_901);
            assert_eq!(store.now(), 1_700_000_901);
            for bad in [
                r#"{"advance_seconds":-1}"#,
                r#"{"advance_seconds":"1"}"#,
                r#"{"advance_seconds":34560001}"#,
                r#"{}"#,
            ] {
                let response = app.clone().oneshot(post(bad)).await.unwrap();
                assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            }
            assert_eq!(store.now(), 1_700_000_901);
        }

        #[test]
        fn refuses_non_loopback_listeners() {
            assert!(require_loopback("127.0.0.1:18080").is_ok());
            assert!(require_loopback("[::1]:18080").is_ok());
            assert!(require_loopback("0.0.0.0:8080").is_err());
            assert!(require_loopback("100.85.46.118:8080").is_err());
            assert!(require_loopback("localhost:8080").is_err());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use tower::ServiceExt;

    fn now_for_test() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
    }

    #[tokio::test]
    async fn idle_broker_prunes_retained_bodies_on_interval() {
        let path = std::env::temp_dir().join(format!(
            "swb-idle-prune-{}-{}.sqlite3",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let store = Arc::new(Store::open(&path).unwrap());
        let sent = store.send(&json!({"from":"claude:honey:1:a","to":"pi:sting:2:b","ticket":"none","body":"idle body"})).unwrap();
        let id = sent.envelope["msg_id"].as_str().unwrap();
        let main = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = main.local_addr().unwrap();
        let metrics = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let server = tokio::spawn(serve_bound(
            store,
            main,
            metrics,
            Duration::from_millis(20),
            Duration::from_secs(1),
            async move {
                let _ = stopped.await;
            },
        ));
        // A successful request proves startup pruning completed before the
        // row is aged. No send or inbox request occurs after that point.
        let ready = tokio::task::spawn_blocking(move || {
            use std::io::{Read, Write};
            let mut stream = std::net::TcpStream::connect(address).unwrap();
            stream
                .write_all(
                    b"GET /v1/peers HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
                )
                .unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).unwrap();
            response
        })
        .await
        .unwrap();
        assert!(ready.contains("200 OK"));
        let db = rusqlite::Connection::open(&path).unwrap();
        assert_eq!(
            db.query_row("SELECT COUNT(*) FROM messages", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        db.execute(
            "UPDATE messages SET created_at=unixepoch()-2592001 WHERE msg_id=?1",
            [id],
        )
        .unwrap();
        drop(db);
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let count: i64 = rusqlite::Connection::open(&path)
                    .unwrap()
                    .query_row("SELECT COUNT(*) FROM messages", [], |row| row.get(0))
                    .unwrap();
                if count == 0 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("idle prune did not run");
        stop.send(()).unwrap();
        server.await.unwrap().unwrap();
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(format!("{}-wal", path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", path.display()));
    }

    #[tokio::test]
    async fn shutdown_waits_for_an_active_inbox_request() {
        let path = std::env::temp_dir().join(format!(
            "swb-drain-{}-{}.sqlite3",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let store = Arc::new(Store::open(&path).unwrap());
        store
            .register(
                &json!({"harness":"pi","host":"sting","pid":2,"session_id":"b","proc_start":"1"}),
            )
            .unwrap();
        rusqlite::Connection::open(&path)
            .unwrap()
            .execute("UPDATE sessions SET last_seen=unixepoch()-100", [])
            .unwrap();
        let main = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = main.local_addr().unwrap();
        let metrics = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let server = tokio::spawn(serve_bound(
            store,
            main,
            metrics,
            Duration::from_secs(3600),
            Duration::from_secs(2),
            async move {
                let _ = stopped.await;
            },
        ));
        let request = tokio::task::spawn_blocking(move || {
            use std::io::{Read, Write};
            let mut stream = std::net::TcpStream::connect(address).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            stream.write_all(b"GET /v1/inbox?me=pi:sting:2:b&wait_seconds=1 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).unwrap();
            response
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let seen: i64 = rusqlite::Connection::open(&path)
                    .unwrap()
                    .query_row(
                        "SELECT last_seen FROM sessions WHERE agent_id='pi:sting:2:b'",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                if seen >= now_for_test() - 2 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("inbox handler did not start");
        stop.send(()).unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!server.is_finished(), "shutdown dropped an active request");
        assert!(request.await.unwrap().contains("200 OK"));
        tokio::time::timeout(Duration::from_secs(2), server)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(format!("{}-wal", path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", path.display()));
    }

    #[tokio::test]
    async fn shutdown_cancels_an_initialized_mcp_sse_stream_after_drain() {
        use std::io::{Read, Write};
        let main = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = main.local_addr().unwrap();
        let metrics = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let server = tokio::spawn(serve_bound(
            Arc::new(Store::memory().unwrap()),
            main,
            metrics,
            Duration::from_secs(3600),
            Duration::from_millis(150),
            async move {
                let _ = stopped.await;
            },
        ));
        let session = tokio::task::spawn_blocking(move || {
            let init = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"drain-test","version":"1"}}}).to_string();
            let mut stream = std::net::TcpStream::connect(address).unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            write!(stream, "POST /mcp HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", init.len(), init).unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).unwrap();
            assert!(response.contains("200 OK"), "{response}");
            response.lines().find_map(|line| line.strip_prefix("mcp-session-id: ").map(str::trim).map(str::to_owned)).expect("MCP session header")
        }).await.unwrap();
        let (headers_tx, headers_rx) = tokio::sync::oneshot::channel();
        let sse = tokio::task::spawn_blocking(move || {
            let mut stream = std::net::TcpStream::connect(address).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            write!(stream, "GET /mcp HTTP/1.1\r\nHost: localhost\r\nAccept: text/event-stream\r\nmcp-session-id: {session}\r\nConnection: close\r\n\r\n").unwrap();
            let mut headers = Vec::new();
            while !headers.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                stream.read_exact(&mut byte).unwrap();
                headers.push(byte[0]);
            }
            headers_tx
                .send(String::from_utf8(headers).unwrap())
                .unwrap();
            let mut rest = Vec::new();
            stream
                .read_to_end(&mut rest)
                .expect("SSE stream did not close");
            std::time::Instant::now()
        });
        let headers = tokio::time::timeout(Duration::from_secs(2), headers_rx)
            .await
            .unwrap()
            .unwrap();
        assert!(headers.contains("200 OK"), "{headers}");
        assert!(
            headers.to_ascii_lowercase().contains("text/event-stream"),
            "{headers}"
        );
        let stopped_at = std::time::Instant::now();
        stop.send(()).unwrap();
        let closed_at = tokio::time::timeout(Duration::from_secs(2), sse)
            .await
            .unwrap()
            .unwrap();
        assert!(closed_at.duration_since(stopped_at) >= Duration::from_millis(150));
        tokio::time::timeout(Duration::from_secs(2), server)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
    async fn mcp_call(app: &Router, session: &str, id: u32, name: &str, args: Value) -> Value {
        let call = json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":args}});
        let request = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("host", "localhost")
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .header("mcp-session-id", session)
            .body(Body::from(call.to_string()))
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 65536).await.unwrap();
        let raw = std::str::from_utf8(&body).unwrap();
        let payload = raw
            .lines()
            .filter_map(|line| line.strip_prefix("data: "))
            .find(|line| line.trim_start().starts_with('{'))
            .unwrap_or(raw);
        let value: Value =
            serde_json::from_str(payload).unwrap_or_else(|error| panic!("{error}: {raw:?}"));
        assert!(value.get("error").is_none(), "{value}");
        value["result"]["structuredContent"].clone()
    }
    #[test]
    fn broker_stamps_peer() {
        assert_eq!(stamped_authority(), Authority::Peer);
    }
    #[test]
    fn audit_excludes_body_and_unapproved_state() {
        let store = Store::memory().unwrap();
        let args = json!({"from":"claude:honey:1:a","to":"pi:sting:2:b","ticket":"none","body":"secret sentinel"});
        let envelope = store.send(&args).unwrap().envelope;
        let record = audit_record("send", &args, &envelope, Some("keyed-hmac"));
        let line = record.to_string();
        assert!(!line.contains("secret sentinel"));
        assert_eq!(record["state"], Value::Null);
        assert_eq!(record["body_hmac"], "keyed-hmac");
        let mut invalid = args;
        invalid["state"] = json!("operator");
        assert_eq!(store.send(&invalid).unwrap_err(), "unknown envelope field");
    }
    #[test]
    fn lists_five_typed_tools() {
        assert_eq!(tools().len(), 5);
        assert!(
            tools()
                .iter()
                .all(|t| t.input_schema.contains_key("properties"))
        );
        let peers = tools().into_iter().find(|t| t.name == "peers").unwrap();
        assert_eq!(peers.input_schema["properties"]["me"]["type"], "string");
        let send = tools().into_iter().find(|t| t.name == "send").unwrap();
        let props = &send.input_schema["properties"];
        for key in ["artifacts", "reply_expires", "reply_format"] {
            assert!(
                props.get(key).is_some(),
                "missing {key} in advertised send schema"
            );
        }
    }
    #[tokio::test]
    async fn rmcp_streamable_http_initialize_and_tools() {
        let app = router(Arc::new(Store::memory().unwrap()), vec!["localhost".into()]);
        let init = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"swb-test","version":"1"}}});
        let request = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("host", "localhost")
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .body(Body::from(init.to_string()))
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let session = response
            .headers()
            .get("mcp-session-id")
            .expect("rmcp session id")
            .to_str()
            .unwrap()
            .to_owned();
        let body = to_bytes(response.into_body(), 65536).await.unwrap();
        assert!(String::from_utf8_lossy(&body).contains("agent-switchboard"));
        let list = json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}});
        let request = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("host", "localhost")
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .header("mcp-session-id", &session)
            .body(Body::from(list.to_string()))
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 65536).await.unwrap();
        assert!(String::from_utf8_lossy(&body).contains("register"));
        let from = "claude:honey:1:a";
        let to = "pi:sting:2:b";
        let first = mcp_call(
            &app,
            &session,
            3,
            "register",
            json!({"harness":"claude","host":"honey","pid":1,"session_id":"a","proc_start":"1"}),
        )
        .await;
        assert_eq!(first["agent_id"], from);
        let second = mcp_call(
            &app,
            &session,
            4,
            "register",
            json!({"harness":"pi","host":"sting","pid":2,"session_id":"b","proc_start":"2"}),
        )
        .await;
        assert_eq!(second["agent_id"], to);
        let peers = mcp_call(&app, &session, 5, "peers", json!({})).await;
        assert_eq!(peers["peers"].as_array().unwrap().len(), 2);
        let msg_id = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
        let sent = mcp_call(&app, &session, 6, "send", json!({"from":from,"to":to,"ticket":"none","body":"hello","msg_id":msg_id,"authority":"operator"})).await;
        assert_eq!(sent["authority"], "peer");
        assert_eq!(sent["seq"], 1);
        let received = mcp_call(&app, &session, 7, "inbox", json!({"me":to})).await;
        assert_eq!(received["messages"][0]["msg_id"], msg_id);
        let acked = mcp_call(&app, &session, 8, "ack", json!({"me":to,"msg_id":msg_id})).await;
        assert_eq!(acked["state"], "acked");
        let empty = mcp_call(&app, &session, 9, "inbox", json!({"me":to})).await;
        assert!(empty["messages"].as_array().unwrap().is_empty());
    }
    #[tokio::test]
    async fn conflict_is_opaque_over_rest() {
        let app = router(Arc::new(Store::memory().unwrap()), vec!["localhost".into()]);
        let msg = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
        for body in ["A", "B"] {
            let args = json!({"from":"claude:honey:1:a","to":"pi:sting:2:b","body":body,"ticket":"none","msg_id":msg});
            let request = Request::builder()
                .method("POST")
                .uri("/v1/send")
                .header("content-type", "application/json")
                .body(Body::from(args.to_string()))
                .unwrap();
            let response = app.clone().oneshot(request).await.unwrap();
            if body == "B" {
                assert_eq!(response.status(), StatusCode::CONFLICT);
                let content = to_bytes(response.into_body(), 65536).await.unwrap();
                assert_eq!(content.as_ref(), br#"{"error":"msg_id conflict"}"#);
            } else {
                assert_eq!(response.status(), StatusCode::OK);
            }
        }
    }
}
