//! SQLite coordination state. Mutations and their receipts commit in one writer
//! transaction (SWB-R02, SWB-R09, SWB-R14, SWB-R46).
//!
//! Every time the store reads or writes comes from an injected [`Clock`]
//! (R-C262). Production uses [`SystemClock`], which reads the same Unix
//! seconds SQLite `unixepoch()` returned before the seam existed. Tests and
//! the spec live adapter use [`ManualClock`], which moves only when told to.
//!
//! v0.3.0 (R-C944): `register` returns a fresh end token (each register
//! invalidates the last) that only `end` accepts, `peers` lists live
//! sessions by default (bounded, paginated, one row per harness, host and
//! session id) and never returns `proc_start`, and retention
//! pruning runs on the broker's interval instead of inside `send`/`inbox`.
//!
//! v0.5.0 (ADR-0003, SWB-R66 to SWB-R68): opt-in sender binding. `register`
//! with `bind: true` also returns a session token. While the binding is live,
//! [`Store::authorize`] demands that token for the binding's agent id, a new
//! registration of the same (harness, host, session id) must present it, and
//! inbox delivery keys on (harness, host, session id) so a restarted service
//! keeps its unread mail. An unbound id behaves as in v0.4.0.

use hmac::{Hmac, Mac};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde_json::{Value, json};
use sha2::Sha256;
use std::fmt;
use std::path::Path;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use ulid::Ulid;

mod claims;
mod ratelimit;
pub use claims::{
    CLAIM_LEASE_DEFAULT_HOURS, CLAIM_LEASE_MAX_HOURS, CLAIMS_DEFAULT_LIMIT, CLAIMS_MAX_LIMIT,
    ClaimsQuery, NOTE_MAX_BYTES, SUBJECT_MAX_BYTES, THREAD_DEFAULT_LIMIT, THREAD_MAX_LIMIT,
    effective_claim_lease_seconds, valid_note, valid_subject,
};
pub use ratelimit::{SEND_BURST_DEFAULT, SEND_REFILL_PER_MINUTE_DEFAULT, SendLimit};

pub const RETENTION_ACKED_DAYS: u32 = 7;
pub const RETENTION_UNACKED_DAYS: u32 = 30;
pub const DEFAULT_TTL_HOURS: u32 = 72;
pub const MAX_TTL_DAYS: u32 = 14;
/// Lease length `register` reports, and the live band of `peers`.
pub const LEASE_SECONDS: i64 = 900;
/// Upper bound of the idle band; older non-ended sessions are gone.
pub const IDLE_MAX_AGE_SECONDS: i64 = 21_600;
/// `peers` page size when the caller names none, and the largest it accepts.
pub const PEERS_DEFAULT_LIMIT: u32 = 50;
pub const PEERS_MAX_LIMIT: u32 = 200;
/// Hex length of an end token (an HMAC-SHA256).
pub const END_TOKEN_HEX_LEN: usize = 64;
/// Hex length of a session token (an HMAC-SHA256, ADR-0003).
pub const SESSION_TOKEN_HEX_LEN: usize = 64;

pub fn effective_ttl_hours(requested: Option<u32>) -> u32 {
    requested
        .unwrap_or(DEFAULT_TTL_HOURS)
        .clamp(1, MAX_TTL_DAYS * 24)
}

/// What kind of failure a store operation hit. The broker maps each kind to
/// one HTTP status and a stable machine-readable `code` (R-C944).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// The request is malformed or names an invalid value.
    Invalid,
    /// `me` names no registered session.
    Unregistered,
    /// The message does not exist for this caller, or has expired.
    Unavailable,
    /// A `msg_id` was reused with a different sender or body (SWB-R46).
    Conflict,
    /// The claim or handoff does not exist for this caller, or is no longer
    /// active (SWB-R16, P2).
    ClaimUnavailable,
    /// The caller named a bound agent id without its valid session token
    /// (ADR-0003, SWB-R66).
    Unauthorized,
    /// The sender's send bucket is empty (ADR-0003, SWB-R66).
    RateLimited,
    /// The store itself failed; not the caller's fault.
    Internal,
}

impl ErrorKind {
    /// The stable `code` the REST and MCP error bodies carry.
    pub fn code(self) -> &'static str {
        match self {
            Self::Invalid => "invalid_argument",
            Self::Unregistered => "unregistered",
            Self::Unavailable => "message_unavailable",
            Self::Conflict => "msg_id_conflict",
            Self::ClaimUnavailable => "claim_unavailable",
            Self::Unauthorized => "unauthorized",
            Self::RateLimited => "rate_limited",
            Self::Internal => "internal",
        }
    }
}

/// A store failure: a kind plus a short fixed message that never contains a
/// message body or a stored receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreError {
    pub kind: ErrorKind,
    pub message: String,
    /// Seconds until a refused send may succeed, for
    /// [`ErrorKind::RateLimited`] with a bucket that refills (TIN-7577).
    pub retry_after: Option<u64>,
}

impl StoreError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            retry_after: None,
        }
    }
    /// The same error with `retry_after` set.
    pub fn with_retry_after(mut self, seconds: Option<u64>) -> Self {
        self.retry_after = seconds;
        self
    }
    pub fn code(&self) -> &'static str {
        self.kind.code()
    }
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for StoreError {}

/// A bare message is a validation failure; every other kind is explicit.
impl From<&str> for StoreError {
    fn from(message: &str) -> Self {
        Self::new(ErrorKind::Invalid, message)
    }
}

impl From<String> for StoreError {
    fn from(message: String) -> Self {
        Self::new(ErrorKind::Invalid, message)
    }
}

impl PartialEq<&str> for StoreError {
    fn eq(&self, other: &&str) -> bool {
        self.message == *other
    }
}

pub type StoreResult<T> = Result<T, StoreError>;

/// Source of the current time, in whole seconds since the Unix epoch
/// (R-C262). The store asks it once per operation and binds the answer into
/// its SQL, so no statement reads SQLite's own clock.
pub trait Clock: Send + Sync {
    fn now(&self) -> i64;
}

/// The wall clock. Truncated Unix seconds, the value SQLite `unixepoch()`
/// returns, so the store's production behaviour is unchanged by the seam.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64)
    }
}

/// A clock that stands still until [`ManualClock::advance`] moves it. It
/// never moves backwards. For tests and the spec live adapter only; no
/// production path constructs one.
#[derive(Debug)]
pub struct ManualClock(AtomicI64);

impl ManualClock {
    pub fn new(start: i64) -> Self {
        Self(AtomicI64::new(start))
    }
    /// Start at the current wall-clock second.
    pub fn starting_now() -> Self {
        Self::new(SystemClock.now())
    }
    /// Move forward by `seconds` and return the new time.
    pub fn advance(&self, seconds: u32) -> i64 {
        let step = i64::from(seconds);
        self.0.fetch_add(step, Ordering::SeqCst) + step
    }
}

impl Clock for ManualClock {
    fn now(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

impl<C: Clock + ?Sized> Clock for Arc<C> {
    fn now(&self) -> i64 {
        (**self).now()
    }
}

pub struct Store(Mutex<Connection>, Arc<dyn Clock>, Mutex<ratelimit::Limiter>);
#[derive(Debug)]
pub struct SendOutcome {
    pub envelope: Value,
    pub created: bool,
    /// Where mail to the recipient stands when the send commits: `live`,
    /// `idle`, `gone`, `ended` or `unregistered` (A5, TIN-7577). A send
    /// result carries it; the stored envelope never does.
    pub recipient_state: &'static str,
}

/// A `peers` request (R-C944). The default lists live sessions only, one
/// row per (harness, host, session id), at most [`PEERS_DEFAULT_LIMIT`] rows.
#[derive(Debug, Default, Clone, Copy)]
pub struct PeersQuery<'a> {
    /// A registered caller whose own lease this call refreshes first.
    pub me: Option<&'a str>,
    /// Include idle, gone and ended sessions too.
    pub all: bool,
    /// Only sessions on this host.
    pub host: Option<&'a str>,
    /// Page size; clamped to 1..=[`PEERS_MAX_LIMIT`].
    pub limit: Option<u32>,
    /// The `next_cursor` of the previous page.
    pub cursor: Option<&'a str>,
    /// Only registrations that used this `proc_start`, then one row per
    /// harness, host and session id among them (A4, TIN-7577). The caller
    /// supplies it; listings still never return it.
    pub proc_start: Option<&'a str>,
}

fn field<'a>(v: &'a Value, key: &str) -> StoreResult<&'a str> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("missing {key}").into())
}

/// The harness kinds an agent id may name, in the order the docs list them.
/// The store, the broker's `register` schema and the `swb` CLI all read this
/// one list, so a new kind cannot reach one of them and miss another.
///
/// - `kimi` is Claude Code running on a Kimi model (lab `kimi-claude.sh`);
/// - `kimicode` is the official Kimi Code CLI;
/// - `hermes` is a Hermes agent.
pub const HARNESS_KINDS: [&str; 8] = [
    "claude", "kimi", "codex", "junie", "opencode", "pi", "hermes", "kimicode",
];

/// Whether `harness` is one of [`HARNESS_KINDS`].
pub fn valid_harness(harness: &str) -> bool {
    HARNESS_KINDS.contains(&harness)
}

fn valid_part(p: &str) -> bool {
    !p.is_empty()
        && p.chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
}
fn valid_agent_id(id: &str) -> bool {
    if id.len() > 512 {
        return false;
    }
    let parts: Vec<_> = id.split(':').collect();
    parts.len() == 4
        && valid_harness(parts[0])
        && parts[2].parse::<u32>().is_ok()
        && parts[1..].iter().all(|p| valid_part(p))
}
fn valid_ulid(s: &str) -> bool {
    s.len() == 26 && s.parse::<Ulid>().is_ok_and(|id| id.to_string() == s)
}
fn valid_rfc3339_shape(s: &str) -> bool {
    let b = s.as_bytes();
    if !(20..=40).contains(&b.len())
        || b.get(4) != Some(&b'-')
        || b.get(7) != Some(&b'-')
        || b.get(10) != Some(&b'T')
        || b.get(13) != Some(&b':')
        || b.get(16) != Some(&b':')
        || [0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18]
            .iter()
            .any(|&i| !b[i].is_ascii_digit())
    {
        return false;
    }
    let mut suffix = &b[19..];
    if suffix.first() == Some(&b'.') {
        suffix = &suffix[1..];
        let digits = suffix.iter().take_while(|c| c.is_ascii_digit()).count();
        if digits == 0 {
            return false;
        }
        suffix = &suffix[digits..];
    }
    suffix == b"Z"
        || (suffix.len() == 6
            && matches!(suffix[0], b'+' | b'-')
            && suffix[1].is_ascii_digit()
            && suffix[2].is_ascii_digit()
            && suffix[3] == b':'
            && suffix[4].is_ascii_digit()
            && suffix[5].is_ascii_digit())
}
/// A single-line field with every control character removed. v0.5.0 and
/// older stored message bodies this way; a replay compares against it so a
/// stored body keeps matching across the upgrade (TIN-7577).
fn clean(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).collect()
}
/// A message body as the store keeps it (TIN-7577). Newline and tab stay,
/// so a multi-line body keeps its lines. Every carriage return goes: CRLF
/// becomes LF and a lone CR is dropped, so a CR cannot move a terminal's
/// cursor back over a line it already printed. Every other control
/// character goes too, as `clean` drops them.
fn clean_body(s: &str) -> String {
    s.chars()
        .filter(|&c| matches!(c, '\n' | '\t') || !c.is_control())
        .collect()
}
/// Whether a `--msg-id` retry carries the stored body, so it is a replay
/// (TIN-7577). A row that kept its lines (`body_lines`, written by v0.5.1
/// and later) matches the cleaned retry body exactly, so a retry that adds
/// or moves a newline, tab or CR is a conflict. A row from v0.5.0 or older
/// holds the body with every control character removed, so only such a row
/// also matches the retry in that flattened form.
fn same_body(old_body: &str, old_lines: bool, body: &str, raw_body: &str) -> bool {
    old_body == body
        || (!old_lines && !old_body.contains(['\n', '\t']) && old_body == clean(raw_body))
}
fn sql_err(e: rusqlite::Error) -> StoreError {
    StoreError::new(ErrorKind::Internal, e.to_string())
}
fn random_bytes<const N: usize>() -> StoreResult<[u8; N]> {
    let mut bytes = [0u8; N];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| std::io::Read::read_exact(&mut f, &mut bytes))
        .map_err(|e| StoreError::new(ErrorKind::Internal, format!("randomness: {e}")))?;
    Ok(bytes)
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| {
            text.get(i..i + 2)
                .filter(|pair| pair.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
                .and_then(|pair| u8::from_str_radix(pair, 16).ok())
        })
        .collect()
}
/// The session id is the fourth part of an agent id.
fn session_of(agent_id: &str) -> &str {
    agent_id.splitn(4, ':').nth(3).unwrap_or(agent_id)
}
/// The delivery key of an agent id: `harness:host:session_id`, the id
/// without its pid (ADR-0003). A bound registration owns its key, and its
/// inbox reads every message addressed to any agent id with that key.
pub fn delivery_key(agent_id: &str) -> String {
    match agent_id.splitn(4, ':').collect::<Vec<_>>()[..] {
        [harness, host, _, session] => format!("{harness}:{host}:{session}"),
        _ => agent_id.to_owned(),
    }
}
/// Renew `me`'s lease and clear `ended`, as every call by a registered
/// session does. A binding that already lapsed (its session ended, or it
/// went [`IDLE_MAX_AGE_SECONDS`] without a call) is marked unbound first, so
/// a later call, with or without a token, revives an unbound session and
/// never the binding (ADR-0003). Returns the rows renewed: 0 for an
/// unregistered `me`.
pub(crate) fn touch(db: &Connection, me: &str, now: i64) -> StoreResult<usize> {
    db.execute(
        "UPDATE sessions SET bound=0 WHERE agent_id=?1 AND bound=1 AND (ended=1 OR last_seen<?2)",
        params![me, now - IDLE_MAX_AGE_SECONDS],
    )
    .map_err(sql_err)?;
    db.execute(
        "UPDATE sessions SET last_seen=?2, ended=0 WHERE agent_id=?1",
        params![me, now],
    )
    .map_err(sql_err)
}
fn unauthorized(message: &str) -> StoreError {
    StoreError::new(ErrorKind::Unauthorized, message)
}
/// The latest registration of a delivery key while it is bound: not ended
/// and not gone (ADR-0003).
struct Binding {
    agent_id: String,
    nonce: Vec<u8>,
}
/// A key's latest session row: agent id, nonce, bound, ended, last_seen.
type BindingRow = (String, Option<Vec<u8>>, i64, i64, i64);
/// The latest session row of `agent_id`'s delivery key (harness, host and
/// session id), or `None` for a malformed id or a key with no row.
fn latest_key_row(db: &Connection, agent_id: &str) -> StoreResult<Option<BindingRow>> {
    let [harness, host, _, session] = match agent_id.splitn(4, ':').collect::<Vec<_>>()[..] {
        [a, b, c, d] => [a, b, c, d],
        _ => return Ok(None),
    };
    db.query_row(
        "SELECT agent_id, token_nonce, bound, ended, last_seen FROM sessions
         WHERE harness=?1 AND host=?2 AND session_id=?3
         ORDER BY registered_seq DESC, agent_id DESC LIMIT 1",
        params![harness, host, session],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
    )
    .optional()
    .map_err(sql_err)
}
/// Whether a key's latest row is a live binding: bound, not ended, with a
/// token nonce, and seen within [`IDLE_MAX_AGE_SECONDS`]. The inbox gate and
/// `recipient_state_of` share this one test, so a send never reports a
/// binding the inbox does not honour (TIN-7577).
fn is_live_binding(row: &BindingRow, now: i64) -> bool {
    matches!(row, (_, Some(_), 1, 0, seen) if *seen >= now - IDLE_MAX_AGE_SECONDS)
}
/// Where mail to `recipient` stands (A5, TIN-7577). A live binding of its
/// delivery key reads mail for every pid of that key, so the binding's own
/// lease counts. Otherwise the recipient's own row decides, and an id with
/// no row is `unregistered`. Read-only: it never marks a lapsed binding.
fn recipient_state_of(db: &Connection, recipient: &str, now: i64) -> StoreResult<&'static str> {
    if let Some(row) = latest_key_row(db, recipient)?
        && is_live_binding(&row, now)
    {
        return Ok(lease_state(row.4, false, now));
    }
    let row: Option<(i64, i64)> = db
        .query_row(
            "SELECT last_seen, ended FROM sessions WHERE agent_id=?1",
            [recipient],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(sql_err)?;
    Ok(row.map_or("unregistered", |(seen, ended)| {
        lease_state(seen, ended != 0, now)
    }))
}
fn lease_state(seen: i64, ended: bool, now: i64) -> &'static str {
    if ended {
        "ended"
    } else if seen >= now - LEASE_SECONDS {
        "live"
    } else if seen >= now - IDLE_MAX_AGE_SECONDS {
        "idle"
    } else {
        "gone"
    }
}

