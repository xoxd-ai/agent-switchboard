---
title: "Switchboard owned fixture acceptance entrypoint"
date: 2026-09-30
status: active
summary: >-
  Existing store and CLI fixtures now have a bounded-job receipt driver;
  native harness dialog, pod restart and broker-down fallback remain live gates.
refs:
  - TIN-4655
---

Receipt: LAB-TAKEOVER-20260930, SWB-R49, R-N13. Separate source tree starts
at signed integration b5158729355e836a3a98ade90d7649690e7a6900. No design,
registry, service, deployment or process-control behavior is changed.

`just e2e-fixtures-plan` prints exact source/targets/cases without a build.
On Honey or Sting, `just e2e-fixtures /absolute/new/receipt-directory` runs
existing CLI/store tests with one job, one CPU and serial Rust test threads,
ambient rc files disabled and SWB environment removed. Receipt verifies the
six specific Rust test success lines, source HEAD and log digest. No live
broker address is accepted. Fixture temporary SQLite/listeners are already
owned by their existing tests and end normally. The driver adds no lifecycle
control, timeout supervisor, harness startup or telemetry requirement.

This does not replace the live `e2e` gate. Store reopen is not pod-restart
acceptance; synthetic fixture session IDs are not native Claude/Codex/Pi
self-registration. ADR-0001 Identity explicitly keeps Codex notify runtime-owned and routes
registration through swb whoami. The native calling-session binding is a
separate P1b sub-exit, never inferred from process ancestry or fixture IDs.
The registry is intentionally disabled/url null. A separate lab adapter lane
owns opt-in native binding; this fixture changes no Codex surface.

Live v1 still requires custody/backend proof; exact image/source and issued
broker URL; native Neo Claude and Sting Codex/Pi identities and real mailbox
reply/ack; operator-owned named pod-restart gate preserving message/thread
sequence and delivery state; broker-down nonblocking hook and ordinary host
workflow continuation with an explicit degraded receipt; declared registry
endpoint matching the delivered host and active harness profile. Telemetry
qualification follows functional v1 per LAB-TAKEOVER-20260930 and supplies no
substitute for these gates. An operator word is required for each live
restart and harness-start ceremony; agents do not infer it from this fixture.

Validation: plan command and just recipe parsing succeeded; git diff check
and commit-time process-safety audit passed. No Bazel compilation ran on Neo.
Honey heavy admission was held by root at 06:46Z (load31/32cores); Sting's
heavy slot is occupied. Executed Rust-fixture receipt remains pending a
separately approved slot. The fixture driver is ready to run on exact signed
source, not reported as a fixture pass.

Root review follow-up (LAB-TAKEOVER-20260930/R-N13): post-run tracked and
untracked source cleanliness is required even when HEAD remains identical.
Bazel testlog-discovery errors now retain a failed receipt. Three mock cases
cover clean completion, same-HEAD source dirt and testlog-query failure; all
pass without builds. Every execution lowers its inherited per-process address
space limit to at most 6 GiB; the unique run output-base uses batch mode and a
512 MiB JVM heap, avoiding any shared server. These are per-process bounds,
not an aggregate cgroup limit. One-job/one-CPU execution remains owner-admitted.

The first mock run used Linux's numeric RLIM_INFINITY on Darwin, causing one
assertion failure; the mock now uses the executing platform's resource
constant. Corrected three-case run passed. This was test fixture portability,
not a runtime-limit change; the initial commit preserved the failed attempt.

First actual attempt at exact signed33bc253 on Honey ended naturally with a
failed receipt at07:02:25Z: test exit2 rejected old --local_cpu_resources;
following info exit134 reported native std::bad_alloc. No fixture ran and
no root cause beyond that native allocation failure was established. The
Bazel9.2 ExecutionOptions source declares --local_resources=cpu=1; the driver
now uses it, caps JVM visible processors1 and thread stacks512KiB in addition
to the existing512MiB heap and6GiB address-space bound. A failed test command
no longer starts a second JVM for info. Root authorized this source-supported
correction and a new bounded attempt; no command spelling evades a guard.

Source references: https://raw.githubusercontent.com/bazelbuild/bazel/9.2.0/src/main/java/com/google/devtools/build/lib/exec/ExecutionOptions.java
(Bazel local_resources option, lines308–347). Java ActiveProcessorCount and
-Xss are standard supported JVM resource options; allocation cause remains
unproved and these settings do not raise the memory ceiling.

