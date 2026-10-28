//! Tailnet broker REST and rmcp Streamable HTTP server (SWB-R02, SWB-R14,
//! SWB-R33, SWB-R46). Audit output omits bodies until SWB-R33/34 proof.
//! Every route on the main listener passes the Host/Origin guard
//! ([`RequestGuard`], ADR-0001 → Exposure and auth).
//!
//! v0.3.0 (R-C944): every state-changing call is a POST with a JSON body
//! (`GET /v1/inbox` and `GET /v1/peers?me=` answer 405), `end` needs the
//! end token `register` returned, errors are typed JSON
//! (`{"error": <message>, "code": <code>}`), and `/healthz` answers on both
//! listeners.
//!
//! v0.5.0 (ADR-0003, SWB-R66): opt-in sender binding. For an agent id whose
//! (harness, host, session id) is bound, `send` (by `from`), `inbox`, `ack`,
//! `claim`, `release`, `handoff` and `end` (by `me`) need the
//! `session_token` its latest `register` returned, or answer 401
//! `unauthorized`. The store checks the token inside each operation's own
//! transaction, so an `inbox` long-poll re-checks it on every read. A `send`
//! that stores a new message also takes one token from its sender's bucket,
//! or answers 429 `rate_limited`. The token is stripped from the arguments
//! before the store sees them and never reaches an audit line.
//! Authority stays `peer` on every message (SWB-R14).

use axum::{
    Json, Router,
    extract::{Query, Request, State, rejection::JsonRejection},
    http::{HeaderMap, HeaderValue, StatusCode, Uri, header, uri::Authority as UriAuthority},
    middleware::{self, Next},
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
use swb_proto::{Authority, VERSION};
use swb_store::{
    ClaimsQuery, ErrorKind, HARNESS_KINDS, PeersQuery, SEND_BURST_DEFAULT,
    SEND_REFILL_PER_MINUTE_DEFAULT, SendLimit, Store, StoreError,
};

pub const HOOK_TIMEOUT_MS: u64 = 2_000;
/// Production retention interval. The store no longer prunes inside `send`
/// or the 250 ms inbox long-poll (R-C944).
pub const PRUNE_INTERVAL: Duration = Duration::from_secs(3600);
pub fn stamped_authority() -> Authority {
    Authority::Peer
}

/// An optional field that must have one JSON type when present.
fn opt<'a, T>(
    args: &'a Value,
    key: &str,
    read: impl Fn(&'a Value) -> Option<T>,
) -> Result<Option<T>, StoreError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => read(value)
            .map(Some)
            .ok_or_else(|| format!("invalid {key}").into()),
    }
}

fn peers_op(store: &Store, args: &Value) -> Result<Value, StoreError> {
    let limit = opt(args, "limit", Value::as_u64)?;
    store.peers(&PeersQuery {
        me: opt(args, "me", Value::as_str)?,
        all: opt(args, "all", Value::as_bool)?.unwrap_or(false),
        host: opt(args, "host", Value::as_str)?,
        limit: limit.map(|n| n.min(u64::from(u32::MAX)) as u32),
        cursor: opt(args, "cursor", Value::as_str)?,
        proc_start: opt(args, "proc_start", Value::as_str)?,
    })
}

fn claims_op(store: &Store, args: &Value) -> Result<Value, StoreError> {
    let limit = opt(args, "limit", Value::as_u64)?;
    store.claims(&ClaimsQuery {
        me: opt(args, "me", Value::as_str)?,
        subject: opt(args, "subject", Value::as_str)?,
        holder: opt(args, "holder", Value::as_str)?,
        all: opt(args, "all", Value::as_bool)?.unwrap_or(false),
        limit: limit.map(|n| n.min(u64::from(u32::MAX)) as u32),
    })
}

fn required<'a>(args: &'a Value, key: &str) -> Result<&'a str, StoreError> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing {key}").into())
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// [`operation_via`] on the REST route, for tests.
#[cfg(test)]
fn operation(store: &Store, name: &str, args: &Value) -> Result<Value, StoreError> {
    operation_via(store, name, args, "rest")
}

/// One operation for a named route: `rest` from the REST handlers, `mcp`
/// from the MCP server. A send refused for its rate limit writes one
/// `send_refused` line (TIN-7577), so a 429 leaves a trace although no
/// audit line follows a failed operation.
fn operation_via(
    store: &Store,
    name: &str,
    args: &Value,
    route: &'static str,
) -> Result<Value, StoreError> {
    let result = operation_inner(store, name, args);
    if let Err(error) = &result
        && let Some(line) = refusal_record(name, args, error, route)
    {
        println!("{line}");
    }
    result
}

/// The `send_refused` line for a send the rate limit refused, and `None`
/// for any other outcome (TIN-7577). It names the sender's harness kind,
/// `retry_after` and the route, and never an agent id or a body.
fn refusal_record(
    name: &str,
    args: &Value,
    error: &StoreError,
    route: &'static str,
) -> Option<Value> {
    if name != "send" || error.kind != ErrorKind::RateLimited {
        return None;
    }
    let harness = args
        .get("from")
        .and_then(Value::as_str)
        .and_then(|from| from.split(':').next())
        .filter(|kind| HARNESS_KINDS.contains(kind));
    Some(json!({
        "ts": now_secs(),
        "op": "send_refused",
        "code": error.code(),
        "harness": harness,
        "retry_after": error.retry_after,
        "route": route,
    }))
}

/// Take the caller's session token out of the arguments and run the
/// operation with it. The store checks it, for `send` (by `from`), `inbox`,
/// `ack`, `claim`, `release`, `handoff` and `end` (by `me`), inside the
/// operation's own transaction (ADR-0003, SWB-R66), and `register` checks
/// its own. `peers`, `claims`, `heartbeat` and `thread` take none.
fn operation_inner(store: &Store, name: &str, args: &Value) -> Result<Value, StoreError> {
    let token = match args.get("session_token") {
        None | Some(Value::Null) => None,
        Some(Value::String(token)) => Some(token.as_str()),
        Some(_) => return Err("invalid session_token".into()),
    };
    if name == "register" || args.get("session_token").is_none() {
        return perform(store, name, args, token);
    }
    let mut stripped = args.clone();
    if let Some(map) = stripped.as_object_mut() {
        map.remove("session_token");
    }
    perform(store, name, &stripped, token)
}

