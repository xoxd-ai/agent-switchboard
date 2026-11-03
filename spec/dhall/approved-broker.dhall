-- Source of docs/releases/approved-broker.json. `just spec-dhall` (and the
-- flake check spec-dhall) renders this with dhall-to-json and fails unless it
-- is identical, after `jq -S`, to the committed JSON. Edit this file and the
-- JSON together; release-check.py keeps reading the JSON. SWB-R73 (TIN-7577 d59d35a1, SWB-R74, R-C435), R-C229.
let ApprovedRelease = ./ApprovedRelease.dhall

in    { file_sha256 =
          [ { mapKey = ".bazelversion", mapValue = "1b9487d55bea47fea50d226cc9c53bc548877ad2a318bac8fbc8b320f429e5c5" }
          , { mapKey = "BUILD.bazel", mapValue = "56e6cc630af92bd173f7d9031a4773af3d7b8bb14d7c92d3604b48bf34f31e3c" }
          , { mapKey = "Cargo.lock", mapValue = "d6d1d5f753d4f19810994d8f67395eb54ac3c8204dcf07adb086c4012ad3e613" }
          , { mapKey = "Cargo.toml", mapValue = "8dd6d64c0a714aceb4f9718b5c896391090f73bcfb26f3076ef2362449b279ea" }
          , { mapKey = "MODULE.bazel", mapValue = "30c046fef75b3466b67347adf241fe39e3aea1c3169f3f5e5dec228fd01a492c" }
          , { mapKey = "MODULE.bazel.lock", mapValue = "a2090bf27cad0f4bfb69e34c01790ad9d5d61f8c909191614ede970275b52dad" }
          , { mapKey = "cargo-bazel-lock.json", mapValue = "dc2612d1ebb7480b0408421ea5134c38ef841dff25b73a81bfd8e1715e5ae1cf" }
          , { mapKey = "crates/swb-agentd/BUILD.bazel", mapValue = "4c53792c73662c8cf3737e36559acc042f53a28c4c5d638586e177df1f56adb7" }
          , { mapKey = "crates/swb-agentd/Cargo.toml", mapValue = "9a9241ecf6f202db1835a13ff1a7191be18e29283adecd0542ff0a5a2cb097e8" }
          , { mapKey = "crates/swb-agentd/src/lib.rs", mapValue = "03c8ff21db3b6556a4de5436ecf22589bf4ed43303cd6504b743394ad218bef7" }
          , { mapKey = "crates/swb-broker/BUILD.bazel", mapValue = "e2913c2d01c34aa7dd5571d9d8690f96989b412e8b53fffe77ed25f5727d96b8" }
          , { mapKey = "crates/swb-broker/Cargo.toml", mapValue = "b92e7edb6b245a708f197de03c8e644d166d7275084604eb2e67825122e21dc7" }
          , { mapKey = "crates/swb-broker/src/lib.rs", mapValue = "4ba55d18e71d3d777b846d336d8a2c6331c6c46a91b9fcc2dfc3efe596f01bc8" }
          , { mapKey = "crates/swb-broker/src/service_roundtrip_tests.rs", mapValue = "c192483ffe811b51bab9895bfee105d7739c1d30c8a6d1a2bbfc707154dc26c6" }
          , { mapKey = "crates/swb-broker/tests/stamp.rs", mapValue = "f07fa0a5d40fbb81aa4062354d87f3baf216aa03f66b15520ce0bb6897b117f3" }
          , { mapKey = "crates/swb-proto/BUILD.bazel", mapValue = "25fcf691c3b25c02249fba3b5f880672b5da088a5566f9d8b9ab4076ae604e8c" }
          , { mapKey = "crates/swb-proto/Cargo.toml", mapValue = "b88b6f22fe6d6f6fc34aff96fa459628085e44cd1841f615dfc77b8925c01471" }
          , { mapKey = "crates/swb-proto/src/lib.rs", mapValue = "8a574dde848ab9c6c4ceb6830625b0577e57cc7654bbb47ec178062f2a151621" }
          , { mapKey = "crates/swb-store/BUILD.bazel", mapValue = "49b6d7f5468c388810a9afe15a0a6596b1238978629a682f104280bde54c83b0" }
          , { mapKey = "crates/swb-store/Cargo.toml", mapValue = "1c7dcf3a8476f2dbb4e6097a623326906753f919d00c8abea9722e25d332c459" }
          , { mapKey = "crates/swb-store/src/claims.rs", mapValue = "a3d35a26d0ec6d9e2670f58dc082049d92dcf125aca7fd25d5b8fed9687ae77f" }
          , { mapKey = "crates/swb-store/src/lib.rs", mapValue = "a64baad973a246653951b4acfddc4d67c742a5862b38b29c0840449834870bde" }
          , { mapKey = "crates/swb-store/src/ratelimit.rs", mapValue = "d2c59dfd29ae77944da7cfcae8dbba8e7982cc11d18ab7254c014771d5223190" }
          , { mapKey = "crates/swb/BUILD.bazel", mapValue = "b2bd00a02f1acb7445105e20c5ba4a4078463a525e34a4ba4ebe1c6f8ed2bcef" }
          , { mapKey = "crates/swb/Cargo.toml", mapValue = "66eeb9a4fb4d22b4735f688bc8dfa359db870b23d744cd7af74895f79b71de35" }
          , { mapKey = "crates/swb/src/channel.rs", mapValue = "40438e0574e5285d41f5c6983fa1f5a5e10d65373c869bf05027ef04ca91cf74" }
          , { mapKey = "crates/swb/src/main.rs", mapValue = "e9fc2322c66d6c961036ac66f54b6b2753386952da0063da6c280df3b9796498" }
          , { mapKey = "crates/swb/tests/hook_blackhole_test.sh", mapValue = "ab2fcabd4bbbb10aed0c27f6def3a2a6111d3720b4e60751a943c2279da53c00" }
          , { mapKey = "deploy/BUILD.bazel", mapValue = "414e9a927e0dfb89f8236367db755014ce8b56b815621ee1d0f9da99a1265ac0" }
          , { mapKey = "platforms/BUILD.bazel", mapValue = "6d5c7d3dada37113084e621e8065a05e4e58e0c7dd7d206dfd7f0cb3f4507d25" }
          , { mapKey = "tools/BUILD.bazel", mapValue = "d7ae7983138ad3c50004efd36fb29cac5583647b5dc3f46189f594f2470bd31c" }
          , { mapKey = "tools/cc/BUILD.bazel", mapValue = "b4fdf94422a4c57858f0d456b6ae4460755ef2ab061768861086440f18b45436" }
          , { mapKey = "tools/cc/nixpkgs.nix", mapValue = "910583bcfa3664bdc3abef72d1bbc248d51f25ceb9e394bbe775b077b8789206" }
          , { mapKey = "tools/cc/nixpkgs_pin_test.sh", mapValue = "2b6d116888fa88ce9c4ed101ceddfbeb6f933b3940dde9f34083458458790fbc" }
          , { mapKey = "tools/cc/zig_cc.BUILD", mapValue = "93048670eac2e0e9a226627a762dc28ef86ee5cd223756398aa42d17fc30501f" }
          , { mapKey = "tools/cc/zig_cc.nix", mapValue = "f536da29f96dfce255ab24bb75999fbcf93d2196561c9aca5affa4ca7e7ab26d" }
          , { mapKey = "tools/clippy_test.sh", mapValue = "1a0c16f7e334bdfae84d8f25dab84cc2dfb45a8adcd95189e0d9662d10559261" }
          ]
      , image = "ghcr.io/xoxd-ai/agent-switchboard@sha256:77e004b2a0a93b44b1555239066856621c4f00e604e66b272b22dd94f94256f5"
      , live_acceptance = False
      , manifest_size = 4579
      , platform =
        { architecture = "amd64"
        , cmd = [ "serve" ]
        , entrypoint = [ "/usr/local/bin/swb" ]
        , os = "linux"
        , user = "65532"
        }
      , pr_heads =
          [ { mapKey = "60", mapValue = "7c88e73816d9f728c3d2aebe6b51cacb0d18c30e" }
          ]
      , publication_receipt =
          "PUBLISHED 2026-11-03T02:44:33Z as GitHub release v0.5.1 (SWB-R73) under the signed v0.5.1 tag (tag object f2c51157f2c8719dff84d539d034d5ff481b58a6 on 8911e40465b4ce3b31d2d34b803dc0dfce666ba5): run https://github.com/xoxd-ai/agent-switchboard/actions/runs/41550219988 rebuilt the approved digest, pushed it by digest with no registry tag (2026-11-03T02:38:46Z-02:38:59Z) and read it back; release-check --registry-manifest passed (supplied_registry_manifest_matches true); registry readback digest sha256:77e004b2a0a93b44b1555239066856621c4f00e604e66b272b22dd94f94256f5, 4579 bytes, byte-identical to the approved manifest; evidence artifact release-evidence-v0.5.1 (id 13199403319, zip sha256:5566355ceb91ac6b7ba0aa7b0980fbb6b2ef18e1b74fc3649bf7c606d165474a, kept 90 days); an anonymous readback at 2026-11-03T02:46Z is byte-identical to that manifest and release-check --registry-manifest passed on it; release assets read back after publication with sha256sum -c OK: swb-v0.5.1-x86_64-linux sha256:7834352297f9e4ba45fd4ae8ad39793b9e7941f44921e77324668ad14d0ca625, swb-v0.5.1-aarch64-darwin sha256:96a71db46b750aeacbe2f8d115afb01dcf0d9f24f47ecf410d7e4e342cd23bde, install.sh sha256:c4b2ca2ebc7cb848b05a9cb4a4a0475eaf8f4987c10adfc2792dea86af7bf423; SHA256SUMS.asc verified against release-signers.asc (primary 161895136D2E5C292D2A663D0B01977B8DD5DA60); install.sh v0.5.1 on x86_64-linux with SWB_REQUIRE_SIGNATURE=1 printed swb 0.5.1 envelope v3 at 2026-11-03T02:45Z; install.sh v0.5.1 on aarch64-darwin with its checksum check printed swb 0.5.1 envelope v3 at 2026-11-03T02:45Z; receipt on TIN-7577; ratified SWB-R73 under TIN-7577 comment d59d35a1 (operator, 2026-10-28) and R-C435 (TIN-5770 comment d3d90700)"
      , qualification = "published-candidate-only"
      , rulings = [ "SWB-R73", "SWB-R74", "R-C416", "R-C435", "R-N13" ]
      , schema = "swb.approved-release.v1"
      , source = "154cfb19f4242a0c995249c153dcb7bf887d3a35"
      , source_inputs =
          [ ".bazelversion", "BUILD.bazel", "MODULE.bazel", "MODULE.bazel.lock", "Cargo.toml", "Cargo.lock", "cargo-bazel-lock.json", "crates/", "deploy/", "platforms/", "tools/" ]
      , upstream_main = "96a5060a5c3777eb8cdd5b0a79230fc12864523e"
      }
    : ApprovedRelease
