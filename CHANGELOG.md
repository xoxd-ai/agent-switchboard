# Changelog

All notable changes to agent-switchboard (`swb`) are recorded here. The
format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and
the project uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
(pre-1.0: a minor version may change the wire format or REST routes).

Each release is a signed annotated tag on `main`. The broker image is
published to `ghcr.io/xoxd-ai/agent-switchboard` by digest only, with no
registry tag; the approved digest and its publication receipt are in
[docs/releases/approved-broker.json](docs/releases/approved-broker.json) and
[ADR-0001](docs/adr/0001-agent-switchboard.md) → Rulings (`SWB-R##`).

## [Unreleased]

## [0.5.1] - Unreleased

Scope ruling SPR-80 (TIN-7125 comment `2ff3e637`), tracking issue
TIN-7577. The release chain is SWB-R73 (TIN-7577 comment `d59d35a1`).
Release record: filled at the cut under SWB-R73. The workspace version
and the lock files move to 0.5.1 in the release bump.

### Upgrade notes

- Envelope v3 is unchanged. Its schema changes only the `body`
  description, which now says newline and tab are kept. Stored and delivered
  envelopes keep the same keys. `recipient_state` is a send-result field,
  described in REFERENCE, and a submitted envelope that carries it is
  refused with `unknown envelope field`, as any unknown key is.
- `swb send --in-reply-to` now acknowledges the original by default. A
  script that used it to keep that message open must add `--no-ack`.
- A v0.5.0 client against a v0.5.1 broker gets two extra keys it ignores:
  `next_after` on `inbox` and `recipient_state` on `send`. The wider
  `proc_start` filter fixes A4 for v0.5.0 channels as soon as the broker
  upgrades.
- A v0.5.1 client against a v0.5.0 or older broker: the channel sends
  `after` only when the head page carried `next_after`, so it never sends
  one there, and A1 stays open until the broker upgrades. A2 and A3 work
  against any broker. No `recipient_state` arrives, so no A5 warning
  shows. The channel `inbox` tool answers an error for `after`, because
  that broker would return its head page instead.
- The store gains `messages.arrival_seq`, two indexes and an
  `arrival_seq` counter at startup. Existing messages get positions in
  `created_at`, `seq` order. A downgrade to v0.5.0 runs: it writes rows
  without a position, and the next v0.5.1 start fills them in.
- An inbox now returns mail in arrival order. That matches the old
  `created_at` order unless the broker's clock stepped back.
- A message body keeps its newlines and tabs. A reader that split
  `swb inbox` or REST output on raw line breaks is unaffected, because
  that output is JSON and escapes them. `swb send` drops one trailing
  line break from stdin, so a body sent with `printf '%s\n'` is stored
  as before. A retry with `--msg-id` of a message a v0.5.0 broker stored
  without its newlines is still a replay. A new `messages.body_lines`
  column marks rows v0.5.1 writes, and only an older row matches a retry
  in that flat form. A retry that changes a newline or tab of a v0.5.1
  body is a conflict.

### Fixed

- **A1, paging stall.** The channel showed only the 8 oldest unacked
  messages and never read past them. `inbox` now takes `after` and
  returns `next_after`, in arrival order, and the channel poller walks
  past a full page of messages it already emitted.
- **A2, lost re-emit.** An ack or a reply that failed, for example during
  a registration gap, left its message unacknowledged and never pushed
  again. The channel now pushes it once more on a later read when the
  failure may pass on a later try: no answer, 401, a 5xx or a session not
  registered yet. A refusal such as 429 never re-emits, so a rate-limited
  reply cannot loop.
- **A3, `send --in-reply-to` left the original unread.** The CLI and the
  full-mode `send` tool now acknowledge the original, as `reply` does.
- **A4, `peers` dedupe before the proc_start filter.** Two live processes
  of one session hid each other's rows. The filter now runs first.
- **A5, send to a dead recipient.** A send now reports `recipient_state`,
  and the clients warn when the recipient is not live. SPR-80 rules a
  warning: the send still succeeds.
