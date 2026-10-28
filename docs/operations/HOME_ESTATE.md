# swb in its home estate

How `swb` is built, released and run in the Tinyland estate that develops it.
This page holds the estate-specific detail that used to live in the README
(moved 2026-10-19, R-C942/R-C946, TIN-4655). Nothing here is needed to use
`swb` elsewhere; outsiders start at the [README](../../README.md).

Linear: TIN-4655 (project P-TIN-184). Rollout gates and the release lock are
in [PRODUCTIONIZATION.md](PRODUCTIONIZATION.md); SLIs in [SLO.md](SLO.md).

## Deployment shape

- **Broker:** one Rust binary in a blahaj pod, tailnet-only. blahaj owns the
  stack, the retained PVC and restore, tailnet routing and rollout (SWB-R51);
  this repo owns source and the image; lab owns harness and Home Manager
  integration.
  - It serves MCP at `:8080/mcp` (registry key `agents`), the REST twin
    `/v1/*` for hooks, and `:9090/metrics`.
  - It stores state in SQLite on a PVC, replicated to RustFS by Litestream.
  - The image runs as UID 65532; the PVC must be writable by that UID, and
    `SWB_MCP_ALLOWED_HOSTS` is set to the tailnet name.
- **Identity:** lab's launchers export `SWB_HOST`, `SWB_SESSION_PID`,
  `SWB_PROC_START` and `SWB_BROKER_URL` from their own pid behind the single
  `tinyland.switchboard.enable` switch (R-C273).
- **Push:** the UserPromptSubmit notice everywhere; for Claude Code,
  `swb channel` (SWB-R56). `swb agentd` (P2) is no longer the Claude push
  path.
- **LGTM plane:** designed, not built ([ADR-0002](../adr/0002-lgtm-plane.md)).
  The broker has no OTLP exporter and no Loki client, and writes nothing to
  Tempo, Loki or Mimir. What it emits today:
  - `/metrics` on `:9090`: six series (see
    [SLO.md](SLO.md#what-the-broker-exposes-today)). Nothing scrapes them
    yet.
  - One JSON audit line per accepted operation on stdout. Since v0.3.0 the
    message lines (`send`, `fetch`, `ack`) carry `msg_id`, `thread_id`,
    `seq` and `ticket` (ADR-0002 L1-lite), so a log collector could follow a
    thread. No collector picks them up yet.

  SWB-R02's write-through stays the design: LGTM would be the
  read/query/context plane, never the commit path.

## Release state

v0.5.0 (SWB-R71) is the current release. It keeps the v0.4.0 routes and
adds opt-in sender binding ([ADR-0003](../adr/0003-sender-binding.md)) and
a per-sender send rate limit. A v0.4.0 broker ignores `bind`, so the broker
and every client move to v0.5.0 together. v0.4.0 (SWB-R64) added the
`hermes` and `kimicode` harness kinds, which a v0.3.0 broker or client
refuses. blahaj main pins the SWB-R62 v0.3.0 digest
([blahaj#2359](https://github.com/xoxd-ai/blahaj/pull/2359)). The live
broker repin and the lab pin bump to v0.5.0 happen together, each by its
owner (SWB-R63, SWB-R71). v0.3.0 broke the v0.2 wire format; its lab
changes are in [v0.3.0-migration.md](../releases/v0.3.0-migration.md). The approved
digest, protected-input hashes and publication receipt are in
[docs/releases/approved-broker.json](../releases/approved-broker.json);
release history is in the [CHANGELOG](../../CHANGELOG.md). v0.1.0 (SWB-R55)
was pushed by hand under R-C312; v0.2.1, v0.2.2, v0.3.0, v0.4.0 and v0.5.0
were published by the release workflow.

Publication is the signed-tag release workflow
(`.github/workflows/release.yml`). It pushes only the approved digest in
`approved-broker.json`, by digest with no mutable tag, and reads it back. PR
CI builds the image and digest but never publishes. A different image needs
its own ruling and approved-release entry first.

## Develop

```sh
nix develop            # bazelisk, just, cargo (diagnostic), gh, jq
just fork-setup        # origin = your fork, upstream = xoxd-ai (push disabled)
just check             # rustfmt, clippy, unit + integration tests (Bazel)
just build             # swb binary (bazel-bin/crates/swb/swb) and OCI image
just image             # build image and print its immutable GHCR reference
just remote-check      # from neo: run `just check` on sting
just spec-dhall        # formal-spec Dhall check (runs anywhere; see spec/README.md)
just secrets-scan      # TruffleHog + gitleaks, as ci-ok requires
just release-check     # verify the approved release's protected inputs
just lock              # regenerate all three lock files together (linux x86_64)
```

Never build on neo; use `just remote-check` there. Fork convention, signed
commits, CI and how a PR lands: [AGENTS.md](../../AGENTS.md) → "Fork
convention and CI".

## Run locally on a build host

On sting or honey, after `just build`, the README quickstart works with the
Bazel binary:

```sh
swb=$PWD/bazel-bin/crates/swb/swb
d="$(mktemp -d "$HOME/scratch-swb.XXXXXX")"
timeout 600 env SWB_DB_PATH="$d/swb.sqlite3" SWB_LISTEN=127.0.0.1:18080 \
  SWB_METRICS_LISTEN=127.0.0.1:19090 "$swb" serve &
```

The broker binds loopback only, keeps its database in a scratch directory and
exits on its own `timeout`, so nothing needs to be signalled afterwards. Then
follow the README from the readiness wait on, with `"$swb"` in place of
`swb`.