/// One operation and its audit lines. `token` is the caller's session
/// token, already removed from `args` (except for `register`).
fn perform(
    store: &Store,
    name: &str,
    args: &Value,
    token: Option<&str>,
) -> Result<Value, StoreError> {
    let mut created = true;
    let result = match name {
        "register" => store.register(args),
        "heartbeat" => store.heartbeat(required(args, "me")?),
        "thread" => {
            let limit = opt(args, "limit", Value::as_u64)?;
            store.thread(
                required(args, "me")?,
                required(args, "thread_id")?,
                opt(args, "after_seq", Value::as_u64)?,
                limit.map(|n| n.min(u64::from(u32::MAX)) as u32),
            )
        }
        "claim" => {
            let outcome = store.claim_as(args, token)?;
            created = outcome["recorded"] == json!(true);
            Ok(outcome)
        }
        "release" => {
            let outcome =
                store.release_as(required(args, "me")?, required(args, "claim_id")?, token)?;
            created = outcome["released"] == json!(true);
            Ok(outcome)
        }
        "handoff" => {
            let outcome = store.handoff_as(args, token)?;
            created = outcome["created"] == json!(true);
            Ok(outcome)
        }
        "claims" => claims_op(store, args),
        "end" => {
            if args.get("end_token").is_none() && args.get("proc_start").is_some() {
                return Err(
                    "end needs end_token; proc_start is no longer accepted (v0.3.0)".into(),
                );
            }
            store.end_as(
                args.get("me").and_then(Value::as_str).ok_or("missing me")?,
                args.get("end_token")
                    .and_then(Value::as_str)
                    .ok_or("missing end_token")?,
                token,
            )
        }
        "peers" => peers_op(store, args),
        "send" => {
            let outcome = store.send_as(args, token)?;
            created = outcome.created;
            // A5 (TIN-7577): the result says whether anyone reads the
            // recipient's mail. The stored envelope stays unchanged.
            let mut envelope = outcome.envelope;
            envelope["recipient_state"] = json!(outcome.recipient_state);
            Ok(envelope)
        }
        "inbox" => {
            let me = args.get("me").and_then(Value::as_str).ok_or("missing me")?;
            let limit = opt(args, "limit", Value::as_u64)?
                .unwrap_or(20)
                .clamp(1, 100) as u32;
            let wait = opt(args, "wait_seconds", Value::as_u64)?
                .unwrap_or(0)
                .min(25);
            // A1 (TIN-7577): a page may start after a message the caller
            // already holds, and a long-poll then waits for mail past it.
            let after = opt(args, "after", Value::as_str)?;
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(wait);
            loop {
                // Each read checks the binding again, so a key bound during
                // the wait is never read without its token (ADR-0003).
                let batch = store.inbox_page(me, limit, after, token)?;
                if batch["messages"].as_array().is_some_and(|v| !v.is_empty())
                    || std::time::Instant::now() >= deadline
                {
                    break Ok(batch);
                }
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
        }
        "ack" => store.ack_as(
            args.get("me").and_then(Value::as_str).ok_or("missing me")?,
            args.get("msg_id")
                .and_then(Value::as_str)
                .ok_or("missing msg_id")?,
            token,
        ),
        _ => Err("unknown operation".into()),
    }?;
    for line in audit_lines(store, name, args, &result, created)? {
        println!("{line}");
    }
    Ok(result)
}

/// The audit lines one operation writes to stdout, in order. Every line is
/// built by `audit_record`, so no body, note or end token reaches it.
fn audit_lines(
    store: &Store,
    name: &str,
    args: &Value,
    result: &Value,
    created: bool,
) -> Result<Vec<Value>, StoreError> {
    let mut lines = Vec::new();
    // R-C950 (4): every claim an end released is logged as its own release.
    if name == "end" {
        for released in result["released_claims"].as_array().into_iter().flatten() {
            let mut line = audit_record("release", args, released, None);
            line["reason"] = json!("session_ended");
            lines.push(line);
        }
    }
    // The one refusal (R-C950 (1)) is logged too, so a blocked exclusive
    // request leaves a trace; nothing about it was recorded in the store.
    if name == "claim" && result["outcome"] == json!("refused") {
        lines.push(audit_record(name, args, result, None));
    }
    if created && matches!(name, "claim" | "release" | "handoff") {
        // An accepted handoff ends its source claim: that transition gets
        // its own line, before the receiver's claim.
        if name == "claim"
            && let Some(source) = result.get("handoff_from").filter(|v| v.is_string())
        {
            let mut line = audit_record("handed_off", args, result, None);
            line["claim_id"] = source.clone();
            line["state"] = json!("handed_off");
            line["outcome"] = Value::Null;
            lines.push(line);
        }
        let mut line = audit_record(name, args, result, None);
        if name == "release" {
            line["reason"] = json!("released");
        }
        lines.push(line);
    }
    if created && matches!(name, "register" | "send" | "ack") {
        let body = result.get("body").and_then(Value::as_str).unwrap_or("");
        let body_hmac = if name == "send" {
            Some(store.body_hmac(body)?)
        } else {
            None
        };
        let mut line = audit_record(name, args, result, body_hmac.as_deref());
        // ADR-0002 L1-lite (TIN-6749): an ack result carries only msg_id and
        // state, so the line takes thread_id, seq, in_reply_to and ticket
        // from the stored message. The ack is already committed; a failed
        // lookup leaves those fields null rather than failing it.
        if name == "ack"
            && let Some(meta) = result
                .get("msg_id")
                .and_then(Value::as_str)
                .and_then(|id| store.message_meta(id).ok().flatten())
        {
            for key in ["thread_id", "seq", "in_reply_to", "ticket"] {
                line[key] = meta[key].clone();
            }
        }
        lines.push(line);
    }
    // ADR-0002 L1-lite (TIN-6749): one `fetch` line (the ADR-0002 op name)
    // per message an inbox read returns, with its delivery count, so Loki can
    // follow a thread from send through delivery to ack. An empty read
    // writes nothing.
    if name == "inbox" {
        for message in result["messages"].as_array().into_iter().flatten() {
            let mut line = audit_record("fetch", args, message, None);
            line["delivery_count"] = message["delivery_count"].clone();
            lines.push(line);
        }
    }
    Ok(lines)
}

/// The audit line. It names fields explicitly, so neither the message body,
/// a claim or handoff note, nor a register's `end_token` can reach it.
/// `msg_id`, `thread_id`, `seq` and `ticket` are the keys a Loki reader
/// follows a thread by (ADR-0002 L1-lite). A send line also carries
/// `recipient_state`, so a send to a dead id shows (A5, TIN-7577).
fn audit_record(name: &str, args: &Value, result: &Value, body_hmac: Option<&str>) -> Value {
    let body_size = result
        .get("body")
        .and_then(Value::as_str)
        .map_or(0, str::len);
    let mut record = json!({
        "ts":now_secs(),
        "op":name,"me":args.get("me").or_else(||args.get("from")),"to":result.get("to"),
        "ticket":result.get("ticket"),"ruling":result.get("ruling"),
        "msg_id":result.get("msg_id"),"thread_id":result.get("thread_id"),
        "seq":result.get("seq"),"in_reply_to":result.get("in_reply_to"),
        "size":body_size,"body_hmac":body_hmac,
        "agent_id":result.get("agent_id"),"state":result.get("state"),
        "claim_id":result.get("claim_id"),"subject":result.get("subject"),
        "handoff_id":result.get("handoff_id"),"outcome":result.get("outcome"),
        "exclusive":result.get("exclusive")
    });
    // Only a send result carries it, so other lines keep their keys.
    if let Some(state) = result.get("recipient_state") {
        record["recipient_state"] = state.clone();
    }
    record
}

/// The typed error body every REST and MCP failure carries (R-C944).
pub fn error_body(code: &str, message: &str) -> Value {
    json!({"error": message, "code": code})
}

fn status_of(kind: ErrorKind) -> StatusCode {
    match kind {
        ErrorKind::Invalid => StatusCode::BAD_REQUEST,
        ErrorKind::Unauthorized => StatusCode::UNAUTHORIZED,
        ErrorKind::RateLimited => StatusCode::TOO_MANY_REQUESTS,
        ErrorKind::Unregistered | ErrorKind::Unavailable | ErrorKind::ClaimUnavailable => {
            StatusCode::NOT_FOUND
        }
        ErrorKind::Conflict => StatusCode::CONFLICT,
        ErrorKind::Internal => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

/// The agent id field description, naming every kind in [`HARNESS_KINDS`].
fn agent_id_desc() -> String {
    format!(
        "Agent id <harness>:<host>:<pid>:<session_id>; harness is one of {}; the other parts use [A-Za-z0-9._-]; at most 512 bytes",
        HARNESS_KINDS.join(", ")
    )
}
const ULID_DESC: &str = "Canonical 26-character Crockford base32 ULID";
const SESSION_TOKEN_DESC: &str = "The session_token the latest register of a bound session returned (ADR-0003). Required while the acting id is bound; ignored for an unbound id. Never repeat it";
const SUBJECT_DESC: &str = "What is claimed: a ticket (TIN-4655), a path (lab:nix/hosts/sting.nix) or another token in [A-Za-z0-9._:/#@+-], at most 256 bytes";

fn tools() -> Vec<Tool> {
    let agent_id = agent_id_desc();
    [
        ("register", "Register (or re-register) a self-asserted agent session. Returns agent_id, lease_seconds, end_token and bound. Every register mints a new end_token and invalidates earlier ones. With bind true it also binds the session's harness, host and session id and returns a session_token, which send, inbox, ack, claim, release, handoff and end then need; while that binding is live, registering the same session again (any pid) needs the current session_token and rotates it. Over MCP both tokens are part of this result and so enter the caller's context; an adapter should keep them and strip them before the model sees them. Never repeat them.", json!({"type":"object","required":["harness","host","pid","session_id","proc_start"],"properties":{
            "harness":{"type":"string","enum":HARNESS_KINDS,"description":"The harness kind"},
            "host":{"type":"string","pattern":"^[A-Za-z0-9._-]+$","description":"Short host name"},
            "pid":{"type":"integer","minimum":1,"description":"The harness launcher's process id"},
            "session_id":{"type":"string","pattern":"^[A-Za-z0-9._-]+$","description":"The harness's own session id"},
            "proc_start":{"type":"string","minLength":1,"maxLength":256,"description":"Opaque process start stamp. peers never returns it; a caller may filter peers on its own."},
            "bind":{"type":"boolean","description":"Bind this harness, host and session id to the caller (ADR-0003, default false). A binding lasts until end or 6 h without a call"},
            "session_token":{"type":"string","pattern":"^[0-9a-f]{64}$","description":"The current session_token, required to register a session whose binding is live"}}})),
        ("end", "End this session. Needs the end_token its latest register returned; any other token answers state unknown and ends nothing.", json!({"type":"object","required":["me","end_token"],"properties":{
            "me":{"type":"string","description":agent_id},
            "end_token":{"type":"string","pattern":"^[0-9a-f]{64}$","description":"The end_token register returned"},
            "session_token":{"type":"string","description":SESSION_TOKEN_DESC}}})),
        ("peers", "List sessions: live only by default, one row per harness, host and session id (latest registration wins), paginated. Each row says whether the session is bound. A registered me also renews that caller's own lease; omitting me is passive discovery.", json!({"type":"object","properties":{
            "me":{"type":"string","description":format!("{agent_id}. Optional; renews only this caller's lease")},
            "all":{"type":"boolean","description":"Include idle, gone and ended sessions (default false: live only)"},
            "host":{"type":"string","pattern":"^[A-Za-z0-9._-]+$","description":"Only sessions on this host"},
            "limit":{"type":"integer","minimum":1,"maximum":200,"description":"Page size, default 50, at most 200"},
            "cursor":{"type":"string","description":"next_cursor from the previous page"},
            "proc_start":{"type":"string","minLength":1,"maxLength":256,"description":"Only registrations that used this proc_start, then one row per harness, host and session id among them (the caller's own stamp; rows never show it)"}}})),
        ("send", "Send a peer-authority message. The broker stamps authority peer whatever the sender claims. A bound sender needs its session_token. Each sender has a token bucket; an empty one answers rate_limited. The result adds recipient_state (live, idle, gone, ended or unregistered); a send to a recipient that is not live still succeeds and waits in its mailbox until it expires.", json!({"type":"object","required":["from","to","ticket","body"],"properties":{
            "from":{"type":"string","description":format!("Sender. {agent_id}")},
            "to":{"type":"string","description":format!("Recipient. {agent_id}")},
            "ticket":{"type":"string","pattern":"^(TIN-[0-9]+|none)$","maxLength":64,"description":"TIN-<digits> or none"},
            "body":{"type":"string","maxLength":16384,"description":"Message text, at most 16 KiB. Newline and tab are kept; CR and other control characters are removed"},
            "msg_id":{"type":"string","description":format!("{ULID_DESC}. Optional idempotency key; a retry with the same sender and body returns the stored envelope")},
            "thread_id":{"type":"string","description":format!("{ULID_DESC}. Defaults to msg_id")},
            "in_reply_to":{"type":"string","description":format!("{ULID_DESC} of the message answered")},
            "operator_directed":{"type":"boolean","description":"The sender's claim that the operator directed this; needs ruling"},
            "ruling":{"type":"string","minLength":1,"maxLength":512,"description":"Ruling the sender cites"},
            "reply_expires":{"type":"string","format":"date-time","description":"RFC 3339 time after which a reply is no longer useful"},
            "reply_format":{"type":"string","maxLength":512,"description":"Requested reply shape"},
            "artifacts":{"type":"array","maxItems":20,"uniqueItems":true,"items":{"type":"string","minLength":1,"maxLength":2048},"description":"Up to 20 artifact references"},
            "ttl_hours":{"type":"integer","minimum":1,"maximum":336,"description":"Message lifetime, default 72 h, clamped to 1..336"},
            "session_token":{"type":"string","description":SESSION_TOKEN_DESC}}})),
        ("inbox", "Fetch this caller's unacknowledged messages, oldest first in arrival order. Each fetch counts a delivery and renews the caller's lease. The result carries next_after, the last msg_id of a full page (null otherwise); pass it as after to read past messages already held. A bound session reads the mail of every pid of its harness, host and session id, so a restart keeps unread mail.", json!({"type":"object","required":["me"],"properties":{
            "me":{"type":"string","description":agent_id},
            "limit":{"type":"integer","minimum":1,"maximum":100,"description":"Page size, default 20, at most 100"},
            "after":{"type":"string","description":format!("{ULID_DESC}. Only messages that arrived after this one, which must be a message of this mailbox; pass next_after from the previous page")},
            "wait_seconds":{"type":"integer","minimum":0,"maximum":25,"description":"Long-poll up to this many seconds when the mailbox is empty (default 0)"},
            "session_token":{"type":"string","description":SESSION_TOKEN_DESC}}})),
        ("ack", "Acknowledge a received message so it is not delivered again.", json!({"type":"object","required":["me","msg_id"],"properties":{
            "me":{"type":"string","description":agent_id},
            "msg_id":{"type":"string","description":ULID_DESC},
            "session_token":{"type":"string","description":SESSION_TOKEN_DESC}}})),
        ("heartbeat", "Renew this session's lease without reading or sending anything.", json!({"type":"object","required":["me"],"properties":{
            "me":{"type":"string","description":agent_id}}})),
        ("thread", "Read one thread's messages that this session sent or received, in seq order. Changes no delivery state; renews the caller's lease.", json!({"type":"object","required":["me","thread_id"],"properties":{
            "me":{"type":"string","description":agent_id},
            "thread_id":{"type":"string","description":ULID_DESC},
            "after_seq":{"type":"integer","minimum":0,"description":"Only messages after this seq; pass next_after_seq from the previous page"},
            "limit":{"type":"integer","minimum":1,"maximum":100,"description":"Page size, default 20, at most 100"}}})),
        ("claim", "Claim a subject. Advisory: a non-exclusive claim is always recorded and lists every other holder in overlaps. An exclusive claim is refused (outcome refused, held_by lists the holders, nothing recorded) while another session holds any active claim on the subject; repeating your own exclusive claim renews it (same claim_id, later expiry). With handoff_id, accept a handoff offered to you. Ending your session releases your claims.", json!({"type":"object","required":["me"],"properties":{
            "me":{"type":"string","description":agent_id},
            "subject":{"type":"string","pattern":"^[A-Za-z0-9._:/#@+-]{1,256}$","description":format!("{SUBJECT_DESC}. Required unless handoff_id is given")},
            "exclusive":{"type":"boolean","description":"Ask for exclusive use (default false)"},
            "lease_hours":{"type":"integer","minimum":1,"maximum":24,"description":"Claim lease, default 4 h, clamped to 1..24"},
            "note":{"type":"string","maxLength":512,"description":"Short note shown in claims; no control characters. Never put a secret here"},
            "handoff_id":{"type":"string","description":format!("{ULID_DESC} of a handoff offered to you; accepting it moves the claim to you")},
            "session_token":{"type":"string","description":SESSION_TOKEN_DESC}}})),
        ("release", "Release one of your own claims. Releasing it again changes nothing. Another session's claim answers claim_unavailable.", json!({"type":"object","required":["me","claim_id"],"properties":{
            "me":{"type":"string","description":agent_id},
            "claim_id":{"type":"string","description":ULID_DESC},
            "session_token":{"type":"string","description":SESSION_TOKEN_DESC}}})),
        ("handoff", "Offer one of your active claims to another session. Creates a receipt in offered; the claim moves only when the receiver calls claim with the handoff_id.", json!({"type":"object","required":["me","claim_id","to"],"properties":{
            "me":{"type":"string","description":agent_id},
            "claim_id":{"type":"string","description":format!("{ULID_DESC} of your active claim")},
            "to":{"type":"string","description":format!("Receiver. {agent_id}")},
            "note":{"type":"string","maxLength":512,"description":"Short note for the receiver; no control characters. Never put a secret here"},
            "session_token":{"type":"string","description":SESSION_TOKEN_DESC}}})),
        ("claims", "List claims: active ones by default, each marked orphaned while its holder is gone. With me, also lists the handoffs offered to or by you and renews your lease.", json!({"type":"object","properties":{
            "me":{"type":"string","description":format!("{agent_id}. Optional; renews only this caller's lease and adds its open handoffs")},
            "subject":{"type":"string","pattern":"^[A-Za-z0-9._:/#@+-]{1,256}$","description":format!("Only claims on this subject. {SUBJECT_DESC}")},
            "holder":{"type":"string","description":format!("Only claims this session holds. {agent_id}")},
            "all":{"type":"boolean","description":"Include released, handed-off and expired claims still retained (default false)"},
            "limit":{"type":"integer","minimum":1,"maximum":500,"description":"Page size, default 100, at most 500"}}})),
    ].into_iter().map(|(name,description,schema)| Tool::new(name,description,Arc::new(schema.as_object().expect("static object schema").clone()))).collect()
}

const TOOL_NAMES: [&str; 12] = [
    "register",
    "end",
    "peers",
    "send",
    "inbox",
    "ack",
    "heartbeat",
    "thread",
    "claim",
    "release",
    "handoff",
    "claims",
];

#[derive(Clone)]
struct BrokerMcp {
    store: Arc<Store>,
}
impl ServerHandler for BrokerMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("agent-switchboard", VERSION))
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
        if !TOOL_NAMES.contains(&name.as_str()) {
            return Err(McpError::invalid_params("unknown tool", None));
        }
        let args = Value::Object(request.arguments.unwrap_or_default());
        let store = Arc::clone(&self.store);
        match tokio::task::spawn_blocking(move || operation_via(&store, &name, &args, "mcp")).await
        {
            Ok(Ok(value)) => Ok(CallToolResult::structured(value).into()),
            Ok(Err(error)) => Ok(CallToolResult::error(vec![ContentBlock::text(
                store_error_body(&error).to_string(),
            )])
            .into()),
            Err(error) => Err(McpError::internal_error(error.to_string(), None)),
        }
    }
}