Second actual attempt a968969 ended07:05:58Z exit33 during package loading:
globbing-pool pthread_create returned EAGAIN. No Rust fixture ran. Bazel9.2
PackageOptions names legacy_globbing_threads with default100, independent of
JVM visible processors. Source correction now sets legacy_globbing_threads1
and loading_phase_threads1 as well as action/test limits. Memory ceiling
is unchanged. Raw receipt and bazel log copies from both attempts were
preserved under ~/.claude/agent-notes-rescue/2026-09-30/swb-e2e-<sha>/ on Neo.
The second clone's fresh post-attempt HEAD remained a968969 and tracked/
untracked status was clean; failure receipt false source fields mean the
postcheck was not reached, not established source mutation. Heavy execution
remains held pending root's fresh release; no new attempt followed this edit.

Globbing flag reference: https://raw.githubusercontent.com/bazelbuild/bazel/9.2.0/src/main/java/com/google/devtools/build/lib/pkgcache/PackageOptions.java
(lines132–143). Supported JVM resource options: https://docs.oracle.com/en/java/javase/21/docs/specs/man/java.html

Third actual attempt exact51ba55875db77afc4d64d33a2910177d0c263681
started07:12:55Z after root health clearance and ended naturally07:13:25Z
exit33. Analysis reached322packages/10061targets, then native thread creation
failed at Non-eager Symlink planter50. No Rust fixture executed. Official
Bazel9.2 IncrementalPackageRoots has fixed symlink parallelism200, so the
globbing/loading limits did not bound this separate pool. No further heavy
execution is authorized pending root capacity decision.

Admission lapse: the fresh probe immediately before this third launch showed
load52.67/38.68/32.24, CPU PSI some40.87%, full0%, available memory101872184kB,
materially changed from root's earlier healthy sample. I launched instead of
pausing for the changed pressure. Root was notified promptly. Future launches
must evaluate the fresh probe as a separate conditional decision before
spawning, and pause when it exceeds the admitted envelope. No signals,
deadlines, shared-server intervention, or live harness action occurred.

Third raw receipt/log preserved in the same durable rescue tree under
swb-e2e-51ba558; logSHA256
528f9f00aeeae57dbf81942e7184ec355241352065f3d953b6c3e65dcb157260.
Fresh post-run tracked/untracked status was clean. Tool handle12594 ended
naturally and the shared heavy lock was released; the queued devcontainer
peer was informed. Authority LAB-TAKEOVER-20260930/SWB-R49/R-N13.

Pool source: https://raw.githubusercontent.com/bazelbuild/bazel/9.2.0/src/main/java/com/google/devtools/build/lib/skyframe/IncrementalPackageRoots.java
(lines60–108).

Allocator diagnosis authorized separately, not a Bazel retry. Honey glibc2.39;
RLIMIT_NPROC898890; user cgroup pids.max593267/current558/events.max0;
SSH scope PID maximum unlimited/current9. Observed user130processes/1072threads
and kernel thread maximum1797781 do not show PID-limit pressure at sampling.
These are later metadata, not historical proof of the failed JVM's limits.
No hs_err numerical resource receipt was found. No raw environment exported.

Prepared Linux-only small diagnostic compares two sequential processes:
default glibc arena selection versus process-start MALLOC_ARENA_MAX2, oneCPU
affinity,6GiB AS,128threads maximum,512KiB stacks, one4KiB libc malloc/free
per thread. Owned Event release and join in finally; no deadlines or signals.
Sequential startup/ready wait ensures each allocation completed before the
numeric snapshot; it does not simulate JVM allocation patterns or malloc
contention, so a difference supports mechanism plausibility rather than
proving the earlier Bazel failure. No compilation or broker action.
Authority LAB-TAKEOVER-20260930/SWB-R49/R-N13; root explicitly authorized
this tiny diagnostic only, with Bazel retry still held. Linux gate refused
on Neo before output creation; Python syntax check passed.
Official contract: https://sourceware.org/glibc/manual/latest/html_node/Malloc-Tunable-Parameters.html
(M_ARENA_MAX/startup environment parameter);
https://sourceware.org/glibc/manual/latest/html_node/Memory-Allocation-Tunables.html
(glibc.malloc.arena_max supersedes MALLOC_ARENA_MAX).

