set shell := ["bash", "-euo", "pipefail", "-c"]

# Mirror the CI driver: never let an ambient crate_universe repin or generator
# override leak into a normal build.
clean_bazel_env := "env -u CARGO_BAZEL_DEBUG -u CARGO_BAZEL_GENERATOR_SHA256 -u CARGO_BAZEL_GENERATOR_URL -u CARGO_BAZEL_ISOLATED -u CARGO_BAZEL_REPIN -u CARGO_BAZEL_REPIN_ONLY -u CARGO_BAZEL_TIMEOUT -u REPIN"

upstream_repo := "xoxd-ai/agent-switchboard"

default:
    @just --list

# Bazel is the build and test authority. Never run on neo (teletype seat;
# the local-build guard refuses it): use `just remote-check` from neo.
# Run rustfmt, clippy, unit and integration tests (the labels CI runs).
check:
    {{clean_bazel_env}} bazelisk test --lockfile_mode=error //:check

# Build the application and package targets CI builds.
build:
    {{clean_bazel_env}} bazelisk build --lockfile_mode=error //:build //deploy:image //deploy:image.digest

# Commit all three. Run on linux x86_64 (sting or honey), the platform CI
# checks the locks on.
# Regenerate Cargo.lock, cargo-bazel-lock.json and MODULE.bazel.lock together.
lock:
    {{clean_bazel_env}} CARGO_BAZEL_REPIN=1 bazelisk mod deps --lockfile_mode=update >/dev/null
    {{clean_bazel_env}} CARGO_BAZEL_REPIN=1 bazelisk build --lockfile_mode=update @crates//:defs.bzl
    {{clean_bazel_env}} bazelisk build --lockfile_mode=update --nobuild //...
    {{clean_bazel_env}} bazelisk mod deps --lockfile_mode=update >/dev/null
    git status --short -- MODULE.bazel.lock Cargo.lock cargo-bazel-lock.json

# The tree is rsynced to a scratch dir on the build host; neo builds nothing.
# Run `just check` on a build host from a teletype seat (neo).
remote-check host="sting" dir="~/scratch/agent-switchboard-check":
    ssh {{host}} 'mkdir -p {{dir}}'
    rsync -a --delete --exclude '/bazel-*' --exclude '/target/' --exclude '/.git/' ./ {{host}}:{{dir}}/
    ssh {{host}} 'cd {{dir}} && just check'

# SWB-R49 local integration while GF is in development (TIN-4655).
# Pin each PR to its reviewed full SHA. This creates a separate,
# signed local merge tree; it never pushes or changes GitHub PR/main state.
# Example: just local-integrate '2@<full-sha> 3@<full-sha> 4@<full-sha> 5@<full-sha>'
local-integrate refs:
    bash ./scripts/local-integrate.sh {{ quote(refs) }}

local-integrate-dry-run refs:
    bash ./scripts/local-integrate.sh --dry-run {{ quote(refs) }}

# Keep the existing integration candidate and create a second, immutable one.
# Example: just local-integrate-named second '2@<full-sha> 3@<full-sha>'
local-integrate-named name refs:
    bash ./scripts/local-integrate.sh --name {{ quote(name) }} {{ quote(refs) }}

local-integrate-named-dry-run name refs:
    bash ./scripts/local-integrate.sh --dry-run --name {{ quote(name) }} {{ quote(refs) }}

# SWB-R57 / R-N13: read-only approved-source and supplied OCI evidence checks.
# No build, publication, registry credentials or deployment admission.
[positional-arguments]
release-check *args:
    python3 ./scripts/release-check.py "$@"

release-check-test:
    python3 -m unittest discover -s scripts -p test_release_check.py

# TIN-4655 (SWB-R57): verify a signed annotated release tag at HEAD that is on
# upstream main, plus the approved source inputs. Read-only; the release
# workflow runs the same gate before it builds or pushes anything.
release-check-tag tag main_ref="upstream/main":
    python3 ./scripts/release-check.py --tag {{ quote(tag) }} --tag-signer 161895136D2E5C292D2A663D0B01977B8DD5DA60 --main-ref {{ quote(main_ref) }}

