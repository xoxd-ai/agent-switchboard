# TIN-4655: exact owned fixture source prepared for Sting

Authority: LAB-TAKEOVER-20260930, SWB-R49, R-N13. No heavy execution,
compiler launch, live harness, cluster operation or process signal occurred.
Root retains capacity admission and serial slot ownership.

Executable candidate is signed 5ce2ab45ffdabb088f48b6c7e96c802839f070a8.
It requires nine Rust cases across the existing swb, swb-store and swb-broker
test targets, including the independently reviewed actual TCP/production
server/SQLite reopen fixture. Five driver mock tests pass. Actual Rust
qualification remains pending; native registration and live rollout remain
separate gates.

The exact Git bundle is 245351 bytes, SHA256
65d58ba845b9c1c9c77ec796cee7d738d10f9bf0e8769cafb04274bd30986fcc.
Neo durable copy is
`~/.claude/agent-notes-rescue/2026-09-30/swb-e2e-5ce2ab4.bundle`.
Sting copy and clean detached candidate checkout are respectively:

- `/srv/fast-local/jess/tmp/swb-e2e-5ce2ab4.bundle`
- `/srv/fast-local/jess/tmp/swb-e2e-source-5ce2ab4-20260930`

The received archive digest, full HEAD/signature and tracked/untracked clean
status were checked before declaring preparation complete. Repository source
only was transported; no credentials or private host state were bundled.
This documentation followup does not alter that prepared executable candidate.

Source pins Bazel9.2.0, rules_rust0.73.0 and Rust1.97.1, with146 crates in
cargo-bazel-lock.json. Sting PATH rustc1.95.0 is not the Bazel toolchain.
Read-only existing cache metadata established the matching Bazel installbase
`3e6f3b7d6fdac67aed908160850e082b` and Rust toolchain BUILD.version1.97.1.
The resolved user-owned Rust repository input cache occupies838620KiB;
Bazelisk downloads251484KiB. An initial du on the external symlink reported
zero, which was not interpreted as emptiness: its resolved repository-content
path was measured instead. Existing disk action cache221104KiB is not
configured by this driver, so no compiled action-cache hit is promised.
A private outputbase will still require compilation. Several GiB of output
is an estimate, not an observed result or a proven peak; duration is unknown.

Read-only filesystem headroom showed283893092352 free bytes on fast-local
and86019051520 free bytes on the home/cache filesystem. CPU capacity was not
reprobed during preparation. The last readiness-driven Honey sample at
07:48:33Z was load66.50/59.35/52.61, busy92.4%, available105651016kB,
CPU PSI some58.34%/full0: no heavy launch followed that failed envelope.
Root's current serial preference is native11 fixtures, then SWB owned broker
proof, then final CLI/image work after fresh capacity and slot reservation.

The proposed execution keeps one inherited allowed CPU, per-process6GiB AS
(not aggregate), JVM512MiB, bounded loading/globbing/test/action concurrency,
fixed arena2 for JVM and compiler/test environments, private batch outputbase
and the shared host heavy lock. No deadline or signal controls a process;
owned socket deadlines are local I/O bounds. Root must supply the Sting lock
path and reserve the slot before execution; the prepared checkout alone is
not admission.

## Revised executable snapshot and exact-source review correction

Root subsequently authorized including existing //crates/swb:swb in the same
invocation to supply the actual managed-hook artifact without duplicate
compilation. Prepared executable snapshot is now signed
24c03ab90cfe6139531c58c93084192e33a7e228, with clean detached Sting checkout
`/srv/fast-local/jess/tmp/swb-e2e-source-24c03ab-20260930`.
The adjacent Git bundle and Neo durable rescue copy agree on SHA256
ec2a66f3e8cc98dbf0b94dcc50b4af262847a3d328ed5391cabd10e4e4f96c4d.
The older5ce2ab4 preparation above remains historical, not the queued snapshot.
Driver copies the successful compiled executable into external
`artifacts/swb`, verifies ELF/executable status, applies0500 permissions and
records exact artifact/source hashes. Permissions are not a sameUID security
barrier; the hash is the acceptance binding. No binary exists yet.

An inner-hook-only read led me to incorrectly report a missing invocation-wide
hook deadline. I promptly retracted that false positive after reading main
and the peer's exact-source check: main.rs256–266 already wraps the entire
hook in bounded(HOOK_LIMIT1800ms), covering stdin and both broker calls.
Individual calls use LIMIT1500ms. No Rust source was changed. The managed-hook
peer is adding delayed-register followed by stalled-inbox cases to verify
that existing outer boundary through actual rendered hooks; this is acceptance
coverage, not a duplicate product fix. Root received the correction before
any corresponding source or runtime action. Source/build remains inert under
the existing capacity hold. Authority LAB-TAKEOVER-20260930/SWB-R49/R-N13.