fn error_response(status: StatusCode, code: &str, message: &str) -> Response {
    (status, Json(error_body(code, message))).into_response()
}

/// The typed error body for a failed operation. A rate-limited send adds
/// `retry_after` in seconds when its bucket refills (TIN-7577); a client
/// that reads only `error` and `code` sees the v0.5.0 body.
fn store_error_body(error: &StoreError) -> Value {
    let mut body = error_body(error.code(), &error.message);
    if let Some(seconds) = error.retry_after {
        body["retry_after"] = json!(seconds);
    }
    body
}

/// The REST answer for a failed operation. A rate-limited send also
/// carries a `Retry-After` header when its bucket refills (TIN-7577).
fn store_error_response(error: &StoreError) -> Response {
    let mut response = (status_of(error.kind), Json(store_error_body(error))).into_response();
    if let Some(seconds) = error.retry_after {
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from(seconds));
    }
    response
}

async fn run_operation(store: Arc<Store>, name: &'static str, args: Value) -> Response {
    match tokio::task::spawn_blocking(move || operation_via(&store, name, &args, "rest")).await {
        Ok(Ok(value)) => (StatusCode::OK, Json(value)).into_response(),
        Ok(Err(error)) => store_error_response(&error),
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "broker unavailable",
        ),
    }
}

/// A JSON request body, or the typed error for a missing, mistyped or
/// malformed one. A JSON body needs `Content-Type: application/json`, which
/// a browser can only send after a CORS preflight.
fn json_body(body: Result<Json<Value>, JsonRejection>) -> Result<Value, Box<Response>> {
    match body {
        Ok(Json(value @ Value::Object(_))) => Ok(value),
        Ok(Json(_)) => Err(Box::new(error_response(
            StatusCode::BAD_REQUEST,
            "invalid_json",
            "request body must be a JSON object",
        ))),
        Err(JsonRejection::MissingJsonContentType(_)) => Err(Box::new(error_response(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_media_type",
            "Content-Type must be application/json",
        ))),
        Err(_) => Err(Box::new(error_response(
            StatusCode::BAD_REQUEST,
            "invalid_json",
            "request body is not valid JSON",
        ))),
    }
}

macro_rules! post_operation {
    ($handler:ident, $name:literal) => {
        async fn $handler(
            State(store): State<Arc<Store>>,
            body: Result<Json<Value>, JsonRejection>,
        ) -> Response {
            match json_body(body) {
                Ok(args) => run_operation(store, $name, args).await,
                Err(response) => *response,
            }
        }
    };
}
post_operation!(register, "register");
post_operation!(end, "end");
post_operation!(send, "send");
post_operation!(ack, "ack");
post_operation!(peers_post, "peers");
post_operation!(inbox_post, "inbox");
post_operation!(heartbeat, "heartbeat");
post_operation!(thread, "thread");
post_operation!(claim, "claim");
post_operation!(release, "release");
post_operation!(handoff, "handoff");
post_operation!(claims, "claims");

fn method_not_allowed(allow: &'static str, message: &str) -> Response {
    let mut response = error_response(
        StatusCode::METHOD_NOT_ALLOWED,
        "method_not_allowed",
        message,
    );
    response
        .headers_mut()
        .insert(header::ALLOW, HeaderValue::from_static(allow));
    response
}

/// `GET /v1/peers` is passive discovery only (`all`, `host`, `limit`,
/// `cursor`). A lease refresh (`me`) moved to `POST /v1/peers` in v0.3.0,
/// and is refused here rather than silently ignored.
async fn peers_get(
    State(store): State<Arc<Store>>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    if query.contains_key("me") {
        return method_not_allowed(
            "POST",
            "lease refresh moved to POST /v1/peers with a JSON body {\"me\": ...} in v0.3.0",
        );
    }
    let mut args = serde_json::Map::new();
    for (key, value) in &query {
        let parsed = match key.as_str() {
            "all" => match value.as_str() {
                "1" | "true" => json!(true),
                "0" | "false" => json!(false),
                _ => {
                    return error_response(
                        StatusCode::BAD_REQUEST,
                        "invalid_argument",
                        "invalid all",
                    );
                }
            },
            "limit" => match value.parse::<u64>() {
                Ok(n) => json!(n),
                Err(_) => {
                    return error_response(
                        StatusCode::BAD_REQUEST,
                        "invalid_argument",
                        "invalid limit",
                    );
                }
            },
            "host" | "cursor" => json!(value),
            _ => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "invalid_argument",
                    &format!(
                        "unknown query parameter {}",
                        key.chars()
                            .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
                            .take(32)
                            .collect::<String>()
                    ),
                );
            }
        };
        args.insert(key.clone(), parsed);
    }
    run_operation(store, "peers", Value::Object(args)).await
}

async fn inbox_get() -> Response {
    method_not_allowed(
        "POST",
        "GET /v1/inbox was removed in v0.3.0; POST /v1/inbox with a JSON body {\"me\": ..., \"limit\": ..., \"wait_seconds\": ...}",
    )
}

async fn not_found() -> Response {
    error_response(StatusCode::NOT_FOUND, "not_found", "no such route")
}

async fn wrong_method() -> Response {
    error_response(
        StatusCode::METHOD_NOT_ALLOWED,
        "method_not_allowed",
        "method not allowed on this route",
    )
}

/// `GET /healthz`: 200 with the version when the store answers a read, 503
/// otherwise. No identity, no body, no write.
async fn healthz(State(store): State<Arc<Store>>) -> Response {
    match tokio::task::spawn_blocking(move || store.health()).await {
        Ok(Ok(())) => (
            StatusCode::OK,
            Json(json!({"status":"ok","version":VERSION})),
        )
            .into_response(),
        _ => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"status":"unavailable","version":VERSION})),
        )
            .into_response(),
    }
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

/// The metrics listener's router. It carries no Host/Origin guard: it serves
/// three aggregate counters and `/healthz`, with no identity, body or write
/// path, it is a separate listener (`SWB_METRICS_LISTEN`) that a deployment
/// keeps off the tailnet Service, and Prometheus and kubelet probes reach it
/// by pod address, a Host no broker allowlist names.
fn metrics_router(store: Arc<Store>) -> Router {
    Router::new()
        .route("/metrics", get(metrics))
        .route("/healthz", get(healthz))
        .with_state(store)
}

/// One normalized `Host` authority: lowercase host without IPv6 brackets,
/// plus an optional port.
#[derive(Clone, Debug, PartialEq, Eq)]
struct HostAuthority {
    host: String,
    port: Option<u16>,
}

fn normalize_host(host: &str) -> String {
    host.trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase()
}

/// Parse a request `Host` value. Userinfo and anything that is not an
/// RFC 3986 authority are refused. A bracketed host must be an IPv6 literal
/// (`[localhost]` is refused, not unwrapped), and a `:` after the host must
/// carry a valid port (`host:` and `host:99999` are refused, not read as
/// "no port").
fn parse_request_authority(raw: &str) -> Option<HostAuthority> {
    let authority = UriAuthority::try_from(raw.trim()).ok()?;
    let host = authority.host();
    if authority.as_str().contains('@') || host.is_empty() {
        return None;
    }
    if let Some(inner) = host.strip_prefix('[') {
        inner
            .strip_suffix(']')?
            .parse::<std::net::Ipv6Addr>()
            .ok()?;
    }
    let port = if authority.as_str().len() > host.len() {
        Some(authority.port_u16()?)
    } else {
        None
    };
    Some(HostAuthority {
        host: normalize_host(host),
        port,
    })
}

/// Parse an allowlist entry. Same rules as rmcp's `/mcp` check: an entry
/// that is not an authority (a bare IPv6 literal such as `::1`) is kept as a
/// host with no port.
fn parse_allowed_authority(raw: &str) -> Option<HostAuthority> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    parse_request_authority(raw).or_else(|| {
        Some(HostAuthority {
            host: normalize_host(raw),
            port: None,
        })
    })
}

/// A browser `Origin`: `null`, or a `(scheme, host, port)` tuple.
#[derive(Clone, Debug, PartialEq, Eq)]
enum BrowserOrigin {
    Null,
    Tuple {
        scheme: String,
        host: String,
        port: Option<u16>,
    },
}

fn parse_origin(raw: &str) -> Option<BrowserOrigin> {
    let raw = raw.trim();
    if raw.eq_ignore_ascii_case("null") {
        return Some(BrowserOrigin::Null);
    }
    let uri = Uri::try_from(raw).ok()?;
    let authority = uri.authority()?;
    if authority.as_str().contains('@') {
        return None;
    }
    Some(BrowserOrigin::Tuple {
        scheme: uri.scheme_str()?.to_ascii_lowercase(),
        host: normalize_host(authority.host()),
        port: authority.port_u16(),
    })
}

/// DNS-rebinding guard for every route on the main listener: `/mcp`,
/// every `/v1/*` route, the test-clock route and the 404 fallback.
///
/// - `Host` must be present, single, well formed and on the allowlist
///   (`SWB_MCP_ALLOWED_HOSTS`). An entry without a port admits any port; an
///   entry with a port admits only that port. Matching is case-insensitive.
///   With no `Host` header the URI authority (HTTP/2 `:authority`) is used;
///   an absolute-form request target's authority must be allowed as well.
/// - `Origin`, when present, must be on `SWB_ALLOWED_ORIGINS`, which is
///   empty by default, so every request a browser marks with `Origin`
///   (fetch, XHR, any form POST) is refused. Browsers omit `Origin` on
///   simple GETs such as image loads and navigations, so this does not stop
///   a blind cross-site GET with the real Host; see ADR-0001. The broker's
///   clients (`swb`, hooks, channel, MCP harnesses) send no Origin.
///
/// Every refusal is HTTP 403 with a JSON `error`, before any handler runs.
#[derive(Clone, Debug)]
pub struct RequestGuard {
    hosts: Vec<HostAuthority>,
    origins: Vec<BrowserOrigin>,
}

impl RequestGuard {
    /// Build a guard. An empty host allowlist is refused: rmcp reads an empty
    /// list as "allow every Host", which is exactly the rebinding hole.
    pub fn new(allowed_hosts: &[String], allowed_origins: &[String]) -> Result<Self, String> {
        let hosts: Vec<_> = allowed_hosts
            .iter()
            .filter_map(|h| parse_allowed_authority(h))
            .collect();
        if hosts.is_empty() {
            return Err("SWB_MCP_ALLOWED_HOSTS names no host; refusing to serve every Host".into());
        }
        let origins = allowed_origins
            .iter()
            .filter(|o| !o.trim().is_empty())
            .map(|o| {
                parse_origin(o).ok_or_else(|| format!("invalid SWB_ALLOWED_ORIGINS entry {o:?}"))
            })
            .collect::<Result<_, _>>()?;
        Ok(Self { hosts, origins })
    }

    fn allows_host(&self, host: &HostAuthority) -> bool {
        self.hosts.iter().any(|allowed| {
            allowed.host == host.host && allowed.port.is_none_or(|p| host.port == Some(p))
        })
    }

    fn check(&self, uri: &Uri, headers: &HeaderMap) -> Result<(), &'static str> {
        let mut values = headers.get_all(header::HOST).iter();
        let header_host = match (values.next(), values.next()) {
            (Some(_), Some(_)) => return Err("host not allowed"),
            (Some(value), None) => Some(
                value
                    .to_str()
                    .ok()
                    .and_then(parse_request_authority)
                    .ok_or("host not allowed")?,
            ),
            (None, _) => None,
        };
        // The URI authority is HTTP/2's `:authority`, or the authority of an
        // HTTP/1 absolute-form target, which RFC 9112 3.2.2 says takes
        // precedence over Host. Whichever is present must be allowed.
        let uri_host = match uri.authority() {
            Some(a) => Some(parse_request_authority(a.as_str()).ok_or("host not allowed")?),
            None => None,
        };
        if header_host.is_none() && uri_host.is_none() {
            return Err("missing host");
        }
        if !header_host
            .iter()
            .chain(uri_host.iter())
            .all(|host| self.allows_host(host))
        {
            return Err("host not allowed");
        }
        let mut origins = headers.get_all(header::ORIGIN).iter();
        match (origins.next(), origins.next()) {
            (None, _) => Ok(()),
            (Some(_), Some(_)) => Err("origin not allowed"),
            (Some(value), None) => {
                let origin = value
                    .to_str()
                    .ok()
                    .and_then(parse_origin)
                    .ok_or("origin not allowed")?;
                if self.origins.iter().any(|allowed| match (allowed, &origin) {
                    (BrowserOrigin::Null, BrowserOrigin::Null) => true,
                    (
                        BrowserOrigin::Tuple { scheme, host, port },
                        BrowserOrigin::Tuple {
                            scheme: s,
                            host: h,
                            port: p,
                        },
                    ) => scheme == s && host == h && (port.is_none() || port == p),
                    _ => false,
                }) {
                    Ok(())
                } else {
                    Err("origin not allowed")
                }
            }
        }
    }
}