Owned allocator comparison exact5739ef1885c5454d11b5cf7530bd38dd3d9daed5
completed naturally07:25:27Z, parent and both children exit0,128requested and
128created threads each,129total live threads, zero allocation failures.
Default VmSize/VmPeak6276948kB and RSS19612kB; arena2 VmSize/VmPeak378228kB
and RSS18920kB. This confirms a large virtual reservation difference in
this fixture. It neither proves historical JVM causality nor rules out
transient historical PID or other pressure. Receipt preserved under
~/.claude/agent-notes-rescue/2026-09-30/swb-allocator-5739ef1-20260930.json.

Meaningful runner resource correction now sets task-child MALLOC_ARENA_MAX2
and removes ambient GLIBC_TUNABLES so an inherited arena override cannot
weaken it. No host-wide environment change; AS6GiB/JVM512MiB/oneCPU limits
remain unchanged. Mock acceptance includes inherited256arena/tunable override
and confirms fixed2 bound; the three driver regression cases pass. No Bazel
retry is part of this mutation or diagnostic; fresh root capacity admission
is required before another heavy launch. Authority LAB-TAKEOVER-20260930/
SWB-R49/R-N13.

Version precision correction: getconf GNU_LIBC_VERSION reported2.39 for
that host utility, while the diagnostic Python's direct libc
gnu_get_libc_version reports2.42. The observed allocator comparison
therefore belongs to Python-linked glibc2.42. Diagnostic source now records
that direct library version in future observations. The historical JVM's
linked library version was not established. The official current glibc
contract applies to this diagnostic; do not label it a2.39 JVM reproduction.
Fresh post-correction capacity sample load31.19/42.59/42.04, busy53.1%,
available97259452kB, CPU PSI some23.64%/full0. No heavy launch followed:
pressure exceeds root's prior admission envelope, and fresh admission
is pending. Authority LAB-TAKEOVER-20260930/SWB-R49/R-N13.

Functional audit correction: CLI explicit_broker_round_trip_and_session_identity
uses a canned owned TcpListener response, not a real broker. Existing store
fixtures prove SQLite semantics, not broker lifecycle. Driver now includes
//crates/swb-broker:swb_broker_test and requires existing
rmcp_streamable_http_initialize_and_tools and conflict_is_opaque_over_rest
cases. The former executes real Axum/RMCP initialization, register, peers,
send, inbox and ack against Store::memory with synthetic identities. It
proves in-process service routing, not native harness registration or wire
transport. A peer is preparing an owned production serve_bound+SQLite reopen
fixture separately; no duplicate fixture or heavy run is being launched here.

Selected failed JVM metadata: outputbase/install points to installbase
3e6f3b7d6fdac67aed908160850e082b; embedded_tools/jdk/bin/java requests
/lib64/ld-linux-x86-64.so.2 and libc.so.6, with RPATH only local bin/lib.
Neither RPATH directory contains libc.so.6. /usr/lib64/libc.so.6 embedded
version metadata says stable2.39; current LD_PRELOAD/LD_LIBRARY_PATH are absent,
and no corresponding prior client_env keys appeared in the allowlisted
failed Java log inspection. This supports host2.39 linkage without launching
Java, but is metadata inference rather than a captured JVM process map.

Bazelisk1.28.1 is an ELF executable at the selected Nix profile path. Its
versioned primary source makeBazelCmd preserves os.Environ when launching
Bazel. Driver supplies arena2 to both task test and batch info invocations.
Bazel action/test environments are separately declared: fixed
--action_env=MALLOC_ARENA_MAX=2 and --test_env=MALLOC_ARENA_MAX=2 plus empty
GLIBC_TUNABLES for actions/tests ensure hermetic compiler/test children receive
the same bound. This touches only owned subprocesses, not host configuration.
Three meaningful driver mocks pass with the expanded real broker requirement
and arena override checks. Heavy retry remains held.
Authority LAB-TAKEOVER-20260930/SWB-R49/R-N13.
Source: https://raw.githubusercontent.com/bazelbuild/bazelisk/v1.28.1/core/core.go
(makeBazelCmd, environment propagation).

Execution-configuration actions receive matching fixed host_action_env
arena2/emptytunables settings as well as target action/test environments.
The real broker target requirement has a negative driver fixture: existing
CLI/store successes with missing real broker cases cannot qualify. Four
mock receipt tests pass; no compilation. Authority LAB-TAKEOVER-20260930/
SWB-R49/R-N13. Option contracts: https://raw.githubusercontent.com/bazelbuild/bazel/9.2.0/src/main/java/com/google/devtools/build/lib/analysis/config/CoreOptions.java
(action_env and host_action_env).

