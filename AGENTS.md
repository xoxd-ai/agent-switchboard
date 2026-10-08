# AGENTS.md

`agent-switchboard` (binary `swb`) is the tailnet-only broker for cross-harness
agent session discovery, threaded dialog and advisory task claims/handoff. It
also holds the Claude Code channel emitter (`swb channel`, SWB-R56), the
planned per-host adapter (`swb agentd`) and the hook client (`swb hook`).
Linear: TIN-4655.

**Read first:** [ADR-0001](docs/adr/0001-agent-switchboard.md), the broker
design. Its Rulings table holds `SWB-R01`..`SWB-R23`, the source/release
rulings `SWB-R49`..`SWB-R55` the Claude channel ruling `SWB-R56` and the v0.2.0 release `SWB-R57`; `SWB-R24` is recorded under its History.

**Read second:** [ADR-0002](docs/adr/0002-lgtm-plane.md), the LGTM plane and
the phase order. Its Rulings table holds `SWB-R25`..`SWB-R48`.

A design change needs a new ruling, recorded as a dated TIN-4655 comment, and
then an ADR update. An operator question or aside is not a ruling. The ADR
Rulings tables are the normative record; the summary under "Product
invariants" below never outranks them.

## Estate rulings that bind here

The fuller text lives in `xoxd-ai/lab` `AGENTS.md` (TIN-3692):

- **R-N11: process control is absolute.** Agents never signal any process, on
  any host, in any form. That covers `kill`/`pkill`/`killall`, the tmux kill
  subcommands, `systemctl stop|kill` and `launchctl kill|bootout`, including
  their literal-PID forms. The product is bound the same way: no broker tool,
  hook or agentd path acts on a process, a tmux pane or another session's
  claim. agentd never unlinks a socket.
- **R-N12: a guard-hook refusal is a stop.** Quote it verbatim, propose at
  most one materially different alternative, and ask before running it.
- **R-N13: ratification.**
  - Every mutating step cites a ruling ID in its receipt.
  - Every session writes a `docs/agent-notes/` entry before ending.
  - Ticket descriptions are superseded by dated comments, never rewritten.

## Source of truth

When sources disagree, prefer them in this order:

1. `MODULE.bazel`, `.bazelversion`, `BUILD.bazel` files and the three lock
   files;
2. `justfile`;
3. `.github/workflows/ci.yml`;
4. `docs/adr/`;
5. `README.md`, which is orientation only.

## Build placement

- **Bazel is the build, test and package authority.** Cargo is a diagnostic
  mirror only.
- **Never compile on neo.** neo is the teletype seat, and its local-build
  guard refuses `bazel`/`cargo` builds. Run `just remote-check` from neo, or
  run `just check` on sting or honey. `cargo metadata` and
  `cargo generate-lockfile` are fine anywhere.
- **Change the three lock files only together, with `just lock`.** They are
  `Cargo.lock`, `cargo-bazel-lock.json` and `MODULE.bazel.lock`. Run it on
  linux x86_64 (sting or honey), which is the platform CI checks them on.
- **CI runs Bazel with `--ignore_all_rc_files`.** Nothing a CI target needs
  may live in `.bazelrc`.
- **The C toolchain is pinned Nix, never the host's (R-C416).** Linux x86_64
  builds use `//tools/cc` (zig cc from the flake.lock nixpkgs, fixed target
  `x86_64-linux-gnu.2.34`), registered in `MODULE.bazel`. Bazel needs
  `nix-build` on PATH; no host gcc is needed or used. Change
  `tools/cc/nixpkgs.nix` only together with `flake.lock`.

## Formal spec (R-C229)

- `spec/` holds the Dhall types and records (approved-broker.json, the blahaj
  owners.json shape, broker constants) and a Haskell QuickCheck model with
  five trace properties. See [spec/README.md](spec/README.md).
- `just spec-dhall` runs anywhere, neo included (interpreters only).
  `just spec-quickcheck` and `just spec-check` compile Haskell: run them on
  sting or honey, or `just remote-spec-check` from neo.
- CI's `spec-dhall` job runs the same Dhall check and `ci-ok` requires it
  (R-C255). It is skipped on a fork PR like the other jobs, so run
  `just spec-dhall` before queueing.
- Keep properties few and parsimonious. Never assert an `Unruled` SWB-R16
  case. Edit `spec/dhall/approved-broker.dhall` together with
  `docs/releases/approved-broker.json`.

## Fork convention and CI

- **Remotes:** `origin` is your private fork and the only push target.
  `upstream` is `xoxd-ai/agent-switchboard`, with its push URL set to
  `DISABLED`. `just fork-setup` configures both.
