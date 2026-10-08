# R-C416 (TIN-4655): the nixpkgs that supplies the release C toolchain.
#
# The same revision and NAR hash as flake.lock's `nixpkgs` node, so Bazel's
# C toolchain and the devShells come from one nixpkgs. Fetched by NAR hash,
# not by tarball bytes, so a re-encoded GitHub archive cannot change it.
# Change it only together with flake.lock; //tools/cc:nixpkgs_pin_test
# refuses a mismatch.
import (builtins.fetchTarball {
  url = "https://github.com/NixOS/nixpkgs/archive/c508844df6c28fa6dabc1b6af70f3ccbd65c5201.tar.gz";
  sha256 = "sha256-6e4Na3z008XpdVyOXgfasXIn1aN+z8AXFG+jXdQSyuI=";
})