Final metadata analysis and revised retry proposal (execution held): Bazel9.2
POSIX ExecuteServerJvm calls execv, preserving the task-local environment;
versioned Bazelisk preserves environment too. Version-specific glibc2.39
contract confirms MALLOC_ARENA_MAX and default64-bit limit8xonlinecores,
which may differ from JVM visibleCPU count. Scope inference remains distinct
from observed Python2.42 diagnostic. No Java executable was launched.
Sources: https://raw.githubusercontent.com/bazelbuild/bazel/9.2.0/src/main/cpp/blaze_util_posix.cc
(ExecuteServerJvm/ExecuteProgram);
https://sourceware.org/glibc/manual/2.39/html_node/Malloc-Tunable-Parameters.html
(M_ARENA_MAX).

One fresh capacity sample after this useful analysis showed Honey
load43.66/44.81/44.36, busy87.8%, available106376440kB, CPU PSI
some41.34%/full0; no heavy-lock holder observed. This exceeds the earlier
admitted pressure envelope. No command was spawned for heavy execution;
root coordinates an owner window, and no pressure polling loop follows.
The proposed later retry retains per-process6GiBAS (not an aggregate
memory bound), JVM512MiB, jobs/localCPU/loading/globbing/tests1, fixed
arena2 JVM/action/host-action/test environments, clean signed exact source,
private batch outputbase and shared heavy lock. It will include the real
broker tests plus the peer's owned wire/SQLite lifecycle fixture after
independent source review. Authority LAB-TAKEOVER-20260930/SWB-R49/R-N13.

CPU bound precision correction: Bazel local_resources/jobs and JVM visibleCPU
flags are scheduler inputs, not a strict compiler affinity limit. Root
authorized task-local inherited singleCPU affinity to make the proposed
<=oneCPU bound concrete. Driver selects one CPU from its existing nonempty
allowed set, sets only its own affinity, then verifies exact singleCPU
readback is a subset of original permissions before output creation or
child launch. Failure produces a clear admission error and no Bazel/output.
No host-wide change or signal. Earlier attempts used scheduler inputs only;
this correction does not relabel their historical bounds as strict affinity.
Five mock receipt tests pass, including affinity-denied/no child/no output,
missing real broker proof, dirty source and failed log discovery. Memory
remains6GiB per process, not aggregate. Heavy hold unchanged.
Authority LAB-TAKEOVER-20260930/SWB-R49/R-N13.

Integrated peer signed fixture chain ae9492a9 then1d33d298 after independent
portable_image_review cleared all remainingP0–P2 and this lane read actual
HTTP/server/SQLite code. cfg(test) fragment lives under src/ to avoid Cargo
autodiscovery; production API stays private. Connect/write/read share finite
absolute2s deadline and64KiB response cap. Driver requires
tests::owned_http_round_trip_survives_graceful_service_reopen in existing
swb_broker_test. This executes production serve_bound with owned loopback
listeners across3graceful oneshot lifecycles and sameSQLite, synthetic
NeoClaude→StingCodex thenCodex→StingPi reply linkage, persisted sessions,
unacked redelivery, idempotent sequence, ack durability and next sequence.
No live harness/native identity/PVC/pod restart or telemetry proof follows
from this fixture. Five driver mocks pass; Rust compilation remains queued
under root's fresh resource admission. No heavy command started.
Authority LAB-TAKEOVER-20260930/SWB-R49/R-N13.

Root authorized adding existing //crates/swb:swb executable to the same
Bazel invocation so actual managed-hook acceptance can reuse the compiled
source without a duplicate build. Driver verifies regular executable ELF,
copies it into owned external receipt artifacts/swb, freezes artifact/file
permissions0500, and records sourceSHA,target,originalBazelpath,copySHA256.
This is hash-bound owner-readable/executable output, not a sameUID security
barrier or a Nix closure/signature admission claim. Nine required Rustcases
and source cleanliness still govern qualification. Source changes only; no
compiler launch. Hook peer receives the copied artifact path and hash only
after actual owned execution passes. Authority LAB-TAKEOVER-20260930/
SWB-R49/R-N13.
