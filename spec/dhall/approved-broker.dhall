-- Source of docs/releases/approved-broker.json. `just spec-dhall` (and the
-- flake check spec-dhall) renders this with dhall-to-json and fails unless it
-- is identical, after `jq -S`, to the committed JSON. Edit this file and the
-- JSON together; release-check.py keeps reading the JSON. SWB-R58 (R-C451), R-C229.
let ApprovedRelease = ./ApprovedRelease.dhall

in    { file_sha256 =
          [ { mapKey = ".bazelversion", mapValue = "1b9487d55bea47fea50d226cc9c53bc548877ad2a318bac8fbc8b320f429e5c5" }
          , { mapKey = "BUILD.bazel", mapValue = "02051c3539cc0cba44203172c035d68e59a3e77ff279e4c227dfb8e9b8114780" }
          , { mapKey = "Cargo.lock", mapValue = "cf22f6fde95f57df7c8fcaf2296c7234dd555fa74e6fd466a1c9c7ed8dfa95bd" }
          , { mapKey = "Cargo.toml", mapValue = "b0a887a07c7b33d22ac7c87c6a80198657c4e14cf60224c2d061c005f29bf7ec" }
          , { mapKey = "MODULE.bazel", mapValue = "d68ede0f08ea630b15af927cc2c73e99824872114397e62faab414f252188df6" }
          , { mapKey = "MODULE.bazel.lock", mapValue = "d16b75d7ebf7e326b0055c52aff8f8abebad4637ae58f38ce07513e6b3d6bf0e" }
          , { mapKey = "cargo-bazel-lock.json", mapValue = "6c580f8738e8d1c99f48d1e0315bafaccb589798a54970e126f209289bea0282" }
          , { mapKey = "crates/swb-agentd/BUILD.bazel", mapValue = "4c53792c73662c8cf3737e36559acc042f53a28c4c5d638586e177df1f56adb7" }
          , { mapKey = "crates/swb-agentd/Cargo.toml", mapValue = "bd1e24061d70fa7e774eadcb87a83e81ff002e38c626603ba5d9e0462cfb0220" }
          , { mapKey = "crates/swb-agentd/src/lib.rs", mapValue = "03c8ff21db3b6556a4de5436ecf22589bf4ed43303cd6504b743394ad218bef7" }
          , { mapKey = "crates/swb-broker/BUILD.bazel", mapValue = "7bfc5fe7429424f4b9a9fd1c4a090854e78cdbf41a633d97bd61dddb3425accc" }
          , { mapKey = "crates/swb-broker/Cargo.toml", mapValue = "630fefce1b21b5b104f42c7a40bd36c6b965b8563be40ef37c75be69fca3531d" }
          , { mapKey = "crates/swb-broker/src/lib.rs", mapValue = "31f4f7ec3b0acb1bb3ab4c5282dd06a1129ed5d259d8f5ae224cc3c1c67cf549" }
          , { mapKey = "crates/swb-broker/tests/stamp.rs", mapValue = "f07fa0a5d40fbb81aa4062354d87f3baf216aa03f66b15520ce0bb6897b117f3" }
          , { mapKey = "crates/swb-proto/BUILD.bazel", mapValue = "f886fc6c14a076e6f5996fb862f857e487454afcfe6271037988e52e03b35f1c" }
          , { mapKey = "crates/swb-proto/Cargo.toml", mapValue = "af016b200f0a54f49e374ad058825af9b79490b001af20e6136ab27d4f3fd4c3" }
          , { mapKey = "crates/swb-proto/src/lib.rs", mapValue = "6965fd48241e8a14969114a7154fcc984f562540f0229a36693b01d51a1cd1ec" }
          , { mapKey = "crates/swb-store/BUILD.bazel", mapValue = "f2c380aae0ff2697183171ceac6f3de5ddef63e4702899779e7a3829a7efa39d" }
          , { mapKey = "crates/swb-store/Cargo.toml", mapValue = "41e95b19c399e874202c2725a57425883381d96ac6e98072307702841fc0f437" }
          , { mapKey = "crates/swb-store/src/lib.rs", mapValue = "7614f3c91aa4201e16850817bda03963cf86ea067cf0302cbe4d581e961679fc" }
          , { mapKey = "crates/swb/BUILD.bazel", mapValue = "6eba47ac963e621f40a16d1327996021574cda3bddb592d1f38150c6af408215" }
          , { mapKey = "crates/swb/Cargo.toml", mapValue = "601bbc80f9ec9f76ee0be84a6ea6e8b264a291a1563b37910cd0f7c5fdf46cb1" }
          , { mapKey = "crates/swb/src/channel.rs", mapValue = "311e3e792cea8a8e28074f3ca765355ea5c94311d140e5f17c69e9bbe02dad16" }
          , { mapKey = "crates/swb/src/main.rs", mapValue = "fa18eb5af85e7b472a2b7938a24588b662fc4926118b531585c6a6437f22f420" }
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
      , image = "ghcr.io/xoxd-ai/agent-switchboard@sha256:2d53153f4321fbfb06704b94f4820af0d6be28b473684de3cfcae152e7a1b453"
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
          [ { mapKey = "26", mapValue = "51e0346432dc9083ad675333ff58fa990680582f" }
          ]
      , publication_receipt =
          "PENDING: v0.2.1 release workflow registry readback; ratified R-C451 (TIN-5770 comment 0fc1217c-7f00-4dec-a276-d2c12164f94e) under R-C435 (TIN-5770 comment d3d90700)"
      , qualification = "published-candidate-only"
      , rulings = [ "SWB-R58", "R-C416", "R-C435", "R-C451", "R-N13" ]
      , schema = "swb.approved-release.v1"
      , source = "9bc96349129a7e595dd7d3795da0ce0480f44f22"
      , source_inputs =
          [ ".bazelversion", "BUILD.bazel", "MODULE.bazel", "MODULE.bazel.lock", "Cargo.toml", "Cargo.lock", "cargo-bazel-lock.json", "crates/", "deploy/", "platforms/", "tools/" ]
      , upstream_main = "363ebcde6777e5a6f67816a4e8b132d4f5c91d2b"
      }
    : ApprovedRelease