# The scanners CI's `secrets-scan` job runs (TruffleHog --only-verified, then
# gitleaks with .gitleaks.toml) over the full history. Both ship in the
# devShell: `nix develop --command just secrets-scan`. This is the pre-queue
# signal for a fork PR, whose CI scan is skipped until merge_group.
# nixpkgs wraps trufflehog with `--no-update` already, and kingpin refuses a
# repeated flag ("flag 'no-update' cannot be repeated"), so the recipe adds it
# only when trufflehog on PATH is not a wrapper script carrying it (R-C255, TIN-4655).
secrets-scan:
    #!/usr/bin/env bash
    set -euo pipefail
    for tool in trufflehog gitleaks; do
      command -v "$tool" >/dev/null || { echo "secrets-scan: $tool is not on PATH (use nix develop)" >&2; exit 1; }
    done
    th="$(command -v trufflehog)"
    no_update=(--no-update)
    if [[ "$(head -c 2 "$th")" == '#!' ]] && grep -q -- '--no-update' "$th"; then
      no_update=()
    fi
    trufflehog ${no_update[@]+"${no_update[@]}"} git "file://$PWD" --only-verified --fail
    gitleaks git --config .gitleaks.toml --redact --exit-code 1 .

# PRs go from the fork to upstream main through the merge queue (ADR-0001).
# Set remotes: origin = your private fork (the only push target), upstream = xoxd-ai with push DISABLED.
fork-setup fork_owner="Jesssullivan":
    #!/usr/bin/env bash
    set -euo pipefail
    upstream_url="https://github.com/{{upstream_repo}}.git"
    fork_url="https://github.com/{{fork_owner}}/agent-switchboard.git"
    if git remote get-url upstream >/dev/null 2>&1; then
      git remote set-url upstream "$upstream_url"
    else
      git remote add upstream "$upstream_url"
    fi
    git remote set-url --push upstream DISABLED
    if git remote get-url origin >/dev/null 2>&1; then
      git remote set-url origin "$fork_url"
    else
      git remote add origin "$fork_url"
    fi
    git fetch upstream
    git fetch origin
    if git show-ref --verify --quiet refs/heads/main; then
      git branch --set-upstream-to=upstream/main main
    fi
    git remote -v

# Start a branch named per the convention: <type>/tin-####-<slug>-<yyyymmdd>.
branch type tin slug:
    git fetch upstream
    git switch -c "{{type}}/tin-{{tin}}-{{slug}}-$(date -u +%Y%m%d)" upstream/main

# Build the immutable Linux/amd64 OCI image and print the exact manifest digest.
# R-C268: the image's org.opencontainers.image.revision label is the commit
# given here (default: HEAD, and then the worktree must be clean). Two builds
# of one commit give one digest. The C toolchain is the pinned Nix zig cc
# from //tools/cc (R-C416), whatever compiler the host or shell has, so
# `nix-build` must be on PATH and no host gcc is needed.
# For a release use `just release-image`, never the HEAD default.
image revision="":
    #!/usr/bin/env bash
    set -euo pipefail
    rev="{{revision}}"
    if [ -z "$rev" ]; then
      if [ -n "$(git status --porcelain --untracked-files=no)" ]; then
        echo "image: worktree has changes; commit them or pass an explicit revision" >&2
        exit 1
      fi
      rev="$(git rev-parse HEAD)"
    fi
    {{clean_bazel_env}} bazelisk build --lockfile_mode=error --embed_label="$rev" //deploy:image //deploy:image.digest
    digest="$(cat bazel-bin/deploy/image.json.sha256)" && printf 'ghcr.io/xoxd-ai/agent-switchboard@%s\n' "$digest"