impl Store {
    /// The store's current time, in Unix seconds.
    pub fn now(&self) -> i64 {
        self.1.now()
    }
    fn db(&self) -> StoreResult<MutexGuard<'_, Connection>> {
        self.0
            .lock()
            .map_err(|_| StoreError::new(ErrorKind::Internal, "store lock poisoned"))
    }
    /// Retention (SWB-R09): acked rows after 7 days, every other row 30 days
    /// after creation. The broker runs this at startup and on its interval;
    /// `send` and `inbox` no longer do (R-C944).
    pub fn prune(&self) -> StoreResult<()> {
        let db = self.db()?;
        let now = self.now();
        db.execute("DELETE FROM messages WHERE (state='acked' AND acked_at<=?1-604800) OR (state!='acked' AND created_at<=?1-2592000)", [now]).map_err(sql_err)?;
        claims::prune(&db, now)?;
        Ok(())
    }
    /// A cheap read that proves the store answers (`/healthz`).
    pub fn health(&self) -> StoreResult<()> {
        let db = self.db()?;
        db.query_row("SELECT 1", [], |r| r.get::<_, i64>(0))
            .map_err(sql_err)?;
        Ok(())
    }
    pub fn open(path: impl AsRef<Path>) -> StoreResult<Self> {
        Self::open_with_clock(path, Arc::new(SystemClock))
    }
    pub fn open_with_clock(path: impl AsRef<Path>, clock: Arc<dyn Clock>) -> StoreResult<Self> {
        let db = Connection::open(path).map_err(sql_err)?;
        let store = Self::init(db, clock)?;
        store.prune()?;
        Ok(store)
    }
    pub fn memory() -> StoreResult<Self> {
        Self::memory_with_clock(Arc::new(SystemClock))
    }
    pub fn memory_with_clock(clock: Arc<dyn Clock>) -> StoreResult<Self> {
        Self::init(Connection::open_in_memory().map_err(sql_err)?, clock)
    }
    fn init(db: Connection, clock: Arc<dyn Clock>) -> StoreResult<Self> {
        db.busy_timeout(Duration::from_secs(5)).map_err(sql_err)?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;
            CREATE TABLE IF NOT EXISTS sessions (
              agent_id TEXT PRIMARY KEY, harness TEXT NOT NULL, host TEXT NOT NULL,
              proc_start TEXT NOT NULL, last_seen INTEGER NOT NULL, ended INTEGER NOT NULL DEFAULT 0);
            CREATE TABLE IF NOT EXISTS messages (
              msg_id TEXT PRIMARY KEY, thread_id TEXT NOT NULL, seq INTEGER NOT NULL,
              sender TEXT NOT NULL, recipient TEXT NOT NULL, body TEXT NOT NULL,
              envelope TEXT NOT NULL, state TEXT NOT NULL DEFAULT 'queued',
              delivery_count INTEGER NOT NULL DEFAULT 0, created_at INTEGER NOT NULL,
              expires_at INTEGER NOT NULL, acked_at INTEGER,
              UNIQUE(thread_id,seq));
            CREATE INDEX IF NOT EXISTS messages_inbox ON messages(recipient,state,created_at);
            CREATE TABLE IF NOT EXISTS counters (name TEXT PRIMARY KEY, value INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS thread_counters (thread_id TEXT PRIMARY KEY, high_water INTEGER NOT NULL);
            INSERT INTO thread_counters(thread_id, high_water)
              SELECT thread_id, MAX(seq) FROM messages GROUP BY thread_id
              ON CONFLICT(thread_id) DO UPDATE SET high_water=MAX(high_water, excluded.high_water);
            CREATE TABLE IF NOT EXISTS settings (name TEXT PRIMARY KEY, value BLOB NOT NULL);").map_err(sql_err)?;
        // v0.3.0 (R-C944) session columns, added in place to a v0.2 store:
        // the session id `peers` deduplicates on, the registration sequence
        // that decides which row of a session is latest, and the nonce the
        // end token is derived from. A v0.2 row has no nonce, so it cannot be
        // ended; it decays to gone instead.
        let columns: Vec<String> = db
            .prepare("SELECT name FROM pragma_table_info('sessions')")
            .and_then(|mut stmt| {
                stmt.query_map([], |r| r.get::<_, String>(0))?
                    .collect::<Result<_, _>>()
            })
            .map_err(sql_err)?;
        for (column, ddl) in [
            (
                "session_id",
                "ALTER TABLE sessions ADD COLUMN session_id TEXT",
            ),
            (
                "registered_seq",
                "ALTER TABLE sessions ADD COLUMN registered_seq INTEGER NOT NULL DEFAULT 0",
            ),
            (
                "token_nonce",
                "ALTER TABLE sessions ADD COLUMN token_nonce BLOB",
            ),
            // v0.5.0 (ADR-0003): whether this registration asked for, or
            // kept, a binding of its (harness, host, session id).
            (
                "bound",
                "ALTER TABLE sessions ADD COLUMN bound INTEGER NOT NULL DEFAULT 0",
            ),
        ] {
            if !columns.iter().any(|c| c == column) {
                db.execute(ddl, []).map_err(sql_err)?;
            }
        }
        let unfilled: Vec<String> = db
            .prepare("SELECT agent_id FROM sessions WHERE session_id IS NULL")
            .and_then(|mut stmt| {
                stmt.query_map([], |r| r.get::<_, String>(0))?
                    .collect::<Result<_, _>>()
            })
            .map_err(sql_err)?;
        for id in unfilled {
            db.execute(
                "UPDATE sessions SET session_id=?2 WHERE agent_id=?1",
                params![id, session_of(&id)],
            )
            .map_err(sql_err)?;
        }
        db.execute_batch(
            "CREATE INDEX IF NOT EXISTS sessions_by_session ON sessions(session_id, registered_seq);",
        )
        .map_err(sql_err)?;
        // v0.5.0 (ADR-0003): each message's delivery key, so a bound
        // service's inbox survives a restart that changes its pid. Rows from
        // an older store are filled in place.
        let has_key = db
            .prepare("SELECT name FROM pragma_table_info('messages')")
            .and_then(|mut stmt| {
                stmt.query_map([], |r| r.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()
            })
            .map_err(sql_err)?
            .iter()
            .any(|c| c == "recipient_key");
        if !has_key {
            db.execute("ALTER TABLE messages ADD COLUMN recipient_key TEXT", [])
                .map_err(sql_err)?;
        }
        let unkeyed: Vec<(String, String)> = db
            .prepare("SELECT msg_id, recipient FROM messages WHERE recipient_key IS NULL")
            .and_then(|mut stmt| {
                stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect::<Result<_, _>>()
            })
            .map_err(sql_err)?;
        for (msg_id, recipient) in unkeyed {
            db.execute(
                "UPDATE messages SET recipient_key=?2 WHERE msg_id=?1",
                params![msg_id, delivery_key(&recipient)],
            )
            .map_err(sql_err)?;
        }
        db.execute_batch(
            "CREATE INDEX IF NOT EXISTS sessions_by_key ON sessions(harness, host, session_id, registered_seq);
             CREATE INDEX IF NOT EXISTS messages_by_key ON messages(recipient_key, state, created_at);",
        )
        .map_err(sql_err)?;
        // v0.5.1 (TIN-7577): each message's arrival position, from a
        // store-wide counter that only grows. An inbox cursor compares it, so
        // a later send always sorts after the cursor, whatever its msg_id or
        // clock second. Rows without one (an older store, or rows a v0.5.0
        // broker wrote after a downgrade) are filled in send order, after
        // every position already handed out.
        let has_arrival = db
            .prepare("SELECT name FROM pragma_table_info('messages')")
            .and_then(|mut stmt| {
                stmt.query_map([], |r| r.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()
            })
            .map_err(sql_err)?
            .iter()
            .any(|c| c == "arrival_seq");
        if !has_arrival {
            db.execute("ALTER TABLE messages ADD COLUMN arrival_seq INTEGER", [])
                .map_err(sql_err)?;
        }
        // One transaction for the backfill and the counter, so a large store
        // syncs once at startup, not once per row. A crash before the commit
        // leaves every row unfilled, and the next start fills them again.
        let fill = db.unchecked_transaction().map_err(sql_err)?;
        let base: i64 = fill
            .query_row(
                "SELECT MAX(COALESCE((SELECT MAX(arrival_seq) FROM messages),0),
                            COALESCE((SELECT value FROM counters WHERE name='arrival_seq'),0))",
                [],
                |r| r.get(0),
            )
            .map_err(sql_err)?;
        let unordered: Vec<String> = fill
            .prepare(
                "SELECT msg_id FROM messages WHERE arrival_seq IS NULL ORDER BY created_at, seq, rowid",
            )
            .and_then(|mut stmt| {
                stmt.query_map([], |r| r.get::<_, String>(0))?
                    .collect::<Result<_, _>>()
            })
            .map_err(sql_err)?;
        for (offset, msg_id) in (1i64..).zip(&unordered) {
            fill.execute(
                "UPDATE messages SET arrival_seq=?2 WHERE msg_id=?1",
                params![msg_id, base + offset],
            )
            .map_err(sql_err)?;
        }
        fill.execute(
            "INSERT INTO counters(name,value) VALUES('arrival_seq',?1)
             ON CONFLICT(name) DO UPDATE SET value=MAX(value, excluded.value)",
            [base + unordered.len() as i64],
        )
        .map_err(sql_err)?;
        fill.commit().map_err(sql_err)?;
        // v0.5.1 (TIN-7577): whether the stored body kept its newlines and
        // tabs. A v0.5.1 send writes 1. Every older row reads 0: the column
        // is added with that default, and a v0.5.0 broker running on this
        // store after a downgrade names its columns and leaves it at 0. Only
        // a row at 0 can match a retry in the flattened form.
        let has_lines = db
            .prepare("SELECT name FROM pragma_table_info('messages')")
            .and_then(|mut stmt| {
                stmt.query_map([], |r| r.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()
            })
            .map_err(sql_err)?
            .iter()
            .any(|c| c == "body_lines");
        if !has_lines {
            db.execute(
                "ALTER TABLE messages ADD COLUMN body_lines INTEGER NOT NULL DEFAULT 0",
                [],
            )
            .map_err(sql_err)?;
        }
        db.execute_batch(
            "CREATE INDEX IF NOT EXISTS messages_inbox_arrival ON messages(recipient, state, arrival_seq);
             CREATE INDEX IF NOT EXISTS messages_key_arrival ON messages(recipient_key, state, arrival_seq);",
        )
        .map_err(sql_err)?;
        // P2 (SWB-R16, R-C950): the claim and handoff tables.
        claims::migrate(&db)?;
        for name in ["audit_hmac_key", "end_token_key", "session_token_key"] {
            let key = random_bytes::<32>()?;
            db.execute(
                "INSERT OR IGNORE INTO settings(name,value) VALUES(?1,?2)",
                params![name, &key[..]],
            )
            .map_err(sql_err)?;
        }
        Ok(Self(
            Mutex::new(db),
            clock,
            Mutex::new(ratelimit::Limiter::default()),
        ))
    }
    fn keyed_mac(db: &Connection, key_name: &str) -> StoreResult<Hmac<Sha256>> {
        let key: Vec<u8> = db
            .query_row(
                "SELECT value FROM settings WHERE name=?1",
                [key_name],
                |r| r.get(0),
            )
            .map_err(sql_err)?;
        Hmac::<Sha256>::new_from_slice(&key)
            .map_err(|e| StoreError::new(ErrorKind::Internal, e.to_string()))
    }
    pub fn body_hmac(&self, body: &str) -> StoreResult<String> {
        let db = self.db()?;
        let mut mac = Self::keyed_mac(&db, "audit_hmac_key")?;
        mac.update(body.as_bytes());
        Ok(hex(&mac.finalize().into_bytes()))
    }
    /// The end token of one registration: HMAC(end_token_key, agent id,
    /// nonce). Nothing secret is stored beside the key, and a new nonce
    /// (new session row, new `proc_start`, or a register after `end`)
    /// invalidates every earlier token.
    fn end_token_mac(db: &Connection, agent_id: &str, nonce: &[u8]) -> StoreResult<Hmac<Sha256>> {
        let mut mac = Self::keyed_mac(db, "end_token_key")?;
        mac.update(b"swb-end-token-v1\0");
        mac.update(agent_id.as_bytes());
        mac.update(b"\0");
        mac.update(nonce);
        Ok(mac)
    }
    /// The session token of one bound registration: HMAC(session_token_key,
    /// agent id, nonce) under its own key and label, so it is never equal to
    /// the end token of the same registration (ADR-0003).
    fn session_token_mac(
        db: &Connection,
        agent_id: &str,
        nonce: &[u8],
    ) -> StoreResult<Hmac<Sha256>> {
        let mut mac = Self::keyed_mac(db, "session_token_key")?;
        mac.update(b"swb-session-token-v1\0");
        mac.update(agent_id.as_bytes());
        mac.update(b"\0");
        mac.update(nonce);
        Ok(mac)
    }
    /// The live binding of `agent_id`'s delivery key, if any: the latest
    /// registration of that (harness, host, session id) asked for a binding,
    /// is not ended and is not gone (ADR-0003). A binding lapses when its
    /// session ends or after [`IDLE_MAX_AGE_SECONDS`] without a call, and a
    /// lapse is permanent: the row is marked unbound here, so no later call
    /// can bring the binding or its old token back. Only a new `register`
    /// with `bind: true` binds the key again, under a new token.
    fn live_binding(db: &Connection, agent_id: &str, now: i64) -> StoreResult<Option<Binding>> {
        Ok(match latest_key_row(db, agent_id)? {
            Some(row) if is_live_binding(&row, now) => {
                let (agent_id, nonce, ..) = row;
                nonce.map(|nonce| Binding { agent_id, nonce })
            }
            Some((agent_id, _, 1, _, _)) => {
                db.execute("UPDATE sessions SET bound=0 WHERE agent_id=?1", [&agent_id])
                    .map_err(sql_err)?;
                None
            }
            _ => None,
        })
    }
    /// Whether `presented` is the session token of `binding`.
    fn token_matches(db: &Connection, binding: &Binding, presented: &str) -> StoreResult<bool> {
        if presented.len() != SESSION_TOKEN_HEX_LEN {
            return Ok(false);
        }
        let Some(bytes) = unhex(presented) else {
            return Ok(false);
        };
        Ok(
            Self::session_token_mac(db, &binding.agent_id, &binding.nonce)?
                .verify_slice(&bytes)
                .is_ok(),
        )
    }
    /// Sender binding (ADR-0003, SWB-R66): may a request act as `agent_id`?
    ///
    /// An id whose delivery key has no live binding needs nothing, as in
    /// v0.4.0, and a token it presents is ignored. While the key is bound,
    /// only the binding's own agent id may act, and only with the session
    /// token its latest registration returned. Every refusal is
    /// [`ErrorKind::Unauthorized`]. An invalid id passes here so the
    /// operation itself reports it. [`Store::send_as`] (for `from`),
    /// [`Store::inbox_as`], [`Store::ack_as`], [`Store::claim_as`],
    /// [`Store::release_as`], [`Store::handoff_as`] and [`Store::end_as`]
    /// (for `me`) run this check inside their own transaction.
    pub fn authorize(&self, agent_id: &str, session_token: Option<&str>) -> StoreResult<()> {
        let db = self.db()?;
        Self::gate(&db, agent_id, session_token, self.now()).map(|_| ())
    }
    /// [`Store::authorize`] on `db`, inside the caller's transaction, so the
    /// check and the operation it guards see the same binding (ADR-0003).
    /// `Ok(true)` means `agent_id` holds a live binding and presented its
    /// token; `Ok(false)` means its key is unbound.
    fn gate(
        db: &Connection,
        agent_id: &str,
        session_token: Option<&str>,
        now: i64,
    ) -> StoreResult<bool> {
        if !valid_agent_id(agent_id) {
            return Ok(false);
        }
        let Some(binding) = Self::live_binding(db, agent_id, now)? else {
            return Ok(false);
        };
        let Some(token) = session_token else {
            return Err(unauthorized("session token required"));
        };
        if binding.agent_id != agent_id {
            return Err(unauthorized(
                "agent id superseded by a bound registration of its session",
            ));
        }
        if !Self::token_matches(db, &binding, token)? {
            return Err(unauthorized("invalid session token"));
        }
        Ok(true)
    }
    /// Set the per-sender send limit (ADR-0003). The broker reads it from its
    /// environment at startup; `burst` 0 turns the limit off.
    pub fn set_send_limit(&self, limit: SendLimit) -> StoreResult<()> {
        let mut limiter = self
            .2
            .lock()
            .map_err(|_| StoreError::new(ErrorKind::Internal, "limiter lock poisoned"))?;
        limiter.limit = limit;
        Ok(())
    }
    /// Take one send from `sender`'s token bucket, or answer
    /// [`ErrorKind::RateLimited`] with the seconds until the next one, in
    /// the message and in `retry_after`. A refusal is counted by the
    /// sender's harness kind (TIN-7577). An invalid sender passes, so `send`
    /// itself reports it.
    pub fn admit_send(&self, sender: &str) -> StoreResult<()> {
        if !valid_agent_id(sender) {
            return Ok(());
        }
        let now = self.now();
        let mut limiter = self
            .2
            .lock()
            .map_err(|_| StoreError::new(ErrorKind::Internal, "limiter lock poisoned"))?;
        match limiter.admit(sender, now) {
            Ok(()) => Ok(()),
            Err(wait) => {
                let kind = sender.split(':').next().unwrap_or_default();
                if let Some(i) = HARNESS_KINDS.iter().position(|k| *k == kind) {
                    limiter.refused[i] = limiter.refused[i].saturating_add(1);
                }
                let message = if wait == u64::MAX {
                    "rate limited".to_owned()
                } else {
                    format!("rate limited; retry in {wait} s")
                };
                Err(StoreError::new(ErrorKind::RateLimited, message)
                    .with_retry_after((wait != u64::MAX).then_some(wait)))
            }
        }
    }
    /// Register (or re-register) a self-asserted session and return its
    /// agent id, lease and end token (R-C944). Every registration mints a
    /// fresh token and invalidates every earlier one, so knowing a row's
    /// `proc_start` never retrieves the token its registrant holds. A hook
    /// that registers on every prompt saves the newest token each time.
    ///
    /// Sender binding (ADR-0003, SWB-R66, SWB-R67): `bind: true` binds the
    /// session's (harness, host, session id) and the result adds
    /// `session_token`. While that key is bound, a registration of it (any
    /// pid, with or without `bind`) must present the current token as
    /// `session_token`, keeps the binding and rotates the token. Every
    /// result says `bound`.
    pub fn register(&self, input: &Value) -> StoreResult<Value> {
        let harness = field(input, "harness")?;
        if !valid_harness(harness) {
            return Err("invalid harness".into());
        }
        let host = field(input, "host")?;
        let pid = input
            .get("pid")
            .and_then(Value::as_u64)
            .filter(|p| *p > 0)
            .ok_or("invalid pid")?;
        let session_id = field(input, "session_id")?;
        let proc_start = field(input, "proc_start")?;
        if proc_start.len() > 256 || proc_start.chars().any(char::is_control) {
            return Err("invalid proc_start".into());
        }
        let id = format!("{harness}:{host}:{pid}:{session_id}");
        if !valid_agent_id(&id) {
            return Err("invalid agent id".into());
        }
        let bind = match input.get("bind") {
            None | Some(Value::Null) => false,
            Some(Value::Bool(b)) => *b,
            Some(_) => return Err("invalid bind".into()),
        };
        let presented = match input.get("session_token") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) => Some(s.as_str()),
            Some(_) => return Err("invalid session_token".into()),
        };
        let mut db = self.db()?;
        let now = self.now();
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_err)?;
        let held = Self::live_binding(&tx, &id, now)?;
        if let Some(binding) = &held {
            let Some(token) = presented else {
                return Err(unauthorized("session token required"));
            };
            if !Self::token_matches(&tx, binding, token)? {
                return Err(unauthorized("invalid session token"));
            }
        }
        let bound = bind || held.is_some();
        let nonce = random_bytes::<16>()?.to_vec();
        tx.execute("INSERT INTO counters(name,value) VALUES('registrations',1) ON CONFLICT(name) DO UPDATE SET value=value+1",[]).map_err(sql_err)?;
        let seq: i64 = tx
            .query_row(
                "SELECT value FROM counters WHERE name='registrations'",
                [],
                |r| r.get(0),
            )
            .map_err(sql_err)?;
        tx.execute("INSERT INTO sessions(agent_id,harness,host,proc_start,last_seen,session_id,registered_seq,token_nonce,bound) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)
                    ON CONFLICT(agent_id) DO UPDATE SET proc_start=excluded.proc_start,last_seen=excluded.last_seen,ended=0,
                      session_id=excluded.session_id,registered_seq=excluded.registered_seq,token_nonce=excluded.token_nonce,
                      bound=excluded.bound",
            params![id,harness,host,proc_start,now,session_id,seq,nonce,bound]).map_err(sql_err)?;
        // The newest registration of a key is the only row that may carry its
        // binding, so an earlier pid's row can never become bound again.
        tx.execute(
            "UPDATE sessions SET bound=0 WHERE harness=?1 AND host=?2 AND session_id=?3 AND agent_id!=?4 AND bound=1",
            params![harness, host, session_id, id],
        )
        .map_err(sql_err)?;
        let token = hex(&Self::end_token_mac(&tx, &id, &nonce)?
            .finalize()
            .into_bytes());
        let mut result =
            json!({"agent_id":id,"lease_seconds":LEASE_SECONDS,"end_token":token,"bound":bound});
        if bound {
            result["session_token"] = json!(hex(&Self::session_token_mac(&tx, &id, &nonce)?
                .finalize()
                .into_bytes()));
        }
        tx.commit().map_err(sql_err)?;
        Ok(result)
    }
    /// End a session. Only the end token its latest registration returned
    /// ends it; any other token, or an unknown session, answers `unknown`
    /// without saying which (R-C944). Ending a session releases every claim
    /// it holds in the same transaction and lists them in
    /// `released_claims` (R-C950).
    pub fn end(&self, me: &str, end_token: &str) -> StoreResult<Value> {
        self.end_as(me, end_token, None)
    }
    /// [`Store::end`] acting as `me` with its session token. Ending a bound
    /// session lapses its binding for good (ADR-0003).
    pub fn end_as(
        &self,
        me: &str,
        end_token: &str,
        session_token: Option<&str>,
    ) -> StoreResult<Value> {
        if !valid_agent_id(me) {
            return Err("invalid me".into());
        }
        if end_token.len() != END_TOKEN_HEX_LEN {
            return Err("invalid end_token".into());
        }
        let presented = unhex(end_token).ok_or("invalid end_token")?;
        let mut db = self.db()?;
        let now = self.now();
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_err)?;
        Self::gate(&tx, me, session_token, now)?;
        let nonce: Option<Option<Vec<u8>>> = tx
            .query_row(
                "SELECT token_nonce FROM sessions WHERE agent_id=?1",
                [me],
                |r| r.get(0),
            )
            .optional()
            .map_err(sql_err)?;
        let matches = match nonce.flatten() {
            Some(nonce) => Self::end_token_mac(&tx, me, &nonce)?
                .verify_slice(&presented)
                .is_ok(),
            None => false,
        };
        if !matches {
            return Ok(json!({"agent_id":me,"state":"unknown"}));
        }
        tx.execute(
            "UPDATE sessions SET ended=1, bound=0 WHERE agent_id=?1",
            [me],
        )
        .map_err(sql_err)?;
        let released = claims::release_all_held(&tx, me, now)?;
        tx.commit().map_err(sql_err)?;
        Ok(json!({"agent_id":me,"state":"ended","released_claims":released}))
    }
    /// List sessions (R-C944): live only unless `all`, one row per
    /// (harness, host, session id) (the latest registration wins), ordered
    /// by agent id, paginated by `cursor`. Rows never carry `proc_start`; a
    /// caller may filter on one it already knows. A registered `me`
    /// refreshes its own lease first; listing alone refreshes nothing.
    pub fn peers(&self, query: &PeersQuery) -> StoreResult<Value> {
        if let Some(host) = query.host
            && !valid_part(host)
        {
            return Err("invalid host".into());
        }
        if let Some(cursor) = query.cursor
            && !valid_agent_id(cursor)
        {
            return Err("invalid cursor".into());
        }
        if let Some(proc_start) = query.proc_start
            && (proc_start.is_empty()
                || proc_start.len() > 256
                || proc_start.chars().any(char::is_control))
        {
            return Err("invalid proc_start".into());
        }
        let limit = query
            .limit
            .unwrap_or(PEERS_DEFAULT_LIMIT)
            .clamp(1, PEERS_MAX_LIMIT);
        let db = self.db()?;
        let now = self.now();
        if let Some(me) = query.me {
            if !valid_agent_id(me) {
                return Err("invalid me".into());
            }
            if touch(&db, me, now)? == 0 {
                return Err(StoreError::new(ErrorKind::Unregistered, "unregistered me"));
            }
        }
        let mut stmt = db
            .prepare(
                // A4 (TIN-7577): the proc_start filter runs before the
                // one-row-per-session dedupe, so two live processes of one
                // session each find their own row.
                "WITH latest AS (
                   SELECT agent_id,harness,host,last_seen,ended,bound,
                     ROW_NUMBER() OVER (PARTITION BY harness,host,COALESCE(session_id,agent_id)
                                        ORDER BY registered_seq DESC, agent_id DESC) AS pos
                   FROM sessions WHERE (?7 IS NULL OR proc_start=?7))
                 SELECT agent_id,harness,host,last_seen,ended,bound FROM latest
                 WHERE pos=1 AND (?1 IS NULL OR host=?1) AND (?2 IS NULL OR agent_id>?2)
                   AND (?3 OR (ended=0 AND last_seen>=?4-?5))
                 ORDER BY agent_id LIMIT ?6",
            )
            .map_err(sql_err)?;
        let rows = stmt
            .query_map(
                params![
                    query.host,
                    query.cursor,
                    query.all,
                    now,
                    LEASE_SECONDS,
                    i64::from(limit) + 1,
                    query.proc_start
                ],
                |r| {
                    let seen: i64 = r.get(3)?;
                    let ended: i64 = r.get(4)?;
                    let state = lease_state(seen, ended != 0, now);
                    // SWB-R68: a row is bound while its binding is live.
                    let bound = r.get::<_, i64>(5)? == 1 && matches!(state, "live" | "idle");
                    Ok(
                        json!({"agent_id":r.get::<_,String>(0)?,"harness":r.get::<_,String>(1)?,
                        "host":r.get::<_,String>(2)?,"last_seen":seen,
                        "state":state,"bound":bound}),
                    )
                },
            )
            .map_err(sql_err)?;
        let mut peers = rows.collect::<Result<Vec<_>, _>>().map_err(sql_err)?;
        let next_cursor = if peers.len() > limit as usize {
            peers.truncate(limit as usize);
            peers.last().map(|row| row["agent_id"].clone())
        } else {
            None
        };
        Ok(json!({"peers":peers,"next_cursor":next_cursor}))
    }
    pub fn send(&self, input: &Value) -> StoreResult<SendOutcome> {
        self.send_as(input, None)
    }
    /// [`Store::send`] acting as `from` with its session token (ADR-0003).
    /// The binding check runs inside the send's transaction. Only a send
    /// that stores a new message takes a token from the sender's bucket: a
    /// refused, invalid or replayed (SWB-R46) send costs nothing.
    pub fn send_as(&self, input: &Value, session_token: Option<&str>) -> StoreResult<SendOutcome> {
        let sender = field(input, "from")?;
        let recipient = field(input, "to")?;
        if !valid_agent_id(sender) || !valid_agent_id(recipient) {
            return Err("invalid agent id".into());
        }
        let raw_body = input
            .get("body")
            .and_then(Value::as_str)
            .ok_or("missing body")?;
        let body = clean_body(raw_body);
        if body.len() > 16384 {
            return Err("body exceeds 16 KiB".into());
        }
        let ticket = field(input, "ticket")?;
        if ticket.len() > 64
            || ticket != "none"
                && (!ticket.starts_with("TIN-")
                    || !ticket[4..].chars().all(|c| c.is_ascii_digit())
                    || ticket.len() == 4)
        {
            return Err("invalid ticket".into());
        }
        let directed = input
            .get("operator_directed")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if input
            .get("operator_directed")
            .is_some_and(|v| !v.is_boolean())
        {
            return Err("invalid operator_directed".into());
        }
        if directed
            && input
                .get("ruling")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .is_none()
        {
            return Err("operator_directed requires ruling".into());
        }
        if input
            .get("artifacts")
            .and_then(Value::as_array)
            .is_some_and(|v| v.len() > 20)
        {
            return Err("too many artifacts".into());
        }
        if let Some(artifacts) = input.get("artifacts") {
            let items = artifacts.as_array().ok_or("invalid artifacts")?;
            let mut seen = std::collections::HashSet::new();
            for item in items {
                let reference = item
                    .as_str()
                    .filter(|s| !s.is_empty() && s.len() <= 2048)
                    .ok_or("invalid artifact reference")?;
                if !seen.insert(reference) {
                    return Err("duplicate artifact reference".into());
                }
            }
        }
        let input_map = input.as_object().ok_or("expected object")?;
        const ALLOWED: &[&str] = &[
            "from",
            "to",
            "ticket",
            "body",
            "msg_id",
            "thread_id",
            "in_reply_to",
            "operator_directed",
            "ruling",
            "reply_expires",
            "reply_format",
            "artifacts",
            "ttl_hours",
            "authority",
            "v",
        ];
        if input_map.keys().any(|key| !ALLOWED.contains(&key.as_str())) {
            return Err("unknown envelope field".into());
        }
        for key in ["msg_id", "thread_id", "reply_expires"] {
            if input.get(key).is_some_and(|value| !value.is_string()) {
                return Err(format!("invalid {key}").into());
            }
        }
        if input.get("ttl_hours").is_some_and(|v| !v.is_u64()) {
            return Err("invalid ttl_hours".into());
        }
        if input
            .get("in_reply_to")
            .is_some_and(|v| !v.as_str().is_some_and(valid_ulid))
        {
            return Err("invalid in_reply_to".into());
        }
        for key in ["ruling", "reply_format"] {
            if input.get(key).is_some_and(|v| {
                !v.as_str().is_some_and(|s| {
                    (key != "ruling" || !s.is_empty())
                        && s.len() <= 512
                        && !s.chars().any(char::is_control)
                })
            }) {
                return Err(format!("invalid {key}").into());
            }
        }
        let msg_id = input
            .get("msg_id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| Ulid::new().to_string());
        let thread_id = input
            .get("thread_id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| msg_id.clone());
        if !valid_ulid(&msg_id) || !valid_ulid(&thread_id) {
            return Err("invalid ULID".into());
        }
        let ttl = effective_ttl_hours(
            input
                .get("ttl_hours")
                .and_then(Value::as_u64)
                .map(|n| n.min(u32::MAX as u64) as u32),
        );
        let mut db = self.db()?;
        let now = self.now();
        if let Some(value) = input.get("reply_expires") {
            let stamp = value
                .as_str()
                .filter(|s| valid_rfc3339_shape(s))
                .ok_or("invalid reply_expires")?;
            let parsed: Option<i64> = db
                .query_row("SELECT unixepoch(?1)", [stamp], |row| row.get(0))
                .map_err(sql_err)?;
            if parsed.is_none() {
                return Err("invalid reply_expires".into());
            }
        }
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_err)?;
        Self::gate(&tx, sender, session_token, now)?;
        let old: Option<(String, String, String, String, bool)> = tx
            .query_row(
                "SELECT sender,body,envelope,recipient,body_lines FROM messages WHERE msg_id=?1",
                [&msg_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .optional()
            .map_err(sql_err)?;
        if let Some((old_sender, old_body, envelope, old_recipient, old_lines)) = old {
            // SWB-R46: disclose neither the stored receipt nor its metadata on conflict.
            if old_sender != sender || !same_body(&old_body, old_lines, &body, raw_body) {
                return Err(StoreError::new(ErrorKind::Conflict, "msg_id conflict"));
            }
            let envelope = serde_json::from_str(&envelope)
                .map_err(|e| StoreError::new(ErrorKind::Internal, e.to_string()))?;
            touch(&tx, sender, now)?;
            // The stored recipient, not this request's `to`: a replay answers
            // for the envelope it returns, so it cannot probe another id
            // without spending a bucket token (TIN-7577).
            let recipient_state = recipient_state_of(&tx, &old_recipient, now)?;
            tx.commit().map_err(sql_err)?;
            return Ok(SendOutcome {
                envelope,
                created: false,
                recipient_state,
            });
        }
        self.admit_send(sender)?;
        tx.execute(
            "INSERT INTO thread_counters(thread_id,high_water) VALUES(?1,1)
             ON CONFLICT(thread_id) DO UPDATE SET high_water=high_water+1",
            [&thread_id],
        )
        .map_err(sql_err)?;
        let seq: i64 = tx
            .query_row(
                "SELECT high_water FROM thread_counters WHERE thread_id=?1",
                [&thread_id],
                |r| r.get(0),
            )
            .map_err(sql_err)?;
        let mut envelope = json!({
            "from":sender,"to":recipient,"ticket":ticket,"body":body
        });
        let map = envelope.as_object_mut().ok_or("expected object")?;
        for key in [
            "in_reply_to",
            "ruling",
            "reply_expires",
            "reply_format",
            "artifacts",
        ] {
            if let Some(value) = input.get(key) {
                map.insert(key.into(), value.clone());
            }
        }
        map.insert("v".into(), json!(3));
        map.insert("msg_id".into(), json!(msg_id));
        map.insert("thread_id".into(), json!(thread_id));
        map.insert("seq".into(), json!(seq));
        map.insert("authority".into(), json!("peer"));
        map.insert("transport".into(), json!("broker"));
        map.insert("body".into(), json!(body));
        map.insert("operator_directed".into(), json!(directed));
        map.insert("reply_to".into(), json!(format!("ag:{sender}")));
        map.remove("ttl_hours");
        let sent_at: String = tx
            .query_row(
                "SELECT strftime('%Y-%m-%dT%H:%M:%SZ',?1,'unixepoch')",
                [now],
                |r| r.get(0),
            )
            .map_err(sql_err)?;
        map.insert("sent_at".into(), json!(sent_at));
        let expires_at: String = tx
            .query_row(
                "SELECT strftime('%Y-%m-%dT%H:%M:%SZ',?1,'unixepoch',?2)",
                params![now, format!("+{} seconds", i64::from(ttl) * 3600)],
                |r| r.get(0),
            )
            .map_err(sql_err)?;
        map.insert("expires_at".into(), json!(expires_at));
        // v0.5.1 (TIN-7577): the message's arrival position for inbox paging.
        tx.execute("INSERT INTO counters(name,value) VALUES('arrival_seq',1) ON CONFLICT(name) DO UPDATE SET value=value+1",[]).map_err(sql_err)?;
        let arrival: i64 = tx
            .query_row(
                "SELECT value FROM counters WHERE name='arrival_seq'",
                [],
                |r| r.get(0),
            )
            .map_err(sql_err)?;
        tx.execute("INSERT INTO messages(msg_id,thread_id,seq,sender,recipient,body,envelope,created_at,expires_at,recipient_key,arrival_seq,body_lines)
                    VALUES(?1,?2,?3,?4,?5,?6,?7,?9,?9+?8,?10,?11,1)",
            params![msg_id,thread_id,seq,sender,recipient,body,envelope.to_string(),i64::from(ttl)*3600,now,delivery_key(recipient),arrival]).map_err(sql_err)?;
        tx.execute("INSERT INTO counters(name,value) VALUES('messages_total',1) ON CONFLICT(name) DO UPDATE SET value=value+1",[]).map_err(sql_err)?;
        touch(&tx, sender, now)?;
        let recipient_state = recipient_state_of(&tx, recipient, now)?;
        tx.commit().map_err(sql_err)?;
        Ok(SendOutcome {
            envelope,
            created: true,
            recipient_state,
        })
    }
    /// Fetch `me`'s unacknowledged messages, as an unbound caller (no
    /// session token). See [`Store::inbox_as`].
    pub fn inbox(&self, me: &str, limit: u32) -> StoreResult<Value> {
        self.inbox_as(me, limit, None)
    }
    /// Fetch `me`'s unacknowledged messages. A bound agent id that presents
    /// its session token reads every message addressed to its delivery key,
    /// so the mail sent to an earlier pid of the same service stays
    /// deliverable after a restart (ADR-0003). The binding check runs in the
    /// same transaction as the read, so a key bound after a caller's last
    /// call is never read without its token.
    pub fn inbox_as(
        &self,
        me: &str,
        limit: u32,
        session_token: Option<&str>,
    ) -> StoreResult<Value> {
        self.inbox_page(me, limit, None, session_token)
    }
    /// One page of `me`'s unacknowledged messages, oldest first in arrival
    /// order (A1, TIN-7577). `after` names a message of this mailbox, in any
    /// state, and the page starts after it. An id this mailbox never held,
    /// another mailbox's id and a pruned id all answer `unknown after`.
    /// The result carries `next_after`: the last msg_id when the page is
    /// full, so a caller can walk past messages it already holds; `null`
    /// otherwise. Binding works as in [`Store::inbox_as`].
    pub fn inbox_page(
        &self,
        me: &str,
        limit: u32,
        after: Option<&str>,
        session_token: Option<&str>,
    ) -> StoreResult<Value> {
        if !valid_agent_id(me) {
            return Err("invalid agent id".into());
        }
        if after.is_some_and(|id| !valid_ulid(id)) {
            return Err("invalid after".into());
        }
        let mut db = self.db()?;
        let now = self.now();
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_err)?;
        tx.execute("UPDATE messages SET state='expired' WHERE state!='acked' AND state!='expired' AND expires_at<=?1",[now]).map_err(sql_err)?;
        let by_key = Self::gate(&tx, me, session_token, now)?;
        let target = if by_key {
            delivery_key(me)
        } else {
            me.to_owned()
        };
        let floor: Option<i64> = match after {
            None => None,
            Some(id) => {
                let sql = if by_key {
                    "SELECT arrival_seq FROM messages WHERE msg_id=?1 AND recipient_key=?2"
                } else {
                    "SELECT arrival_seq FROM messages WHERE msg_id=?1 AND recipient=?2"
                };
                let found: Option<Option<i64>> = tx
                    .query_row(sql, params![id, target], |r| r.get(0))
                    .optional()
                    .map_err(sql_err)?;
                Some(
                    found
                        .flatten()
                        .ok_or_else(|| StoreError::new(ErrorKind::Invalid, "unknown after"))?,
                )
            }
        };
        let messages: Vec<(String, String)> = {
            let sql = if by_key {
                "SELECT msg_id,envelope FROM messages WHERE recipient_key=?1 AND state IN ('queued','notified','fetched') AND (?3 IS NULL OR arrival_seq>?3) ORDER BY arrival_seq,created_at,seq LIMIT ?2"
            } else {
                "SELECT msg_id,envelope FROM messages WHERE recipient=?1 AND state IN ('queued','notified','fetched') AND (?3 IS NULL OR arrival_seq>?3) ORDER BY arrival_seq,created_at,seq LIMIT ?2"
            };
            let mut stmt = tx.prepare(sql).map_err(sql_err)?;
            let rows = stmt
                .query_map(params![target, limit.min(100), floor], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })
                .map_err(sql_err)?;
            rows.collect::<Result<_, _>>().map_err(sql_err)?
        };
        let next_after = if limit > 0 && messages.len() == limit.min(100) as usize {
            messages.last().map(|(id, _)| id.clone())
        } else {
            None
        };
        for (id, _) in &messages {
            tx.execute("UPDATE messages SET state='fetched',delivery_count=delivery_count+1 WHERE msg_id=?1",[id]).map_err(sql_err)?;
        }
        touch(&tx, me, now)?;
        let values = messages
            .into_iter()
            .map(|(id, body)| {
                let mut v: Value = serde_json::from_str(&body)
                    .map_err(|e| StoreError::new(ErrorKind::Internal, e.to_string()))?;
                let count: i64 = tx
                    .query_row(
                        "SELECT delivery_count FROM messages WHERE msg_id=?1",
                        [&id],
                        |r| r.get(0),
                    )
                    .map_err(sql_err)?;
                v["delivery_count"] = json!(count);
                Ok(v)
            })
            .collect::<StoreResult<Vec<_>>>()?;
        tx.commit().map_err(sql_err)?;
        Ok(json!({"messages":values,"next_after":next_after}))
    }
    /// The thread id, seq, in_reply_to and ticket of a stored message, for
    /// the broker's audit line (ADR-0002 L1-lite). Metadata only, never the
    /// body; `None` when no such message is stored.
    pub fn message_meta(&self, msg_id: &str) -> StoreResult<Option<Value>> {
        let db = self.db()?;
        let row: Option<(String, String)> = db
            .query_row(
                "SELECT thread_id, envelope FROM messages WHERE msg_id=?1",
                [msg_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(sql_err)?;
        Ok(row.map(|(thread_id, envelope)| {
            let envelope = serde_json::from_str::<Value>(&envelope).unwrap_or(Value::Null);
            json!({
                "thread_id": thread_id,
                "seq": envelope.get("seq"),
                "in_reply_to": envelope.get("in_reply_to"),
                "ticket": envelope.get("ticket"),
            })
        }))
    }
    pub fn ack(&self, me: &str, msg_id: &str) -> StoreResult<Value> {
        self.ack_as(me, msg_id, None)
    }
    /// Acknowledge one of `me`'s messages. A bound agent id with its session
    /// token acknowledges by its delivery key (ADR-0003), checked in the
    /// same transaction as the update.
    pub fn ack_as(
        &self,
        me: &str,
        msg_id: &str,
        session_token: Option<&str>,
    ) -> StoreResult<Value> {
        if !valid_agent_id(me) || !valid_ulid(msg_id) {
            return Err("invalid identity or msg_id".into());
        }
        let mut db = self.db()?;
        let now = self.now();
        let db = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_err)?;
        let key = if Self::gate(&db, me, session_token, now)? {
            Some(delivery_key(me))
        } else {
            None
        };
        let n = db.execute("UPDATE messages SET state='acked',acked_at=?3 WHERE msg_id=?1 AND (recipient=?2 OR recipient_key=?4) AND expires_at>?3 AND state IN ('queued','notified','fetched')",params![msg_id,me,now,key]).map_err(sql_err)?;
        if n == 0 {
            let prior: Option<String> = db
                .query_row(
                    "SELECT state FROM messages WHERE msg_id=?1 AND (recipient=?2 OR recipient_key=?3)",
                    params![msg_id, me, key],
                    |r| r.get(0),
                )
                .optional()
                .map_err(sql_err)?;
            if prior.as_deref() != Some("acked") {
                return Err(StoreError::new(
                    ErrorKind::Unavailable,
                    "message unavailable",
                ));
            }
        }
        touch(&db, me, now)?;
        db.commit().map_err(sql_err)?;
        Ok(json!({"msg_id":msg_id,"state":"acked"}))
    }
    pub fn metrics(&self) -> StoreResult<String> {
        let db = self.db()?;
        let now = self.now();
        let sent: i64 = db
            .query_row(
                "SELECT COALESCE(value,0) FROM counters WHERE name='messages_total'",
                [],
                |r| r.get(0),
            )
            .optional()
            .map_err(sql_err)?
            .unwrap_or(0);
        let unacked: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE state NOT IN ('acked','expired') AND expires_at>?1",
                [now],
                |r| r.get(0),
            )
            .map_err(sql_err)?;
        let live: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM sessions WHERE ended=0 AND last_seen>=?1-900",
                [now],
                |r| r.get(0),
            )
            .map_err(sql_err)?;
        let claims_active = Self::claims_active(&db, now)?;
        let claim_overlaps = Self::claim_overlaps(&db, now)?;
        let handoffs: i64 = db
            .query_row(
                "SELECT value FROM counters WHERE name='handoffs_total'",
                [],
                |r| r.get(0),
            )
            .optional()
            .map_err(sql_err)?
            .unwrap_or(0);
        let mut text = format!(
            "# TYPE swb_messages_total counter\nswb_messages_total {sent}\n# TYPE swb_mailbox_unacked gauge\nswb_mailbox_unacked {unacked}\n# TYPE swb_sessions gauge\nswb_sessions{{state=\"live\"}} {live}\n# TYPE swb_claims_active gauge\nswb_claims_active {claims_active}\n# TYPE swb_claim_overlaps gauge\nswb_claim_overlaps {claim_overlaps}\n# TYPE swb_handoffs_total counter\nswb_handoffs_total {handoffs}\n"
        );
        // TIN-7577: send refusals by harness kind and the limit's settings.
        // The lock order is the store, then the limiter, as in send_as.
        // No series carries an agent id.
        let limiter = self
            .2
            .lock()
            .map_err(|_| StoreError::new(ErrorKind::Internal, "limiter lock poisoned"))?;
        text.push_str("# TYPE swb_send_rate_limited_total counter\n");
        for (kind, count) in HARNESS_KINDS.iter().zip(limiter.refused) {
            text.push_str(&format!(
                "swb_send_rate_limited_total{{harness=\"{kind}\"}} {count}\n"
            ));
        }
        text.push_str(&format!(
            "# TYPE swb_send_limit_burst gauge\nswb_send_limit_burst {}\n# TYPE swb_send_limit_refill_per_minute gauge\nswb_send_limit_refill_per_minute {}\n# TYPE swb_send_buckets gauge\nswb_send_buckets {}\n",
            limiter.limit.burst,
            limiter.limit.refill_per_minute,
            limiter.buckets_held()
        ));
        Ok(text)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    /// 2023-11-14T22:13:20Z.
    const T0: i64 = 1_700_000_000;

    fn clocked() -> (Store, Arc<ManualClock>) {
        let clock = Arc::new(ManualClock::new(T0));
        (Store::memory_with_clock(clock.clone()).unwrap(), clock)
    }

    fn all() -> PeersQuery<'static> {
        PeersQuery {
            all: true,
            ..PeersQuery::default()
        }
    }

    fn refresh(me: &str) -> PeersQuery<'_> {
        PeersQuery {
            me: Some(me),
            all: true,
            ..PeersQuery::default()
        }
    }

    fn reg(
        s: &Store,
        harness: &str,
        host: &str,
        pid: u32,
        session: &str,
        proc_start: &str,
    ) -> Value {
        s.register(&json!({"harness":harness,"host":host,"pid":pid,"session_id":session,"proc_start":proc_start}))
            .unwrap()
    }

    fn row_count(s: &Store) -> i64 {
        s.0.lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn system_clock_matches_sqlite_unixepoch() {
        let db = Connection::open_in_memory().unwrap();
        let before = SystemClock.now();
        let sqlite: i64 = db
            .query_row("SELECT unixepoch()", [], |r| r.get(0))
            .unwrap();
        let after = SystemClock.now();
        assert!(before <= sqlite && sqlite <= after);
        assert!((Store::memory().unwrap().now() - SystemClock.now()).abs() <= 2);
    }

    #[test]
    fn manual_clock_moves_only_forward_and_only_when_told() {
        let clock = ManualClock::new(T0);
        assert_eq!(clock.now(), T0);
        assert_eq!(clock.now(), T0);
        assert_eq!(clock.advance(0), T0);
        assert_eq!(clock.advance(901), T0 + 901);
        assert_eq!(clock.advance(u32::MAX), T0 + 901 + i64::from(u32::MAX));
        let shared: Arc<dyn Clock> = Arc::new(ManualClock::new(5));
        assert_eq!(shared.now(), 5);
    }

    #[test]
    fn lease_bands_follow_the_injected_clock() {
        let (s, clock) = clocked();
        let me = s.register(&json!({"harness":"claude","host":"honey","pid":1,"session_id":"a","proc_start":"1"})).unwrap()["agent_id"].as_str().unwrap().to_owned();
        let state = |s: &Store| s.peers(&all()).unwrap()["peers"][0]["state"].clone();
        assert_eq!(s.peers(&all()).unwrap()["peers"][0]["last_seen"], T0);
        clock.advance(900);
        assert_eq!(state(&s), "live");
        clock.advance(1);
        assert_eq!(state(&s), "idle");
        clock.advance(21600 - 901);
        assert_eq!(state(&s), "idle");
        assert!(
            s.metrics()
                .unwrap()
                .contains("swb_sessions{state=\"live\"} 0\n")
        );
        clock.advance(1);
        assert_eq!(state(&s), "gone");
        let renewed = s.peers(&refresh(&me)).unwrap();
        assert_eq!(renewed["peers"][0]["state"], "live");
        assert_eq!(renewed["peers"][0]["last_seen"], T0 + 21601);
        assert!(
            s.metrics()
                .unwrap()
                .contains("swb_sessions{state=\"live\"} 1\n")
        );
    }

    #[test]
    fn envelope_times_and_expiry_follow_the_injected_clock() {
        let (s, clock) = clocked();
        let to = "pi:sting:2:b";
        let sent = s
            .send(&json!({"from":"claude:honey:1:a","to":to,"ticket":"none","body":"hi","ttl_hours":1}))
            .unwrap();
        assert_eq!(sent.envelope["sent_at"], "2023-11-14T22:13:20Z");
        assert_eq!(sent.envelope["expires_at"], "2023-11-14T23:13:20Z");
        let id = sent.envelope["msg_id"].as_str().unwrap().to_owned();
        clock.advance(3599);
        assert_eq!(
            s.inbox(to, 10).unwrap()["messages"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert!(s.metrics().unwrap().contains("swb_mailbox_unacked 1\n"));
        clock.advance(1);
        assert!(s.metrics().unwrap().contains("swb_mailbox_unacked 0\n"));
        assert!(
            s.inbox(to, 10).unwrap()["messages"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert_eq!(s.ack(to, &id).unwrap_err(), "message unavailable");
    }

    #[test]
    fn retention_follows_the_injected_clock() {
        let (s, clock) = clocked();
        let from = "claude:honey:1:a";
        let to = "pi:sting:2:b";
        let thread = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
        let send = |s: &Store, body: &str| {
            s.send(&json!({"from":from,"to":to,"ticket":"none","body":body,"thread_id":thread}))
                .unwrap()
                .envelope
        };
        let acked = send(&s, "acked")["msg_id"].as_str().unwrap().to_owned();
        send(&s, "unacked");
        clock.advance(10);
        s.ack(to, &acked).unwrap();
        clock.advance(604_799);
        s.prune().unwrap();
        assert_eq!(row_count(&s), 2);
        clock.advance(1);
        s.prune().unwrap();
        assert_eq!(row_count(&s), 1);
        clock.advance(2_592_000 - 604_810);
        s.prune().unwrap();
        assert_eq!(row_count(&s), 0);
        assert_eq!(send(&s, "after prune")["seq"], 3);
    }

    #[test]
    fn ttl_defaults_and_clamps() {
        assert_eq!(effective_ttl_hours(None), 72);
        assert_eq!(effective_ttl_hours(Some(10_000)), 336);
        assert_eq!(effective_ttl_hours(Some(0)), 1);
    }
    #[test]
    fn every_harness_kind_registers_and_round_trips_its_agent_id() {
        let s = Store::memory().unwrap();
        for (pid, harness) in (1u32..).zip(HARNESS_KINDS) {
            let row = reg(&s, harness, "host-a", pid, "s-1", "start-1");
            let id = format!("{harness}:host-a:{pid}:s-1");
            assert_eq!(row["agent_id"], id.as_str());
            assert!(valid_agent_id(&id), "{id}");
            // The id is usable as a sender and a receiver.
            let sent = s
                .send(&json!({"from":id,"to":id,"ticket":"none","body":"ping"}))
                .unwrap();
            assert_eq!(sent.envelope["from"], id.as_str());
        }
        let listed = s.peers(&all()).unwrap();
        let mut kinds: Vec<&str> = listed["peers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["harness"].as_str().unwrap())
            .collect();
        kinds.sort_unstable();
        let mut expected = HARNESS_KINDS.to_vec();
        expected.sort_unstable();
        assert_eq!(kinds, expected);
        // `kimi` stays Claude Code on Kimi; near misses are not kinds.
        assert!(!valid_harness(""));
        for bad in ["kimi-code", "Hermes", "kimi_code", "hermes-agent"] {
            assert!(!valid_harness(bad), "{bad}");
            let refused = s.register(&json!({"harness":bad,"host":"host-a","pid":9,
                "session_id":"s-1","proc_start":"start-1"}));
            assert_eq!(refused.unwrap_err(), "invalid harness");
            assert!(!valid_agent_id(&format!("{bad}:host-a:9:s-1")));
        }
    }

    #[test]
    fn end_requires_the_latest_registration_token() {
        let s = Store::memory().unwrap();
        let first = reg(&s, "claude", "honey", 10, "same", "start-1");
        let me = first["agent_id"].as_str().unwrap();
        let token = first["end_token"].as_str().unwrap().to_owned();
        assert_eq!(token.len(), END_TOKEN_HEX_LEN);
        // The listing never carries proc_start or a token.
        let listed = s.peers(&all()).unwrap().to_string();
        assert!(
            !listed.contains("start-1") && !listed.contains(&token),
            "{listed}"
        );
        assert!(!listed.contains("proc_start"), "{listed}");
        // A forged or malformed token ends nothing, and says nothing.
        let forged = "0".repeat(END_TOKEN_HEX_LEN);
        assert_eq!(s.end(me, &forged).unwrap()["state"], "unknown");
        assert_eq!(s.end(me, "start-1").unwrap_err(), "invalid end_token");
        assert_eq!(
            s.end(me, &token.to_uppercase()).unwrap_err(),
            "invalid end_token"
        );
        assert_eq!(
            s.end("codex:honey:9:none", &token).unwrap()["state"],
            "unknown"
        );
        assert_eq!(s.peers(&all()).unwrap()["peers"][0]["state"], "live");
        // Every register rotates the token, even with the same proc_start,
        // so knowing proc_start never retrieves the live token.
        let again = reg(&s, "claude", "honey", 10, "same", "start-1");
        assert_ne!(again["end_token"], token.as_str());
        assert_eq!(s.end(me, &token).unwrap()["state"], "unknown");
        let rotated = reg(&s, "claude", "honey", 10, "same", "start-2");
        assert_ne!(rotated["end_token"], again["end_token"]);
        assert_eq!(
            s.end(me, again["end_token"].as_str().unwrap()).unwrap()["state"],
            "unknown"
        );
        let current = rotated["end_token"].as_str().unwrap().to_owned();
        assert_eq!(s.end(me, &current).unwrap()["state"], "ended");
        assert_eq!(s.peers(&all()).unwrap()["peers"][0]["state"], "ended");
        // After end, a register mints a fresh token even with the same proc_start.
        let revived = reg(&s, "claude", "honey", 10, "same", "start-2");
        assert_ne!(revived["end_token"], current.as_str());
        assert_eq!(s.end(me, &current).unwrap()["state"], "unknown");
    }

    fn bind(
        s: &Store,
        harness: &str,
        host: &str,
        pid: u32,
        session: &str,
        token: Option<&str>,
    ) -> StoreResult<Value> {
        let mut input = json!({"harness":harness,"host":host,"pid":pid,"session_id":session,
            "proc_start":format!("start-{pid}"),"bind":true});
        if let Some(token) = token {
            input["session_token"] = json!(token);
        }
        s.register(&input)
    }

    fn token_of(v: &Value) -> String {
        v["session_token"].as_str().unwrap().to_owned()
    }

    #[test]
    fn a_bound_registration_needs_its_session_token_and_rotates_it() {
        let s = Store::memory().unwrap();
        let first = bind(&s, "hermes", "sting", 100, "svc", None).unwrap();
        let me = first["agent_id"].as_str().unwrap();
        assert_eq!(first["bound"], true);
        let token = token_of(&first);
        assert_eq!(token.len(), SESSION_TOKEN_HEX_LEN);
        assert_ne!(first["end_token"], token.as_str());
        // The spoof test (SWB-R66): acting as a bound id without its token,
        // or with another token, fails.
        let missing = s.authorize(me, None).unwrap_err();
        assert_eq!(missing.kind, ErrorKind::Unauthorized);
        assert_eq!(missing.code(), "unauthorized");
        assert_eq!(missing, "session token required");
        assert_eq!(
            s.authorize(me, Some(&"0".repeat(64))).unwrap_err(),
            "invalid session token"
        );
        assert_eq!(
            s.authorize(me, Some("short")).unwrap_err(),
            "invalid session token"
        );
        assert_eq!(
            s.authorize(me, Some(first["end_token"].as_str().unwrap()))
                .unwrap_err()
                .kind,
            ErrorKind::Unauthorized
        );
        s.authorize(me, Some(&token)).unwrap();
        // Nobody can take the binding over by registering again, bound or
        // not, with the same pid or another one.
        for (pid, wants) in [(100, true), (100, false), (101, true)] {
            let input = json!({"harness":"hermes","host":"sting","pid":pid,"session_id":"svc",
                "proc_start":"x","bind":wants});
            assert_eq!(
                s.register(&input).unwrap_err().kind,
                ErrorKind::Unauthorized
            );
        }
        assert_eq!(
            bind(&s, "hermes", "sting", 100, "svc", Some(&"f".repeat(64)))
                .unwrap_err()
                .kind,
            ErrorKind::Unauthorized
        );
        // SWB-R67: every re-register by the holder rotates the token, and the
        // old one stops working. Registering without bind keeps the binding.
        let again = s
            .register(
                &json!({"harness":"hermes","host":"sting","pid":100,"session_id":"svc",
                "proc_start":"start-100","session_token":token}),
            )
            .unwrap();
        assert_eq!(again["bound"], true);
        let rotated = token_of(&again);
        assert_ne!(rotated, token);
        assert_eq!(
            s.authorize(me, Some(&token)).unwrap_err(),
            "invalid session token"
        );
        s.authorize(me, Some(&rotated)).unwrap();
        // Another session id on the same host is its own, unbound key.
        s.authorize("hermes:sting:100:other", None).unwrap();
        // Malformed ids pass here so the operation reports them.
        s.authorize("not-an-id", None).unwrap();
        assert_eq!(
            s.register(&json!({"harness":"pi","host":"h","pid":1,"session_id":"a",
                "proc_start":"1","bind":"yes"}))
                .unwrap_err(),
            "invalid bind"
        );
    }

    #[test]
    fn unbound_ids_behave_as_in_v040() {
        let s = Store::memory().unwrap();
        let row = reg(&s, "claude", "honey", 1, "a", "1");
        assert_eq!(row["bound"], false);
        assert!(row.get("session_token").is_none());
        let me = row["agent_id"].as_str().unwrap();
        s.authorize(me, None).unwrap();
        // A token an unbound id presents is ignored.
        s.authorize(me, Some("anything")).unwrap();
        // Anyone may still re-register an unbound id (SWB-R05 for unbound ids).
        let other = reg(&s, "claude", "honey", 1, "a", "2");
        assert_eq!(other["bound"], false);
        // An unregistered id is unbound too.
        s.authorize("pi:sting:2:b", None).unwrap();
        let listed = s.peers(&all()).unwrap();
        assert_eq!(listed["peers"][0]["bound"], false);
    }

    #[test]
    fn a_restarted_bound_service_keeps_its_unread_mail() {
        let s = Store::memory().unwrap();
        let first = bind(&s, "hermes", "sting", 100, "svc", None).unwrap();
        let old = first["agent_id"].as_str().unwrap().to_owned();
        let send = |to: &str, body: &str| {
            s.send(&json!({"from":"claude:honey:1:a","to":to,"ticket":"none","body":body}))
                .unwrap()
                .envelope["msg_id"]
                .as_str()
                .unwrap()
                .to_owned()
        };
        let before = send(&old, "sent before the restart");
        // The process restarts with a new pid and presents its saved token.
        let second = bind(&s, "hermes", "sting", 200, "svc", Some(&token_of(&first))).unwrap();
        let new = second["agent_id"].as_str().unwrap().to_owned();
        assert_ne!(new, old);
        let token = token_of(&second);
        let after = send(&old, "addressed to the old pid after the restart");
        let direct = send(&new, "addressed to the new pid");
        // A bound id reads only with its token.
        assert_eq!(s.inbox(&new, 10).unwrap_err().kind, ErrorKind::Unauthorized);
        let read = s.inbox_as(&new, 10, Some(&token)).unwrap();
        let ids: Vec<&str> = read["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["msg_id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, vec![before.as_str(), after.as_str(), direct.as_str()]);
        // The new id acknowledges mail addressed to the old pid.
        assert_eq!(
            s.ack(&new, &before).unwrap_err().kind,
            ErrorKind::Unauthorized
        );
        assert_eq!(
            s.ack_as(&new, &before, Some(&token)).unwrap()["state"],
            "acked"
        );
        assert_eq!(
            s.ack_as(&new, &before, Some(&token)).unwrap()["state"],
            "acked"
        );
        assert_eq!(
            s.inbox_as(&new, 10, Some(&token)).unwrap()["messages"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        // Only the binding's own agent id may act; the old pid is superseded.
        assert_eq!(
            s.authorize(&old, Some(&token)).unwrap_err(),
            "agent id superseded by a bound registration of its session"
        );
        s.authorize(&new, Some(&token)).unwrap();
        // peers shows the one latest row, bound (SWB-R68).
        let listed = s.peers(&PeersQuery::default()).unwrap();
        assert_eq!(listed["peers"].as_array().unwrap().len(), 1);
        assert_eq!(listed["peers"][0]["agent_id"], new.as_str());
        assert_eq!(listed["peers"][0]["bound"], true);
        // An unbound session keeps reading by its exact agent id.
        reg(&s, "pi", "sting", 300, "plain", "p");
        send("pi:sting:299:plain", "for an earlier pid");
        assert!(
            s.inbox("pi:sting:300:plain", 10).unwrap()["messages"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_binding_lapses_when_its_session_ends_or_goes() {
        let (s, clock) = clocked();
        let first = bind(&s, "hermes", "sting", 100, "svc", None).unwrap();
        let me = first["agent_id"].as_str().unwrap().to_owned();
        let end_token = first["end_token"].as_str().unwrap();
        assert_eq!(
            s.end(&me, end_token).unwrap_err().kind,
            ErrorKind::Unauthorized
        );
        assert_eq!(
            s.end_as(&me, end_token, Some(&token_of(&first))).unwrap()["state"],
            "ended"
        );
        s.authorize(&me, None).unwrap();
        assert_eq!(s.peers(&all()).unwrap()["peers"][0]["bound"], false);
        let rebound = bind(&s, "hermes", "sting", 100, "svc", None).unwrap();
        assert_eq!(rebound["bound"], true);
        assert!(s.authorize(&me, None).is_err());
        // Idle keeps it; gone (6 h without a call) lapses it.
        clock.advance(u32::try_from(IDLE_MAX_AGE_SECONDS).unwrap());
        assert!(s.authorize(&me, None).is_err());
        assert_eq!(s.peers(&all()).unwrap()["peers"][0]["bound"], true);
        clock.advance(1);
        s.authorize(&me, None).unwrap();
        assert_eq!(s.peers(&all()).unwrap()["peers"][0]["bound"], false);
        let taken = reg(&s, "hermes", "sting", 100, "svc", "start-100");
        assert_eq!(taken["bound"], false);
    }

    /// Review of #54: a lapse is permanent. A tokenless call after the lapse
    /// (a stray heartbeat, or a channel poller whose token file is gone)
    /// revives an unbound session, never the binding or its old token, so
    /// the owner can bind again without a token.
    #[test]
    fn a_lapsed_binding_stays_lapsed_after_tokenless_calls() {
        type Poke = fn(&Store, &str);
        let heartbeat: Poke = |s, me| {
            s.heartbeat(me).unwrap();
        };
        let inbox: Poke = |s, me| {
            s.inbox(me, 10).unwrap();
        };
        let others: Poke = |s, me| {
            s.peers(&PeersQuery {
                me: Some(me),
                ..PeersQuery::default()
            })
            .unwrap();
            s.thread(me, &Ulid::new().to_string(), None, None).unwrap();
            s.claims(&ClaimsQuery {
                me: Some(me),
                ..ClaimsQuery::default()
            })
            .unwrap();
            s.send(&json!({"from":me,"to":"claude:honey:1:a","ticket":"none","body":"b"}))
                .unwrap();
            let sent = s
                .send(&json!({"from":"claude:honey:1:a","to":me,"ticket":"none","body":"b"}))
                .unwrap();
            s.ack(me, sent.envelope["msg_id"].as_str().unwrap())
                .unwrap();
        };
        for (lapse_by_end, poke) in [
            (true, heartbeat),
            (true, inbox),
            (false, heartbeat),
            (false, inbox),
            (true, others),
            (false, others),
        ] {
            let (s, clock) = clocked();
            let first = bind(&s, "hermes", "sting", 100, "svc", None).unwrap();
            let me = first["agent_id"].as_str().unwrap().to_owned();
            let old_token = token_of(&first);
            if lapse_by_end {
                s.end_as(&me, first["end_token"].as_str().unwrap(), Some(&old_token))
                    .unwrap();
            } else {
                clock.advance(u32::try_from(IDLE_MAX_AGE_SECONDS).unwrap() + 1);
            }
            s.authorize(&me, None).unwrap();
            poke(&s, &me);
            // Still unbound after the tokenless call.
            s.authorize(&me, None).unwrap();
            assert_eq!(s.peers(&all()).unwrap()["peers"][0]["bound"], false);
            // The owner binds again without a token, same pid or a new one,
            // and gets a new token; the old one never works again.
            let again = bind(&s, "hermes", "sting", 100, "svc", None).unwrap();
            assert_eq!(again["bound"], true);
            let new_token = token_of(&again);
            assert_ne!(new_token, old_token);
            assert_eq!(
                s.authorize(&me, Some(&old_token)).unwrap_err(),
                "invalid session token"
            );
            s.authorize(&me, Some(&new_token)).unwrap();
        }
    }

    /// Review of #54: the binding check runs in the operation's own
    /// transaction. A caller that read an id's inbox while it was unbound
    /// cannot read or ack its key's mail once the id is bound.
    #[test]
    fn a_key_bound_after_an_unbound_read_needs_the_token() {
        let s = Store::memory().unwrap();
        let me = "hermes:sting:100:svc";
        assert!(
            s.inbox(me, 10).unwrap()["messages"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        let bound = bind(&s, "hermes", "sting", 100, "svc", None).unwrap();
        let sent = s
            .send(&json!({"from":"claude:honey:1:a","to":me,"ticket":"none","body":"secret"}))
            .unwrap();
        let msg_id = sent.envelope["msg_id"].as_str().unwrap();
        assert_eq!(s.inbox(me, 10).unwrap_err().kind, ErrorKind::Unauthorized);
        assert_eq!(s.ack(me, msg_id).unwrap_err().kind, ErrorKind::Unauthorized);
        let read = s.inbox_as(me, 10, Some(&token_of(&bound))).unwrap();
        assert_eq!(read["messages"][0]["msg_id"], msg_id);
        assert_eq!(read["messages"][0]["delivery_count"], 1);
    }

    /// Review of #54: only a send that stores a new message spends the
    /// sender's bucket. An invalid send, a refused bound send and an
    /// idempotent replay (SWB-R46) cost nothing.
    #[test]
    fn only_a_created_send_spends_the_bucket() {
        let s = Store::memory().unwrap();
        s.set_send_limit(SendLimit {
            burst: 1,
            refill_per_minute: 1,
        })
        .unwrap();
        let me = "claude:honey:1:a";
        let msg = |ticket: &str, id: &str| json!({"from":me,"to":"pi:sting:2:b","ticket":ticket,"body":"x","msg_id":id});
        let id = Ulid::new().to_string();
        for _ in 0..3 {
            assert_eq!(s.send(&msg("bad", &id)).unwrap_err(), "invalid ticket");
        }
        assert!(s.send(&msg("none", &id)).unwrap().created);
        for _ in 0..3 {
            assert!(!s.send(&msg("none", &id)).unwrap().created);
        }
        let limited = s.send(&msg("none", &Ulid::new().to_string())).unwrap_err();
        assert_eq!(limited.kind, ErrorKind::RateLimited);
        // A bound sender refused for its token keeps its bucket.
        let bound = bind(&s, "hermes", "sting", 100, "svc", None).unwrap();
        let from = bound["agent_id"].as_str().unwrap();
        let spoof = json!({"from":from,"to":"pi:sting:2:b","ticket":"none","body":"x"});
        for _ in 0..3 {
            assert_eq!(s.send(&spoof).unwrap_err().kind, ErrorKind::Unauthorized);
        }
        assert!(s.send_as(&spoof, Some(&token_of(&bound))).unwrap().created);
    }

    #[test]
    fn the_send_limit_refuses_with_rate_limited_and_refills() {
        let (s, clock) = clocked();
        s.set_send_limit(SendLimit {
            burst: 2,
            refill_per_minute: 6,
        })
        .unwrap();
        let me = "claude:honey:1:a";
        s.admit_send(me).unwrap();
        s.admit_send(me).unwrap();
        let limited = s.admit_send(me).unwrap_err();
        assert_eq!(limited.kind, ErrorKind::RateLimited);
        assert_eq!(limited.code(), "rate_limited");
        assert_eq!(limited, "rate limited; retry in 10 s");
        // Each sender has its own bucket.
        s.admit_send("pi:sting:2:b").unwrap();
        clock.advance(10);
        s.admit_send(me).unwrap();
        assert!(s.admit_send(me).is_err());
        // An invalid sender is left to send to report.
        s.admit_send("not-an-id").unwrap();
        s.set_send_limit(SendLimit {
            burst: 0,
            refill_per_minute: 0,
        })
        .unwrap();
        for _ in 0..100 {
            s.admit_send(me).unwrap();
        }
        assert_eq!(SendLimit::default().burst, SEND_BURST_DEFAULT);
    }

    /// TIN-7577: refusals are counted by harness kind, and /metrics shows
    /// them with the limit settings, with no agent id.
    #[test]
    fn rate_limit_refusals_are_counted_by_harness_kind() {
        let (s, _clock) = clocked();
        s.set_send_limit(SendLimit {
            burst: 1,
            refill_per_minute: 6,
        })
        .unwrap();
        for sender in ["claude:host-a:1:a", "codex:host-b:2:b"] {
            s.admit_send(sender).unwrap();
            let refused = s.admit_send(sender).unwrap_err();
            assert_eq!(refused.kind, ErrorKind::RateLimited);
            assert_eq!(refused.retry_after, Some(10));
            assert_eq!(refused, "rate limited; retry in 10 s");
        }
        let text = s.metrics().unwrap();
        for line in [
            "# TYPE swb_send_rate_limited_total counter\n",
            "swb_send_rate_limited_total{harness=\"claude\"} 1\n",
            "swb_send_rate_limited_total{harness=\"codex\"} 1\n",
            "swb_send_rate_limited_total{harness=\"kimi\"} 0\n",
            "swb_send_limit_burst 1\n",
            "swb_send_limit_refill_per_minute 6\n",
            "swb_send_buckets 2\n",
        ] {
            assert!(text.contains(line), "{line:?} not in {text}");
        }
        assert_eq!(
            text.matches("swb_send_rate_limited_total{").count(),
            HARNESS_KINDS.len()
        );
        assert!(
            !text.contains("host-a") && !text.contains("host-b"),
            "{text}"
        );
        // Other errors carry no retry_after.
        assert_eq!(StoreError::new(ErrorKind::Invalid, "x").retry_after, None);
    }

    #[test]
    fn retry_after_is_none_when_the_bucket_never_refills() {
        let s = Store::memory().unwrap();
        s.set_send_limit(SendLimit {
            burst: 1,
            refill_per_minute: 0,
        })
        .unwrap();
        let me = "pi:host-b:2:b";
        s.admit_send(me).unwrap();
        let refused = s.admit_send(me).unwrap_err();
        assert_eq!(refused, "rate limited");
        assert_eq!(refused.retry_after, None);
        assert!(
            s.metrics()
                .unwrap()
                .contains("swb_send_rate_limited_total{harness=\"pi\"} 1\n")
        );
    }

    #[test]
    fn an_older_store_gains_delivery_keys_for_its_messages() {
        let path = std::env::temp_dir().join(format!("swb-key-{}.sqlite3", Ulid::new()));
        let id;
        {
            let s = Store::open(&path).unwrap();
            id = s
                .send(&json!({"from":"claude:honey:1:a","to":"hermes:sting:100:svc","ticket":"none","body":"old"}))
                .unwrap()
                .envelope["msg_id"]
                .as_str()
                .unwrap()
                .to_owned();
            s.0.lock()
                .unwrap()
                .execute("UPDATE messages SET recipient_key=NULL", [])
                .unwrap();
        }
        let s = Store::open(&path).unwrap();
        let key: String =
            s.0.lock()
                .unwrap()
                .query_row(
                    "SELECT recipient_key FROM messages WHERE msg_id=?1",
                    [&id],
                    |r| r.get(0),
                )
                .unwrap();
        assert_eq!(key, "hermes:sting:svc");
        assert_eq!(delivery_key("hermes:sting:100:svc"), "hermes:sting:svc");
        let bound = bind(&s, "hermes", "sting", 200, "svc", None).unwrap();
        let read = s
            .inbox_as(
                bound["agent_id"].as_str().unwrap(),
                10,
                Some(&token_of(&bound)),
            )
            .unwrap();
        assert_eq!(read["messages"][0]["msg_id"], id.as_str());
        drop(s);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(format!("{}-wal", path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", path.display()));
    }

    /// A ULID string with the given value. A small value gives a small,
    /// canonical id, so a test can choose the msg_id order.
    fn ulid_of(value: u128) -> String {
        Ulid(value << 80).to_string()
    }

    fn no_send_limit(s: &Store) {
        s.set_send_limit(SendLimit {
            burst: 0,
            refill_per_minute: 0,
        })
        .unwrap();
    }

    fn page_ids(page: &Value) -> Vec<String> {
        page["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["msg_id"].as_str().unwrap().to_owned())
            .collect()
    }

    /// A1 (TIN-7577): a reader walks past a full page of messages it holds.
    #[test]
    fn inbox_pages_past_a_full_page_with_next_after() {
        let (s, _clock) = clocked();
        no_send_limit(&s);
        let to = "pi:host-b:2:b";
        // The msg_ids fall as the sends go on, and every send lands in one
        // second, so neither msg_id nor created_at gives the send order.
        let ids: Vec<String> = (0..20u128).map(|n| ulid_of(1000 - n)).collect();
        for (n, id) in ids.iter().enumerate() {
            s.send(&json!({"from":"claude:host-a:1:a","to":to,"ticket":"none",
                "body":format!("m{n}"),"msg_id":id}))
                .unwrap();
        }
        let first = s.inbox_page(to, 8, None, None).unwrap();
        assert_eq!(page_ids(&first), ids[..8].to_vec());
        assert_eq!(first["next_after"], ids[7].as_str());
        let second = s.inbox_page(to, 8, Some(ids[7].as_str()), None).unwrap();
        assert_eq!(page_ids(&second), ids[8..16].to_vec());
        assert_eq!(second["next_after"], ids[15].as_str());
        let third = s.inbox_page(to, 8, Some(ids[15].as_str()), None).unwrap();
        assert_eq!(page_ids(&third), ids[16..].to_vec());
        assert_eq!(third["next_after"], Value::Null);
        // The head page stays the same 8 while they are unacknowledged.
        let head = s.inbox(to, 8).unwrap();
        assert_eq!(page_ids(&head), ids[..8].to_vec());
        assert_eq!(head["next_after"], ids[7].as_str());
    }

    #[test]
    fn a_send_after_the_cursor_is_never_skipped() {
        let (s, _clock) = clocked();
        no_send_limit(&s);
        let to = "pi:host-b:2:b";
        let send = |id: &str| {
            s.send(
                &json!({"from":"claude:host-a:1:a","to":to,"ticket":"none","body":"x","msg_id":id}),
            )
            .unwrap();
        };
        let high = ulid_of(9000);
        let low = ulid_of(1);
        send(&high);
        let page = s.inbox_page(to, 1, None, None).unwrap();
        assert_eq!(page["next_after"], high.as_str());
        // A later send in the same second, with a smaller msg_id.
        send(&low);
        let next = s.inbox_page(to, 1, Some(high.as_str()), None).unwrap();
        assert_eq!(page_ids(&next), vec![low.clone()]);
    }

    #[test]
    fn after_must_name_a_message_of_this_mailbox() {
        let s = Store::memory().unwrap();
        let (a, b) = ("pi:host-b:2:b", "codex:host-b:3:c");
        let sent = |to: &str| {
            s.send(&json!({"from":"claude:host-a:1:a","to":to,"ticket":"none","body":"x"}))
                .unwrap()
                .envelope["msg_id"]
                .as_str()
                .unwrap()
                .to_owned()
        };
        let mine = sent(a);
        let theirs = sent(b);
        for foreign in [theirs, Ulid::new().to_string()] {
            let error = s
                .inbox_page(a, 8, Some(foreign.as_str()), None)
                .unwrap_err();
            assert_eq!(error.kind, ErrorKind::Invalid);
            assert_eq!(error, "unknown after");
        }
        assert_eq!(
            s.inbox_page(a, 8, Some("nope"), None).unwrap_err(),
            "invalid after"
        );
        // An acknowledged cursor still resolves.
        s.ack(a, &mine).unwrap();
        let page = s.inbox_page(a, 8, Some(mine.as_str()), None).unwrap();
        assert!(page["messages"].as_array().unwrap().is_empty());
        assert_eq!(page["next_after"], Value::Null);
    }

    #[test]
    fn a_bound_mailbox_pages_by_delivery_key() {
        let s = Store::memory().unwrap();
        let first = bind(&s, "hermes", "host-b", 100, "svc", None).unwrap();
        let old = first["agent_id"].as_str().unwrap().to_owned();
        let send = |body: &str| {
            s.send(&json!({"from":"claude:host-a:1:a","to":old,"ticket":"none","body":body}))
                .unwrap()
                .envelope["msg_id"]
                .as_str()
                .unwrap()
                .to_owned()
        };
        let cursor = send("one");
        send("two");
        let second = bind(&s, "hermes", "host-b", 200, "svc", Some(&token_of(&first))).unwrap();
        let new = second["agent_id"].as_str().unwrap();
        let token = token_of(&second);
        let page = s
            .inbox_page(new, 8, Some(cursor.as_str()), Some(&token))
            .unwrap();
        assert_eq!(page["messages"].as_array().unwrap().len(), 1);
        assert_eq!(page["messages"][0]["body"], "two");
        assert_eq!(
            s.inbox_page(new, 8, Some(cursor.as_str()), None)
                .unwrap_err()
                .kind,
            ErrorKind::Unauthorized
        );
    }

    #[test]
    fn an_older_store_gains_arrival_positions_in_send_order() {
        let path = std::env::temp_dir().join(format!("swb-arrival-{}.sqlite3", Ulid::new()));
        let first;
        let second;
        {
            let s = Store::open(&path).unwrap();
            let send = |body: &str| {
                s.send(&json!({"from":"claude:host-a:1:a","to":"pi:host-b:2:b","ticket":"none","body":body}))
                    .unwrap()
                    .envelope["msg_id"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            };
            first = send("one");
            second = send("two");
            s.0.lock()
                .unwrap()
                .execute_batch(
                    "UPDATE messages SET arrival_seq=NULL; DELETE FROM counters WHERE name='arrival_seq';",
                )
                .unwrap();
        }
        let s = Store::open(&path).unwrap();
        let db = s.0.lock().unwrap();
        let positions: Vec<(String, i64)> = db
            .prepare("SELECT msg_id, arrival_seq FROM messages ORDER BY arrival_seq")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(positions, vec![(first, 1), (second, 2)]);
        let counter: i64 = db
            .query_row(
                "SELECT value FROM counters WHERE name='arrival_seq'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(counter, 2);
        drop(db);
        let third = s
            .send(&json!({"from":"claude:host-a:1:a","to":"pi:host-b:2:b","ticket":"none","body":"three"}))
            .unwrap();
        let position: i64 =
            s.0.lock()
                .unwrap()
                .query_row(
                    "SELECT arrival_seq FROM messages WHERE msg_id=?1",
                    [third.envelope["msg_id"].as_str().unwrap()],
                    |r| r.get(0),
                )
                .unwrap();
        assert_eq!(position, 3);
        drop(s);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(format!("{}-wal", path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", path.display()));
    }

    #[test]
    fn peers_defaults_to_live_bounded_and_one_row_per_session() {
        let (s, clock) = clocked();
        for pid in 1..=5 {
            reg(&s, "claude", "honey", pid, &format!("s{pid}"), "p");
        }
        clock.advance(901);
        reg(&s, "pi", "sting", 7, "fresh", "p");
        // An older registration of the same session, from another process
        // of the same harness on the same host.
        reg(&s, "codex", "sting", 8, "shared", "p");
        reg(&s, "codex", "sting", 9, "shared", "p");
        // A fixed session label (a Junie IDE channel) on another host, and
        // another harness on the same host, are different sessions.
        reg(&s, "codex", "honey", 10, "shared", "p");
        reg(&s, "junie", "sting", 11, "shared", "p");
        let ids_of = |page: &Value| -> Vec<String> {
            page["peers"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| p["agent_id"].as_str().unwrap().to_owned())
                .collect()
        };
        let live = s.peers(&PeersQuery::default()).unwrap();
        assert_eq!(
            ids_of(&live),
            vec![
                "codex:honey:10:shared",
                "codex:sting:9:shared",
                "junie:sting:11:shared",
                "pi:sting:7:fresh"
            ]
        );
        assert_eq!(live["next_cursor"], Value::Null);
        // The latest registration of a session wins, whichever is newer.
        reg(&s, "codex", "sting", 8, "shared", "p");
        assert_eq!(
            ids_of(&s.peers(&PeersQuery::default()).unwrap()),
            vec![
                "codex:honey:10:shared",
                "codex:sting:8:shared",
                "junie:sting:11:shared",
                "pi:sting:7:fresh"
            ]
        );
        // A host filter never hides that host's own row behind another
        // host's registration of the same label.
        assert_eq!(
            ids_of(
                &s.peers(&PeersQuery {
                    host: Some("honey"),
                    ..PeersQuery::default()
                })
                .unwrap()
            ),
            vec!["codex:honey:10:shared"]
        );
        // A caller can filter on a proc_start it knows; rows never show it.
        reg(&s, "claude", "honey", 12, "own", "start-x");
        assert_eq!(
            ids_of(
                &s.peers(&PeersQuery {
                    all: true,
                    host: Some("honey"),
                    proc_start: Some("start-x"),
                    ..PeersQuery::default()
                })
                .unwrap()
            ),
            vec!["claude:honey:12:own"]
        );
        assert_eq!(
            s.peers(&PeersQuery {
                proc_start: Some(""),
                ..PeersQuery::default()
            })
            .unwrap_err(),
            "invalid proc_start"
        );
        // all=true pages through every session, idle ones included.
        let mut cursor: Option<String> = None;
        let mut seen = Vec::new();
        loop {
            let page = s
                .peers(&PeersQuery {
                    all: true,
                    limit: Some(2),
                    cursor: cursor.as_deref(),
                    ..PeersQuery::default()
                })
                .unwrap();
            let rows = page["peers"].as_array().unwrap();
            assert!(rows.len() <= 2);
            seen.extend(
                rows.iter()
                    .map(|p| p["agent_id"].as_str().unwrap().to_owned()),
            );
            match page["next_cursor"].as_str() {
                Some(next) => cursor = Some(next.to_owned()),
                None => break,
            }
        }
        assert_eq!(seen.len(), 10, "{seen:?}");
        assert!(seen.windows(2).all(|w| w[0] < w[1]));
        let sting = s
            .peers(&PeersQuery {
                all: true,
                host: Some("sting"),
                ..PeersQuery::default()
            })
            .unwrap();
        assert_eq!(sting["peers"].as_array().unwrap().len(), 3);
        let bounded = s
            .peers(&PeersQuery {
                all: true,
                limit: Some(10_000),
                ..PeersQuery::default()
            })
            .unwrap();
        assert_eq!(bounded["peers"].as_array().unwrap().len(), 10);
        assert_eq!(
            s.peers(&PeersQuery {
                cursor: Some("not an id"),
                ..PeersQuery::default()
            })
            .unwrap_err(),
            "invalid cursor"
        );
        assert_eq!(
            s.peers(&PeersQuery {
                host: Some("a/b"),
                ..PeersQuery::default()
            })
            .unwrap_err(),
            "invalid host"
        );
    }

    /// A4 (TIN-7577): two live processes of one session each find their
    /// own row by proc_start; without a filter the latest one wins.
    #[test]
    fn peers_filters_proc_start_before_it_keeps_one_row_per_session() {
        let s = Store::memory().unwrap();
        reg(&s, "claude", "host-b", 1, "shared", "p1");
        reg(&s, "claude", "host-b", 2, "shared", "p2");
        let ids = |proc_start: Option<&str>| -> Vec<String> {
            let page = s
                .peers(&PeersQuery {
                    proc_start,
                    ..PeersQuery::default()
                })
                .unwrap();
            page["peers"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| p["agent_id"].as_str().unwrap().to_owned())
                .collect()
        };
        assert_eq!(ids(Some("p1")), vec!["claude:host-b:1:shared"]);
        assert_eq!(ids(Some("p2")), vec!["claude:host-b:2:shared"]);
        assert_eq!(ids(None), vec!["claude:host-b:2:shared"]);
        // pid 1 registers again: it is now the latest, and pid 2 still
        // finds its own row.
        reg(&s, "claude", "host-b", 1, "shared", "p1");
        assert_eq!(ids(Some("p2")), vec!["claude:host-b:2:shared"]);
        assert_eq!(ids(Some("p1")), vec!["claude:host-b:1:shared"]);
        assert_eq!(ids(None), vec!["claude:host-b:1:shared"]);
        assert!(ids(Some("p3")).is_empty());
    }

    /// A5 (TIN-7577): a send reports where mail to its recipient stands,
    /// and the stored envelope never carries it.
    #[test]
    fn send_reports_the_recipient_state() {
        let (s, clock) = clocked();
        let to = "pi:host-b:2:b";
        let send = |body: &str| {
            s.send(&json!({"from":"claude:host-a:1:a","to":to,"ticket":"none","body":body}))
                .unwrap()
        };
        assert_eq!(send("before").recipient_state, "unregistered");
        let row = reg(&s, "pi", "host-b", 2, "b", "p");
        assert_eq!(send("live").recipient_state, "live");
        clock.advance(901);
        assert_eq!(send("idle").recipient_state, "idle");
        clock.advance(20_700);
        assert_eq!(send("gone").recipient_state, "gone");
        s.end(to, row["end_token"].as_str().unwrap()).unwrap();
        let msg = json!({"from":"claude:host-a:1:a","to":to,"ticket":"none","body":"ended",
            "msg_id":Ulid::new().to_string()});
        let first = s.send(&msg).unwrap();
        assert!(first.created);
        assert_eq!(first.recipient_state, "ended");
        // A replay carries it too.
        let replay = s.send(&msg).unwrap();
        assert!(!replay.created);
        assert_eq!(replay.recipient_state, "ended");
        assert!(replay.envelope.get("recipient_state").is_none());
        // A replay that names another `to` answers for the stored
        // recipient, not for the id it names.
        reg(&s, "codex", "host-b", 3, "c", "p");
        let mut elsewhere = msg.clone();
        elsewhere["to"] = json!("codex:host-b:3:c");
        let replay = s.send(&elsewhere).unwrap();
        assert!(!replay.created);
        assert_eq!(replay.envelope["to"], to);
        assert_eq!(replay.recipient_state, "ended");
        // A submitted envelope never carries recipient_state.
        let mut forged = msg.clone();
        forged["msg_id"] = json!(Ulid::new().to_string());
        forged["recipient_state"] = json!("live");
        assert_eq!(s.send(&forged).unwrap_err(), "unknown envelope field");
        let read = s.inbox(to, 10).unwrap();
        let messages = read["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 5);
        assert!(messages.iter().all(|m| m.get("recipient_state").is_none()));
    }

    /// TIN-7577: a body keeps its newlines and tabs through send, inbox
    /// and thread. CRLF becomes LF; a lone CR and every other control
    /// character are dropped.
    #[test]
    fn a_body_keeps_its_lines_through_the_store() {
        let s = Store::memory().unwrap();
        let (from, to) = ("claude:host-a:1:a", "pi:host-b:2:b");
        // `thread` answers a registered reader only.
        reg(&s, "pi", "host-b", 2, "b", "p");
        let raw = "line one\r\n\tindented\nlast\rx\u{1b}[2J\u{7}\u{85}\0end\n";
        let want = "line one\n\tindented\nlastx[2Jend\n";
        let sent = s
            .send(&json!({"from":from,"to":to,"ticket":"none","body":raw}))
            .unwrap();
        assert!(sent.created);
        assert_eq!(sent.envelope["body"], want);
        let stored: String =
            s.0.lock()
                .unwrap()
                .query_row("SELECT body FROM messages", [], |r| r.get(0))
                .unwrap();
        assert_eq!(stored, want);
        let read = s.inbox(to, 10).unwrap();
        assert_eq!(read["messages"][0]["body"], want);
        let thread_id = sent.envelope["thread_id"].as_str().unwrap();
        let thread = s.thread(to, thread_id, None, None).unwrap();
        assert_eq!(thread["messages"][0]["body"], want);
        // A body of only line breaks and tabs is kept as sent.
        let blank = s
            .send(&json!({"from":from,"to":to,"ticket":"none","body":"\n\t\n"}))
            .unwrap();
        assert_eq!(blank.envelope["body"], "\n\t\n");
        // The 16 KiB limit counts the cleaned body: dropped CRs do not count.
        let long = format!("{}{}", "a".repeat(16384), "\r".repeat(10));
        let kept = s
            .send(&json!({"from":from,"to":to,"ticket":"none","body":long}))
            .unwrap();
        assert_eq!(kept.envelope["body"].as_str().unwrap().len(), 16384);
        let over = format!("{}\n", "a".repeat(16384));
        assert_eq!(
            s.send(&json!({"from":from,"to":to,"ticket":"none","body":over}))
                .unwrap_err(),
            "body exceeds 16 KiB"
        );
    }

    /// TIN-7577: a retry of a message that v0.5.0 or older stored without
    /// its newlines is still a replay after the upgrade. The store is
    /// opened once in the v0.5.0 shape, with a flat body and no
    /// `body_lines` column, and the upgrade adds the column at 0. Any other
    /// body under that msg_id is still a conflict (SWB-R46).
    #[test]
    fn a_retry_of_a_message_stored_before_v051_without_newlines_is_a_replay() {
        let path = std::env::temp_dir().join(format!("swb-lines-{}.sqlite3", Ulid::new()));
        let msg_id = Ulid::new().to_string();
        let msg = json!({"from":"claude:host-a:1:a","to":"pi:host-b:2:b","ticket":"none",
            "body":"one\ntwo","msg_id":msg_id});
        {
            let s = Store::open(&path).unwrap();
            assert!(s.send(&msg).unwrap().created);
            s.0.lock()
                .unwrap()
                .execute_batch(
                    "UPDATE messages SET body='onetwo'; ALTER TABLE messages DROP COLUMN body_lines;",
                )
                .unwrap();
        }
        let s = Store::open(&path).unwrap();
        let lines: i64 =
            s.0.lock()
                .unwrap()
                .query_row("SELECT body_lines FROM messages", [], |r| r.get(0))
                .unwrap();
        assert_eq!(lines, 0);
        assert!(!s.send(&msg).unwrap().created);
        let mut tab = msg.clone();
        tab["body"] = json!("one\ttwo");
        assert!(!s.send(&tab).unwrap().created);
        let mut flat = msg.clone();
        flat["body"] = json!("onetwo");
        assert!(!s.send(&flat).unwrap().created);
        let mut other = msg.clone();
        other["body"] = json!("one\nthree");
        assert_eq!(s.send(&other).unwrap_err().kind, ErrorKind::Conflict);
        drop(s);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(format!("{}-wal", path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", path.display()));
    }

    /// TIN-7577: a body that v0.5.1 stored is compared exactly. A retry that
    /// adds a newline, tab or CRLF to a flat body is a conflict, and so is a
    /// flat retry of a multi-line body (SWB-R46).
    #[test]
    fn a_retry_that_changes_the_lines_of_a_v051_body_is_a_conflict() {
        let s = Store::memory().unwrap();
        let send = |body: &str, msg_id: &str| {
            s.send(
                &json!({"from":"claude:host-a:1:a","to":"pi:host-b:2:b","ticket":"none",
                "body":body,"msg_id":msg_id}),
            )
        };
        let flat_id = Ulid::new().to_string();
        assert!(send("onetwo", &flat_id).unwrap().created);
        for retry in ["one\ntwo", "one\ttwo", "one\r\ntwo", "onetwo\n"] {
            assert_eq!(
                send(retry, &flat_id).unwrap_err().kind,
                ErrorKind::Conflict,
                "{retry:?}"
            );
        }
        assert!(!send("onetwo", &flat_id).unwrap().created);
        let multi_id = Ulid::new().to_string();
        assert!(send("one\ntwo", &multi_id).unwrap().created);
        for retry in ["onetwo", "one\ttwo", "one\n\ntwo"] {
            assert_eq!(
                send(retry, &multi_id).unwrap_err().kind,
                ErrorKind::Conflict,
                "{retry:?}"
            );
        }
        // CRLF is stored as LF, so the same text sent with CRLF is a replay.
        assert!(!send("one\r\ntwo", &multi_id).unwrap().created);
        let lines: Vec<i64> =
            s.0.lock()
                .unwrap()
                .prepare("SELECT body_lines FROM messages")
                .unwrap()
                .query_map([], |r| r.get(0))
                .unwrap()
                .map(Result::unwrap)
                .collect();
        assert_eq!(lines, vec![1, 1]);
    }

    #[test]
    fn a_live_binding_makes_mail_to_an_old_pid_live() {
        let (s, clock) = clocked();
        let first = bind(&s, "hermes", "host-b", 100, "svc", None).unwrap();
        let old = first["agent_id"].as_str().unwrap().to_owned();
        // The old pid goes quiet; the restarted service binds again.
        clock.advance(1000);
        bind(&s, "hermes", "host-b", 200, "svc", Some(&token_of(&first))).unwrap();
        let state = |to: &str| {
            s.send(&json!({"from":"claude:host-a:1:a","to":to,"ticket":"none","body":"x"}))
                .unwrap()
                .recipient_state
        };
        assert_eq!(state(&old), "live");
        // A bound row without a token nonce is no binding to the inbox gate,
        // so the old pid's own row decides.
        s.0.lock()
            .unwrap()
            .execute(
                "UPDATE sessions SET token_nonce=NULL WHERE agent_id!=?1 AND harness='hermes'",
                [&old],
            )
            .unwrap();
        assert_eq!(state(&old), "idle");
        // An unbound session that went quiet stays gone, and another pid
        // of it with no row is unregistered.
        reg(&s, "pi", "host-b", 2, "b", "p");
        clock.advance(21_601);
        assert_eq!(state("pi:host-b:2:b"), "gone");
        assert_eq!(state("pi:host-b:3:b"), "unregistered");
    }

    #[test]
    fn a_v02_store_gains_the_new_columns_and_its_rows_cannot_be_ended() {
        let path = std::env::temp_dir().join(format!("swb-migrate-{}.sqlite3", Ulid::new()));
        {
            let db = Connection::open(&path).unwrap();
            db.execute_batch("CREATE TABLE sessions (
                agent_id TEXT PRIMARY KEY, harness TEXT NOT NULL, host TEXT NOT NULL,
                proc_start TEXT NOT NULL, last_seen INTEGER NOT NULL, ended INTEGER NOT NULL DEFAULT 0);
                INSERT INTO sessions VALUES('claude:honey:1:old','claude','honey','p',unixepoch(),0);").unwrap();
        }
        let s = Store::open(&path).unwrap();
        let listed = s.peers(&PeersQuery::default()).unwrap();
        assert_eq!(listed["peers"][0]["agent_id"], "claude:honey:1:old");
        let session: String =
            s.0.lock()
                .unwrap()
                .query_row("SELECT session_id FROM sessions", [], |r| r.get(0))
                .unwrap();
        assert_eq!(session, "old");
        assert_eq!(
            s.end("claude:honey:1:old", &"a".repeat(64)).unwrap()["state"],
            "unknown"
        );
        drop(s);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(format!("{}-wal", path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", path.display()));
    }

    #[test]
    fn send_and_inbox_leave_retention_to_the_interval() {
        let (s, clock) = clocked();
        let to = "pi:sting:2:b";
        let id = s
            .send(&json!({"from":"claude:honey:1:a","to":to,"ticket":"none","body":"x"}))
            .unwrap()
            .envelope["msg_id"]
            .as_str()
            .unwrap()
            .to_owned();
        s.ack(to, &id).unwrap();
        clock.advance(604_800);
        s.send(&json!({"from":"claude:honey:1:a","to":to,"ticket":"none","body":"y"}))
            .unwrap();
        s.inbox(to, 10).unwrap();
        assert_eq!(row_count(&s), 2, "send or inbox pruned");
        s.prune().unwrap();
        assert_eq!(row_count(&s), 1);
    }

    #[test]
    fn errors_carry_a_kind_and_a_code() {
        let s = Store::memory().unwrap();
        assert_eq!(
            s.ack("pi:sting:2:b", "01ARZ3NDEKTSV4RRFFQ69G5FAV")
                .unwrap_err()
                .code(),
            "message_unavailable"
        );
        assert_eq!(s.inbox("nope", 1).unwrap_err().code(), "invalid_argument");
        let input = json!({"from":"claude:honey:1:a","to":"pi:sting:2:b","ticket":"none","body":"a","msg_id":"01ARZ3NDEKTSV4RRFFQ69G5FAV"});
        s.send(&input).unwrap();
        let mut other = input;
        other["body"] = json!("b");
        assert_eq!(s.send(&other).unwrap_err().code(), "msg_id_conflict");
    }
    #[test]
    fn thread_sequence_survives_pruned_latest_ack() {
        let s = Store::memory().unwrap();
        let from = "claude:honey:1:a";
        let to = "pi:sting:2:b";
        let thread = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
        let first = s
            .send(&json!({"from":from,"to":to,"ticket":"none","body":"one","thread_id":thread}))
            .unwrap();
        let second = s
            .send(&json!({"from":from,"to":to,"ticket":"none","body":"two","thread_id":thread}))
            .unwrap();
        assert_eq!(first.envelope["seq"], 1);
        assert_eq!(second.envelope["seq"], 2);
        let id = second.envelope["msg_id"].as_str().unwrap();
        s.ack(to, id).unwrap();
        s.0.lock()
            .unwrap()
            .execute(
                "UPDATE messages SET acked_at=unixepoch()-604801 WHERE msg_id=?1",
                [id],
            )
            .unwrap();
        s.prune().unwrap();
        let third = s
            .send(&json!({"from":from,"to":to,"ticket":"none","body":"three","thread_id":thread}))
            .unwrap();
        assert_eq!(third.envelope["seq"], 3);
    }

    #[test]
    fn reopening_prunes_old_bodies_without_mailbox_traffic() {
        let path = std::env::temp_dir().join(format!("swb-retention-{}.sqlite3", Ulid::new()));
        let from = "claude:honey:1:a";
        let to = "pi:sting:2:b";
        let retained;
        {
            let store = Store::open(&path).unwrap();
            let send = |body| {
                store
                    .send(&json!({"from":from,"to":to,"ticket":"none","body":body}))
                    .unwrap()
                    .envelope["msg_id"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            };
            let old_acked = send("old acked body");
            let old_unacked = send("old unacked body");
            retained = send("recent body");
            store.ack(to, &old_acked).unwrap();
            let db = store.0.lock().unwrap();
            db.execute(
                "UPDATE messages SET acked_at=unixepoch()-604801 WHERE msg_id=?1",
                [&old_acked],
            )
            .unwrap();
            db.execute(
                "UPDATE messages SET created_at=unixepoch()-2592001 WHERE msg_id=?1",
                [&old_unacked],
            )
            .unwrap();
        }
        let reopened = Store::open(&path).unwrap();
        let db = reopened.0.lock().unwrap();
        let bodies: Vec<String> = db
            .prepare("SELECT body FROM messages ORDER BY created_at")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(bodies, vec!["recent body"]);
        let surviving: String = db
            .query_row("SELECT msg_id FROM messages", [], |row| row.get(0))
            .unwrap();
        assert_eq!(surviving, retained);
        drop(db);
        drop(reopened);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(format!("{}-wal", path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", path.display()));
    }

    #[test]
    fn sender_and_reader_refresh_only_their_own_lease() {
        let s = Store::memory().unwrap();
        let sender = s.register(&json!({"harness":"claude","host":"honey","pid":1,"session_id":"a","proc_start":"1"})).unwrap();
        let receiver = s
            .register(
                &json!({"harness":"pi","host":"sting","pid":2,"session_id":"b","proc_start":"2"}),
            )
            .unwrap();
        let from = sender["agent_id"].as_str().unwrap();
        let to = receiver["agent_id"].as_str().unwrap();
        s.0.lock()
            .unwrap()
            .execute("UPDATE sessions SET last_seen=unixepoch()-30000", [])
            .unwrap();
        let sent = s
            .send(&json!({"from":from,"to":to,"ticket":"none","body":"hello"}))
            .unwrap();
        let peers = s.peers(&all()).unwrap();
        assert_eq!(peers["peers"][0]["state"], "live");
        assert_eq!(peers["peers"][1]["state"], "gone");
        s.inbox(to, 1).unwrap();
        let peers = s.peers(&all()).unwrap();
        assert_eq!(peers["peers"][1]["state"], "live");
        s.0.lock()
            .unwrap()
            .execute("UPDATE sessions SET last_seen=unixepoch()-30000", [])
            .unwrap();
        s.ack(to, sent.envelope["msg_id"].as_str().unwrap())
            .unwrap();
        let peers = s.peers(&all()).unwrap();
        assert_eq!(peers["peers"][0]["state"], "gone");
        assert_eq!(peers["peers"][1]["state"], "live");
    }

    #[test]
    fn peers_renews_only_registered_explicit_caller() {
        let s = Store::memory().unwrap();
        let caller = "claude:honey:1:a";
        let other = "pi:sting:2:b";
        for (harness, host, pid, session_id) in
            [("claude", "honey", 1, "a"), ("pi", "sting", 2, "b")]
        {
            s.register(&json!({"harness":harness,"host":host,"pid":pid,"session_id":session_id,"proc_start":"1"})).unwrap();
        }
        s.0.lock()
            .unwrap()
            .execute("UPDATE sessions SET last_seen=unixepoch()-30000", [])
            .unwrap();
        let passive = s.peers(&all()).unwrap();
        assert_eq!(passive["peers"][0]["state"], "gone");
        assert_eq!(passive["peers"][1]["state"], "gone");
        assert_eq!(
            s.peers(&refresh("codex:honey:3:unknown")).unwrap_err(),
            "unregistered me"
        );
        assert_eq!(s.peers(&refresh("invalid")).unwrap_err(), "invalid me");
        assert_eq!(
            s.peers(&refresh("codex:honey:3:unknown")).unwrap_err().kind,
            ErrorKind::Unregistered
        );
        let active = s.peers(&refresh(caller)).unwrap();
        assert_eq!(active["peers"][0]["agent_id"], caller);
        assert_eq!(active["peers"][0]["state"], "live");
        assert_eq!(active["peers"][1]["agent_id"], other);
        assert_eq!(active["peers"][1]["state"], "gone");
    }

    #[test]
    fn metrics_excludes_expired_message_without_inbox_poll() {
        let s = Store::memory().unwrap();
        let sent = s.send(&json!({"from":"claude:honey:1:a","to":"pi:sting:2:b","ticket":"none","body":"hello"})).unwrap();
        assert!(s.metrics().unwrap().contains("swb_mailbox_unacked 1\n"));
        s.0.lock()
            .unwrap()
            .execute(
                "UPDATE messages SET expires_at=unixepoch()-1 WHERE msg_id=?1",
                [sent.envelope["msg_id"].as_str().unwrap()],
            )
            .unwrap();
        assert!(s.metrics().unwrap().contains("swb_mailbox_unacked 0\n"));
    }

    #[test]
    fn rejects_unknown_and_ill_typed_envelope_fields() {
        let s = Store::memory().unwrap();
        let base =
            json!({"from":"claude:honey:1:a","to":"pi:sting:2:b","ticket":"none","body":"hello"});
        let mut state = base.clone();
        state["state"] = json!("operator");
        assert_eq!(s.send(&state).unwrap_err(), "unknown envelope field");
        let mut expires = base.clone();
        expires["reply_expires"] = json!({"unexpected":"object"});
        assert_eq!(s.send(&expires).unwrap_err(), "invalid reply_expires");
        expires["reply_expires"] = json!("now");
        assert_eq!(s.send(&expires).unwrap_err(), "invalid reply_expires");
        let mut ruling = base.clone();
        ruling["ruling"] = json!("");
        assert_eq!(s.send(&ruling).unwrap_err(), "invalid ruling");
        let mut extra = base;
        extra["authority"] = json!("operator");
        let sent = s.send(&extra).unwrap();
        assert_eq!(sent.envelope["authority"], "peer");
        assert!(sent.envelope.get("state").is_none());
    }
    #[test]
    fn round_trip_and_opaque_conflict() {
        let s = Store::memory().unwrap();
        let me = "claude:honey:1:a";
        let to = "pi:sting:2:b";
        s.register(
            &json!({"harness":"claude","host":"honey","pid":1,"session_id":"a","proc_start":"1"}),
        )
        .unwrap();
        let input = json!({"from":me,"to":to,"ticket":"TIN-4655","body":"hello","msg_id":"01ARZ3NDEKTSV4RRFFQ69G5FAV"});
        let sent = s.send(&input).unwrap();
        assert_eq!(sent.envelope["authority"], "peer");
        assert!(sent.created);
        let retry = s.send(&input).unwrap();
        assert_eq!(retry.envelope["seq"], 1);
        assert!(!retry.created);
        let conflict=s.send(&json!({"from":me,"to":to,"ticket":"TIN-4655","body":"different","msg_id":"01ARZ3NDEKTSV4RRFFQ69G5FAV"})).unwrap_err();
        assert_eq!(conflict, "msg_id conflict");
        let got = s.inbox(to, 10).unwrap();
        assert_eq!(got["messages"][0]["delivery_count"], 1);
        assert_eq!(s.inbox(to, 10).unwrap()["messages"][0]["delivery_count"], 2);
        assert_eq!(
            s.ack(to, "01ARZ3NDEKTSV4RRFFQ69G5FAV").unwrap()["state"],
            "acked"
        );
        assert!(
            s.inbox(to, 10).unwrap()["messages"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }
    #[test]
    fn wal_survives_reopen_and_sequences_thread() {
        let path = std::env::temp_dir().join(format!("swb-{}.sqlite3", Ulid::new()));
        let from = "claude:honey:1:a";
        let to = "pi:sting:2:b";
        let thread = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
        {
            let s = Store::open(&path).unwrap();
            let mode: String =
                s.0.lock()
                    .unwrap()
                    .query_row("PRAGMA journal_mode", [], |r| r.get(0))
                    .unwrap();
            assert_eq!(mode, "wal");
            let first = s
                .send(
                    &json!({"from":from,"to":to,"ticket":"none","body":"first","thread_id":thread}),
                )
                .unwrap();
            assert_eq!(first.envelope["seq"], 1);
        }
        {
            let s = Store::open(&path).unwrap();
            let second = s.send(&json!({"from":from,"to":to,"ticket":"none","body":"second","thread_id":thread})).unwrap();
            assert_eq!(second.envelope["seq"], 2);
            assert_eq!(
                s.inbox(to, 10).unwrap()["messages"]
                    .as_array()
                    .unwrap()
                    .len(),
                2
            );
        }
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(format!("{}-wal", path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", path.display()));
    }
}
