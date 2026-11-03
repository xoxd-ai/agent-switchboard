# Security policy

## Supported versions

| Version | Supported |
| --- | --- |
| 0.5.1 | Yes (current release) |
| `main` | Yes: fixes land here first |
| 0.5.0 and earlier | No: upgrade to 0.5.1 |
| 0.2.0 | Never published; do not use the tag |

Pre-1.0, only the latest release and `main` get fixes. An older client
can still talk to a 0.5.x broker: a 0.4.0 client works for unbound ids
only, and a 0.5.0 client ignores the fields 0.5.1 adds. Those clients
get no fixes.

## Deployment model: tailnet-only, no listener authentication

`swb` is designed to run on a private network that already authenticates its
members: loopback, or a tailnet such as Tailscale where every peer is a
known device. **`swb` does not authenticate its listeners, in any
release.** Up to v0.4.0, and for every unbound id from v0.5.0, agent
identity is self-asserted: any client that can reach the broker's main
listener can register as any agent id (`harness:host:pid:session_id`), read
that agent's inbox, send and acknowledge. In v0.2.2 it can also end any
listed session (residual 1 below). From v0.3.0, ending a session needs the
end token its registration returned, but anyone can re-register an agent id
and receive a fresh token, so the token is not takeover-proof. Messages are
marked `authority: peer` and are teammate information only, which limits
what a spoofed message can make an agent do, but it does not stop the
spoofing.

### Opt-in sender binding (v0.5.0)

From v0.5.0 a session can opt in to sender binding
([ADR-0003](docs/adr/0003-sender-binding.md), SWB-R66 to SWB-R68).
`register` with `bind: true` (`SWB_BIND=1` for the `swb` client) binds the
session's `harness:host:session_id` and returns a session token. While the
binding is live:

- `send` with that id as `from`, and `inbox`, `ack`, `claim`, `release`,
  `handoff` and `end` with it as `me`, need the token. Without it they
  answer 401 `unauthorized` and change nothing. A spoofed `send` from a
  bound id therefore fails.
- A new `register` of the same key, from any pid, needs the current token
  and rotates it. The grace register below is the one exception: it
  needs the token the latest rotation replaced and rotates nothing. Only a
  holder of the current token can take a live binding over by
  registering again.
- From v0.5.1 the token a rotation replaced stays valid for 60 seconds
  (SWB-R67 as amended, TIN-7577 comments `41c4704d` and `ff9be66f`). In
  that window it can act as the bound id, and a `register` with it from
  the binding's own agent id returns the current session token and end
  token and mints nothing. This is the grace register. So a client whose
  register reply was lost recovers, and two registers that present one
  saved token get one answer. The replaced token cannot move the binding
  to another pid, a token two rotations old is refused, and so is the
  replaced token after 60 seconds. A bound session's end token has the
  same grace: while the session token a rotation replaced is in its
  grace, the end token replaced with it still ends the session. An
  unbound session's end token has no grace.
- The graces form a chain of at most 2 (TIN-7577 comment `2f1439dc`,
  "Allow one more grace"). A token the client received from a register
  that minted it gets the first grace when it is replaced. A token a grace
  register handed back gets one more grace when it is replaced, and a
  token handed back inside that second grace gets none. So two register
  replies lost in a row recover, and a third does not. A register that
  mints a token the client receives starts a new chain.
- So a rotation still shuts out a second holder within two of the
  client's registers. A holder of a replaced token can fetch the current
  token in the first grace and again in the second. When the client next
  registers, the token the second grace handed out is refused at once.
  Two holders cannot keep each other's tokens alive for longer than 2
  grace windows of the client's registers.
- The 60 seconds run on the wall clock and on a monotonic clock in the
  broker process, and the grace ends at whichever reaches 60 first. No
  wall clock step can lengthen it. A step back larger than the time since
  the rotation ends it at once, because the wall reading then falls before
  the rotation. A step forward ends it once the wall reading passes 60
  seconds. The monotonic part is held in memory only, so a broker
  restart ends every grace and the replaced token is then refused: the
  grace fails closed. No token is stored for the grace. The broker derives
  the current tokens again from its two token keys, `session_token_key`
  and `end_token_key`, and the row's nonce.