## Actual bounded execution — 08:26 UTC

Authority LAB-TAKEOVER-20260930/SWB-R49/R-N13. Root reserved the Sting slot
following native fixture natural exit. Fresh admission measured load5.30,
CPU busy12.0%, PSI some avg10=2.48 and MemAvailable26997476kB. Nonblocking
shared heavy lock `/srv/fast-local/jess/tmp/lab-takeover-sting-heavy.lock`
covered task40486; it naturally exited0 at08:26:11.244111Z and released the lock.
Exact clean source24c03ab90cfe6139531c58c93084192e33a7e228 passed all nine
required named Rust cases across all three targets. This includes genuine
production TCP/SQLite graceful service reopen and actual RMCP router coverage;
CLI registration fixture remains explicitly mocked. Source stayed unchanged
and clean. CPU affinity[0], per-process6GiB AS, JVM512MiB and arena2 remained.
No signals, shared Bazel server, live harness or live broker actions occurred.

Remote external receipt:
`/srv/fast-local/jess/tmp/swb-e2e-receipts-24c03ab-20260930/receipt.json`.
Neo durable copies of receipt and log:
`/Users/jess/.claude/agent-notes-rescue/2026-09-30/swb-e2e-24c03ab/`.
Log SHA256 eaba02569856a6a594d30b3530ad754025b04ce2311f14a9f37c322232c5665b.
Compiled CLI `artifacts/swb` SHA256
43428ec0299535e2deebfca9ed495baff9fa372b9fc6ad7044ddf5614edf6720.
ELF metadata requires GLIBC_2.39 and /lib64/ld-linux-x86-64.so.2;
NEEDED libc,libm,libgcc_s and loader; no RPATH. The OCI renderer owner received
this compatibility requirement before any renderer launch. Managed-hook18cases
remain unexecuted pending bounded local-store realization and provenance.
Custody, live native session identity, host registry convergence and operator
PVC restart acceptance remain separate unfulfilled gates. No live acceptance
is claimed by this owned-fixture receipt.


## Runtime input preparation for actual managed-hook cases

Authority LAB-TAKEOVER-20260930/SWB-R49/R-N13. At root request, copied
and dereferenced four ELF runtime libraries into task-owned
`/srv/fast-local/jess/tmp/swb-hook-runtime-24c03ab-20260930/lib64`, mode0444.
No host library paths changed. Exact hashes and readelf dependency closure are
in adjacent `runtime-receipt.json`, SHA256
722c9ee976c6686dcf1153833a699b2b41f707ef27cf14af1b89f3fae89ebbaa;
a Neo rescue copy accompanies the successful Rust receipt. ELF needs only
loader,libc,libm,libgcc_s, with maximum GLIBC_2.39. These task-owned copies
supply preparation evidence, not a sameUID security boundary or runtime proof.
Private OCI worker review and fresh capacity admission remain required before
realization/execution. Current197 actual runner already requires actual POST
requests on stalled cases and actual second-call GET inbox on delayed cases,
so suppressed loader errors cannot qualify those cases. Requested additive
sanitized observed-call metadata from the owning author for direct receipt
review; no request payload export and no live harness actions.


The independently cleared additive hook receipt source is now
1774c17b8e49e2a0af6c3cd967a03c40324a53f6 (signedG;20guardtests).
Staged four-file archive separately, preserving197:
`/srv/fast-local/jess/tmp/swb-hook-source-1774c17.tar`, SHA256
1acb2c4cd37248da86c7202253db7c3c62b5631af3f5484a68ed110019d36618.
Extracted tasksourceNAR, measured by read-only nix hash path:
sha256-IS52GMmkUtNK9l6Ml0v3gnpO8KpFs1Qyd/+qitmDtCs=.
RunnerSHA2564e0118d434f96fbbe535936c0abc539a637ec43880b15bdcd08abd4c99575532.
Receipt now retains request_count and method/path category only; raw request
payload and query values are discarded. Expected18cases include12observed
calls across positive stalled/delayed variants; this remains an expectation,
not runtime success. Factory/renderer/flake.lock unchanged. Authority remains
LAB-TAKEOVER-20260930/SWB-R49/R-N13; no worker launched during staging.


## Published-source comparison and frozen renderer review

