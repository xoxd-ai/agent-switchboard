# swb reference

Settings, routes, tools and verbs for `swb` v0.4.0 (SWB-R64). v0.4.0 keeps
the v0.3.0 routes and adds the harness kinds `hermes` and `kimicode`, which
a v0.3.0 broker or client refuses (see [Harness kinds](#harness-kinds) and
the [CHANGELOG](../CHANGELOG.md) upgrade notes). v0.3.0 broke the v0.2 wire
format; what changed from v0.2.2 and the upgrade order are in
[releases/v0.3.0-migration.md](releases/v0.3.0-migration.md). The design and
its rulings are in [ADR-0001](adr/0001-agent-switchboard.md); a walkthrough
is the [README quickstart](../README.md#quickstart); harness configuration is
in [integrations/](../integrations/).

## Agent identity

An agent id is `harness:host:pid:session_id`, for example
`claude:sting:40001:sess-a`. `register` (and the SessionStart hook) returns
it, together with an end token (v0.3.0) that only the registering session
receives. Sessions are `live` for 15 minutes after their last call, `idle` up
to 6 hours, then `gone`. Messages are kept 7 days after acknowledgement and
30 days unacknowledged; a message expires after 72 hours by default (14 days
at most). The broker prunes expired rows at startup and every hour.

Identity is self-asserted: the end token stops a caller ending a session it
did not register, but anyone who can register can re-register any agent id
(see [SECURITY.md](../SECURITY.md)).

### Sender binding (unreleased, v0.5.0)

A session can opt in to sender binding
([ADR-0003](adr/0003-sender-binding.md), SWB-R66). `register` with
`bind: true` binds `harness:host:session_id` (the agent id without its pid)
and returns a `session_token` beside the end token. While the binding is
live:

| Call | Rule |
| --- | --- |
| `send` (as `from`), `inbox`, `ack`, `claim`, `release`, `handoff`, `end` (as `me`) | need `session_token`; otherwise 401 `unauthorized` |
| `register` of the same key, any pid | needs the current `session_token`, keeps the binding and rotates the token |
| an earlier pid of the key | refused, even with the token |
| `inbox` and `ack` | read mail addressed to any pid of the key, so a restart keeps unread mail |
| `peers`, `claims`, `heartbeat`, `thread` | no token |

A binding lapses when its session ends or after 6 hours without a call,
and a lapse is permanent: no later call revives it or its old token.
An unbound id behaves as in v0.4.0 and ignores a token it sends. The
`swb` client binds with `SWB_BIND=1` and keeps the token in a 0600 file
(see `SWB_STATE_DIR` below, SWB-R67).

### Harness kinds

The store, the broker's `register` schema, the envelope schema and the CLI
read one list (`swb_store::HARNESS_KINDS`). A broker and its clients must
know the same kinds: an older client refuses an agent id with a newer kind.
A v0.3.0 client refuses them as a `send` or `reply` recipient, stops `peers`
paging at a `next_cursor` that names one, and a v0.3.0 broker refuses to
register them. `schemas/envelope-v3.schema.json` gained `hermes` and
`kimicode` in its `agent_id` pattern without a version change, because
ADR-0001 marks it a draft. A validator pinned to the older copy rejects the
new kinds.

| Kind | Harness |
| --- | --- |
| `claude` | Claude Code |
| `kimi` | Claude Code on a Kimi model |
| `codex` | Codex |
| `junie` | Junie |
| `opencode` | OpenCode |
| `pi` | Pi |
| `hermes` | a Hermes agent |
| `kimicode` | the Kimi Code CLI |

## Settings

| Variable | Read by | Default | Notes |
| --- | --- | --- | --- |
| `SWB_DB_PATH` | `serve` | `/var/lib/swb/swb.sqlite3` | set it when not running as the image's UID 65532 |
| `SWB_LISTEN` | `serve` | `0.0.0.0:8080` | MCP `/mcp` and REST `/v1/*` |
| `SWB_METRICS_LISTEN` | `serve` | `0.0.0.0:9090` | `/metrics` |
| `SWB_MCP_ALLOWED_HOSTS` | `serve` | `localhost,127.0.0.1,::1` | comma-separated `Host` allowlist for every route on `SWB_LISTEN` (`/mcp` and `/v1/*`); a deployment sets its tailnet name. A host without a port admits any port. Missing or unlisted Host is 403; an empty list refuses to start |
| `SWB_SEND_BURST` | `serve` | `20` | per-sender send bucket size (v0.5.0); `0` turns the send rate limit off |
| `SWB_SEND_REFILL_PER_MINUTE` | `serve` | `30` | sends a bucket regains per minute (v0.5.0); whole numbers up to 100000, another value refuses to start |
| `SWB_ALLOWED_ORIGINS` | `serve` | empty | comma-separated browser origins (`scheme://host[:port]` or `null`) admitted on `SWB_LISTEN`. Any request carrying another `Origin` is 403; requests without `Origin` (every `swb` and MCP client) pass |
| `SWB_BROKER_URL` | every client verb | none | `http://host:port` only: no path, no trailing `/`, no `https` |
| `SWB_HOST` | `hook`, `whoami`, `doctor` | none | host part of the agent id |
| `SWB_SESSION_PID` | `hook`, `whoami`, `doctor` | none | nonzero integer |
| `SWB_PROC_START` | `hook`, `whoami`, `doctor`, `channel` | none | process start stamp; `channel` filters its own row on it. `/v1/end` checks the end token, not this (v0.3.0) |
| `SWB_HARNESS` | `hook`, `whoami`, `doctor`, `channel` | hook: its argument | `claude`, `kimi`, `codex`, `junie`, `opencode`, `pi`, `hermes` or `kimicode` (see [Harness kinds](#harness-kinds)). `channel --tools full` pushes only when it is `claude` |
| `SWB_HOOK_FORMAT` | `hook` | JSON | `text` prints the UserPromptSubmit notice as plain text for a harness that adds a hook's stdout to its context. Any other value keeps the Claude Code `hookSpecificOutput` JSON |
| `SWB_STATE_DIR` | `hook`, `whoami --register`, `doctor --register`, every acting verb, `channel` | `$XDG_STATE_HOME/swb`, then `$HOME/.local/state/swb` | where end tokens are kept (`sessions/<agent_id>.token`, dir 0700, file 0600); files not rewritten for 7 days are pruned. A bound session's token is in `bindings/<harness>:<host>:<session_id>.token` (dir 0700, file 0600), rewritten on every register and never pruned by age |
| `SWB_BIND` | `hook`, `whoami --register` | unset | `1` or `true` asks `register` to bind the session (v0.5.0, ADR-0003) |
| `SWB_SESSION_ID` | `whoami`, `doctor` | none | hooks read `session_id` from stdin instead |
| `SWB_AGENT_ID` | `inbox`, `send`, `ack`, `heartbeat`, `thread`, `claim`, `release`, `handoff`, `claims --mine`, `doctor`, `channel` | none | `harness:host:pid:session_id`, as `register` returned it; `channel` falls back to the launcher identity |
| `SWB_CHANNEL_POLL_SECONDS` | `channel` | `5` | inbox poll interval, clamped to 2–60 |
| `SWB_METRICS_URL` | `doctor` | none | `http://host:port` of `SWB_METRICS_LISTEN`; unset skips the `/metrics` check |

## REST routes

All on `SWB_LISTEN`, behind the Host and Origin guard. Every state change is
a POST with a JSON body (`Content-Type: application/json`).

| Route | Body or query | Notes |
| --- | --- | --- |
| `POST /v1/register` | `{harness, host, pid, session_id, proc_start, bind?, session_token?}` | returns `{agent_id, lease_seconds, end_token, bound}`, plus `session_token` when bound |
| `POST /v1/end` | `{me, end_token, session_token?}` | any other end token answers `{"state":"unknown"}` and ends nothing |
| `POST /v1/peers` | `{me?, all?, host?, limit?, cursor?, proc_start?}` | `me` renews that session's lease; `proc_start` keeps only registrations that used it, then one row per session among them (v0.5.1) |
| `GET /v1/peers` | `?all=&host=&limit=&cursor=` | passive listing; changes nothing |
| `POST /v1/send` | `{from, to, ticket, body, …, session_token?}` (envelope v3) | `ticket` is `TIN-<digits>` or `none`; a body is at most 16 KiB and keeps newline and tab, while CR and other control characters are removed (v0.5.1); an empty sender bucket answers 429; an invalid send or an idempotent retry spends no bucket token; the result adds `recipient_state` (v0.5.1); a 429 carries `retry_after` and a `Retry-After` header when the bucket refills (v0.5.1) |
| `POST /v1/inbox` | `{me, limit, wait_seconds, after?, session_token?}` | long-poll; oldest first in arrival order; a full page carries `next_after` (v0.5.1) |
| `POST /v1/ack` | `{me, msg_id, session_token?}` | |
| `POST /v1/heartbeat` | `{me}` | renews the lease only |
| `POST /v1/thread` | `{me, thread_id, after_seq?, limit?}` | one thread's messages that `me` sent or received, in seq order; pages of `limit` (default 20, at most 100) with `next_after_seq`; changes no delivery state |
| `POST /v1/claim` | `{me, subject?, exclusive?, lease_hours?, note?, handoff_id?, session_token?}` | see Claims below |
| `POST /v1/release` | `{me, claim_id, session_token?}` | your own claim only; releasing it again changes nothing |
| `POST /v1/handoff` | `{me, claim_id, to, note?, session_token?}` | offers your active claim to `to`; the same offer again returns the open receipt |
| `POST /v1/claims` | `{me?, subject?, holder?, all?, limit?}` | active claims (`all` adds released, handed-off and expired ones still kept); `me` renews that lease and adds its open `handoffs` |
| `GET /healthz` | | also on `SWB_METRICS_LISTEN`, where it is not Host-guarded |

`peers` lists live sessions by default (`all` adds idle, gone and ended),
one row per harness, host and session id, in pages of `limit` (default 50,
at most 200) with a `next_cursor`. A row is
`{agent_id, harness, host, last_seen, state, bound}`; `bound` is true
while the row's binding is live (v0.5.0, SWB-R68). `GET /v1/peers?me=` and
`GET /v1/inbox` answer 405.

From v0.5.1 (TIN-7577) an `inbox` answer is
`{messages, next_after}`. Messages come oldest first in arrival order.
`next_after` is the last `msg_id` of a full page and `null` otherwise.
Pass it as `after` to read the messages that arrived after it, so a
reader that holds a full page of unacknowledged mail can still reach the
next message. `after` must name a message of the caller's mailbox, in
any state. Any other id answers 400 `invalid_argument` with
`unknown after`, and so does a pruned one. A long-poll with `after` waits
for mail past it.

A `send` result adds `recipient_state` from v0.5.1. It is `live`, `idle`,
`gone`, `ended` or `unregistered`, and says whether anyone reads the
recipient's mail now. A live binding of the recipient's harness, host
and session id counts for every pid of it. The send succeeds in every
state, and the message waits in its mailbox until it expires. The stored
and delivered envelope never carry the field, and envelope v3 does not
list it: a submitted envelope with it answers `unknown envelope field`.
An idempotent retry answers for the stored recipient, whatever `to` it
names.

A send the rate limit refuses answers 429 `rate_limited`. From v0.5.1 the
body adds `retry_after`, the seconds until the sender's bucket refills,
and the REST answer carries the same value in a `Retry-After` header. A
bucket that never refills gives neither. The MCP error text carries the
same body.

`session_token` is required only for a bound id (see
[Sender binding](#sender-binding-unreleased-v050)).

Errors are `{"error", "code"}` with a status per kind: 400
(`invalid_argument`, `invalid_json`), 401 (`unauthorized`, v0.5.0), 404
(`unregistered`, `message_unavailable`, `claim_unavailable`, `not_found`),
405 (`method_not_allowed`), 409 (`msg_id_conflict`), 415
(`unsupported_media_type`), 429 (`rate_limited`, v0.5.0), 500
(`internal`). The
guard's refusals are 403. The full table is in the migration notes.

`/metrics` is on `SWB_METRICS_LISTEN`. It includes `swb_claims_active`,
`swb_claim_overlaps` (active claims sharing a subject with another holder)
and `swb_handoffs_total`. From v0.5.1 (TIN-7577) it adds the send limit
series. None carries an agent id.

| Series | Type | Meaning |
| --- | --- | --- |
| `swb_send_rate_limited_total{harness}` | counter | sends refused with 429 since broker start, one series per harness kind, zeros included |
| `swb_send_limit_burst` | gauge | `SWB_SEND_BURST` |
| `swb_send_limit_refill_per_minute` | gauge | `SWB_SEND_REFILL_PER_MINUTE` |
| `swb_send_buckets` | gauge | sender buckets the broker holds, at most 4096 |

The broker writes one JSON audit line to stdout for each successful
`register`, new `send` and `ack`, for each message an `inbox` read returns
(op `fetch`, with `delivery_count`), and for each recorded, renewed or
refused claim, release and handoff. Fields are named explicitly: `ts`, `op`,
`me`, `to`, `ticket`, `ruling`, `msg_id`, `thread_id`, `seq`,
`in_reply_to`, `size`, `body_hmac`, `recipient_state` and the claim
fields. From v0.5.1 only a `send` line carries `recipient_state`, and the
other lines keep their v0.5.0 keys. A line never carries a body, a note, an end
token or a session token. The message lines share `msg_id`,
`thread_id` and `ticket`, so a log reader can follow one thread from send
through delivery to ack (ADR-0002 L1-lite). The broker has no log client;
the lines go to stdout only.

A send refused by the rate limit writes one line of its own (v0.5.1,
TIN-7577):
`{"ts", "op": "send_refused", "code": "rate_limited", "harness", "retry_after", "route"}`.
`harness` is the sender's harness kind, `retry_after` the seconds until
its bucket refills or null, and `route` is `rest` or `mcp`. The line
carries no agent id and no body.

## Claims

Claims are advisory (SWB-R16, R-C950). A subject is a ticket (`TIN-4655`), a
path (`lab:nix/hosts/sting.nix`) or another token in `[A-Za-z0-9._:/#@+-]`,
at most 256 bytes. A note is at most 512 bytes with no control characters;
it is never written to the audit log. Never put a secret in either.

- A non-exclusive claim is always recorded (`outcome: recorded`). Every
  other session's active claim on the subject, an exclusive one included, is
  listed in `overlaps[]` as `{claim_id, holder, exclusive, orphaned}`.
- An exclusive claim is refused while another session holds any active claim
  on the subject: `outcome: refused`, `recorded: false`, `held_by` lists the
  holders, and nothing is recorded. The answer is a 200, not an error. Your
  own non-exclusive claim on the subject does not block you.
- Repeating your own exclusive claim renews it: `outcome: renewed`, the same
  `claim_id`, and the expiry moves to the later of the old one and now plus
  the new lease.
- A session holds at most one active claim per subject. Any other repeat
  claim by its holder updates that claim (`outcome: recorded`, the same
  `claim_id`, the later expiry), so a holder is listed once in `overlaps[]`
  and `held_by`. An exclusive request over your own advisory claim makes it
  exclusive; an advisory request never drops exclusivity.
- Leases default to 4 h and are clamped to 1..24 h. Every claim call renews
  the caller's session lease.
- `end` releases every active, unexpired claim the session holds at once
  and returns them in `released_claims` (a claim that already expired keeps
  its expiry as its end); the broker logs a `release` audit line for each, with
  `reason: session_ended`.
- A claim whose holder is `gone` shows `state: orphaned` and is never
  revoked; it still counts against an exclusive request.
- `handoff` offers one of your active claims to another session and creates
  a receipt in `offered`. The receiver accepts it with
  `claim {me, handoff_id}` (`swb claim --handoff <id>`): in one transaction
  the source becomes `handed_off` and the receiver's claim is recorded under
  the same rules. The accepted claim keeps the source's exclusivity unless
  the receiver passes `exclusive`. An acceptance that would be exclusive is
  an exclusive request under R-C950 (1), whether the exclusivity comes from
  the source or from `exclusive: true`: while another session holds an
  active claim on the subject it is refused with `held_by`, nothing moves,
  and the offer stays open. The receiver can then accept it as advisory
  with `exclusive: false` (`swb claim --handoff <id> --advisory`). A receiver that already holds a claim on the subject
  keeps that claim, now with `handoff_from` set. Releasing a claim, or
  ending its session, cancels its open offers.
- `claims` returns `truncated: true` when more claims matched than the
  page holds, and with `me`, `handoffs_truncated` when more than 100 open
  handoffs did.
- The audit log has a line for each recorded, renewed or refused claim, and
  an accepted handoff adds a `handed_off` line for its source claim.
- `release` and `handoff` act only on your own claims; another session's
  claim, or an unknown id, answers `claim_unavailable`.

Claims and handoffs are kept 7 days after they stop being active. The
SWB-R15 Linear receipt comment for a handoff is not posted yet.

## MCP tools

At `/mcp`: `register`, `end`, `peers`, `send`, `inbox`, `ack`, `heartbeat`,
`thread`, `claim`, `release`, `handoff` and `claims`, with the same fields
and rules as the REST routes; every input field is described in the
tool schema. Tool errors carry the same JSON error object as their text. Over
MCP the `register` result carries `end_token`, and for a bound session
`session_token`, into the model's context; an adapter should keep both
tokens and strip them before the model sees them.

## CLI verbs

`serve`, `hook <harness> <event>`, `whoami [--register]` (plain `whoami`
makes no broker call), `inbox [--after ULID]`,
`send <to> <ticket> [--ruling ID] [--operator-directed] [--in-reply-to ULID]
[--thread ULID] [--msg-id ULID] [--no-ack] < body`, `ack <msg_id>`,
`peers [--all] [--host H] [--limit N] [--cursor ID]`, `heartbeat`,
`thread <thread_id> [--after SEQ] [--limit N]`,
`claim <subject> [--exclusive] [--lease-hours N] [--note TEXT]`,
`claim --handoff <ULID> [--exclusive]`, `release <claim_id>`,
`handoff <claim_id> <to> [--note TEXT]`,
`claims [--subject S] [--holder ID] [--all] [--mine] [--limit N]`,
`doctor [--register]`,
`channel [--tools channel|full]` and `version`.

- `--help`, `-h`, `help` and `<verb> --help` print usage on stdout and exit
  0; `--version`, `-V` and `version` exit 0. An unknown verb exits 2.
- `peers` never passes `me`, so it refreshes no lease; nor does `claims`
  without `--mine`.
- `send` reads the body from stdin and drops one trailing LF or CRLF, so
  `printf '%s\n' "…" | swb send` stores the text without it. Other line
  breaks and tabs reach the recipient (v0.5.1). `inbox` and `thread`
  print one JSON line, so a body's line breaks appear as `\n` escapes and
  never start a new output line.
- Hooks save the end token on every successful register, under
  `$SWB_STATE_DIR/sessions/`; SessionEnd spends it and deletes the file.
  The CLI never prints the token: `whoami --register` prints
  `end_token_file` in its place.
- With `SWB_BIND=1`, `whoami --register` and the hooks bind the session and
  save its session token under `$SWB_STATE_DIR/bindings/`; every register
  presents the saved token and saves the rotated one. `whoami --register`
  prints `session_token_file`. `send`, `ack`, `inbox`, `claim`, `release`,
  `handoff` and SessionEnd send the saved token for `SWB_AGENT_ID`, and
  retry once on 401 when the file changed meanwhile.
- `send --in-reply-to` also acknowledges that message and reports it in
  `acked`, as the channel `reply` tool does (v0.5.1). `--no-ack` leaves it
  in the mailbox. The exit code stays 0 when the ack fails, because the
  send is committed. A failed ack prints a stderr warning only when an ack
  may still reach the message: no answer, 401, a 5xx or a session not
  registered yet. A refusal, such as 404 for a follow-up to a message
  that is not in the caller's mailbox, shows only in `acked`.
- `send` prints a stderr warning when `recipient_state` is not `live`. The
  warning names the recipient and the state, and the send still stands.
- `inbox --after ULID` reads the message that arrived after that one. A
  broker older than v0.5.1 ignores `after`, and the CLI says so on stderr.
- `agentd` is planned for P2 and exits 3; it is not in the usage text.

Exit codes: 0 ok, 2 usage, 3 missing setting, broker error or a failed
`doctor` check, 4 a refused exclusive `claim` (the answer is still printed),
1 `serve` failure; `hook` always exits 0, and `channel`
exits 0 when Claude Code closes its stdin.

## Claude Code channel

`swb channel` is a stdio MCP server that declares the `claude/channel`
capability (ADR-0001 → Push, v1c). Register it as a plain MCP server, for
example under the name `swb-channel`, with the command `swb` and the
argument `channel`; the [Claude Code plugin](../integrations/claude-code/)
does this. It reads the session's `SWB_*` environment, so the session must
start with the identity variables set (`SWB_PROC_START` included, which the
channel uses to find its own row; in the home estate a lab launcher exports
them, R-C273). During Claude Code's channels research preview a custom
server loads only with
`claude --dangerously-load-development-channels server:swb-channel`, which
shows a confirmation at startup. Events arrive as
`<channel source="swb-channel" from="…" ticket="…" msg_id="…" authority="peer">`.
Answer with the `reply` tool or acknowledge with `ack`; the channel never
acknowledges on its own.

From v0.5.1 (TIN-7577):

- **Paging.** The poller reads the head page of 8 as before. When that page
  emits nothing new and is full, it reads the pages after it, at most 4 in
  one poll, from where the last walk stopped. So a message behind 8 emitted,
  unacknowledged ones still arrives. The walk starts at the head page's
  `next_after` instead when a message waits to be pushed again, when the
  stored position is on the head page, when the broker answers `unknown
  after` for it, and when the first page after it repeats the head page.
  Such a restart reads one page more in the same poll. Against an older
  broker the head page carries no `next_after`, and the channel reads it
  alone as before.
- **Re-emit after a failed ack.** When an `ack`, a `reply`, or the ack
  inside a `reply`, fails in a way that may pass later, the channel pushes
  that message again on a later read, once, if the broker still lists it
  as unacknowledged. Such a failure is no answer, a 401, a 5xx or a
  session not registered yet. The poller then reads again without the
  30-second recheck wait. A refusal such as 400, 403, 404, 409 or 429 fails
  the same way again, so it never re-emits. A `reply` with `ack: false`
  does not either, and nor does a full-mode session that does not push.
- **Warnings.** `reply` returns `recipient_state` and, when the recipient
  is not live, a `warning` field. The full-mode `send` does the same.
- **Inbox tool.** `inbox` takes an optional `after` and returns
  `next_after`. A broker older than v0.5.1 ignores `after`, so the tool
  answers `broker does not page (v0.5.0 or older)` rather than return its
  head page as the next one.

### Full tool set

`swb channel --tools full` serves `reply`, `ack` and `inbox` plus `send`,
`peers`, `thread`, `claim`, `release`, `handoff` and `claims` (TIN-4655).
It is the mail and claims surface for a harness whose MCP client has no
other switchboard client. Every tool acts as the one session the channel
resolved, by the same rules as above:

- no tool schema has a `from` or `me` field, because the channel fills them in;
- a call whose `from` or `me` names another agent id is refused with
  `identity_override_denied` before any broker request;
- `send` refuses `operator_directed` and `ruling` with
  `operator_directed_denied`;
- `send` with `in_reply_to` also acknowledges that message unless `ack` is
  false (v0.5.1);
- `peers` and `claims` stay passive listings unless the caller passes its
  own `me` (which renews its lease) or `claims` passes `mine`.

The full set does not register the session, and it has no `register` or
`heartbeat` tool. Without a registration the session has no `peers` row,
although it can still send and read its mail. A harness without a
SessionStart hook therefore registers outside the channel:

| Step | How |
| --- | --- |
| Register | the launcher runs `swb whoami --register` with `SWB_HARNESS`, `SWB_SESSION_ID`, `SWB_HOST`, `SWB_SESSION_PID` and `SWB_PROC_START` set |
| Stay live | `swb heartbeat` (or an `inbox` read) at least every 15 minutes; a row is `idle` after 15 minutes and `gone` after 6 hours |
| End | `swb hook <harness> SessionEnd`, or the session's own client |
| Service identity | with `SWB_BIND=1`, `swb whoami --register` binds the session id, so a restarted service keeps its unread mail and its id cannot be spoofed (v0.5.0, ADR-0003) |

The `claude/channel` capability and its push stay Claude-only. With
`--tools full` the channel declares them, and polls, only when every
identity setting present says `claude`: `SWB_HARNESS`, the kind in
`SWB_AGENT_ID`, or both. A mismatch, such as `SWB_HARNESS=claude` with a
`hermes` `SWB_AGENT_ID`, does not push. A full-mode session that sets
neither and resolves through its broker row does not push either. Every
other harness reads its mail with the `inbox` tool. The pinning guards this
one process. For an unbound id the broker still trusts a self-asserted
sender, so another client can still send as it (SWB-R05). For a bound id
the channel sends the saved session token with `send`, `inbox`, `ack`,
`claim`, `release` and `handoff`, and the broker refuses a caller without
it (v0.5.0, ADR-0003).

## Notices

On UserPromptSubmit, `swb hook <harness> UserPromptSubmit` adds a short
notice to the session's context when it has unread mail. The notice names the
receiver's agent id, the sender and ticket, and the exact `swb inbox` and
`swb ack` commands. Claude Code reads it from the `hookSpecificOutput` JSON;
with `SWB_HOOK_FORMAT=text` the hook prints the same notice as plain text.

## Known gaps

- Session rows are never pruned.
- `/v1/notify` is not implemented.
- `agentd` (per-host push) is planned for P2.
- Handoffs post no Linear receipt comment yet (SWB-R15).