async fn guard_request(
    State(guard): State<Arc<RequestGuard>>,
    request: Request,
    next: Next,
) -> Response {
    match guard.check(request.uri(), request.headers()) {
        Ok(()) => next.run(request).await,
        Err(reason) => (StatusCode::FORBIDDEN, Json(json!({"error":reason}))).into_response(),
    }
}

/// The per-sender send limit (ADR-0003) from `SWB_SEND_BURST` (default
/// [`SEND_BURST_DEFAULT`]) and `SWB_SEND_REFILL_PER_MINUTE` (default
/// [`SEND_REFILL_PER_MINUTE_DEFAULT`]). `SWB_SEND_BURST=0` turns it off. A
/// value that is not a whole number up to 100000 refuses to start.
pub fn send_limit_from(lookup: &dyn Fn(&str) -> Option<String>) -> Result<SendLimit, String> {
    let read = |name: &str, default: u32| -> Result<u32, String> {
        match lookup(name).map(|v| v.trim().to_owned()) {
            None => Ok(default),
            Some(v) if v.is_empty() => Ok(default),
            Some(v) => v
                .parse::<u32>()
                .ok()
                .filter(|n| *n <= 100_000)
                .ok_or_else(|| format!("{name} must be a whole number from 0 to 100000")),
        }
    };
    Ok(SendLimit {
        burst: read("SWB_SEND_BURST", SEND_BURST_DEFAULT)?,
        refill_per_minute: read("SWB_SEND_REFILL_PER_MINUTE", SEND_REFILL_PER_MINUTE_DEFAULT)?,
    })
}

fn env_list(name: &str) -> Option<Vec<String>> {
    std::env::var(name).ok().map(|v| {
        v.split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect()
    })
}

#[cfg(test)]
fn router(store: Arc<Store>, allowed_hosts: Vec<String>) -> Router {
    let guard = RequestGuard::new(&allowed_hosts, &[]).expect("test allowlist");
    router_with_config(
        store,
        StreamableHttpServerConfig::default().with_allowed_hosts(allowed_hosts),
        guard,
        Router::new(),
    )
}