Root-requested read-only comparison of published candidate
b5158729355e836a3a98ade90d7649690e7a6900 against tested24c shows no production
Rust logic, lockfile, module/toolchain or image input changes. The broker
lib only adds cfg(test) include of the owned service fixture; BUILDsrcs adds
that fragment. Remaining diffs are just/driver/tests/docs. Exact unchanged
main.rs blob2bacb49cd67d7f363a585409b3e0606ca12d3804,
Cargo.lockc85f7bd667c37b37ebfd01b6996c6c59157a01fd,
MODULE.bazel.lockf29665cc063544c93ca64df6b88d829e0ae2fb52.
Production source semantics therefore match, while artifact digest/image
publication and deployment equivalence are not established by this source
comparison. Exact24c compiled receipt must not be relabelled published-image
or live acceptance evidence.

Reviewed frozen worker script d0cbe0012c01e8482a02a567f9f9ba38235f5ffb78f196c52e0f430d9f4053ae
at /Users/jess/tmp/tin5148-qualified-f247ea3/sting-hooks-1774.REVIEW.sh.
Earlier version flag and missing runtime hash checks corrected: version
subcommand now matches exactsource; four runtimefiles hashchecked both before
launch and inside readonly mounts. Source/binary NAR sealing, pinnedNixpkgs,
explicitx86_64-linux, registered five factory attrs plus Python/source and
1774helper invocation match reviewed interfaces. Independent reviewer
confirmed source mechanics; root fresh capacity/slot admission still required.
No worker has launched at this note. Authority LAB-TAKEOVER-20260930/SWB-R49/R-N13.


## Actual managed-hook worker attempt — startup failure

Authority LAB-TAKEOVER-20260930/SWB-R49/R-N13. Owner launched exact frozen
worker d0cb once, handle51591, after immediate09:36:52 capacity admission:
15769016KiB available, worker-parent PSI0.00avg10/OOM0 and nonblocking lock.
It naturally exited1 before Bash/Nix/renderer/hooks. OCI stderr verbatim:
`exec container process `/bin/bash`: Permission denied`.
Collector confirms exited/runningfalse/exit1/OOMfalse. hook-cases.json absent.
No18cases, registered renderer or managed-hook result exists. No next image
phase admission, retry, cleanup, security weakening or relabel occurred.
Raw external output:
`/srv/fast-local/jess/tin5148-fallback-20260930/swb-hooks-1774c17-output`.
Neo rescue copies:
`/Users/jess/.claude/agent-notes-rescue/2026-09-30/swb-hooks-1774c17-failed51591/`.

My runtime preparation made all four copied files0444, including the dynamic
loader. Missing executable loader permission is a concrete preparation defect
requiring read-only diagnosis of actual Bash interpreter and file mode before
attributing SELinux. Root assigned that diagnosis to the worker owner. This
failure belongs to the runtime worker startup boundary, not hook product tests
or admission-pressure refusal. Existing successful Rust9case evidence stands.


## Second managed-hook worker attempt — unlocalized early failure

Authority LAB-TAKEOVER-20260930/SWB-R49/R-N13. Root-authorized task-only
loader-mode repair completed0555 with originalSHA unchanged; remaininglibs
0444. Runtime-receipt-v2 SHA256
74fbcdd58b4ca843df04291e7145472faf2ebd8e87b0cece7943ab6a455b8219
preserves the original receipt. Independently reviewed r2 worker recipe
bbcd496d630a45548de6ad7b348d21259981520ef531987452af1773d6017f07
checks modes/hashes before and inside the worker. Owner reports one admitted
launch09:44:54:16568760KiB available,parentPSI0.02/OOM0,nonblockinglock.

Actual terminal collector independently read:
container a8c0e702d9cda42ee8442389f60ef7b1785b08198cdce77d1ee70f508cc6efec
exited/runningfalse/exit1/OOMfalse. stdout and stderr are both zero bytes.
worker.limits and hook-cases.json are absent. Host runtime mode/hash receipt
exists and matches the repaired inputs. The early failure is not localized
by available receipts; no Bash/Nix/renderer/hook execution qualification is
claimed. Observability-only r3 source is being authored and reviewed by other
owners; this ledger action authorizes no worker launch, retry or security
change. No image phase, rollout, custody or registry acceptance changed.

Remote evidence:
`/srv/fast-local/jess/tin5148-fallback-20260930/swb-hooks-1774c17-r2-output`.
Neo rescue:
`/Users/jess/.claude/agent-notes-rescue/2026-09-30/swb-hooks-1774c17-r2-failed/`.
Acceptance ledger still has actual24c Rust9cases/3targets/compiledCLI passed;
actual18managed-hookcases and expected12observedcalls remain pending. Neither
startup failure falsifies the product-hook tests, since they did not run.