- A register that a grace answered writes the audit line
  `register_grace` with the grace that answered it, not `register`, so an
  operator can see a fetch of the current token.
- `peers` rows say `bound`.

A binding lapses when its session ends or after 6 hours without a call;
the key then registers like an unbound one. A lapse is permanent: a later
call, with or without a token, cannot bring the binding or its old token
back. The broker checks the token inside each operation's transaction, and
an `inbox` long-poll checks it on every read. The client keeps the token in a
0600 file under `$SWB_STATE_DIR/bindings/` and never prints it.

Each sender also has a token bucket for `send` (default burst 20, refill 30
per minute, set by `SWB_SEND_BURST` and `SWB_SEND_REFILL_PER_MINUTE`). An
empty bucket answers 429 `rate_limited`. Only a send that stores a new
message spends a token. The bucket stops a client that loops under one
agent id. It does not stop a flood that changes its unbound `from` on every
send: the tailnet ACL bounds that client. The broker keeps at most 4096
buckets.

Binding does not change what a message can make an agent do: `authority`
stays `peer` on every message (SWB-R14).

Residuals of binding:

- The session token is a bearer secret. Anyone who can read the 0600 file,
  or a model context that received an MCP `register` result, can act as
  the bound id until the next register rotates it, and for 60 seconds
  after that (v0.5.1). During those 60 seconds two tokens are valid, and
  a register with the replaced one fetches the current one. A holder who
  fetches the current token keeps it until the client next registers,
  however long that takes. It holds the same token as the client, so
  either of them can then register and take the binding. If the chain is
  not spent, the other has one more grace to fetch the new token, and
  after the next register it is refused. A rotation removes a leaked token for good once its
  holder has not registered within 60 seconds of it, or once the client
  has registered twice after the holder's first fetch.
- The grace still leaves these lockouts, each until the 6-hour lapse:
  - A first bind whose reply is lost. The client holds no token, and a
    first bind records no previous token.
  - A register reply lost just before a broker restart, which ends every
    grace.
  - A third register reply lost in a row: a reply lost on a rotation
    whose replaced token the second grace had handed back. Two concurrent
    registers that present one saved token count as one grace answer. On
    a fresh chain, after them one lost reply still recovers and a second
    in a row does not. If the pair follows an earlier grace answer, it
    spends the second grace, and the next lost reply locks the session
    out.
  - A lost reply that the client does not follow with a register within
    60 seconds, for example when its one retry also misses the deadline
    and no hook runs in time.
  - A wall clock step inside the grace in either direction: a step back
    larger than the time since the rotation ends it at once, and a step
    forward ends it once the wall reading passes 60 seconds. This fails
    closed, as intended.
- The broker keeps `session_token_key`, `end_token_key` and every row's
  token nonce in its SQLite database. Read access to the database file
  therefore yields every live session and end token, as it has since
  v0.5.0. The grace does not change this.
- A caller with a wrong token can tell that the binding rotated within the
  last 60 seconds from the time a refusal takes. `peers` already shows
  `registered_seq`, so this reveals little.
- `thread`, `heartbeat`, `peers` and `claims` need no token. Anyone can
  read a bound id's threads and refresh its lease, which also delays the
  6-hour lapse.
- Unbound ids keep every v0.4.0 property above, including takeover by
  re-registering. Anyone can also spend an unbound sender's send bucket by
  sending as it.
- The token travels in the request body over plain http, so it relies on
  the tailnet's encryption, like every message body.

So:

- Never expose the broker's listeners to the internet or to an untrusted
  network.
- Set `SWB_MCP_ALLOWED_HOSTS` to the exact name clients use, and keep
  `SWB_ALLOWED_ORIGINS` empty unless you have a browser client.
- Keep the metrics listener (`SWB_METRICS_LISTEN`) off any shared Service;
  it is not Host-guarded.
- Never put a secret in a message body.

### Delivery fixes and refusal counters (v0.5.1)

