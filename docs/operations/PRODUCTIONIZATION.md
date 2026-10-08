# Switchboard source and productionization lane

Authority: SWB-R49–SWB-R57, LAB-TAKEOVER-20260930 and R-N13.
Source/image ownership is here; lab delivers harness/Home Manager integration;
blahaj owns custody, retained storage, routing and cluster rollout (SWB-R51).

The approved release is SWB-R57, v0.2.0 (R-C411): signed source
`26f38b9ac4652866ea30d6553bac2374f7494e4d` at
`ghcr.io/xoxd-ai/agent-switchboard@sha256:66e439667c25791002e6af86d3bf088c5cbe5b35021874437fd9e1b202284555`.
The previous release, SWB-R55 v0.1.0 (`d8ebfdbf` at `sha256:c9170c71…`),
stays published; its record is in git history.
Its protected-input hashes and PR heads are in
[`approved-broker.json`](../releases/approved-broker.json), whose
`qualification` is `published-candidate-only` and `live_acceptance` false.
SWB-R53 (`b5158729`, `b0633ecb`) is never published. Later main commits that
leave the 27 protected inputs unchanged need no new approval.

## Source integration and upstream landing

1. Use `just local-integrate-named NAME 'PR@FULL_SHA ...'` for reviewed PR
   heads. It fetches current canonical main and exact heads, verifies source
   signatures, and creates signed merges in a new write-once worktree. It
   refuses reused paths/branches. Existing candidate trees remain intact.
   `local-integrate-named-dry-run` checks inputs without creating the tree.
2. `just release-check` checks the published source signature and ancestors,
   all 27 build/runtime input hashes, and absence of additional protected
   inputs. Supplied `--oci-layout PATH` checks the exact manifest plus every
   layer/config blob and linux/amd64, UID 65532, entrypoint and command.
   `--registry-manifest PATH` checks raw bytes from a recorded registry GET.
   This is read-only evidence validation; it makes no registry request and
   cannot admit a deployment or authorize a successor publication.
3. Source changes affecting protected inputs require a new reviewed build
   and bounded remote qualification on Honey, with Sting fallback. Bazel is
   the authority; never compile on Neo. The immutable candidate already has
   Honey `just check`, `build`, `image` and rootless startup receipts. Retain
   those exact-source receipts when only non-runtime tooling changes.
