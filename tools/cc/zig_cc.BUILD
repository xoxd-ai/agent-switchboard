# R-C416 (TIN-4655): BUILD file for the rules_nixpkgs_core import of
# zig_cc.nix (MODULE.bazel nix_pkg `swb_zig_cc`). The tools are wrapper
# scripts in /nix/store that call a pinned zig by absolute store path.
package(default_visibility = ["//visibility:public"])

exports_files(glob(["bin/*"]))

filegroup(
    name = "toolchain_files",
    srcs = glob([
        "bin/*",
        "target",
        "zig-version",
    ]),
)