- **Bodies lost their line breaks** (TIN-7577 comment `e2d6b6f5`). The
  store removed every control character from a body, newline and tab
  included, although the channel frame keeps both. The store now keeps
  newline and tab. It removes every carriage return, so CRLF becomes LF
  and a lone CR goes, and every other control character, as before. The
  16 KiB limit counts the cleaned body. CLI and REST output stay one JSON
  line per result, and the channel header stays the first paragraph of
  its frame, so a body line cannot pass for a header or another message.

### Added

- `/metrics`: `swb_send_rate_limited_total{harness}` for every harness
  kind, and the gauges `swb_send_limit_burst`,
  `swb_send_limit_refill_per_minute` and `swb_send_buckets`. No series
  carries an agent id.
- A `send_refused` stdout line for each send the rate limit refuses, with
  the harness kind, `retry_after` and the route, and no ids or body.
- `inbox` takes `after` and returns `next_after` over REST, MCP and the
  channel `inbox` tool.
- `send` results carry `recipient_state`, and so does the send audit
  line.
- A 429 from a rate-limited send carries `retry_after` in its body and a
  `Retry-After` header when the bucket refills.
- `swb send --no-ack` and `swb inbox --after ULID`.
- The full-mode channel `send` tool takes `ack`. The `reply` and full
  `send` results carry `recipient_state` and a `warning` field.

### Known gaps

- The UserPromptSubmit hook notice still names the sender of the oldest
  unacked message. With a stuck head page, a new message from another
  peer gets a notice that names the older sender.
- Default `peers` still lists one row per session, so a second live
  process of one session shows only through its own `proc_start`. The
  investigation's item 6 asked to show those rows too.
- A channel walk that starts again rereads up to 4 pages it already
  emitted, and each read counts a delivery and writes a `fetch` line. An
  inbox read that counts no delivery would remove that cost.

## [0.5.0] - 2026-10-27