4. Open PR heads are in GitHub; the heads the SWB-R55 release was built
   from (#13, #16, #18) stay pinned in the release ledger. R49 authorizes local integration while GF is in development, but
   local integration does not satisfy the protected-main gate (signed
   commits, a fork PR and `ci-ok`, landed by the queue or by an R-C228 admin
   merge after sting validation). Do not enqueue or dispatch GF as part of
   this lane.

## Injectable clock and the SWB-R55 release (R-C262)

R-C262 (operator interview 2026-10-04, TIN-4655 comment `230af90b`) added a
clock seam to `swb-store`. It edited `crates/` and the root `BUILD.bazel`,
which are protected inputs, so it needed a new approved release: SWB-R55,
recorded below.

- **Production behaviour is unchanged.** `Store::open` and `Store::memory`
  use `SystemClock`, the truncated Unix seconds SQLite `unixepoch()` returns.
  Each operation reads the clock once and binds the value into its SQL. The
  envelope `sent_at`/`expires_at` strings keep their format.
- **The test control path is off in production builds.** `ManualClock` and
  the `POST /v1/test/clock` route exist only for the spec live adapter. The
  route is compiled only with the `test-clock` Cargo feature, carried by the
  testonly Bazel targets `//crates/swb-broker:swb_broker_test_clock` and
  `//crates/swb:swb_test_clock`. `//:build`, `//crates/swb:swb` and
  `//deploy:image` do not enable it. Even the feature binary keeps the wall
  clock unless `swb serve` starts with `SWB_TEST_CLOCK=1`, and it then
  refuses any non-loopback listener.
- **No v2 release shape is needed.** The protected file set (27 paths) and
  `SOURCE_INPUTS_V1` roots are unchanged; only file contents change. The
  successor record keeps `swb.approved-release.v1` and needs: the signed
  source sha that contains this change, its `upstream_main` and `pr_heads`,
  recomputed `file_sha256` for all 27 paths, and the image digest, manifest
  size and registry readback from the Sting build. The digest is recorded
  after that build, in the Land step, never before.
- **Sequencing with v0.1.0.** R-C268 (operator interview 2026-10-04,
  TIN-4655 comment `67ebf4f4`) supersedes R-C262's fallback: fix build
  determinism first, so two clean builds of one source give one digest,
  then v0.1.0 ships the clock-seam image. SWB-R53 is never published.
- **Build determinism (R-C268).** SWB-R53 `b0633ecb` cannot be rebuilt:
  libsqlite3-sys compiles SQLite with `-g`, and gcc recorded the build
  script's sandbox directory (output-base hash plus sandbox counter) in the
  debug info, which rules_rust does not remap. `//crates/swb:swb` now links
  with `-Cstrip=debuginfo`, and `//deploy:swb_checked` refuses a binary that
  contains `/execroot/`, `/sandbox/` or `/nix/store/` (a Nix C compiler from
  `nix develop` leaking into rules_cc). The image carries
  `org.opencontainers.image.source` and an `org.opencontainers.image.revision`
  label from `--embed_label=<sha>` (`just image` passes HEAD; without it the
  value is `unstamped`). These change the protected inputs; the file set
  stays at 27 paths.
- **Release revision label (TIN-4655, comment 3126a35f).** The label is part
  of the digest, and `release-check` lets the tag sit on a descendant of the
  approved `source` (an approval or workflow commit). A release therefore
  labels the image with approved-broker.json `source`, never the tag or HEAD:
  `just release-image` runs `release-check`, then builds with
  `--embed_label=$(just release-source)`. The release workflow must pass that
  same `source` to both its build and its push step, and keep the signature,
  ancestry, protected-input and digest checks unchanged.
- **Ruling ID: SWB-R55.** R-C262 and R-C268 named the successor release
  `SWB-R54`, but ADR-0001 already uses SWB-R54 for state custody
  (2026-09-27). R-C274 (operator interview 2026-10-04, TIN-4655 comment
  `c4dfd4c8`) keeps SWB-R54 as custody, and the dated correction in
  TIN-4655 comment `cdde44c6` names this release SWB-R55. Its record is
  written under that ID.
- **SWB-R55 recorded (R-C304).** The operator ratified SWB-R55 (TIN-4655
  comment `dcb5b687`) at `sha256:c9170c71…`, 4579-byte manifest, from two
  clean Sting builds of main `d8ebfdbf` with `--embed_label=d8ebfdbf…`.
  `approved-broker.json` and its Dhall source carry it. `d8ebfdbf` is a
  GitHub merge commit signed by GitHub's key `B5690EEEBB952194`, so
  `docs/releases/release-signers.asc` holds that key next to the operator
  release key `161895136D2E5C29…`.
- **SWB-R57 recorded (R-C411).** The operator ratified the v0.2.0 channel
  release (TIN-5770 comment `cdeb84f6`) at `sha256:66e43966…`, 4579-byte
  manifest, from two clean Sting builds of main `26f38b9a` with
  `--embed_label=26f38b9a…`, confirmed by a third rebuild that mirrors
  `release.yml`. The protected-input map adds `crates/swb/src/channel.rs`.
  The tag sits on the merge of this record, not on `26f38b9a`: the workflow
  reads the record from the tagged tree and labels the image with its
  `source`.
- **Hermetic C toolchain (R-C416, amends R-C411 and R-C282).** The v0.2.0
  tag run (37678965278) failed with `Cannot find gcc or CC`: GloriousFlywheel
  runners have no host C compiler, and the SWB-R57 digest embeds SQLite as
  compiled by Sting's gcc 14.3.1. The operator chose "Hermetic toolchain"
  (TIN-5770 comment `9b9667d2`). `MODULE.bazel` now registers
  `//tools/cc:zig_x86_64_linux_toolchain` ahead of rules_cc's host
  autodetection. It is zig cc 0.15.2 from the nixpkgs revision in
  `flake.lock`, imported with rules_nixpkgs_core (`tools/cc/nixpkgs.nix`
  pins it by NAR hash, `tools/cc/zig_cc.nix` wraps it, and
  `//tools/cc:nixpkgs_pin_test` keeps the two pins equal). The wrapper fixes
  the target at `x86_64-linux-gnu.2.34`: the binary links against glibc 2.34
  stubs and asks for `/lib64/ld-linux-x86-64.so.2`, so it runs on the
  distroless cc-debian13 base (glibc 2.41) and on the build hosts (2.39). A
  Nix gcc-wrapper could not do this: it links Nix glibc 2.42 and writes a
  `/nix/store` interpreter. The wrapper also turns off zig's default UBSan
  at `-O0`, whose source locations would put `/nix/store` header paths in
  the binary. Every Linux x86_64 build, `just check` included, now needs
  `nix-build` on PATH and no host gcc. The image digest changes, so
  `sha256:66e43966…` is not published; the new digest needs its own ruling
  and record before any tag can publish it.

## Secrets scan, CODEOWNERS and tag-triggered release

- `ci-ok` now also requires the `secrets-scan` job: the pinned
  `xoxd-ai/ci-templates` secrets-scan action (TruffleHog `--only-verified`,
  then gitleaks 8.30.1 with `.gitleaks.toml`) over the full history. Like the
  Rust lane it is skipped on a fork PR and runs on `merge_group` and
  `push: main`. Run `nix develop --command just secrets-scan` on Sting (or any
  seat; it builds nothing) before a fork PR lands.
- `.github/CODEOWNERS` names the owner for every path and the release
  surfaces. The ruleset does not require code-owner review.
- `.github/workflows/release.yml` runs only for a signed annotated tag
  `vMAJOR.MINOR.PATCH[-pre]` pushed to `xoxd-ai/agent-switchboard`. It depends
  on no `merge_group` result (R-C228). It lands in its own PR before
  `v0.1.0` (R-C238). Fail-closed order:
  1. import `docs/releases/release-signers.asc` into an empty key ring and
     require exactly the two pinned primaries: the operator release key and
     GitHub's merge key `B5690EEEBB952194` (the SWB-R55 source signer);
  2. `release-check --tag TAG --tag-signer <operator key> --main-ref
     origin/main`: the tag is annotated, signed by the operator release key
     (GitHub's key never satisfies this), points at the checked-out commit and
     that commit is on main; the signed source and all 27 protected inputs
     match. The approved `source` is then read once from
     `approved-broker.json` and checked to be an ancestor of HEAD;
  3. build `//deploy:image.digest` and `release-check --built-digest`: refuse
     to push unless it equals the approved immutable digest. The C toolchain
     is the pinned Nix zig cc from `//tools/cc` (R-C416), so the step needs
     `nix-build` on PATH and no host compiler; it refuses if `CC` or `CXX`
     is set, so rules_cc's host fallback stays unused, and passes the
     approved `source` as `--embed_label` so the revision label matches the
     approved build;
  4. `bazelisk run //deploy:push` (digest only, `packages: write` token),
     with the same approved `source` as `--embed_label`. Both Bazel steps use
     the `release` devShell's flake.lock-pinned `bazelisk` and refuse one that
     does not resolve into `/nix/store`; the former runner-supplied Bazelisk
     custody step was removed (operator direction 2026-10-05, applies from
     the release after v0.1.0);
  5. read the manifest back from ghcr.io by digest, verify digest and byte
     size with `release-check --registry-manifest`, and keep the report,
     manifest bytes and built digest as the `release-evidence-TAG` artifact.
- `just release-check-tag TAG [MAIN_REF]` runs the step 2 gate locally.
- The workflow writes evidence only. `publication_authorized` and
  `live_acceptance` stay false; it never edits `approved-broker.json`. Today
  the record still names the SWB-R57 digest `sha256:66e43966…`, which the
  R-C416 toolchain no longer builds, so every tag fails closed at
  `--built-digest` until a new digest is ratified and recorded. Publishing
  any image needs a ruling and an approved release entry first.
- The `v0.1.0` tag run of this workflow failed: at the time it required a
  runner-supplied `TINYLAND_CI_BAZELISK_BIN` that GloriousFlywheel runners
  did not provide. v0.1.0 was therefore published by hand under R-C312: two
  clean Sting builds, `release-check --tag v0.1.0 --built-digest --oci-layout`,
  `skopeo copy --preserve-digests` of the OCI layout, and a byte-identical
  registry readback (GitHub release v0.1.0 records it). The flake-pinned
  bazelisk change (#20) applies from the next tag. The package's Actions
  write grant and the tinyland-nix runner's reach to ghcr.io are still
  unexercised by this workflow. The ledger's `publication_receipt` records
  the R-C312 manual push and its readback (TIN-4655 comment `828714be`); it
  is a docs field, not a protected input, so editing it leaves release-check
  unchanged.

## Packaging, retention and shutdown boundaries

The image is the Debian 13 distroless nonroot Linux/amd64 package declared in
`MODULE.bazel` and `deploy/BUILD.bazel`. The runtime base and image are digest
bound. Rootless `swb version` proved executable startup, not broker readiness
or an authenticated node pull. SWB-R55 covers the one approved image only.

SQLite uses WAL and FULL synchronization. `Store::open` prunes on startup;
the broker repeats pruning hourly without requiring mailbox traffic. Acked
bodies retain seven days, unacked bodies thirty days, and thread high-water
sequence survives pruning (SWB-R09). These are broker retention limits,
separate from state-store/backup retention commissioning.

The broker receives its own shutdown request, drains ordinary listeners for
thirty seconds, cancels remaining MCP streams, and allows five more seconds.
The tests use owned futures and channels, with no process signal. This bounds
listener futures; blocked SQLite work can still prevent full process exit.
The sixty-second pod grace does not prove storage failure recovery. Agent
process signaling remains forbidden (R-N11).

The existing service-proof source at `24c03ab9` passed 24 owned broker/store/
CLI cases on Sting, and lab's managed hooks passed 18 cases at exit zero and
under two seconds. That is useful development evidence. The tested CLI's
GLIBC_2.39 environment is separate from the approved packaged image. Do not
silently substitute that binary or its source for the approved release, or call owned
fixtures live harness enrollment or PVC pod-restart proof.

## Actual rollout prerequisites and exit receipts

The cluster source is staged in blahaj
[#1793](https://github.com/xoxd-ai/blahaj/pull/1793) (merged 2026-10-03,
replacing the closed #1629 and #1731) under `tofu/stacks/agent-switchboard`.
It still pins the SWB-R53 digest `b0633ecb`, which is never published; it
must move to the SWB-R55 digest before any apply. TIN-5105 remains the
custody owner lane. Its current CREATEONLY/suspended qualifier and exclusive commissioning
window are unresolved; no broker hostname or live deployment is issued.

| Gate | Concrete next action and required evidence |
| --- | --- |
| Dedicated state custody | Blahaj owner completes the existing CREATEONLY decision, separate identity and encrypted carrier, primary history/deletion retention, off-node backup and isolated scratch recovery destination. Prove state payload, lock contention, unlock and scratch restore before reviewed backend release. Keep issuer credentials and Jobs in that owner's lane. |
| Namespace and image pull | After custody admission, use the reviewed governed namespace/Secret path; verify the exact digest with the target node and retain UID/RV and authenticated pull receipts. Client-side historical GET alone is insufficient. |
| Storage and route | Prove Bumble's retained 1 Gi RWO PVC is writable by UID/GID 65532, backup bootstrap and restore. Read actual Tailscale-issued MagicDNS from the live Service, then set the exact Host allowlist and lab client URL. Alias waits for route proof. |
| Harness delivery | Lab delivers the exact reviewed closure and verifies native Neo Claude/Sting Codex identities and Pi registration. Preserve combined Junie/Pi broker+LGTM paths and measured budgets. Enroll Honey/Bumble next, then Yoga/mbp-13; PZM remains storage held. |
| P1b functional acceptance | Record a threaded Neo Claude↔Sting Codex exchange and Pi reply/ack; durable inbox after an owner-admitted pod restart; broker-down nonblocking hook behavior plus #1920/Linear fallback; and exported registry URL matching the actually issued MagicDNS. Every receipt binds source, execution host and real session/message IDs. |

Usable L0 reads remain required for the combined paths. Telemetry scrubbing
and source-provenance qualification follow v1 and do not block MVP (SWB-R48);
message bodies remain disabled until their separate live ACL/redaction gates.
New backend applies, owner credentials, Jobs, restarts or down-fault tests
require the existing owner admission; this source packet does not grant it.
