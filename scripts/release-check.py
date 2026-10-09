#!/usr/bin/env python3
"""Read-only source/OCI evidence check for the approved broker (SWB-R58).

No network, build, publication, activation or process-control action is taken.
Supplied registry bytes are evidence from the caller, not a fresh pull proof.
With --tag the check also binds a signed annotated release tag to HEAD (and,
with --main-ref, to the protected main history); with --built-digest it refuses
any locally built image whose digest is not the approved immutable digest, so
the release workflow can never publish an image the approved record does not name.
With --tag-signer the tag must also be signed by that primary key: the release
key ring carries GitHub's merge key for the source commit, never for tags.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys

SOURCE_INPUTS_V1 = [
    ".bazelversion", "BUILD.bazel", "MODULE.bazel", "MODULE.bazel.lock",
    "Cargo.toml", "Cargo.lock", "cargo-bazel-lock.json", "crates/",
    "deploy/", "platforms/", "tools/",
]


def digest(data):
    return "sha256:" + hashlib.sha256(data).hexdigest()


def read_blob(layout, descriptor):
    value = descriptor["digest"]
    if not re.fullmatch(r"sha256:[0-9a-f]{64}", value):
        raise ValueError("invalid OCI blob digest")
    raw = (layout / "blobs" / "sha256" / value[7:]).read_bytes()
    if digest(raw) != value or len(raw) != descriptor["size"]:
        raise ValueError("OCI blob hash or size mismatch")
    return raw


def check_manifest(raw, release):
    if digest(raw) != release["image"].rsplit("@", 1)[1]:
        raise ValueError("manifest does not match approved immutable digest")
    if len(raw) != release["manifest_size"]:
        raise ValueError("manifest size differs from publication receipt")
    manifest = json.loads(raw)
    if manifest.get("schemaVersion") != 2:
        raise ValueError("unsupported OCI manifest schema")
    return manifest


def check_layout(layout, release):
    if json.loads((layout / "oci-layout").read_text()) != {"imageLayoutVersion": "1.0.0"}:
        raise ValueError("unsupported OCI layout")
    index = json.loads((layout / "index.json").read_text())
    wanted = release["image"].rsplit("@", 1)[1]
    descriptors = [entry for entry in index["manifests"] if entry["digest"] == wanted]
    if len(descriptors) != 1:
        raise ValueError("layout must contain exactly one approved child manifest")
    raw = read_blob(layout, descriptors[0])
    manifest = check_manifest(raw, release)
    config = json.loads(read_blob(layout, manifest["config"]))
    platform = release["platform"]
    if (config.get("os"), config.get("architecture")) != (platform["os"], platform["architecture"]):
        raise ValueError("image platform differs from approved runtime")
    runtime = config["config"]
    if runtime.get("User") != platform["user"] or runtime.get("Entrypoint") != platform["entrypoint"] or runtime.get("Cmd") != platform["cmd"]:
        raise ValueError("image user, entrypoint or command differs from approved runtime")
    for layer in manifest["layers"]:
        read_blob(layout, layer)
    return {"verified_blob_count": len(manifest["layers"]) + 2, "platform": platform}


TAG_NAME = re.compile(r"v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z][0-9A-Za-z.-]*)?")


def tag_signer(repo, ref):
    """Primary-key fingerprint of a verified tag signature (VALIDSIG field 10)."""
    status = subprocess.run(["git", "-C", str(repo), "verify-tag", "--raw", ref],
                            capture_output=True, text=True, check=True).stderr
    for line in status.splitlines():
        fields = line.split()
        if fields[:2] == ["[GNUPG:]", "VALIDSIG"] and len(fields) >= 12:
            return fields[11].upper()
    raise ValueError("release tag signature has no VALIDSIG status")


def check_tag(repo, tag, main_ref=None, signer=None):
    if not TAG_NAME.fullmatch(tag):
        raise ValueError("release tag name must be vMAJOR.MINOR.PATCH[-pre]")
    ref = "refs/tags/" + tag
    if git(repo, "cat-file", "-t", ref) != "tag":
        raise ValueError("release tag must be an annotated tag object")
    # Verification has no timeout handler: it never signals a subprocess.
    try:
        git(repo, "verify-tag", ref)
    except subprocess.CalledProcessError:
        raise ValueError("release tag signature does not verify against the trusted key ring") from None
    if signer:
        try:
            primary = tag_signer(repo, ref)
        except subprocess.CalledProcessError:
            raise ValueError("release tag signature does not verify against the trusted key ring") from None
        if primary != signer.upper():
            raise ValueError("release tag is not signed by the pinned release key")
    commit = git(repo, "rev-parse", ref + "^{commit}")
    if commit != git(repo, "rev-parse", "HEAD"):
        raise ValueError("release tag does not point at the checked-out HEAD")
    report = {"release_tag": tag, "release_tag_commit": commit}
    if main_ref:
        try:
            git(repo, "merge-base", "--is-ancestor", commit, main_ref)
        except subprocess.CalledProcessError:
            raise ValueError("release tag commit is not on the protected main history") from None
        report["release_tag_on_main"] = main_ref
    return report


def check_built_digest(path, release):
    built = path.read_text().strip()
    if not re.fullmatch(r"sha256:[0-9a-f]{64}", built):
        raise ValueError("built image digest file is malformed")
    if built != release["image"].rsplit("@", 1)[1]:
        raise ValueError("built image differs from the approved immutable digest; a new image needs its own ruling")
    return built


def git(repo, *args):
    return subprocess.check_output(["git", "-C", str(repo), *args], text=True).strip()


def check_source(repo, release):
    if release.get("schema") != "swb.approved-release.v1" or release.get("source_inputs") != SOURCE_INPUTS_V1:
        raise ValueError("release schema or required protected input roots differ")
    if not release.get("file_sha256"):
        raise ValueError("release source file map is empty")
    source = release["source"]
    # Verification has no timeout handler: it never signals a subprocess.
    git(repo, "verify-commit", source)
    for ancestor in [release["upstream_main"], *release["pr_heads"].values()]:
        git(repo, "merge-base", "--is-ancestor", ancestor, source)
    git(repo, "merge-base", "--is-ancestor", source, "HEAD")
    roots = release["source_inputs"]
    selected = lambda path: any(path.startswith(root) if root.endswith("/") else path == root for root in roots)
    tracked = {p for p in git(repo, "ls-files").splitlines() if selected(p)}
    baseline = {p for p in git(repo, "ls-tree", "-r", "--name-only", source).splitlines() if selected(p)}
    expected = release["file_sha256"]
    if tracked != baseline or baseline != set(expected):
        raise ValueError("build/runtime file set differs from approved source")
    # Also reject added untracked inputs beneath protected build directories.
    # Include ignored files: .gitignore/global exclusions cannot hide an input.
    extras = git(repo, "ls-files", "--others").splitlines()
    if any(selected(p) for p in extras):
        raise ValueError("untracked build/runtime input")
    for path, wanted in expected.items():
        raw = subprocess.check_output(["git", "-C", str(repo), "show", f"{source}:{path}"])
        if hashlib.sha256(raw).hexdigest() != wanted:
            raise ValueError("release file map differs from signed source: " + path)
        if hashlib.sha256((repo / path).read_bytes()).hexdigest() != wanted:
            raise ValueError("current build/runtime input differs: " + path)
    return {"signed_release_source": source, "checked_input_count": len(expected), "current_head": git(repo, "rev-parse", "HEAD")}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--oci-layout", type=Path)
    parser.add_argument("--registry-manifest", type=Path)
    parser.add_argument("--tag", help="signed annotated release tag that must point at HEAD")
    parser.add_argument("--main-ref", help="ref whose history must contain the tag commit (requires --tag)")
    parser.add_argument("--tag-signer", help="primary key fingerprint that must have signed --tag")
    parser.add_argument("--built-digest", type=Path, help="Bazel //deploy:image.digest output to compare")
    args = parser.parse_args()
    if (args.main_ref or args.tag_signer) and not args.tag:
        parser.error("--main-ref and --tag-signer require --tag")
    repo = Path(__file__).resolve().parents[1]
    report = {"rulings": ["SWB-R58", "SWB-R49", "R-N13"], "live_acceptance": False, "fresh_registry_pull": False, "publication_authorized": False}
    try:
        release = json.loads((repo / "docs/releases/approved-broker.json").read_text())
        report.update(check_source(repo, release), image=release["image"])
        if args.tag:
            report.update(check_tag(repo, args.tag, args.main_ref, args.tag_signer))
        if args.built_digest:
            report["built_digest_matches"] = check_built_digest(args.built_digest, release)
        if args.oci_layout:
            report["oci_evidence"] = check_layout(args.oci_layout, release)
        if args.registry_manifest:
            check_manifest(args.registry_manifest.read_bytes(), release)
            report["supplied_registry_manifest_matches"] = True
        report["verdict"] = "passed"
    except (OSError, ValueError, KeyError, TypeError, subprocess.CalledProcessError) as error:
        report.update(verdict="failed", error=str(error))
    print(json.dumps(report, indent=2, sort_keys=True))
    return 0 if report["verdict"] == "passed" else 1


if __name__ == "__main__":
    sys.exit(main())
