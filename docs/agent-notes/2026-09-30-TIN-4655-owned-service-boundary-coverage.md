---
title: "Owned switchboard service boundary and functional coverage gaps"
date: 2026-09-30
status: active
summary: >-
  Distinguish CLI transport mocks and Store-only persistence from actual broker
  HTTP functionality; add a separate owned service lifecycle fixture.
refs:
  - TIN-4655
---

Authority: LAB-TAKEOVER-20260930, TIN-4655, SWB-R02, SWB-R49 and R-N13.
Owned tree agent-switchboard-service-roundtrip-takeover-20260930 starts at
9d386627c16a2b7eede2d40a1e24909a5dba0e98. The separate fixture-driver owner
approved ownership of the new test file and minimal cfg(test) include/source
registration; their tree and driver remain untouched by this lane.

## Coverage audit

The original six named cases in scripts/e2e-fixtures.py targeted only the CLI
and Store crates. CLI explicit_broker_round_trip_and_session_identity
(crates/swb/src/main.rs:335) uses an owned TCP listener with a canned response
to one register request. It proves CLI HTTP request identity and response
parsing, not broker behavior or a send/inbox/ack roundtrip.

Store round_trip_and_opaque_conflict (:775) proves real in-memory Store logic;
wal_survives_reopen_and_sequences_thread (:807) proves actual SQLite WAL
reopen and sequence behavior directly through Store. Neither is an HTTP
broker lifecycle or deployed restart acceptance.

CLI stalled_resolution_worker_cannot_hold_caller (:455) and
stalled_http_read_cannot_hold_caller (:469) exercise generic bounded workers
with 75 ms fixtures. They do not call the actual hook entrypoint or prove its
exit-zero behavior within the ruled two-second harness limit. The production
hook path uses HOOK_LIMIT=1800 ms and returns success after bounded work,
but source inspection alone is not its functional acceptance receipt.

Actual broker service functionality already has a meaningful test:
crates/swb-broker/src/lib.rs rmcp_streamable_http_initialize_and_tools (:693)
uses the genuine Axum/RMCP router and Store, initializes MCP, and exercises
register/send/inbox/ack. This is actual service-handler HTTP behavior with
synthetic identities, rather than mocked broker JSON. It has no socket or
persistent database and was omitted from the original driver. The driver
owner is adding this broker target and its REST conflict test now.
Existing serve_bound tests (:405/:483/:560) exercise real owned listeners and
shutdown, active inbox draining and SSE, rather than a full durable roundtrip.

Pi combined adapter unit tests substitute global fetch responses. Its JSON
loader integration launches an owned Pi fixture against a canned Node HTTP
server, proving loader/tool/native identity wiring but not the real broker.
Junie agents-profile assertions in lab nix/tests/mcp-offload.nix:950 evaluate
configuration/tool allowlists and explicit opt-in gating; they do not execute
Junie or the issued mux/broker group. The lab Codex adapter fixtures similarly
prove launch/thread environment contracts, not real Codex or broker operation.
These results must not be called a cross-harness working MVP.

## New actionable service fixture

The cfg(test) module includes crates/swb-broker/src/service_roundtrip_tests.rs.
Case tests::owned_http_round_trip_survives_graceful_service_reopen runs the
production serve_bound on owned loopback listeners with an owned temporary
SQLite file. No public production API changes. It sends real HTTP register,
send, inbox and ack requests. Three naturally graceful service lifecycles use
oneshot futures, no OS processes or signals. Assertions cover peer authority,
idempotent retry, persistent registered sessions, unacked redelivery counts,
ack durability after reopen, and monotonic thread sequence after reopen.
Claude, Pi and Codex identities are explicitly synthetic owned fixture records;
the first is synthetic Neo Claude to Sting Codex; its linked reply is
synthetic Sting Codex to Sting Pi, never a live harness.
The existing //crates/swb-broker:swb_broker_test target declares the included
source; the driver owner will require the exact case result.

Rustfmt parsed/formatted the new source. This lane did not compile or run it;
remote bounded Bazel execution belongs to the existing fixture-driver owner.
Any compile/runtime failure must be recorded and fixed before claiming proof.

## Remaining functional proof

Add an actual CLI hook acceptance test using the compiled owned swb binary,
synthetic stdin identity and an owned endpoint that refuses or stalls. Assert
exit zero and the real two-second envelope for SessionStart/UserPromptSubmit/
SessionEnd; finite fixture servers finish naturally, never OS signals or
subprocess timeout cleanup. Keep generic timeout tests as separate coverage.

For Pi, route the existing deterministic provider/JSON-loader fixture through
the actual owned broker and assert a stored send/inbox/ack exchange; a Grafana
fixture may remain separately labelled. For Junie, test the rendered combined
mux group against the actual owned broker protocol, then conduct an explicitly
attended Junie acceptance separately. The fixture source gaps are actionable
without waiting for GF or production custody.

Owned HTTP/SQLite service reopen proof does not establish a deployed pod/PVC
restart, host delivery, live Pi/Junie cross-host acceptance, real credential
custody, or production transport. Those remain distinct operational exits.

Independent review corrections (LAB-TAKEOVER-20260930, R-N13): the initial
include fragment lived under tests/, which Cargo auto-discovers as an
integration crate without the private unit-module context. It is now under
src/ with the include and Bazel source registration corrected; stamp.rs remains
Cargo's existing integration test. The HTTP reader now shares a true absolute
two-second deadline across connection, each write and each read, and enforces
a 64 KiB response cap; continuously arriving data cannot extend the deadline.
No subprocess timeout cleanup or OS process signals are used.

The flow now includes explicit synthetic Neo Claude to Sting Codex delivery
and acknowledgement, then a Sting Codex to Sting Pi reply with in_reply_to
set to the first message. A third graceful reopen proves the acknowledged
Codex inbox remains empty and the Pi reply linkage and sequence persist.
These refinements are source-only pending the owner's remote fixture run.