v0.5.1 (TIN-7577) fixes message delivery and counts send refusals. What
each change reveals:

- **`recipient_state`.** A `send` result says whether the recipient is
  live, idle, gone, ended, superseded or unregistered. This is new
  disclosure. It
  answers for the exact id sent to, an older pid of a session included,
  which default `peers` hides behind the session's latest row. It also
  shows whether that id's harness, host and session id hold a live
  binding. To ask, a sender must store a message in that mailbox and
  spend a bucket token. An idempotent retry spends no token, so it
  answers for the stored recipient only and cannot ask about another id.
  The stored and delivered envelope never carry the field.
- **`retry_after`.** A 429 states the seconds until the sender's own
  bucket refills, in the body and a `Retry-After` header. It says nothing
  about another sender.
- **The `peers` proc_start filter.** It now matches any registration row
  with that stamp, not only the latest row of a session. The stamp has
  granted nothing since v0.3.0, and listings still never return it. Rows
  listed with it carry `registered_seq`, the store-wide registration
  counter at that row's latest register. It orders registrations and
  reveals roughly how many registrations the broker has taken. Any
  client can read it: a client that registers a session with its own
  stamp reads the counter from its own row.
- **`superseded`.** `recipient_state` says when a newer pid of the
  recipient's harness, host and session id registered after the id sent
  to. Default `peers` already shows that newer row as the session's
  latest one.
- **`inbox` `after`.** A cursor resolves only against the caller's own
  mailbox, by its delivery key when the id is bound. Another mailbox's id
  answers `unknown after`, the same answer as an id that never existed,
  so `after` cannot test whether an id exists outside the mailbox the
  caller reads.
- **Metrics.** The new series carry no identity. Their only label is the
  harness kind, one of 8 fixed values. The metrics listener is still not
  Host-guarded, as before.
- **`send_refused` line.** It carries the harness kind, `retry_after` and
  the route, never an agent id or a body.

## Residuals fixed in v0.3.0

These applied to v0.2.2. v0.3.0 fixed both (R-C944) and shipped the fixes
with the clients that need the changed routes; see
[docs/releases/v0.3.0-migration.md](docs/releases/v0.3.0-migration.md).

1. **`proc_start` disclosure lets anyone end any session.** `GET /v1/peers`
   (and the MCP `peers` tool) returns every session's `proc_start`, and
   `POST /v1/end` accepts `proc_start` as its only check. Any client that
   can reach the broker can therefore end any listed session. v0.3.0 issues
   a per-session end token at registration, returns it only to the
   registering session, checks it on `/v1/end`, and removes `proc_start`
   from `peers`. `peers` also returns live sessions only, deduplicated by
   session id.
2. **Cross-site GET side effects.** v0.2.2 refuses any request that carries
   a non-allowed `Origin` and checks `Host` on every route, which stops
   DNS-rebinding writes. Browsers send no `Origin` on simple GETs (image or
   script loads, navigations), and over plain http to a non-loopback name
   no `Sec-Fetch-*` headers either. A web page open in a browser on the same
   network that already knows an agent id can therefore trigger, blind:
   - `GET /v1/peers?me=<id>`, which refreshes that session's lease and can
     revive an ended session;
   - `GET /v1/inbox?me=<id>`, which marks messages fetched and counts a
     delivery.

   The page cannot read the reply and cannot register, send or acknowledge,
   and fetched messages stay deliverable. v0.3.0 moves lease refresh and
   inbox to POST, so no GET has a side effect.

## Reporting a vulnerability

Please report privately, not with details in a public issue or pull request.

- If the repository's
  [Security tab](https://github.com/xoxd-ai/agent-switchboard/security)
  shows a **Report a vulnerability** button, use it (GitHub private
  vulnerability reporting).
- Otherwise, open a public issue titled "Security contact request" with no
  details, and a maintainer will set up a private channel.

Include the affected version or commit, how to reproduce, and the impact you
see. A maintainer will acknowledge the report and agree a disclosure date
with you once a fix is ready. The residuals above and the self-asserted
identity of unbound ids are already known and public; no report is needed
for them.
