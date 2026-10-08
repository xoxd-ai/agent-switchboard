#!/usr/bin/env bash
# R-C416: tools/cc/nixpkgs.nix (the C toolchain's nixpkgs) must name the same
# nixpkgs revision and NAR hash as flake.lock's `nixpkgs` node.
set -euo pipefail

pin="$1"
lock="$2"

# The `nixpkgs` node's locked block, up to its "original" key.
block="$(awk '/^    "nixpkgs": \{/{n=1} n&&/"original"/{exit} n{print}' "$lock")"
lock_rev="$(sed -n 's/.*"rev": "\([0-9a-f]\{40\}\)".*/\1/p' <<<"$block")"
lock_hash="$(sed -n 's/.*"narHash": "\(sha256-[A-Za-z0-9+/=]*\)".*/\1/p' <<<"$block")"
if [[ -z "$lock_rev" || -z "$lock_hash" ]]; then
  echo "nixpkgs_pin_test: no nixpkgs rev/narHash found in $lock" >&2
  exit 1
fi

pin_rev="$(sed -n 's|.*/nixpkgs/archive/\([0-9a-f]\{40\}\)\.tar\.gz.*|\1|p' "$pin")"
pin_hash="$(sed -n 's/.*sha256 = "\(sha256-[A-Za-z0-9+/=]*\)".*/\1/p' "$pin")"

status=0
if [[ "$pin_rev" != "$lock_rev" ]]; then
  echo "nixpkgs_pin_test: $pin revision '$pin_rev' != flake.lock '$lock_rev'" >&2
  status=1
fi
if [[ "$pin_hash" != "$lock_hash" ]]; then
  echo "nixpkgs_pin_test: $pin hash '$pin_hash' != flake.lock '$lock_hash'" >&2
  status=1
fi
if [[ "$status" == 0 ]]; then
  echo "nixpkgs_pin_test: ok ($lock_rev)"
fi
exit "$status"