Release record SWB-R71. Source `cfff7913` (the #55 merge), image
`sha256:05738e97af06f767d7894b1f3e0697b9eece0888527e9937130a4041bff7498e`,
published by the release workflow under the signed `v0.5.0` tag.

The workspace version is 0.5.0 (SWB-R66, TIN-4655 comment `0753094f`;
release chain SWB-R71, comment `7e292300`).

### Upgrade notes

- **Upgrade the broker and every client together** to use binding. A
  v0.4.0 broker ignores `bind`, so a v0.5.0 client against it stays
  unbound. A v0.4.0 client works against a v0.5.0 broker for unbound ids
  only.
- Unbound ids keep the v0.4.0 behaviour, apart from the send rate limit.
- The store gains `sessions.bound` and `messages.recipient_key` in place
  at startup, and fills the key for existing messages.
- `register` results gain `bound`, and `peers` rows gain `bound`. A
  client that rejects unknown fields needs updating.

### Added

- Opt-in sender binding ([ADR-0003](docs/adr/0003-sender-binding.md),
  SWB-R66). `register` with `bind: true` binds the session's
  `harness:host:session_id` and returns a `session_token`. While the
  binding is live, `send` (by `from`), `inbox`, `ack`, `claim`, `release`,
  `handoff` and `end` (by `me`) need the token or answer 401
  `unauthorized`, so a spoofed send from a bound id fails. A new
  `register` of the key needs the current token and rotates it. Only the
  binding's own agent id may act. A binding lapses when its session ends
  or after 6 hours without a call, and a lapse is permanent: no later
  call revives the binding or its old token. Each operation checks the
  token in its own store transaction, and an `inbox` long-poll checks it
  on every read.
- Stable delivery for bound service ids: `inbox` and `ack` for a bound id
  read mail addressed to any pid of its key, so a restart keeps unread
  mail (SWB-R66).
- Per-sender send rate limit: a token bucket per sender answers 429
  `rate_limited`. Defaults are a burst of 20 and a refill of 30 per
  minute, set with `SWB_SEND_BURST` and `SWB_SEND_REFILL_PER_MINUTE` on
  `swb serve`; `SWB_SEND_BURST=0` turns it off. Only a send that stores a
  new message spends a token, so an invalid send or an idempotent retry
  (SWB-R46) never answers 429. The broker keeps at most 4096 buckets. The
  limit guards against a client looping under one id, not a flood that
  rotates unbound ids.
- `SWB_BIND=1`: `swb whoami --register` and `swb hook` ask for a binding.
  The client keeps the session token in
  `$SWB_STATE_DIR/bindings/<harness>:<host>:<session_id>.token` (directory
  0700, file 0600), presents it on every register and saves the rotated
  one (SWB-R67). `swb send`, `ack`, `inbox`, `claim`, `release`,
  `handoff`, SessionEnd and `swb channel` send it. `whoami --register`
  prints `session_token_file`, never the token.
- `peers` rows carry `bound` (SWB-R68).
- `swb channel` reads `SWB_STATE_DIR`, `XDG_STATE_HOME` and `HOME` to find
  the session token file.

### Changed

- SWB-R05 and SWB-R14 are amended in ADR-0001: a bound id is no longer
  self-asserted, and `authority` stays `peer` on every message.
- MCP tool schemas: `register` takes `bind` and `session_token`; `send`,
  `inbox`, `ack`, `claim`, `release`, `handoff` and `end` take
  `session_token`.
- `SECURITY.md` names v0.4.0 as the current release and describes binding
  and its residuals.

## [0.4.0] - 2026-10-27

Release record SWB-R64. Source `3405bf86` (the #51 merge), image
`sha256:a6303937d593957765cdd593e5efc3dec1208a6471458656e5bbfb4452aae7bc`,
published by the release workflow under the signed `v0.4.0` tag.

The workspace version is 0.4.0 (SWB-R63, TIN-4655 comment `30ca400e`).

### Upgrade notes

- **Upgrade the broker and every client together.** A v0.3.0 broker
  answers `invalid harness` to a `register` with kind `hermes` or
  `kimicode` and refuses any agent id that names one. A v0.3.0 client
  refuses those ids too.
- The REST routes and the envelope fields are the v0.3.0 ones. A v0.3.0
  client can talk to a v0.4.0 broker, but it refuses ids of peers with
  the new kinds.
- SWB-R63: redeploy the live broker and bump the lab pin to v0.4.0 together.
- A validator pinned to the older copy of
  `schemas/envelope-v3.schema.json` rejects the new kinds. Refresh the copy.
- `swb channel` keeps its three-tool mode by default. `--tools full` needs
  a v0.4.0 `swb`, and its peers with the new kinds need a v0.4.0 broker.

### Added

- Harness kinds `hermes` (a Hermes agent) and `kimicode` (the Kimi Code
  CLI). `kimi` still means Claude Code on a Kimi model. The store, the
  `register` schema, the envelope schema and the CLI read one list,
  `swb_store::HARNESS_KINDS`. An older client refuses ids with the new
  kinds, so upgrade the broker and clients together (TIN-4655).
  `schemas/envelope-v3.schema.json` widens its `agent_id` pattern in place
  (the schema is a draft): a validator pinned to the older copy rejects
  the new kinds.
- `SWB_HOOK_FORMAT=text`: `swb hook` prints the UserPromptSubmit notice as
  plain text for a harness that is not Claude Code (TIN-4655).
- `swb channel --tools full`: adds `send`, `peers`, `thread`, `claim`,
  `release`, `handoff` and `claims`, with `from` and `me` pinned to the
  resolved session and `identity_override_denied` for any other id. The
  `claude/channel` push stays Claude-only, and only when every identity
  setting present names `claude`. The full set does not register: a
  harness without a SessionStart hook registers with
  `swb whoami --register` and renews its lease (TIN-4655).

### Changed

- Local Bazel servers are bounded (TIN-5770). `.bazelrc` sets
  `startup --max_idle_secs=120`. `just check` and `just build` pass
  `--remote_local_fallback=false` and print that their result is local,
  non-qualifying (`ci-ok` on merge_group qualifies). `just remote-check`
  shuts down its scratch workspace's server when the check ends. CI is
  unchanged: it runs Bazel with `--ignore_all_rc_files`.

### Documentation

- `docs/demo/TELEPHONE.md`: the audit trail for a ring is 4 sends, 4 acks
  and at least one `fetch` per message, since every inbox read (hook notice,
  channel poll, CLI) writes one; `swb peers` may list more than the cast; the
  operator paces Codex, Pi and Junie hops; Junie's channel `reply` path; a
  scrape may miss short `swb_mailbox_unacked` blips (TIN-6749).

## [0.3.0] - 2026-10-23

Release record SWB-R62. Source `e2bb9f75` (the #43 merge), image
`sha256:2e42fc30cabdee4b076bd80d7d1e4b80080fc81436153210368daaa0caea7172`,
published by the release workflow under the signed `v0.3.0` tag.

The workspace version is 0.3.0 (R-C944, #37). **This release breaks the v0.2
wire format:** upgrade the broker and every client together. A v0.2 client
against a v0.3.0 broker gets typed 400, 405 or 415 errors, never a silent
fallback. Details and the upgrade order:
[docs/releases/v0.3.0-migration.md](docs/releases/v0.3.0-migration.md).

### Install

- **Prebuilt binaries (TIN-6749).** Each release from v0.3.0 attaches
  `swb-<version>-x86_64-linux` (static, musl), `swb-<version>-aarch64-darwin`
  (system libraries only, ad hoc signed), `SHA256SUMS`, its detached
  signature `SHA256SUMS.asc` by the release key, and `install.sh`. The
  script checks the checksum, checks the signature when `gpg` is available,
  and installs to `~/.local/bin`. The binaries are the flake's new
  `swb-release` output; `just release-binaries <tag>` builds and signs them
  (attended), and `just release-binaries-upload <tag>` attaches them.

### Documentation

- [docs/QUICKSTART.md](docs/QUICKSTART.md): a five-minute human quickstart
  (install, join a broker, `peers`, send, reply, ack), summarized in the
  README (TIN-6749).
- [integrations/agent-skill/SKILL.md](integrations/agent-skill/SKILL.md): a
  harness-neutral agent guide to receiving, replying and acknowledging that
  keeps `thread_id` and `in_reply_to` and treats peer messages, and
  `operator_directed` claims, as data.
- [docs/demo/TELEPHONE.md](docs/demo/TELEPHONE.md): the "agent telephone"
  live demo run sheet (TIN-6749, TIN-6750).
- The peer-dialog skill says `swb whoami --register` registers; plain
  `whoami` no longer does in v0.3.0.

### Security

- `register` returns a per-session end token (64 hex characters) that only
  the registering session receives; every register mints a new one. `end`
  (REST and the new MCP tool) checks `{me, end_token}` instead of
  `proc_start`, and any other token ends nothing. Identity is still
  self-asserted: the token stops ending a session without registering it,
  not re-registering an agent id.
- `peers` no longer returns `proc_start`.
- No GET changes state: lease refresh moves to `POST /v1/peers {"me"}` and
  the inbox to `POST /v1/inbox`, closing the cross-site simple-GET residual
  of v0.2.2.

### Changed

- **Breaking:** `POST /v1/end` takes `{me, end_token}`; a body with only
  `proc_start` is a 400 that names `end_token`.
- **Breaking:** `GET /v1/peers?me=` and `GET /v1/inbox` answer 405 (with
  `Allow: POST`); use `POST /v1/peers` and `POST /v1/inbox`.
- **Breaking:** `peers` lists live sessions only by default (`all` adds
  idle, gone and ended), one row per harness, host and session id, in pages
  of `limit` (default 50, at most 200) with `next_cursor`.
- **Breaking:** every error is `{"error", "code"}` with a status per kind
  (400, 404, 405, 409, 415, 500) instead of a blanket 400.
- **Breaking:** `swb whoami` no longer registers; `swb whoami --register`
  does. `swb channel` now needs `SWB_PROC_START` to find its own row.
- Hooks save the end token under `SWB_STATE_DIR` (default
  `$XDG_STATE_HOME/swb`, then `~/.local/state/swb`) and spend it on
  SessionEnd.
- Retention runs at startup and every hour, not on every `send` and every
  250 ms inbox long-poll tick.
- `swb --help`, `-h`, `help` and `<verb> --help` print usage on stdout and
  exit 0; `swb --version` and `-V` exit 0; `agentd` left the usage text.

### Added

- **Audit lines follow a thread (ADR-0002 L1-lite, TIN-6749).** The `send`
  and `ack` audit lines carry `thread_id`, `seq`, `in_reply_to` and
  `ticket` beside `msg_id` (`ack` reads them from the stored message), and
  every message an `inbox` read returns writes a `fetch` line with its
  `delivery_count`. Still no body, note or end token. The broker writes
  stdout only; it ships no Loki or OTLP client.
- **Claims, release and handoff (P2, SWB-R16, R-C950, TIN-6690).** New MCP
  tools, REST routes and CLI verbs `claim`, `release`, `handoff` and
  `claims`, plus `heartbeat` and `thread`, completing the ADR-0001 tool set
  (12 MCP tools with `end`). Claims are advisory:
  - a non-exclusive claim is always recorded and lists every other holder,
    an exclusive one included, in `overlaps[]`;
  - an exclusive claim is refused (`outcome: refused`, `held_by` lists the
    holders, nothing recorded) while another session holds any active claim
    on the subject;
  - repeating your own exclusive claim renews it (same `claim_id`, later
    expiry); a session holds at most one active claim per subject, so any
    other repeat updates that claim and a holder is listed once (SWB-R61);
  - `end` releases the session's unexpired claims at once, returns them in
    `released_claims`, and the broker logs one `release` audit line each.
  - Leases default to 4 h, clamped to 1..24 h; a claim whose holder is
    `gone` shows `orphaned` and is never revoked.
  - `handoff` creates an `offered` receipt; the receiver accepts it with
    `claim --handoff`, which moves the claim and keeps its exclusivity
    unless the receiver says otherwise (`--exclusive` or `--advisory`). An
    acceptance that would be exclusive is refused while another session
    holds a claim on the subject, and the offer stays open (R-C950 (1)). `release` and `handoff` act
    only on the caller's own claims (`claim_unavailable`, 404, otherwise).
  - The store gains `claims` and `handoffs` tables in place on open; claims
    and handoffs are pruned 7 days after they stop being active.
  - `/metrics` adds `swb_claims_active`, `swb_claim_overlaps` and
    `swb_handoffs_total`.
  - `swb claim` exits 4 when an exclusive claim is refused; refusals and
    the source side of an accepted handoff each get an audit line; `claims`
    reports `truncated`.
  - The Linear handoff-receipt comment (SWB-R15) is not posted yet.
- Formal spec: `Broker.dhall` replaces the `Unruled` cases with the R-C950
  `claim.rules` and the SWB-R61 `holder_has_one_claim_per_subject`; the QuickCheck model, `exclusiveRefusal` and
  `noOrphanAfterExpiry` assert them, and the live adapter runs claims.
- `GET /healthz` on the main listener (Host-guarded) and the metrics
  listener (unguarded, for probes).
- MCP `end` tool; every MCP input field is described, with patterns and
  bounds.
- `swb peers --all --host --limit --cursor`.
- `integrations/`: a Claude Code plugin (fail-open hooks, `swb channel`
  registration, the peer-dialog skill), and Codex and Pi configuration
  snippets (#35). The Codex and Pi snippets are unproven pending the P1b
  canaries (R-C949).
- Dual license, MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`), and
  `license` metadata in every crate, the flake package and the spec cabal
  file (R-C942).
- `CONTRIBUTING.md` (DCO sign-off, fork-first flow), `SECURITY.md`,
  `CODE_OF_CONDUCT.md` (Contributor Covenant 2.1), issue and pull request
  templates.
- `docs/REFERENCE.md` (settings, routes, verbs) and
  `docs/operations/HOME_ESTATE.md` (estate-specific operations), split out of
  the README.

## [0.2.2] - 2026-10-13

Release record SWB-R59. Source `daa6d59f` (the #28 merge), image
`sha256:4ebaf180547b5a74ca51489381270792d6d87619a4444eb9a7fcd6c31da94490`,
published by the release workflow (run 38327423160) and read back
byte-identical.

### Security

- The broker now runs one Host and Origin guard over its whole main
  listener: `/mcp`, every `/v1/*` route and the 404 fallback (#28). Up to
  v0.2.1 the Host allowlist reached only `/mcp`, so a DNS-rebinding page in
  a browser on the same network could register, send, ack or end sessions
  through the REST twin.
  - `Host` must be present, single, well formed and on
    `SWB_MCP_ALLOWED_HOSTS`. A listed host without a port admits any port.
  - A request that carries `Origin` is refused unless the origin is on the
    new `SWB_ALLOWED_ORIGINS` (empty by default). Non-browser clients send no
    `Origin` and are unaffected.
  - Refusals are HTTP 403 with a JSON `error`, returned before any handler
    reads a body or touches the store.

### Changed

- An empty `SWB_MCP_ALLOWED_HOSTS` now refuses to start, instead of being
  read as "every host".

### Known issues

- Cross-site simple GETs carry no `Origin` and pass the guard; see
  [SECURITY.md](SECURITY.md). Fixed in v0.3.0.

## [0.2.1] - 2026-10-08

Release record SWB-R58. Source `9bc96349` (the #26 merge), image
`sha256:2d53153f4321fbfb06704b94f4820af0d6be28b473684de3cfcae152e7a1b453`,
published by the release workflow (run 37872283108, 2026-10-09T02:07Z) and
read back. This is the first published release with the v0.2.0 features.

### Added

- Everything listed under 0.2.0 below, which was tagged but never published.

### Changed

- The image builds with a pinned, hermetic C toolchain (zig cc from the
  flake's nixpkgs, registered in `MODULE.bazel` as `//tools/cc`) instead of
  the build host's gcc (#25). The image digest is reproducible across build
  hosts and the release workflow can build it.
- The workspace version is 0.2.1, so `swb version` and the MCP `serverInfo`
  report the version the tag names (#26).

## [0.2.0] - 2026-10-07 [YANKED]

Release record SWB-R57. **This tag was never published.** The release
workflow could not build the approved image: the CI runners have no host
gcc, and the approved digest (`sha256:66e43966…`) depended on the gcc of the
host that built it. The fix (#25, the hermetic toolchain) changes the image,
so the same features shipped as v0.2.1 instead. Do not use the v0.2.0 tag.

### Added

- `swb channel`: a stdio MCP server for Claude Code that declares the
  `claude/channel` capability and pushes each new message into the running
  session as a channel event, with `reply`, `ack` and `inbox` tools (#23,
  SWB-R56). It never acknowledges on its own, marks content as teammate
  information with `authority: peer`, and retries quietly when the broker
  is down.

### Changed

- The UserPromptSubmit notice names the receiver's agent id and the exact
  `SWB_AGENT_ID=<id> swb inbox` and `swb ack <msg_id>` commands.

## [0.1.0] - 2026-10-04

Release record SWB-R55. Source `d8ebfdbf`, image
`sha256:c9170c7121821322376c16230f3b0eb5f10001aebfbce7e83c20c4afa5dd2236`,
published 2026-10-05.

### Added

- First release: the broker (`swb serve`) with MCP tools `register`,
  `peers`, `send`, `inbox`, `ack`, the REST twin `/v1/*` and `/metrics`;
  SQLite store with retention and TTL; harness hooks (`swb hook`); CLI verbs
  `whoami`, `inbox`, `send`, `ack`, `peers` and `doctor`.

[Unreleased]: https://github.com/xoxd-ai/agent-switchboard/compare/v0.5.0...HEAD
[0.5.0]: https://github.com/xoxd-ai/agent-switchboard/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/xoxd-ai/agent-switchboard/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/xoxd-ai/agent-switchboard/compare/v0.2.2...v0.3.0
[0.2.2]: https://github.com/xoxd-ai/agent-switchboard/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/xoxd-ai/agent-switchboard/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/xoxd-ai/agent-switchboard/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/xoxd-ai/agent-switchboard/releases/tag/v0.1.0
