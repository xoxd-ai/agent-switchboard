# R-C416 (TIN-4655): the pinned Nix C toolchain for every Linux x86_64 build.
#
# Bazel imports this through rules_nixpkgs_core (MODULE.bazel, nix_pkg) with
# <nixpkgs> bound to ./nixpkgs.nix, so the toolchain is pinned by this file
# and that one, both release-protected inputs under tools/.
#
# Why zig cc and not the nixpkgs gcc-wrapper: a Nix gcc links against Nix
# glibc (2.42 at this pin, newer than the distroless runtime's 2.41), writes a
# /nix/store dynamic linker and RUNPATH into the binary, and so fails R-C268's
# //deploy:swb_checked guard (R-C282). zig cc is clang plus glibc stubs for a
# chosen glibc version: it links against the stub for GLIBC_FLOOR, writes the
# standard /lib64/ld-linux-x86-64.so.2 interpreter, and needs nothing from the
# host. The output depends only on these pins and the sources, not on the
# host compiler, so a GloriousFlywheel runner with no gcc and Sting build the
# same bytes.
#
# GLIBC_FLOOR is the oldest glibc any of its outputs has to run on: build
# scripts run on the build hosts (Sting and honey, Rocky 10, glibc 2.39; GF
# runners), and the image runs on distroless cc-debian13 (glibc 2.41). 2.34
# also admits RHEL 9 and Ubuntu 22.04 hosts. Changing it changes the image
# digest, so it needs a new release ruling.
{
  pkgs ? import <nixpkgs> { },
}:
let
  zig = pkgs.zig_0_15;
  glibcFloor = "2.34";
  target = "x86_64-linux-gnu.${glibcFloor}";

  # zig needs a writable cache. Bazel's sandbox gives every action a private
  # /tmp (or TMPDIR); zig's cache is content-addressed and changes no output.
  cacheSetup = ''
    cache="''${TMPDIR:-/tmp}/swb-zig-cache"
    export ZIG_GLOBAL_CACHE_DIR="$cache" ZIG_LOCAL_CACHE_DIR="$cache"
  '';

  # The target is fixed here. Callers that add their own target (the cc
  # crate passes --target=x86_64-unknown-linux-gnu to anything clang-like)
  # are ignored, as are rustc's self-contained-linker flags (-B<gcc-ld>,
  # -fuse-ld=lld; zig always links with its own lld) and rustc's -Wl,-O1,
  # which zig's lld ignores with a warning on every link.
  #
  # -fno-sanitize=all: at -O0 (the cc crate's level in fastbuild) zig cc
  # turns on UBSan, whose source locations put the absolute /nix/store paths
  # of zig's libc headers into .rodata. //deploy:swb_checked refuses those
  # (R-C268), and gcc never enabled a sanitizer by default either. A caller
  # that asks for a sanitizer still gets it, because its flag comes later.
  driver =
    name: mode:
    pkgs.writeShellScript name ''
      set -eu
      ${cacheSetup}
      args=()
      skip=0
      for arg in "$@"; do
        if [ "$skip" = 1 ]; then
          skip=0
          continue
        fi
        case "$arg" in
          -target | --target) skip=1 ;;
          --target=* | -B* | -fuse-ld=* | -Wl,-O1) ;;
          *) args+=("$arg") ;;
        esac
      done
      exec ${zig}/bin/zig ${mode} -target ${target} -fno-sanitize=all ''${args[@]+"''${args[@]}"}
    '';

  tool =
    name: subcommand:
    pkgs.writeShellScript name ''
      set -eu
      ${cacheSetup}
      exec ${zig}/bin/zig ${subcommand} "$@"
    '';

  absent =
    name:
    pkgs.writeShellScript name ''
      echo "swb zig toolchain: ${name} is not provided (R-C416)" >&2
      exit 1
    '';
in
pkgs.runCommand "swb-zig-cc-${zig.version}-glibc-${glibcFloor}" { } ''
  mkdir -p "$out/bin"
  ln -s ${driver "cc" "cc"} "$out/bin/cc"
  ln -s ${driver "c++" "c++"} "$out/bin/c++"
  ln -s ${tool "ar" "ar"} "$out/bin/ar"
  ln -s ${tool "objcopy" "objcopy"} "$out/bin/objcopy"
  ln -s ${absent "strip"} "$out/bin/strip"
  ln -s ${absent "gcov"} "$out/bin/gcov"
  printf '%s\n' "${zig.version}" > "$out/zig-version"
  printf '%s\n' "${target}" > "$out/target"
''