# The verified source commit of the approved release:
# docs/releases/approved-broker.json `source`, a 40-hex sha.
release-source:
    @python3 -c 'import json,re,sys; s=json.load(open("docs/releases/approved-broker.json")).get("source",""); sys.exit("release-source: approved-broker.json source is not a 40-hex sha") if not re.fullmatch("[0-9a-f]{40}", s) else print(s)'

# The release image (TIN-4655, comment 3126a35f). The revision label, and so
# the digest, comes from the approved `source`, never from the tag or HEAD:
# a tag may sit on a later approval or workflow commit, and labelling that
# commit would change the digest the record approves. release-check runs
# first (signature, ancestry, protected inputs). The release workflow passes
# this same `source` as --embed_label to both its build and its push.
release-image:
    #!/usr/bin/env bash
    set -euo pipefail
    python3 ./scripts/release-check.py
    src="$(just release-source)"
    git merge-base --is-ancestor "$src" HEAD || { echo "release-image: approved source $src is not an ancestor of HEAD" >&2; exit 1; }
    just image "$src"

# R-C229 formal spec. Dhall checks are interpreters only and run on any
# seat, neo included: `nix shell` substitutes the tools from the locked
# nixpkgs and compiles nothing. Type-check the Dhall,
# require approved-broker.json and the generated broker constants to equal
# their Dhall sources, and round-trip the pinned blahaj owners.json snapshot.
spec-dhall:
    nix shell --inputs-from . nixpkgs#dhall nixpkgs#dhall-json nixpkgs#jq --command bash spec/check-dhall.sh

# Round-trip a live blahaj config/rustfs-iam/owners.json through its Dhall type.
spec-owners owners_json:
    nix shell --inputs-from . nixpkgs#dhall nixpkgs#dhall-json nixpkgs#jq --command bash spec/check-dhall.sh "$PWD" {{ quote(owners_json) }}

# Build hosts only (sting or honey), never neo: compiles the Haskell model
# and runs the QuickCheck properties.
spec-quickcheck:
    nix build --no-link -L .#swb-spec

# Build hosts only (sting or honey), never neo: LiquidHaskell checks the
# refinement types in spec/haskell/src/Swb/Invariants.hs (R-C261).
spec-liquid:
    nix build --no-link -L ".#checks.$(nix eval --raw --impure --expr builtins.currentSystem).spec-liquid"

# Every spec check. Build hosts only; from neo use `just remote-spec-check`.
spec-check: spec-dhall spec-quickcheck spec-liquid

# Build hosts only. Run the properties against a disposable broker that is
# already listening on loopback (see spec/README.md); refuses other URLs.
spec-live url="http://127.0.0.1:18080":
    SWB_SPEC_BROKER_URL={{ quote(url) }} nix run .#spec-live

# The same, with Tick, against a broker on the R-C262 test clock
# (`swb_test_clock serve` with SWB_TEST_CLOCK=1; see spec/README.md).
spec-live-clock url="http://127.0.0.1:18080":
    SWB_SPEC_LIVE_CLOCK=1 SWB_SPEC_BROKER_URL={{ quote(url) }} nix run .#spec-live

# Run `just spec-check` on a build host from a teletype seat (neo). `/.git`
# without a trailing slash also skips a worktree's .git pointer file, so the
# copy is a plain path flake.
remote-spec-check host="sting" dir="~/scratch/agent-switchboard-spec":
    ssh {{host}} 'mkdir -p {{dir}}'
    rsync -a --delete --exclude '/bazel-*' --exclude '/target/' --exclude '/.git' --exclude '/spec/haskell/dist-newstyle/' ./ {{host}}:{{dir}}/
    ssh {{host}} 'cd {{dir}} && just spec-check'

# Stub (P1b): end-to-end round trip against a live broker.
e2e:
    @echo "e2e: lands with the P1b broker MVP (ADR-0001, phase P1b exit)" >&2
    @exit 1