/// The main listener's app: `/mcp`, the `/v1/*` REST twin and `extra`, all
/// behind one [`RequestGuard`] layer.
fn router_with_config(
    store: Arc<Store>,
    config: StreamableHttpServerConfig,
    guard: RequestGuard,
    extra: Router,
) -> Router {
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
        .route("/healthz", get(healthz))
        .route("/v1/register", post(register))
        .route("/v1/end", post(end))
        .route("/v1/peers", post(peers_post).get(peers_get))
        .route("/v1/send", post(send))
        .route("/v1/inbox", post(inbox_post).get(inbox_get))
        .route("/v1/ack", post(ack))
        .route("/v1/heartbeat", post(heartbeat))
        .route("/v1/thread", post(thread))
        .route("/v1/claim", post(claim))
        .route("/v1/release", post(release))
        .route("/v1/handoff", post(handoff))
        .route("/v1/claims", post(claims))
        .method_not_allowed_fallback(wrong_method)
        .fallback(not_found)
        .with_state(store)
        .merge(extra)
        .layer(middleware::from_fn_with_state(
            Arc::new(guard),
            guard_request,
        ))
}
pub async fn serve(store: Arc<Store>, listen: &str, metrics_listen: &str) -> io::Result<()> {
    serve_until(
        store,
        listen,
        metrics_listen,
        PRUNE_INTERVAL,
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
    let allowed_hosts = env_list("SWB_MCP_ALLOWED_HOSTS")
        .unwrap_or_else(|| vec!["localhost".into(), "127.0.0.1".into(), "::1".into()]);
    let allowed_origins = env_list("SWB_ALLOWED_ORIGINS").unwrap_or_default();
    let guard = RequestGuard::new(&allowed_hosts, &allowed_origins)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    store
        .set_send_limit(
            send_limit_from(&|name| std::env::var(name).ok())
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?,
        )
        .map_err(io::Error::other)?;
    store.prune().map_err(io::Error::other)?;
    let config = StreamableHttpServerConfig::default().with_allowed_hosts(allowed_hosts);
    let mcp_cancel = config.cancellation_token.clone();
    let app = router_with_config(Arc::clone(&store), config, guard, extra);
    let metrics_app = metrics_router(Arc::clone(&store));
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
    /// by n seconds (0 reads it), runs one retention prune, and returns
    /// `{"now": <unix seconds>}`. The prune stands in for the production
    /// interval, which runs on the wall clock (R-C944: `send` and `inbox` no
    /// longer prune).
    pub fn router(clock: Arc<ManualClock>, store: Arc<Store>) -> Router {
        Router::new()
            .route("/v1/test/clock", post(advance))
            .with_state((clock, store))
    }

    async fn advance(
        State((clock, store)): State<(Arc<ManualClock>, Arc<Store>)>,
        Json(args): Json<Value>,
    ) -> Response {
        match args
            .get("advance_seconds")
            .and_then(Value::as_u64)
            .filter(|n| *n <= MAX_ADVANCE_SECONDS)
        {
            Some(n) => {
                let now = clock.advance(n as u32);
                match tokio::task::spawn_blocking(move || store.prune()).await {
                    Ok(Ok(())) => (StatusCode::OK, Json(json!({"now":now}))).into_response(),
                    _ => error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "internal",
                        "retention prune failed",
                    ),
                }
            }
            None => error_response(
                StatusCode::BAD_REQUEST,
                "invalid_argument",
                "invalid advance_seconds",
            ),
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
        let extra = router(clock, Arc::clone(&store));
        serve_until_with(
            store,
            listen,
            metrics_listen,
            PRUNE_INTERVAL,
            Duration::from_secs(30),
            shutdown_signal(),
            extra,
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
            let app = router(clock, Arc::clone(&store)).merge(super::super::router(
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

        #[tokio::test]
        async fn clock_advance_runs_the_retention_prune() {
            let clock = Arc::new(ManualClock::new(1_700_000_000));
            let store = Arc::new(Store::memory_with_clock(clock.clone()).unwrap());
            let to = "pi:sting:2:b";
            let id = store
                .send(&json!({"from":"claude:honey:1:a","to":to,"ticket":"none","body":"x"}))
                .unwrap()
                .envelope["msg_id"]
                .as_str()
                .unwrap()
                .to_owned();
            store.ack(to, &id).unwrap();
            let app = router(clock, Arc::clone(&store));
            let response = app
                .oneshot(
                    Request::post("/v1/test/clock")
                        .header("content-type", "application/json")
                        .body(Body::from(r#"{"advance_seconds":604800}"#))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            // The acked row is gone: a second ack is no longer "already acked".
            assert_eq!(store.ack(to, &id).unwrap_err(), "message unavailable");
        }

        #[tokio::test]
        async fn clock_route_is_behind_the_host_guard() {
            let clock = Arc::new(ManualClock::new(1_700_000_000));
            let store = Arc::new(Store::memory_with_clock(clock.clone()).unwrap());
            let app = super::super::router_with_config(
                Arc::clone(&store),
                StreamableHttpServerConfig::default(),
                RequestGuard::new(&["127.0.0.1".into()], &[]).unwrap(),
                router(clock, Arc::clone(&store)),
            );
            let post = |host: &'static str| {
                Request::post("/v1/test/clock")
                    .header("host", host)
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"advance_seconds":60}"#))
                    .unwrap()
            };
            let refused = app.clone().oneshot(post("attacker.example")).await.unwrap();
            assert_eq!(refused.status(), StatusCode::FORBIDDEN);
            assert_eq!(store.now(), 1_700_000_000);
            let allowed = app.oneshot(post("127.0.0.1:18080")).await.unwrap();
            assert_eq!(allowed.status(), StatusCode::OK);
            assert_eq!(store.now(), 1_700_000_060);
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
    include!("service_roundtrip_tests.rs");
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
            let body = r#"{"me":"pi:sting:2:b","wait_seconds":1}"#;
            write!(stream, "POST /v1/inbox HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
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
    fn lists_twelve_typed_and_described_tools() {
        assert_eq!(tools().len(), 12);
        assert_eq!(
            tools()
                .iter()
                .map(|t| t.name.to_string())
                .collect::<Vec<_>>(),
            TOOL_NAMES
        );
        for tool in tools() {
            let props = tool.input_schema["properties"].as_object().unwrap();
            for (key, schema) in props {
                assert!(
                    schema["description"]
                        .as_str()
                        .is_some_and(|d| !d.is_empty()),
                    "{}.{key} has no description",
                    tool.name
                );
            }
        }
        let peers = tools().into_iter().find(|t| t.name == "peers").unwrap();
        assert_eq!(peers.input_schema["properties"]["me"]["type"], "string");
        assert_eq!(peers.input_schema["properties"]["all"]["type"], "boolean");
        let end = tools().into_iter().find(|t| t.name == "end").unwrap();
        assert_eq!(end.input_schema["required"], json!(["me", "end_token"]));
        let inbox = tools().into_iter().find(|t| t.name == "inbox").unwrap();
        assert_eq!(inbox.input_schema["properties"]["after"]["type"], "string");
        let send = tools().into_iter().find(|t| t.name == "send").unwrap();
        let props = &send.input_schema["properties"];
        for key in ["artifacts", "reply_expires", "reply_format"] {
            assert!(
                props.get(key).is_some(),
                "missing {key} in advertised send schema"
            );
        }
    }
    #[test]
    fn harness_kinds_come_from_the_one_store_constant() {
        let register = tools().into_iter().find(|t| t.name == "register").unwrap();
        assert_eq!(
            register.input_schema["properties"]["harness"]["enum"],
            json!(HARNESS_KINDS)
        );
        for kind in ["hermes", "kimicode", "kimi", "claude"] {
            assert!(HARNESS_KINDS.contains(&kind), "{kind}");
        }
        // Every agent id description names every kind.
        for tool in tools() {
            for (key, schema) in tool.input_schema["properties"].as_object().unwrap() {
                let text = schema["description"].as_str().unwrap_or("");
                if text.contains("<harness>") {
                    for kind in HARNESS_KINDS {
                        assert!(text.contains(kind), "{}.{key} misses {kind}", tool.name);
                    }
                }
            }
        }
        // The envelope schema's agent id pattern names exactly these kinds.
        let schema: Value = serde_json::from_str(swb_proto::ENVELOPE_V3_SCHEMA).unwrap();
        let pattern = schema.to_string();
        let expected = format!("^({}):", HARNESS_KINDS.join("|"));
        assert!(
            pattern.contains(&expected),
            "{expected} not in the envelope schema"
        );
    }

    #[test]
    fn register_operation_round_trips_each_harness_kind() {
        let store = Store::memory().unwrap();
        for (pid, kind) in (1u64..).zip(HARNESS_KINDS) {
            let args = json!({"harness":kind,"host":"host-a","pid":pid,
                "session_id":"s-1","proc_start":"start-1"});
            let row = operation(&store, "register", &args).unwrap();
            assert_eq!(row["agent_id"], format!("{kind}:host-a:{pid}:s-1"));
        }
        let refused = operation(
            &store,
            "register",
            &json!({"harness":"kimi-code","host":"host-a","pid":9,
                "session_id":"s-1","proc_start":"start-1"}),
        )
        .unwrap_err();
        assert_eq!(refused.message, "invalid harness");
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
        let token = first["end_token"].as_str().unwrap().to_owned();
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
        let ended = mcp_call(
            &app,
            &session,
            10,
            "end",
            json!({"me":from,"end_token":token}),
        )
        .await;
        assert_eq!(ended["state"], "ended");
        let live = mcp_call(&app, &session, 11, "peers", json!({})).await;
        assert_eq!(live["peers"].as_array().unwrap().len(), 1);
        let every = mcp_call(&app, &session, 12, "peers", json!({"all":true})).await;
        assert_eq!(every["peers"].as_array().unwrap().len(), 2);
        assert!(!every.to_string().contains("proc_start"));
    }
    /// Every state-bearing route on the main listener, with a request body
    /// that would mutate or read state if the guard let it through.
    const GUARDED_ROUTES: [(&str, &str, &str); 13] = [
        (
            "POST",
            "/v1/register",
            r#"{"harness":"claude","host":"honey","pid":1,"session_id":"a","proc_start":"1"}"#,
        ),
        (
            "POST",
            "/v1/end",
            r#"{"me":"claude:honey:1:a","end_token":"0000000000000000000000000000000000000000000000000000000000000000"}"#,
        ),
        ("POST", "/v1/peers", r#"{"me":"claude:honey:1:a"}"#),
        (
            "POST",
            "/v1/send",
            r#"{"from":"claude:honey:1:a","to":"pi:sting:2:b","ticket":"none","body":"rebound"}"#,
        ),
        ("POST", "/v1/inbox", r#"{"me":"pi:sting:2:b"}"#),
        (
            "POST",
            "/v1/ack",
            r#"{"me":"pi:sting:2:b","msg_id":"01ARZ3NDEKTSV4RRFFQ69G5FAV"}"#,
        ),
        ("POST", "/v1/heartbeat", r#"{"me":"pi:sting:2:b"}"#),
        (
            "POST",
            "/v1/thread",
            r#"{"me":"pi:sting:2:b","thread_id":"01ARZ3NDEKTSV4RRFFQ69G5FAV"}"#,
        ),
        (
            "POST",
            "/v1/claim",
            r#"{"me":"pi:sting:2:b","subject":"TIN-1","exclusive":true}"#,
        ),
        (
            "POST",
            "/v1/release",
            r#"{"me":"pi:sting:2:b","claim_id":"01ARZ3NDEKTSV4RRFFQ69G5FAV"}"#,
        ),
        (
            "POST",
            "/v1/handoff",
            r#"{"me":"pi:sting:2:b","claim_id":"01ARZ3NDEKTSV4RRFFQ69G5FAV","to":"claude:honey:1:a"}"#,
        ),
        ("POST", "/v1/claims", r#"{"subject":"TIN-1"}"#),
        (
            "POST",
            "/mcp",
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"rebind","version":"1"}}}"#,
        ),
    ];

    fn guarded_request(
        method: &str,
        uri: &str,
        body: &str,
        headers: &[(&str, &str)],
    ) -> Request<Body> {
        let mut builder = Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream");
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        builder.body(Body::from(body.to_owned())).unwrap()
    }

    async fn error_of(response: Response) -> String {
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        serde_json::from_slice::<Value>(&body).unwrap()["error"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    async fn json_of(response: Response) -> Value {
        let body = to_bytes(response.into_body(), 65536).await.unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    fn every() -> PeersQuery<'static> {
        PeersQuery {
            all: true,
            ..PeersQuery::default()
        }
    }

    fn seeded_store() -> Arc<Store> {
        let store = Arc::new(Store::memory().unwrap());
        store
            .register(
                &json!({"harness":"pi","host":"sting","pid":2,"session_id":"b","proc_start":"2"}),
            )
            .unwrap();
        store
    }

    fn state_snapshot(store: &Store) -> (usize, String) {
        (
            store.peers(&every()).unwrap()["peers"]
                .as_array()
                .unwrap()
                .len(),
            store.metrics().unwrap(),
        )
    }

    #[tokio::test]
    async fn rebinding_host_is_refused_on_every_route_before_any_handler() {
        let store = seeded_store();
        let before = state_snapshot(&store);
        let app = router(Arc::clone(&store), vec!["localhost".into()]);
        for host in [
            "attacker.example",
            "attacker.example:8080",
            "localhost.attacker.example",
            "user@localhost",
            "",
        ] {
            for (method, uri, body) in GUARDED_ROUTES {
                let response = app
                    .clone()
                    .oneshot(guarded_request(method, uri, body, &[("host", host)]))
                    .await
                    .unwrap();
                assert_eq!(
                    response.status(),
                    StatusCode::FORBIDDEN,
                    "{method} {uri} Host {host:?}"
                );
                assert_eq!(error_of(response).await, "host not allowed");
            }
        }
        assert_eq!(
            state_snapshot(&store),
            before,
            "a refused request changed state"
        );
    }

    #[tokio::test]
    async fn missing_or_duplicate_host_is_refused_on_every_route() {
        let store = seeded_store();
        let before = state_snapshot(&store);
        let app = router(Arc::clone(&store), vec!["localhost".into()]);
        for (method, uri, body) in GUARDED_ROUTES {
            let response = app
                .clone()
                .oneshot(guarded_request(method, uri, body, &[]))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN, "{method} {uri}");
            assert_eq!(error_of(response).await, "missing host");
            let response = app
                .clone()
                .oneshot(guarded_request(
                    method,
                    uri,
                    body,
                    &[("host", "localhost"), ("host", "attacker.example")],
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN, "{method} {uri}");
        }
        assert_eq!(state_snapshot(&store), before);
    }

    #[tokio::test]
    async fn allowed_host_with_or_without_port_reaches_the_handler() {
        let app = router(
            Arc::new(Store::memory().unwrap()),
            vec![
                "localhost".into(),
                "::1".into(),
                "mcp-agents.example.ts.net".into(),
            ],
        );
        for host in [
            "localhost",
            "localhost:8080",
            "LOCALHOST:18080",
            "[::1]:8080",
            "mcp-agents.example.ts.net:8080",
        ] {
            let register = app
                .clone()
                .oneshot(guarded_request(
                    "POST",
                    "/v1/register",
                    GUARDED_ROUTES[0].2,
                    &[("host", host)],
                ))
                .await
                .unwrap();
            assert_eq!(register.status(), StatusCode::OK, "register Host {host:?}");
            let peers = app
                .clone()
                .oneshot(guarded_request("GET", "/v1/peers", "", &[("host", host)]))
                .await
                .unwrap();
            assert_eq!(peers.status(), StatusCode::OK, "peers Host {host:?}");
        }
    }

    #[tokio::test]
    async fn allowlist_entry_with_port_admits_only_that_port() {
        let store = Arc::new(Store::memory().unwrap());
        let guard = RequestGuard::new(&["swb.example:8080".into()], &[]).unwrap();
        let app = router_with_config(
            store,
            StreamableHttpServerConfig::default().with_allowed_hosts(["swb.example:8080"]),
            guard,
            Router::new(),
        );
        for (host, status) in [
            ("swb.example:8080", StatusCode::OK),
            ("swb.example:9999", StatusCode::FORBIDDEN),
            ("swb.example", StatusCode::FORBIDDEN),
        ] {
            let response = app
                .clone()
                .oneshot(guarded_request("GET", "/v1/peers", "", &[("host", host)]))
                .await
                .unwrap();
            assert_eq!(response.status(), status, "Host {host:?}");
        }
    }

    #[tokio::test]
    async fn browser_origin_is_refused_unless_allowlisted() {
        let store = seeded_store();
        let before = state_snapshot(&store);
        let app = router(Arc::clone(&store), vec!["localhost".into()]);
        for origin in [
            "http://attacker.example",
            "http://localhost:8080",
            "null",
            "not an origin",
        ] {
            for (method, uri, body) in GUARDED_ROUTES {
                let response = app
                    .clone()
                    .oneshot(guarded_request(
                        method,
                        uri,
                        body,
                        &[("host", "localhost:8080"), ("origin", origin)],
                    ))
                    .await
                    .unwrap();
                assert_eq!(
                    response.status(),
                    StatusCode::FORBIDDEN,
                    "{method} {uri} Origin {origin:?}"
                );
                assert_eq!(error_of(response).await, "origin not allowed");
            }
        }
        assert_eq!(state_snapshot(&store), before);
        let allowed = router_with_config(
            Arc::new(Store::memory().unwrap()),
            StreamableHttpServerConfig::default(),
            RequestGuard::new(&["localhost".into()], &["http://localhost:8080".into()]).unwrap(),
            Router::new(),
        );
        for (origin, status) in [
            ("http://localhost:8080", StatusCode::OK),
            ("http://localhost:9999", StatusCode::FORBIDDEN),
            ("https://localhost:8080", StatusCode::FORBIDDEN),
        ] {
            let response = allowed
                .clone()
                .oneshot(guarded_request(
                    "GET",
                    "/v1/peers",
                    "",
                    &[("host", "localhost:8080"), ("origin", origin)],
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), status, "Origin {origin:?}");
        }
    }

    #[tokio::test]
    async fn unknown_paths_are_guarded_and_metrics_is_not() {
        let store = seeded_store();
        let app = router(Arc::clone(&store), vec!["localhost".into()]);
        for path in ["/nope", "/healthz"] {
            let refused = app
                .clone()
                .oneshot(guarded_request(
                    "GET",
                    path,
                    "",
                    &[("host", "attacker.example")],
                ))
                .await
                .unwrap();
            assert_eq!(refused.status(), StatusCode::FORBIDDEN, "{path}");
        }
        let not_found = app
            .clone()
            .oneshot(guarded_request(
                "GET",
                "/nope",
                "",
                &[("host", "localhost")],
            ))
            .await
            .unwrap();
        assert_eq!(not_found.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            json_of(not_found).await,
            json!({"error":"no such route","code":"not_found"})
        );
        let health = app
            .oneshot(guarded_request(
                "GET",
                "/healthz",
                "",
                &[("host", "localhost")],
            ))
            .await
            .unwrap();
        assert_eq!(health.status(), StatusCode::OK);
        assert_eq!(
            json_of(health).await,
            json!({"status":"ok","version":VERSION})
        );
        let probe = metrics_router(Arc::clone(&store))
            .oneshot(guarded_request(
                "GET",
                "/healthz",
                "",
                &[("host", "10.42.0.17:9090")],
            ))
            .await
            .unwrap();
        assert_eq!(probe.status(), StatusCode::OK);
        // The metrics listener is deliberately unguarded (see metrics_router):
        // Prometheus scrapes it by pod address, and it serves only aggregate
        // counters.
        let metrics = metrics_router(store)
            .oneshot(guarded_request(
                "GET",
                "/metrics",
                "",
                &[("host", "10.42.0.17:9090")],
            ))
            .await
            .unwrap();
        assert_eq!(metrics.status(), StatusCode::OK);
        let body = to_bytes(metrics.into_body(), 4096).await.unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(text.contains("swb_messages_total"), "{text}");
        assert!(
            !text.contains("sting"),
            "metrics leaked an identity: {text}"
        );
    }

    #[tokio::test]
    async fn malformed_host_ports_and_brackets_are_refused() {
        let store = seeded_store();
        let before = state_snapshot(&store);
        let app = router(Arc::clone(&store), vec!["localhost".into(), "::1".into()]);
        for host in [
            "localhost:",
            "localhost:99999",
            "localhost:http",
            "[localhost]",
            "[localhost]:8080",
            "[::1",
        ] {
            for (method, uri, body) in GUARDED_ROUTES {
                let response = app
                    .clone()
                    .oneshot(guarded_request(method, uri, body, &[("host", host)]))
                    .await
                    .unwrap();
                assert_eq!(
                    response.status(),
                    StatusCode::FORBIDDEN,
                    "{method} {uri} Host {host:?}"
                );
                assert_eq!(error_of(response).await, "host not allowed");
            }
        }
        assert_eq!(state_snapshot(&store), before);
    }

    #[tokio::test]
    async fn absolute_form_target_authority_must_be_allowed_too() {
        let store = seeded_store();
        let before = state_snapshot(&store);
        let app = router(Arc::clone(&store), vec!["localhost".into()]);
        let refused = app
            .clone()
            .oneshot(guarded_request(
                "POST",
                "http://attacker.example/v1/register",
                GUARDED_ROUTES[0].2,
                &[("host", "localhost:8080")],
            ))
            .await
            .unwrap();
        assert_eq!(refused.status(), StatusCode::FORBIDDEN);
        assert_eq!(error_of(refused).await, "host not allowed");
        assert_eq!(state_snapshot(&store), before);
        let allowed = app
            .oneshot(guarded_request(
                "GET",
                "http://localhost:8080/v1/peers",
                "",
                &[("host", "localhost:8080")],
            ))
            .await
            .unwrap();
        assert_eq!(allowed.status(), StatusCode::OK);
    }

    #[test]
    fn empty_or_invalid_allowlists_are_refused() {
        assert!(RequestGuard::new(&[], &[]).is_err());
        assert!(RequestGuard::new(&[" ".into()], &[]).is_err());
        assert!(RequestGuard::new(&["localhost".into()], &["attacker.example".into()]).is_err());
        assert!(RequestGuard::new(&["localhost".into()], &["https://app.example".into()]).is_ok());
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
                .header("host", "localhost")
                .header("content-type", "application/json")
                .body(Body::from(args.to_string()))
                .unwrap();
            let response = app.clone().oneshot(request).await.unwrap();
            if body == "B" {
                assert_eq!(response.status(), StatusCode::CONFLICT);
                assert_eq!(
                    json_of(response).await,
                    json!({"error":"msg_id conflict","code":"msg_id_conflict"})
                );
            } else {
                assert_eq!(response.status(), StatusCode::OK);
            }
        }
    }

    async fn call(app: &Router, method: &str, uri: &str, body: &str) -> Response {
        app.clone()
            .oneshot(guarded_request(method, uri, body, &[("host", "localhost")]))
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn state_changing_gets_answer_405_and_change_nothing() {
        let store = seeded_store();
        let app = router(Arc::clone(&store), vec!["localhost".into()]);
        store
            .send(
                &json!({"from":"claude:honey:1:a","to":"pi:sting:2:b","ticket":"none","body":"x"}),
            )
            .unwrap();
        let before = state_snapshot(&store);
        for uri in [
            "/v1/inbox?me=pi:sting:2:b",
            "/v1/inbox",
            "/v1/peers?me=pi:sting:2:b",
        ] {
            let response = call(&app, "GET", uri, "").await;
            assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED, "{uri}");
            assert_eq!(response.headers()[header::ALLOW], "POST", "{uri}");
            let body = json_of(response).await;
            assert_eq!(body["code"], "method_not_allowed", "{uri}");
            assert!(
                body["error"].as_str().unwrap().contains("POST /v1/"),
                "{uri}"
            );
        }
        assert_eq!(state_snapshot(&store), before);
        let inbox = store.inbox("pi:sting:2:b", 10).unwrap();
        assert_eq!(inbox["messages"][0]["delivery_count"], 1, "a GET fetched");
        let wrong = call(&app, "DELETE", "/v1/send", "").await;
        assert_eq!(wrong.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(json_of(wrong).await["code"], "method_not_allowed");
    }

    #[tokio::test]
    async fn post_peers_and_inbox_take_a_json_body() {
        let store = seeded_store();
        let app = router(Arc::clone(&store), vec!["localhost".into()]);
        let refreshed = call(
            &app,
            "POST",
            "/v1/peers",
            r#"{"me":"pi:sting:2:b","all":true}"#,
        )
        .await;
        assert_eq!(refreshed.status(), StatusCode::OK);
        let body = json_of(refreshed).await;
        assert_eq!(body["peers"][0]["agent_id"], "pi:sting:2:b");
        assert_eq!(body["next_cursor"], Value::Null);
        let passive = call(&app, "GET", "/v1/peers?all=1&host=sting&limit=5", "").await;
        assert_eq!(passive.status(), StatusCode::OK);
        assert_eq!(json_of(passive).await["peers"].as_array().unwrap().len(), 1);
        for (uri, code) in [
            ("/v1/peers?all=maybe", "invalid_argument"),
            ("/v1/peers?limit=-1", "invalid_argument"),
            ("/v1/peers?bogus=1", "invalid_argument"),
            ("/v1/peers?host=a%2Fb", "invalid_argument"),
        ] {
            let response = call(&app, "GET", uri, "").await;
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{uri}");
            assert_eq!(json_of(response).await["code"], code, "{uri}");
        }
        let inbox = call(
            &app,
            "POST",
            "/v1/inbox",
            r#"{"me":"pi:sting:2:b","limit":1}"#,
        )
        .await;
        assert_eq!(inbox.status(), StatusCode::OK);
        assert!(
            json_of(inbox).await["messages"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn errors_are_typed_json() {
        let app = router(seeded_store(), vec!["localhost".into()]);
        let cases: [(&str, &str, StatusCode, &str); 7] = [
            (
                "/v1/peers",
                r#"{"me":"codex:honey:3:nobody"}"#,
                StatusCode::NOT_FOUND,
                "unregistered",
            ),
            (
                "/v1/ack",
                r#"{"me":"pi:sting:2:b","msg_id":"01ARZ3NDEKTSV4RRFFQ69G5FAV"}"#,
                StatusCode::NOT_FOUND,
                "message_unavailable",
            ),
            (
                "/v1/send",
                r#"{"from":"x","to":"y","ticket":"none","body":"b"}"#,
                StatusCode::BAD_REQUEST,
                "invalid_argument",
            ),
            (
                "/v1/peers",
                r#"{"all":"yes"}"#,
                StatusCode::BAD_REQUEST,
                "invalid_argument",
            ),
            (
                "/v1/send",
                "{not json",
                StatusCode::BAD_REQUEST,
                "invalid_json",
            ),
            ("/v1/send", "[]", StatusCode::BAD_REQUEST, "invalid_json"),
            (
                "/v1/inbox",
                r#"{"me":"pi:sting:2:b","wait_seconds":"5"}"#,
                StatusCode::BAD_REQUEST,
                "invalid_argument",
            ),
        ];
        for (uri, body, status, code) in cases {
            let response = call(&app, "POST", uri, body).await;
            assert_eq!(response.status(), status, "{uri} {body}");
            let value = json_of(response).await;
            assert_eq!(value["code"], code, "{uri} {body}: {value}");
            assert!(value["error"].as_str().is_some_and(|e| !e.is_empty()));
        }
        let untyped = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/send")
                    .header("host", "localhost")
                    .header("content-type", "text/plain")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(untyped.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
        assert_eq!(json_of(untyped).await["code"], "unsupported_media_type");
    }

    #[tokio::test]
    async fn end_over_rest_needs_the_registered_token() {
        let store = Arc::new(Store::memory().unwrap());
        let app = router(Arc::clone(&store), vec!["localhost".into()]);
        let registered =
            json_of(call(&app, "POST", "/v1/register", GUARDED_ROUTES[0].2).await).await;
        let token = registered["end_token"].as_str().unwrap();
        let legacy = call(
            &app,
            "POST",
            "/v1/end",
            r#"{"me":"claude:honey:1:a","proc_start":"1"}"#,
        )
        .await;
        assert_eq!(legacy.status(), StatusCode::BAD_REQUEST);
        assert!(error_of(legacy).await.contains("end_token"));
        let forged = call(&app, "POST", "/v1/end", GUARDED_ROUTES[1].2).await;
        assert_eq!(json_of(forged).await["state"], "unknown");
        let listed = json_of(call(&app, "GET", "/v1/peers", "").await).await;
        assert_eq!(listed["peers"][0]["state"], "live");
        assert!(listed["peers"][0].get("proc_start").is_none());
        let body = json!({"me":"claude:honey:1:a","end_token":token}).to_string();
        let ended = call(&app, "POST", "/v1/end", &body).await;
        assert_eq!(json_of(ended).await["state"], "ended");
        let listed = json_of(call(&app, "GET", "/v1/peers", "").await).await;
        assert!(listed["peers"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn claims_over_rest_follow_r_c950() {
        let store = Arc::new(Store::memory().unwrap());
        let app = router(Arc::clone(&store), vec!["localhost".into()]);
        let a = json_of(call(&app, "POST", "/v1/register", GUARDED_ROUTES[0].2).await).await;
        let token = a["end_token"].as_str().unwrap().to_owned();
        store
            .register(
                &json!({"harness":"pi","host":"sting","pid":2,"session_id":"b","proc_start":"2"}),
            )
            .unwrap();
        let claim = |me: &str, exclusive: bool| {
            json!({"me":me,"subject":"TIN-1","exclusive":exclusive}).to_string()
        };
        let held =
            json_of(call(&app, "POST", "/v1/claim", &claim("claude:honey:1:a", true)).await).await;
        assert_eq!(held["outcome"], "recorded");
        // A refusal is a 200 with outcome refused, not an error.
        let refused = call(&app, "POST", "/v1/claim", &claim("pi:sting:2:b", true)).await;
        assert_eq!(refused.status(), StatusCode::OK);
        let refused = json_of(refused).await;
        assert_eq!(refused["outcome"], "refused");
        assert_eq!(refused["held_by"][0]["holder"], "claude:honey:1:a");
        let beside =
            json_of(call(&app, "POST", "/v1/claim", &claim("pi:sting:2:b", false)).await).await;
        assert_eq!(beside["overlaps"][0]["exclusive"], true);
        let renewed =
            json_of(call(&app, "POST", "/v1/claim", &claim("claude:honey:1:a", true)).await).await;
        assert_eq!(renewed["outcome"], "renewed");
        assert_eq!(renewed["claim_id"], held["claim_id"]);
        // Another session's claim is not releasable by id.
        let theirs = json!({"me":"pi:sting:2:b","claim_id":held["claim_id"]}).to_string();
        let other = call(&app, "POST", "/v1/release", &theirs).await;
        assert_eq!(other.status(), StatusCode::NOT_FOUND);
        assert_eq!(json_of(other).await["code"], "claim_unavailable");
        let unregistered = call(&app, "POST", "/v1/claim", &claim("codex:neo:3:c", false)).await;
        assert_eq!(unregistered.status(), StatusCode::NOT_FOUND);
        // Ending the holder releases its claims at once.
        let end = json!({"me":"claude:honey:1:a","end_token":token}).to_string();
        let ended = json_of(call(&app, "POST", "/v1/end", &end).await).await;
        assert_eq!(ended["released_claims"][0]["claim_id"], held["claim_id"]);
        let listed =
            json_of(call(&app, "POST", "/v1/claims", r#"{"subject":"TIN-1"}"#).await).await;
        let holders: Vec<&str> = listed["claims"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["holder"].as_str().unwrap())
            .collect();
        assert_eq!(holders, ["pi:sting:2:b"]);
    }

    #[test]
    fn claim_audit_lines_carry_no_note() {
        let store = Store::memory().unwrap();
        store
            .register(&json!({"harness":"claude","host":"honey","pid":1,"session_id":"a","proc_start":"1"}))
            .unwrap();
        let args = json!({"me":"claude:honey:1:a","subject":"TIN-1","note":"note sentinel"});
        let result = store.claim(&args).unwrap();
        let line = audit_record("claim", &args, &result, None).to_string();
        assert!(!line.contains("note sentinel"), "{line}");
        assert_eq!(
            audit_record("claim", &args, &result, None)["subject"],
            "TIN-1"
        );
    }

    async fn mcp_session(app: &Router) -> String {
        let init = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"swb-test","version":"1"}}});
        let response = app
            .clone()
            .oneshot(guarded_request(
                "POST",
                "/mcp",
                &init.to_string(),
                &[("host", "localhost")],
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        response
            .headers()
            .get("mcp-session-id")
            .expect("rmcp session id")
            .to_str()
            .unwrap()
            .to_owned()
    }

    /// One MCP tool call's whole `result`, error results included.
    async fn mcp_result(app: &Router, session: &str, id: u32, name: &str, args: Value) -> Value {
        let call = json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":args}});
        let response = app
            .clone()
            .oneshot(guarded_request(
                "POST",
                "/mcp",
                &call.to_string(),
                &[("host", "localhost"), ("mcp-session-id", session)],
            ))
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), 65536).await.unwrap();
        let raw = std::str::from_utf8(&body).unwrap();
        let payload = raw
            .lines()
            .filter_map(|line| line.strip_prefix("data: "))
            .find(|line| line.trim_start().starts_with('{'))
            .unwrap_or(raw);
        serde_json::from_str::<Value>(payload).unwrap()["result"].clone()
    }

    async fn status_and_json(response: Response) -> (StatusCode, Value) {
        let status = response.status();
        (status, json_of(response).await)
    }

    const BOUND_REGISTER: &str = r#"{"harness":"hermes","host":"sting","pid":100,"session_id":"svc","proc_start":"s-100","bind":true}"#;
    const BOUND_ID: &str = "hermes:sting:100:svc";

    #[tokio::test]
    async fn spoofed_send_as_a_bound_id_fails_over_rest_and_mcp() {
        let store = Arc::new(Store::memory().unwrap());
        let app = router(Arc::clone(&store), vec!["localhost".into()]);
        let (status, registered) =
            status_and_json(call(&app, "POST", "/v1/register", BOUND_REGISTER).await).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(registered["bound"], true);
        let token = registered["session_token"].as_str().unwrap().to_owned();
        let peer = "claude:honey:1:a";
        // The spoof test (SWB-R66): from set to a bound id and no token.
        for body in [
            json!({"from":BOUND_ID,"to":peer,"ticket":"none","body":"spoofed"}),
            json!({"from":BOUND_ID,"to":peer,"ticket":"none","body":"spoofed","session_token":"0".repeat(64)}),
            json!({"from":BOUND_ID,"to":peer,"ticket":"none","body":"spoofed","session_token":registered["end_token"]}),
        ] {
            let (status, error) =
                status_and_json(call(&app, "POST", "/v1/send", &body.to_string()).await).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{error}");
            assert_eq!(error["code"], "unauthorized");
        }
        assert!(
            store.inbox(peer, 10).unwrap()["messages"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        // Over MCP the same send is an error result and stores nothing.
        let session = mcp_session(&app).await;
        let spoof = mcp_result(
            &app,
            &session,
            2,
            "send",
            json!({"from":BOUND_ID,"to":peer,"ticket":"none","body":"spoofed over mcp"}),
        )
        .await;
        assert_eq!(spoof["isError"], true, "{spoof}");
        assert!(spoof.to_string().contains("unauthorized"), "{spoof}");
        assert!(
            store.inbox(peer, 10).unwrap()["messages"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        // Every other listed operation needs the token too.
        for (route, body) in [
            ("/v1/inbox", json!({"me":BOUND_ID})),
            (
                "/v1/ack",
                json!({"me":BOUND_ID,"msg_id":"01ARZ3NDEKTSV4RRFFQ69G5FAV"}),
            ),
            ("/v1/claim", json!({"me":BOUND_ID,"subject":"TIN-4655"})),
            (
                "/v1/release",
                json!({"me":BOUND_ID,"claim_id":"01ARZ3NDEKTSV4RRFFQ69G5FAV"}),
            ),
            (
                "/v1/handoff",
                json!({"me":BOUND_ID,"claim_id":"01ARZ3NDEKTSV4RRFFQ69G5FAV","to":peer}),
            ),
            (
                "/v1/end",
                json!({"me":BOUND_ID,"end_token":registered["end_token"]}),
            ),
            (
                "/v1/register",
                serde_json::from_str::<Value>(BOUND_REGISTER).unwrap(),
            ),
        ] {
            let (status, error) =
                status_and_json(call(&app, "POST", route, &body.to_string()).await).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{route}: {error}");
        }
        assert!(
            store.claims(&ClaimsQuery::default()).unwrap()["claims"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        // With the token the send goes through, as peer, and the token is
        // not part of the stored envelope.
        let body =
            json!({"from":BOUND_ID,"to":peer,"ticket":"none","body":"real","session_token":token});
        let (status, sent) =
            status_and_json(call(&app, "POST", "/v1/send", &body.to_string()).await).await;
        assert_eq!(status, StatusCode::OK, "{sent}");
        assert_eq!(sent["authority"], "peer");
        assert!(!sent.to_string().contains(&token));
        let received = store.inbox(peer, 10).unwrap();
        assert_eq!(received["messages"][0]["from"], BOUND_ID);
        assert!(!received.to_string().contains(&token));
        let claim = json!({"me":BOUND_ID,"subject":"TIN-4655","session_token":token});
        let (status, _) =
            status_and_json(call(&app, "POST", "/v1/claim", &claim.to_string()).await).await;
        assert_eq!(status, StatusCode::OK);
        // SWB-R68: peers shows the binding.
        let listed = json_of(call(&app, "GET", "/v1/peers", "").await).await;
        assert_eq!(listed["peers"][0]["bound"], true);
        // An ill-typed token is a plain invalid argument.
        let (status, _) = status_and_json(
            call(
                &app,
                "POST",
                "/v1/inbox",
                &json!({"me":BOUND_ID,"session_token":7}).to_string(),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn a_bound_client_rotates_its_token_on_every_register() {
        let store = Arc::new(Store::memory().unwrap());
        let app = router(Arc::clone(&store), vec!["localhost".into()]);
        let first = json_of(call(&app, "POST", "/v1/register", BOUND_REGISTER).await).await;
        let old = first["session_token"].as_str().unwrap().to_owned();
        let mut again: Value = serde_json::from_str(BOUND_REGISTER).unwrap();
        again["session_token"] = json!(old);
        let (status, second) =
            status_and_json(call(&app, "POST", "/v1/register", &again.to_string()).await).await;
        assert_eq!(status, StatusCode::OK, "{second}");
        let new = second["session_token"].as_str().unwrap();
        assert_ne!(new, old);
        let with = |token: &str| json!({"me":BOUND_ID,"session_token":token}).to_string();
        let (status, _) = status_and_json(call(&app, "POST", "/v1/inbox", &with(&old)).await).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, _) = status_and_json(call(&app, "POST", "/v1/inbox", &with(new)).await).await;
        assert_eq!(status, StatusCode::OK);
        // The old token cannot register again either.
        let (status, _) =
            status_and_json(call(&app, "POST", "/v1/register", &again.to_string()).await).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn an_empty_send_bucket_answers_429_rate_limited() {
        let store = Arc::new(Store::memory().unwrap());
        store
            .set_send_limit(SendLimit {
                burst: 2,
                refill_per_minute: 1,
            })
            .unwrap();
        let app = router(Arc::clone(&store), vec!["localhost".into()]);
        let send = |from: &str| {
            json!({"from":from,"to":"pi:sting:2:b","ticket":"none","body":"x"}).to_string()
        };
        for _ in 0..2 {
            let (status, _) =
                status_and_json(call(&app, "POST", "/v1/send", &send("claude:honey:1:a")).await)
                    .await;
            assert_eq!(status, StatusCode::OK);
        }
        let (status, error) =
            status_and_json(call(&app, "POST", "/v1/send", &send("claude:honey:1:a")).await).await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(error["code"], "rate_limited");
        assert!(
            error["error"]
                .as_str()
                .unwrap()
                .starts_with("rate limited; retry in")
        );
        // The refused send stored nothing; another sender is unaffected.
        assert_eq!(
            store.inbox("pi:sting:2:b", 10).unwrap()["messages"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        let (status, _) =
            status_and_json(call(&app, "POST", "/v1/send", &send("codex:honey:3:c")).await).await;
        assert_eq!(status, StatusCode::OK);
        // A spoofer without the token cannot drain a bound sender's bucket.
        let registered = json_of(call(&app, "POST", "/v1/register", BOUND_REGISTER).await).await;
        for _ in 0..5 {
            let (status, _) =
                status_and_json(call(&app, "POST", "/v1/send", &send(BOUND_ID)).await).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
        }
        let mut real: Value = serde_json::from_str(&send(BOUND_ID)).unwrap();
        real["session_token"] = registered["session_token"].clone();
        let (status, _) =
            status_and_json(call(&app, "POST", "/v1/send", &real.to_string()).await).await;
        assert_eq!(status, StatusCode::OK);
    }

    /// Review of #54: an idempotent retry of a stored send (SWB-R46) and an
    /// invalid send never spend the bucket, so a client that retries after a
    /// timeout gets its receipt and not 429.
    #[tokio::test]
    async fn a_replayed_or_invalid_send_spends_no_bucket_token() {
        let store = Arc::new(Store::memory().unwrap());
        store
            .set_send_limit(SendLimit {
                burst: 1,
                refill_per_minute: 1,
            })
            .unwrap();
        let app = router(Arc::clone(&store), vec!["localhost".into()]);
        let id = "01JABCDEFGHJKMNPQRSTVWXYZ0";
        let send = |ticket: &str| {
            json!({"from":"claude:honey:1:a","to":"pi:sting:2:b","ticket":ticket,"body":"x","msg_id":id})
                .to_string()
        };
        let (status, _) = status_and_json(call(&app, "POST", "/v1/send", &send("bad")).await).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        for _ in 0..3 {
            let (status, envelope) =
                status_and_json(call(&app, "POST", "/v1/send", &send("none")).await).await;
            assert_eq!(status, StatusCode::OK, "{envelope}");
            assert_eq!(envelope["msg_id"], id);
        }
        let fresh =
            json!({"from":"claude:honey:1:a","to":"pi:sting:2:b","ticket":"none","body":"y"});
        let (status, error) =
            status_and_json(call(&app, "POST", "/v1/send", &fresh.to_string()).await).await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(error["code"], "rate_limited");
    }

    /// Review of #54: an inbox long-poll checks the binding on every read. A
    /// tokenless poll that starts before a service binds its id stops with
    /// 401 once the id is bound and never receives the bound service's mail.
    #[test]
    fn a_tokenless_long_poll_stops_when_its_id_is_bound() {
        let store = Arc::new(Store::memory().unwrap());
        let poller = {
            let store = Arc::clone(&store);
            std::thread::spawn(move || {
                operation(&store, "inbox", &json!({"me":BOUND_ID,"wait_seconds":3}))
            })
        };
        std::thread::sleep(Duration::from_millis(400));
        let registered = operation(
            &store,
            "register",
            &serde_json::from_str(BOUND_REGISTER).unwrap(),
        )
        .unwrap();
        assert_eq!(registered["bound"], true);
        operation(
            &store,
            "send",
            &json!({"from":"claude:honey:1:a","to":BOUND_ID,"ticket":"none","body":"secret for bound svc"}),
        )
        .unwrap();
        let error = poller.join().unwrap().unwrap_err();
        assert_eq!(error.kind, ErrorKind::Unauthorized);
        // The message is still unread for the token holder.
        let read = operation(
            &store,
            "inbox",
            &json!({"me":BOUND_ID,"session_token":registered["session_token"]}),
        )
        .unwrap();
        assert_eq!(read["messages"][0]["body"], "secret for bound svc");
        assert_eq!(read["messages"][0]["delivery_count"], 1);
    }

    #[test]
    fn the_send_limit_reads_conservative_defaults_and_refuses_bad_values() {
        let none = |_: &str| None;
        assert_eq!(
            send_limit_from(&none).unwrap(),
            SendLimit {
                burst: 20,
                refill_per_minute: 30
            }
        );
        let set = |burst: &'static str| {
            move |name: &str| (name == "SWB_SEND_BURST").then(|| burst.to_owned())
        };
        assert_eq!(send_limit_from(&set("0")).unwrap().burst, 0);
        assert_eq!(send_limit_from(&set(" 5 ")).unwrap().burst, 5);
        assert!(send_limit_from(&set("-1")).is_err());
        assert!(send_limit_from(&set("lots")).is_err());
        assert!(send_limit_from(&set("100001")).is_err());
    }

    #[tokio::test]
    async fn unbound_sessions_work_as_in_v040() {
        let store = Arc::new(Store::memory().unwrap());
        let app = router(Arc::clone(&store), vec!["localhost".into()]);
        let registered =
            json_of(call(&app, "POST", "/v1/register", GUARDED_ROUTES[0].2).await).await;
        assert_eq!(registered["bound"], false);
        assert!(registered.get("session_token").is_none());
        let me = registered["agent_id"].as_str().unwrap();
        let send = json!({"from":me,"to":me,"ticket":"none","body":"self"}).to_string();
        let (status, sent) = status_and_json(call(&app, "POST", "/v1/send", &send).await).await;
        assert_eq!(status, StatusCode::OK);
        let inbox =
            json_of(call(&app, "POST", "/v1/inbox", &json!({"me":me}).to_string()).await).await;
        assert_eq!(inbox["messages"][0]["msg_id"], sent["msg_id"]);
        let ack = json!({"me":me,"msg_id":sent["msg_id"]}).to_string();
        let (status, _) = status_and_json(call(&app, "POST", "/v1/ack", &ack).await).await;
        assert_eq!(status, StatusCode::OK);
        // Anyone may still send as an unbound id (SWB-R05 for unbound ids).
        let other =
            json!({"from":"codex:sting:9:x","to":me,"ticket":"none","body":"unregistered sender"});
        let (status, _) =
            status_and_json(call(&app, "POST", "/v1/send", &other.to_string()).await).await;
        assert_eq!(status, StatusCode::OK);
        let listed = json_of(call(&app, "GET", "/v1/peers", "").await).await;
        assert_eq!(listed["peers"][0]["bound"], false);
    }

    #[test]
    fn audit_never_carries_a_session_token() {
        let store = Store::memory().unwrap();
        let args: Value = serde_json::from_str(BOUND_REGISTER).unwrap();
        let result = store.register(&args).unwrap();
        let token = result["session_token"].as_str().unwrap().to_owned();
        let line = audit_record("register", &args, &result, None).to_string();
        assert!(!line.contains(&token), "{line}");
        let send = json!({"from":BOUND_ID,"to":"pi:sting:2:b","ticket":"none","body":"b","session_token":token});
        let sent = operation(&store, "send", &send).unwrap();
        for line in audit_lines(&store, "send", &send, &sent, true).unwrap() {
            assert!(!line.to_string().contains(&token), "{line}");
        }
    }

    #[test]
    fn audit_never_carries_the_end_token() {
        let store = Store::memory().unwrap();
        let args = json!({"harness":"claude","host":"honey","pid":1,"session_id":"a","proc_start":"secret-start"});
        let result = store.register(&args).unwrap();
        let token = result["end_token"].as_str().unwrap();
        let line = audit_record("register", &args, &result, None).to_string();
        assert!(!line.contains(token), "{line}");
        assert_eq!(
            audit_record("register", &args, &result, None)["agent_id"],
            "claude:honey:1:a"
        );
    }

    #[test]
    fn audit_lines_follow_a_thread_through_send_inbox_and_ack() {
        // ADR-0002 L1-lite (TIN-6749): send, each delivery and ack carry
        // msg_id, thread_id and ticket, so Loki can follow a thread.
        let store = Store::memory().unwrap();
        let (a, b) = ("claude:honey:1:a", "pi:sting:2:b");
        let send = json!({"from":a,"to":b,"ticket":"TIN-6749","body":"body sentinel"});
        let first = store.send(&send).unwrap().envelope;
        let thread = first["thread_id"].clone();
        let msg_id = first["msg_id"].clone();
        assert!(thread.is_string());

        let sent = audit_lines(&store, "send", &send, &first, true).unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0]["op"], "send");
        assert_eq!(sent[0]["msg_id"], msg_id);
        assert_eq!(sent[0]["thread_id"], thread);
        assert_eq!(sent[0]["ticket"], "TIN-6749");
        assert_eq!(sent[0]["seq"], 1);

        let inbox_args = json!({"me":b});
        let batch = store.inbox(b, 20).unwrap();
        let delivered = audit_lines(&store, "inbox", &inbox_args, &batch, true).unwrap();
        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0]["op"], "fetch");
        assert_eq!(delivered[0]["me"], b);
        assert_eq!(delivered[0]["msg_id"], msg_id);
        assert_eq!(delivered[0]["thread_id"], thread);
        assert_eq!(delivered[0]["ticket"], "TIN-6749");
        assert_eq!(delivered[0]["delivery_count"], 1);

        let ack_args = json!({"me":b,"msg_id":msg_id});
        let acked = store.ack(b, msg_id.as_str().unwrap()).unwrap();
        let ack_lines = audit_lines(&store, "ack", &ack_args, &acked, true).unwrap();
        assert_eq!(ack_lines.len(), 1);
        assert_eq!(ack_lines[0]["op"], "ack");
        assert_eq!(ack_lines[0]["msg_id"], msg_id);
        assert_eq!(ack_lines[0]["thread_id"], thread);
        assert_eq!(ack_lines[0]["ticket"], "TIN-6749");
        assert_eq!(ack_lines[0]["state"], "acked");
        assert_eq!(ack_lines[0]["seq"], 1);

        // A reply keeps the thread, and its lines say so.
        let reply = json!({"from":b,"to":a,"ticket":"TIN-6749","body":"reply sentinel",
            "thread_id":thread,"in_reply_to":msg_id});
        let second = store.send(&reply).unwrap().envelope;
        let reply_lines = audit_lines(&store, "send", &reply, &second, true).unwrap();
        assert_eq!(reply_lines[0]["thread_id"], thread);
        assert_ne!(reply_lines[0]["msg_id"], msg_id);
        assert_eq!(reply_lines[0]["in_reply_to"], msg_id);
        assert_eq!(reply_lines[0]["seq"], 2);

        for line in sent
            .iter()
            .chain(&delivered)
            .chain(&ack_lines)
            .chain(&reply_lines)
        {
            let text = line.to_string();
            assert!(!text.contains("sentinel"), "{text}");
        }
        // An empty inbox read writes no line.
        let empty = store.inbox(b, 20).unwrap();
        assert!(
            audit_lines(&store, "inbox", &inbox_args, &empty, true)
                .unwrap()
                .is_empty()
        );
        // No stored message: no metadata, and an ack line keeps nulls.
        assert!(
            store
                .message_meta("01ARZ3NDEKTSV4RRFFQ69G5FAV")
                .unwrap()
                .is_none()
        );
    }

    /// A store with `pi:host-b:2:b` registered and live.
    fn store_with_recipient() -> Arc<Store> {
        let store = Arc::new(Store::memory().unwrap());
        store
            .register(
                &json!({"harness":"pi","host":"host-b","pid":2,"session_id":"b","proc_start":"2"}),
            )
            .unwrap();
        store
    }

    /// A1 (TIN-7577): REST and MCP page an inbox with `after`.
    #[tokio::test]
    async fn inbox_over_rest_and_mcp_pages_with_after() {
        let store = store_with_recipient();
        let app = router(Arc::clone(&store), vec!["localhost".into()]);
        let me = "pi:host-b:2:b";
        let mut ids = Vec::new();
        for body in ["one", "two", "three"] {
            let sent = store
                .send(&json!({"from":"claude:host-a:1:a","to":me,"ticket":"none","body":body}))
                .unwrap();
            ids.push(sent.envelope["msg_id"].as_str().unwrap().to_owned());
        }
        let inbox = |args: Value| {
            let app = app.clone();
            async move {
                status_and_json(call(&app, "POST", "/v1/inbox", &args.to_string()).await).await
            }
        };
        let (status, first) = inbox(json!({"me":me,"limit":2})).await;
        assert_eq!(status, StatusCode::OK, "{first}");
        assert_eq!(first["messages"].as_array().unwrap().len(), 2);
        assert_eq!(first["next_after"], ids[1].as_str());
        let (status, rest) = inbox(json!({"me":me,"limit":2,"after":ids[1]})).await;
        assert_eq!(status, StatusCode::OK, "{rest}");
        assert_eq!(rest["messages"].as_array().unwrap().len(), 1);
        assert_eq!(rest["messages"][0]["msg_id"], ids[2].as_str());
        assert_eq!(rest["next_after"], Value::Null);
        for bad in [json!("nope"), json!(7), json!("01ARZ3NDEKTSV4RRFFQ69G5FAV")] {
            let (status, error) = inbox(json!({"me":me,"after":bad})).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
            assert_eq!(error["code"], "invalid_argument");
        }
        let session = mcp_session(&app).await;
        let paged = mcp_call(
            &app,
            &session,
            2,
            "inbox",
            json!({"me":me,"limit":2,"after":ids[0]}),
        )
        .await;
        assert_eq!(paged["messages"].as_array().unwrap().len(), 2);
        assert_eq!(paged["next_after"], ids[2].as_str());
        let refused = mcp_result(&app, &session, 3, "inbox", json!({"me":me,"after":"nope"})).await;
        assert_eq!(refused["isError"], true, "{refused}");
    }

    /// A4 (TIN-7577): each live process of one session finds its own row.
    #[tokio::test]
    async fn peers_proc_start_finds_each_live_process_of_one_session() {
        let store = Arc::new(Store::memory().unwrap());
        let app = router(Arc::clone(&store), vec!["localhost".into()]);
        for (pid, start) in [(1, "p1"), (2, "p2")] {
            store
                .register(&json!({"harness":"claude","host":"host-b","pid":pid,
                    "session_id":"shared","proc_start":start}))
                .unwrap();
        }
        for (start, id) in [
            ("p1", "claude:host-b:1:shared"),
            ("p2", "claude:host-b:2:shared"),
        ] {
            let query = json!({"all":true,"host":"host-b","proc_start":start});
            let (status, listed) =
                status_and_json(call(&app, "POST", "/v1/peers", &query.to_string()).await).await;
            assert_eq!(status, StatusCode::OK, "{listed}");
            assert_eq!(listed["peers"].as_array().unwrap().len(), 1, "{listed}");
            assert_eq!(listed["peers"][0]["agent_id"], id);
        }
    }

    /// A5 (TIN-7577): REST and MCP send results and the send audit line
    /// carry recipient_state; the delivered envelope does not.
    #[tokio::test]
    async fn send_results_carry_recipient_state_and_the_audit_line_too() {
        let store = store_with_recipient();
        let app = router(Arc::clone(&store), vec!["localhost".into()]);
        let live =
            json!({"from":"claude:host-a:1:a","to":"pi:host-b:2:b","ticket":"none","body":"x"});
        let (status, sent) =
            status_and_json(call(&app, "POST", "/v1/send", &live.to_string()).await).await;
        assert_eq!(status, StatusCode::OK, "{sent}");
        assert_eq!(sent["recipient_state"], "live");
        let session = mcp_session(&app).await;
        let over_mcp = mcp_call(
            &app,
            &session,
            2,
            "send",
            json!({"from":"claude:host-a:1:a","to":"codex:host-b:9:none","ticket":"none","body":"y"}),
        )
        .await;
        assert_eq!(over_mcp["recipient_state"], "unregistered");
        let args = json!({"from":"claude:host-a:1:a","to":"codex:host-b:9:none","ticket":"none","body":"z"});
        let result = operation(&store, "send", &args).unwrap();
        let lines = audit_lines(&store, "send", &args, &result, true).unwrap();
        assert_eq!(lines[0]["recipient_state"], "unregistered");
        let fetched = store.inbox("pi:host-b:2:b", 10).unwrap();
        assert!(fetched["messages"][0].get("recipient_state").is_none());
        let fetch_lines = audit_lines(
            &store,
            "inbox",
            &json!({"me":"pi:host-b:2:b"}),
            &fetched,
            true,
        )
        .unwrap();
        assert!(
            fetch_lines[0].get("recipient_state").is_none(),
            "{}",
            fetch_lines[0]
        );
    }

    /// TIN-7577: a REST 429 carries `Retry-After` and `retry_after` when the
    /// bucket refills; a bucket that never refills carries neither, and
    /// other errors keep the v0.5.0 body.
    #[tokio::test]
    async fn a_rate_limited_send_carries_retry_after() {
        let store = Arc::new(Store::memory().unwrap());
        store
            .set_send_limit(SendLimit {
                burst: 1,
                refill_per_minute: 6,
            })
            .unwrap();
        let app = router(Arc::clone(&store), vec!["localhost".into()]);
        let send = |from: &str| {
            json!({"from":from,"to":"pi:host-b:2:b","ticket":"none","body":"x"}).to_string()
        };
        let (status, _) =
            status_and_json(call(&app, "POST", "/v1/send", &send("claude:host-a:1:a")).await).await;
        assert_eq!(status, StatusCode::OK);
        let refused = call(&app, "POST", "/v1/send", &send("claude:host-a:1:a")).await;
        assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
        let header_seconds: u64 = refused
            .headers()
            .get(header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|text| text.parse().ok())
            .unwrap();
        assert!((1..=10).contains(&header_seconds), "{header_seconds}");
        let body = json_of(refused).await;
        assert_eq!(body["code"], "rate_limited");
        assert_eq!(body["retry_after"].as_u64(), Some(header_seconds));
        store
            .set_send_limit(SendLimit {
                burst: 1,
                refill_per_minute: 0,
            })
            .unwrap();
        let (status, _) =
            status_and_json(call(&app, "POST", "/v1/send", &send("codex:host-a:3:c")).await).await;
        assert_eq!(status, StatusCode::OK);
        let refused = call(&app, "POST", "/v1/send", &send("codex:host-a:3:c")).await;
        assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(refused.headers().get(header::RETRY_AFTER).is_none());
        let body = json_of(refused).await;
        assert!(body.get("retry_after").is_none(), "{body}");
        assert_eq!(
            store_error_body(&StoreError::new(ErrorKind::Invalid, "invalid ticket")),
            json!({"error":"invalid ticket","code":"invalid_argument"})
        );
    }

    /// TIN-7577: a send the rate limit refuses yields one line with the
    /// harness kind, retry_after and route, and no id or body.
    #[test]
    fn a_refused_send_logs_one_line_without_ids() {
        let store = Store::memory().unwrap();
        store
            .set_send_limit(SendLimit {
                burst: 1,
                refill_per_minute: 6,
            })
            .unwrap();
        let args = json!({"from":"claude:host-a:1:a","to":"pi:host-b:2:b","ticket":"none",
            "body":"body sentinel"});
        operation(&store, "send", &args).unwrap();
        let error = operation(&store, "send", &args).unwrap_err();
        assert_eq!(error.kind, ErrorKind::RateLimited);
        let line = refusal_record("send", &args, &error, "mcp").unwrap();
        assert_eq!(line["op"], "send_refused");
        assert_eq!(line["code"], "rate_limited");
        assert_eq!(line["harness"], "claude");
        assert_eq!(line["route"], "mcp");
        assert!(
            line["retry_after"].as_u64().is_some_and(|s| s >= 1),
            "{line}"
        );
        assert!(line["ts"].is_u64());
        let text = line.to_string();
        for leak in ["claude:host-a", "pi:host-b", "sentinel", "host-a"] {
            assert!(!text.contains(leak), "{leak} in {text}");
        }
        // A forged from with an unknown kind gives a null harness.
        let mut forged = args.clone();
        forged["from"] = json!("bogus");
        assert_eq!(
            refusal_record("send", &forged, &error, "rest").unwrap()["harness"],
            Value::Null
        );
        // Other errors and other operations write nothing.
        let invalid = StoreError::new(ErrorKind::Invalid, "invalid ticket");
        assert!(refusal_record("send", &args, &invalid, "rest").is_none());
        assert!(refusal_record("ack", &args, &error, "rest").is_none());
    }

    /// TIN-7577: the metrics listener serves the send limit series.
    #[tokio::test]
    async fn metrics_lists_the_send_limit_series() {
        let store = Arc::new(Store::memory().unwrap());
        store
            .set_send_limit(SendLimit {
                burst: 1,
                refill_per_minute: 6,
            })
            .unwrap();
        let args =
            json!({"from":"codex:host-b:9:x","to":"pi:host-b:2:b","ticket":"none","body":"x"});
        operation(&store, "send", &args).unwrap();
        assert_eq!(
            operation(&store, "send", &args).unwrap_err().kind,
            ErrorKind::RateLimited
        );
        let response = metrics_router(Arc::clone(&store))
            .oneshot(guarded_request(
                "GET",
                "/metrics",
                "",
                &[("host", "localhost")],
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 65536).await.unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        for line in [
            "swb_send_rate_limited_total{harness=\"codex\"} 1\n",
            "swb_send_rate_limited_total{harness=\"claude\"} 0\n",
            "swb_send_limit_burst 1\n",
            "swb_send_limit_refill_per_minute 6\n",
            "swb_send_buckets 1\n",
        ] {
            assert!(text.contains(line), "{line:?} not in {text}");
        }
        assert!(!text.contains("codex:host-b"), "{text}");
    }
}