- **Branches:** `<type>/tin-####-<slug>-<yyyymmdd>`. `just branch <type>
  <tin> <slug>` creates one.
- **PRs** go from the fork to `upstream/main` and land through the merge
  queue or, under R-C237/R-C228, by an admin merge after validation on sting.
  - The ruleset on `main` requires signed commits, a PR (0 approvals, merge
    method `merge`) and the `ci-ok` check.
  - The queue settings are MERGE, one entry built at a time, ALLGREEN.
  - Force-push and deletion are blocked.
  - The repository admin role is the ruleset's one bypass actor (R-C237,
    matching lab R-C11). A PR may land by validating `just check` and
    `just release-check` on sting, then an admin merge (R-C228); the queue
    stays configured for everything else.
- **Fork PRs are gated in the merge queue.** The ci-templates Rust lane
  refuses private runners for fork PRs, and hosted runners are forbidden. So
  on a fork PR both jobs are skipped and `ci-ok` reports as passing. The full
  lane runs on `merge_group`. Run `just check` (or `just remote-check`)
  before queueing.
- **Secrets scan:** `ci-ok` also requires the `secrets-scan` job (TruffleHog
  `--only-verified` and gitleaks with `.gitleaks.toml`, full history). It is
  skipped on a fork PR like the Rust lane, so run `nix develop --command just
  secrets-scan` before queueing.
- **Release:** `.github/workflows/release.yml` runs only on a signed annotated
  `v*` tag on main. It pushes only the approved immutable digest from
  `docs/releases/approved-broker.json` (SWB-R57) and verifies the registry
  readback; a different image needs its own ruling first.
- **SWB-R49 local integration while GF is in development:** use
  `just local-integrate` with exact reviewed `PR@FULL_SHA` inputs to assemble
  a separate signed merge tree, then run `just remote-check honey` (Sting
  fallback). The recipe never pushes or changes GitHub main or PR state.
  Its result supports functional development but does not prove the original
  P1a merge-queue exit or replace the separate main-landing gate. Do not
  describe a local integration result as a merged PR.
- **Commits** are GPG-signed, with no AI attribution lines.

## Product invariants

A summary of the rulings code must not break; the ADR Rulings tables carry
the full text. ADR-0002 carries the phase scope and host order
(`SWB-R37`..`SWB-R48`) and the LGTM lookup recipes.

- The broker stamps `authority: peer` on every message and never emits
  `operator`. `operator_directed` is the sender's claim and needs a `ruling`
  pointer, which receivers check before acting (SWB-R14).
- SWB-R02, verbatim: "Self-registration is authoritative; the broker writes
  through to LGTM, which is the read/query/context plane and never the
  commit path." Acks, claims, sequence and leases stay in the broker.
- Readers of LGTM sort by `seq`, enumerate threads only through the broker
  (a Loki or Tempo thread query is a sample, checked against `thread`'s max
  `seq`), and treat an absence as "expired from the view or not projected".
  ADR-0002's refutation review cites this line as the AGENTS.md reader rule.
- Claims are advisory and always succeed. The only refusal is a second
  `exclusive` claim, which returns `held_by` (SWB-R16).
- Hooks time out after 2 s and always exit 0 (SWB-R10).
- `swb channel` bounds every broker request at 1.5 s, retries quietly with
  backoff and never acknowledges on its own. Its events mark content as
  teammate information with `authority` always `peer`, and it never
  declares the permission-relay capability (SWB-R56, SWB-R14).
- Linear access is read plus handoff-receipt comments only. The broker never
  moves state or edits descriptions (SWB-R15).
- Retention is 7 d acked and 30 d unacked; the TTL is 72 h, at most 14 d
  (SWB-R09). Tempo and Loki keep 7 d (SWB-R25).
- A body must never carry a secret. Bodies never go into Tempo span
  attributes, and they stay out of broker stdout until ACL A is enforced live
  and the `loki.process` redaction stage exists (SWB-R19, SWB-R26, SWB-R27,
  SWB-R33, SWB-R34). Harness spans carry tool names, file paths and ticket
  IDs only (SWB-R35); agentd telemetry is metadata only (SWB-R40, SWB-R47).
- A repeated global `msg_id` with a different sender or body returns an
  opaque conflict; exact retries stay idempotent (SWB-R46).

## Durable notes

- **Never leave durable output** in `/tmp`, `/private/tmp` or a harness
  scratchpad. Findings, plans and receipts go to `docs/agent-notes/` (see its
  README) or a dated TIN-4655 comment.
- **Secret-bearing scratch** goes to `~/.claude/agent-notes-rescue/YYYY-MM-DD/`
  and is distilled before the task ends.
