# ADR-0004: stable service names

- **Status:** Proposed. It is not normative. Code needs a TIN-4655 ruling
  first, and the ruling decides the questions under Open rulings.
- **Date:** 2026-10-30, drafted against `main` at `eada83a`.
- **Linear:** TIN-7788, under TIN-4655.
- **Depends on:** opt-in sender binding ([ADR-0003](0003-sender-binding.md),
  SWB-R66 to SWB-R68), the binding lapse rules of SWB-R69 and the claim
  rules SWB-R16, SWB-R60 and SWB-R61 in
  [ADR-0001](0001-agent-switchboard.md). SWB-R69 sets the 6-hour idle
  lapse and the permanent lapse, and it requires the current token to
  re-register a live bound id.

Line references below are to `eada83a`.

## Context

A producer process runs on several hosts. Each copy must reach whichever
agent session currently serves a named role, such as a reviewer or a
renderer. The serving session changes when it restarts, moves to another
host or hands the role to another session. The producer has no stable
address to send to.

What the broker offers today:

- **Send takes only agent ids.** The store refuses any `from` or `to` that
  is not `harness:host:pid:session_id`
  (`crates/swb-store/src/lib.rs:260-270`, checked in `send_as` at
  `crates/swb-store/src/lib.rs:960-965`). The CLI checks the same shape
  before it calls the broker (`crates/swb/src/main.rs:32`, `:624-631` and
  `:697-700`). Send does not check that the recipient is registered. It
  stores mail for any well-formed id
  (`crates/swb-store/src/lib.rs:1183-1185`).
- **Bound delivery keys survive a restart, not a move.** ADR-0003 names the
  restart problem (`docs/adr/0003-sender-binding.md:22-24`) and fixes it
  with the delivery key `harness:host:session_id`
  (`docs/adr/0003-sender-binding.md:93-97`,
  `crates/swb-store/src/lib.rs:338-346`). The key still carries the harness
  and the host. A role that moves to another host or harness gets a new
  key, and the producer must know both parts in advance.
- **A lapse unbinds a row and keeps its claims.** A re-register of a key
  marks every earlier pid's row unbound
  (`crates/swb-store/src/lib.rs:793-797`). An idle or ended binding is
  marked unbound the same way (`crates/swb-store/src/lib.rs:355` and
  `:626`). None of these touch claims. Only `end` releases them
  (`crates/swb-store/src/lib.rs:860-864`,
  `crates/swb-store/src/claims.rs:241`). Claims are held by the full agent
  id, pid included. The binding check `gate` returns false for an unbound
  id, which is then self-asserted (`crates/swb-store/src/lib.rs:666-689`,
  SWB-R05).
- **Claims are advisory subject locks.** They record who works on a
  subject and never route mail (`docs/REFERENCE.md:151-156`, SWB-R16). Any
  session may record a non-exclusive claim on any subject, and an orphaned
  claim is never revoked (`docs/REFERENCE.md:179-180`).
- **OTel identity labels telemetry only.** `service.name` is the resource
  attribute `swb-broker`, which names the process that emitted a span
  (`docs/adr/0002-lgtm-plane.md:405-409`). `gen_ai.agent.id` repeats
  `from` (`docs/adr/0002-lgtm-plane.md:426`). Neither is an address, and
  readers must not treat telemetry identity as proof (SWB-R44,
  `docs/adr/0002-lgtm-plane.md:302-305`).

A producer can work around this today. It reads `peers` or `claims`, picks
a session and sends to that exact id. The pick can be stale by the time
the mail lands. Mail sent to a crashed unbound session then waits under a
pid that never reads again.

## Decision (proposed)

1. **A service address.** `to` may be `svc:<name>`, where `<name>` matches
   `[a-z0-9][a-z0-9.-]{0,62}`. `svc` is not a harness kind and the address
   has two parts, so it never parses as an agent id. `from` stays an agent
   id: a session always sends as itself.
2. **Names are configured, not first-come.** `swb serve` reads
   `SWB_SERVICES_FILE`, a JSON file. With the variable unset no name
   exists and the feature is off. An invalid file refuses to start, as an
   invalid `SWB_SEND_BURST` does. Each entry names:
   - `name`;
   - `holders`, a list of `{harness, host, session_id}` patterns, where
     each part is exact or `*`;
   - `secret_sha256`, the SHA-256 of a per-name claim secret that the
     operator provisions out of band;
   - `lease_minutes`, default 15, clamped to 1 to 60.

   ```json
   {"services": [{"name": "reviewer",
     "holders": [{"harness": "claude", "host": "*", "session_id": "reviewer"}],
     "secret_sha256": "<64 hex>", "lease_minutes": 15}]}
   ```

   A send to a name that is not configured answers 400 `unknown_service`,
   so a typo fails at once.
3. **Holding a name is an exclusive claim on the subject `svc:<name>`,
   held by a bound delivery key.** It reuses `claim`, `release`, `handoff`
   and `claims`, with these extra rules inside the `svc:` subject
   namespace only:
   - **Authorized claim.** In the claim's transaction the claimant must
     pass the binding check with its session token, so `gate` returns
     true. It must also match one of the name's `holders` patterns and
     present `service_secret`. A claim that fails any of these answers 403
     `service_unauthorized` and records nothing. The claim row records the
     `secret_sha256` it was authorized under.
   - **The holder is the key.** Inside `svc:` a claim belongs to the
     delivery key `harness:host:session_id` of its agent id, not to the
     pid. SWB-R61 then reads per key: a key holds at most one active claim
     per name.
   - **Live and inert claims.** An active `svc:` claim is live while the
     binding it was authorized under is still the key's live binding, the
     key still matches a `holders` pattern and the recorded hash equals the
     configured `secret_sha256`. A re-register that presents the current
     token continues a binding. A lapse ends it for good (SWB-R69), so a
     later fresh bind of the key does not revive the claim. Any other
     active `svc:` claim is inert. An inert claim stays recorded and is
     never revoked. It routes no mail and does not count against an
     authorized claim, so it is left out of `held_by`.
   - **Restart keeps the name.** A restart re-registers the key under a
     new pid with the current token (SWB-R69), so the claim stays live.
     The new pid reads the name's mail at once (Decision 4). Its next
     claim renews the key's claim: the same `claim_id`, the holder column
     set to the new agent id and the expiry extended. The audit line names
     the earlier agent id. ADR-0003 already treats the earlier pid as the
     same session, so this is a renewal, not an act on another session's
     claim.
   - **Exclusive.** A non-exclusive request is refused with 403, so no
     session can block a name with an advisory claim. An authorized
     request while another key holds a live claim gets the existing
     refusal with `held_by` (SWB-R60). There is no forced takeover. If a
     config change makes two claims live, the claim recorded later routes,
     and the other key's renewal is refused with `held_by`.
   - **Lease.** The lease is `lease_minutes`, not `lease_hours`. Only the
     holder's repeat claim, which carries its session token, renews it
     (SWB-R60 renewal). `heartbeat` takes no token, so it does not renew a
     name. The client renews at half the lease.
   - **Release and handoff.** `release` and `handoff` of a `svc:` claim
     also need `gate` to return true for a caller with the claim's key.
     An unbound or superseded agent id cannot release or offer a name.
   - **When the name is free.** A name has no holder while it has no live
     claim. That happens when its claim expires, is released or is handed
     off, when its holder's session ends, or when its key's binding lapses.
     `end` releases the claims recorded under the ending agent id
     (SWB-R60). A lapse releases nothing: the claim turns inert, and a new
     authorized claim can be recorded at once.
   - The client reads the secret from
     `$SWB_STATE_DIR/services/<name>.secret`, mode 0600, before each claim.
     The broker stores only the hash. The secret is never logged, printed
     or put on a command line.
4. **Resolution happens at delivery time.** A send to `svc:<name>` stores
   the message with `to` and the delivery key both set to `svc:<name>`. It
   is not tied to any session. A caller holds a name only when two checks
   pass in the same transaction as the read or the ack: the name's routing
   claim is live and has the caller's key, and `gate` returns true for the
   caller with its session token. `inbox` then reads the caller's own mail
   and the mail of every name it holds. `ack` of a name's message succeeds
   only for a caller that holds the name. Any other caller gets 409
   `not_service_holder`, and the message stays unacked. An unbound caller
   never holds a name, so a self-asserted agent id of an earlier holder
   cannot read, ack or release the name's mail. This is routing, not
   revocation, so it fits Decision 7.
5. **Mail waits while a name is unheld.** A message to a configured name
   with no holder is stored and kept until its normal expiry (72 hours by
   default, 14 days at most, SWB-R09). The next holder reads it. The send
   result carries `service_held: true|false` so the producer can see that
   nobody holds the name now. Delivery stays at least once: a message the
   last holder fetched but did not ack is delivered again to the next one,
   with a higher `delivery_count`.
6. **Handoff reuses claim handoff.** The holder offers its `svc:<name>`
   claim with `handoff`. The receiver accepts it with
   `claim {me, handoff_id, service_secret}` and must pass rule 3. In one
   transaction the source becomes `handed_off` and the receiver holds the
   name under a new `claim_id`. Unacked mail moves with the name. A
   planned handoff should ack what it finished first, so the receiver does
   not see it again. A refused acceptance leaves the offer open (SWB-R61).
7. **Config changes route, they do not revoke.** No broker path acts on
   another session's claim (AGENTS.md, Product boundary). When the file
   drops a holder pattern or changes a name's `secret_sha256`, the
   existing claim stays recorded and turns inert (Decision 3). The broker
   stops routing the name to it at the next `inbox` and refuses its acks
   for the name. A newly authorized holder can claim the name at once,
   with no wait for the old lease. Changing the file needs a broker
   restart.
8. **Telemetry.** Names are low cardinality, because only configured names
   exist. The attributes use the `swb.` prefix so nobody reads them as the
   OTel `service.name`.

   | Signal | Addition |
   | --- | --- |
   | `swb.send` span | `swb.to` = `svc:<name>`, `swb.to_service` = `<name>`, `swb.to_harness` and `swb.to_host` empty, `swb.service_held` |
   | `swb.fetch` and `swb.ack` spans | `swb.service`, `swb.service_holder` (the resolved agent id), `swb.service_claim_id` |
   | `swb.claim`, `swb.release` and `swb.handoff` spans | The subject is `svc:<name>`. A renewal under a new pid adds `swb.adopted_from` |
   | Audit lines | `fetch` and `ack` add `service`, `holder` and `claim_id`. A claim renewed under a new pid adds `adopted_from` |
   | Metrics | `swb_service_held{service}` (0 or 1) and `swb_service_pending_messages{service}` |

   Spans and lines are projections. Truth stays in `claims` and `inbox`
   (SWB-R02, SWB-R44).

## Failure modes

| Case | Behaviour | What the producer sees |
| --- | --- | --- |
| Name not configured | Send refused, nothing stored | 400 `unknown_service` |
| No holder | Mail stored and kept until it expires | `service_held: false`, and `swb_service_held` reads 0 |
| Contested claim | The second claimant is refused with `held_by`. The holder keeps the name | Nothing changes |
| Holder crash | The binding stays live, so the name stays held until its lease ends, at most `lease_minutes`. Mail waits, then goes to the next holder | Delivery is delayed, and fetched but unacked mail arrives again |
| Holder restarts under the same bound key | The new pid re-registers with the current token (SWB-R69). The claim stays live, the new pid reads the name's mail at once and its next claim renews the claim under its own agent id | Nothing, except that fetched but unacked mail arrives again |
| Lease expiry with a live holder | The name is free. The old holder's next ack of a name message gets 409 and its next inbox no longer returns name mail | At-least-once delivery, possibly a duplicate |
| Binding lapse | The claim is not released. It turns inert at once and routes nothing. A new authorized claim is recorded with no lease wait | As for no holder until the next holder claims |
| `end` | The claims recorded under the ending agent id are released (SWB-R60) | As for no holder until the next holder claims |
| Holder dropped from config, or secret rotated | Routing stops at once. The claim stays recorded and inert. A newly authorized holder claims at once | As for no holder until the next holder claims |
| A `svc:` claim recorded before the upgrade | Inert. It never routes and never blocks the name | Nothing |
| Broker restart | Claims and mail are in the store. Leases keep their wall-clock expiry | Nothing, if the holder renews in time |

## Security

| Threat | Control | Residual risk |
| --- | --- | --- |
| A session sends as the name | `from` must be an agent id. A bound sender needs its token (SWB-R66) | An unbound sender can still claim any unbound id, as today (SWB-R05) |
| A hostile session claims the name | It must match `holders`, be bound and present the secret | Anyone who reads the secret file and can bind a matching key can hold the name. Rotate by changing the hash and restarting. Claims under the old hash turn inert at once |
| A session acts as an earlier holder | Name mail routes, acks and releases only for a caller whose key holds the live claim and whose token passes `gate` in the same transaction. A lapsed binding never revives a claim (SWB-R69) | None for name mail. Other mail of an unbound id stays self-asserted (SWB-R05) |
| A hostile session blocks the name | Advisory and unauthorized claims on `svc:` subjects are refused. Inert claims, those from before the upgrade included, do not count against an authorized claim | An authorized holder that stops serving but keeps renewing holds the name until the operator drops its pattern or rotates the secret |
| A stale holder acks mail it no longer owns | Ack checks the current holder and its token | None beyond a duplicate delivery |
| A client learns which names exist | None. `unknown_service` and `service_held` tell any network client which names are configured and whether each is held | Names and their held state are visible inside the network ACL. Names must not carry private details. Open ruling 6 decides whether only bound senders see `service_held` |
| Replay of a send | SWB-R46 makes a repeated `msg_id` idempotent or a conflict | None |
| Replay of a claim or handoff | Each claim needs the current session token, which every register rotates. A handoff offer is accepted once and closes on release or `end` | A stolen live token works until it rotates, as in ADR-0003 |
| Flood to a name | The per-sender bucket (ADR-0003 Decision 3) applies to sends to a name | A flood that rotates unbound ids is bounded by the network ACL only |
| A hostile holder reads the role's mail | Secret custody and the network ACL | Out of scope. Bodies must never carry a secret (SWB-R19) |

The broker does not read the caller's network identity, so `host` in a
holder pattern is self-asserted. The secret is the control, and the
pattern narrows who may present it.

## Alternatives considered

- **Use bound delivery keys directly.** The producer sends to the agent id
  of a bound key, and ADR-0003 delivers across restarts. This needs no new
  address. It fails the use case: the key carries the host and harness, so
  a move breaks the address and the producer must know where the role
  runs. Binding is also first-come per key, so it gives no authorization.
- **Client-side lookup through `peers` or `claims`.** The producer looks
  up a holder by a naming convention or an advisory claim, then sends to
  its exact id. This works today. It resolves at send time, so a holder
  change between lookup and delivery strands the mail. Every producer
  repeats the lookup rules. An advisory claim proves nothing, and an
  orphaned one is never revoked.
- **A DNS-like registry.** An external name service maps names to agent
  ids, and clients cache the answer with a TTL. Caches go stale, and
  resolution still happens at send time. It adds a second authority beside
  the broker, against SWB-R02, and it needs its own authentication.
- **Separate service verbs and table.** A `services` table with its own
  claim, release and handoff verbs would leave SWB-R16 untouched. It
  duplicates the claim lifecycle, audit lines, spans and the SWB-R60 and
  SWB-R61 cases. Open rulings keeps this choice open.

## Migration and compatibility

- **Additive.** Agent ids, unbound sessions and bound keys behave as in
  v0.5.0. With `SWB_SERVICES_FILE` unset no `svc:` rule applies.
- **Upgrade order.** Upgrade the broker first. An older broker refuses
  `svc:<name>` as an invalid agent id, so a new client fails closed and
  nothing is misdelivered. An older client refuses the address locally at
  `send_payload`. An older client cannot send `service_secret`, so holders
  upgrade before a name is configured.
- **Existing `svc:` claims.** The claim subject charset already allows
  `svc:`. The broker does not touch a claim recorded before the upgrade.
  It has no recorded hash, so it is inert: it keeps advisory semantics
  until it ends, routes nothing and does not block the name.
  `swb claims --subject svc:<name>` still lists it.
- **Schema.** `schemas/envelope-v3.schema.json` gains a `service_address`
  definition, and `to` becomes either an agent id or a service address.
  ADR-0001 marks the schema a draft, so this follows the ADR-0003
  precedent of no version change. A validator pinned to the older copy
  rejects the new address.
- **Spec.** `spec/dhall/Broker.dhall` gains the `svc:` claim rules after
  the ruling. QuickCheck properties to add: at most one routing holder
  per name, no read or ack by a caller that fails `gate`, an inert claim
  never routes or blocks, a restart under the same key keeps the name and
  no message is lost while a name is unheld before it expires.
- **Docs.** `docs/REFERENCE.md` gains a Service names section, and
  SECURITY.md describes the secret file.

## Consequences

- SWB-R16 gains an exception inside the `svc:` namespace. There a claim
  is exclusive, authorized, held by a key and routes mail. It can be
  refused for a reason other than `held_by`, and it can turn inert
  without being revoked. Outside it, claims stay advisory.
- `authority` stays `peer` (SWB-R14). Holding a name proves which session
  serves a role, never operator authority.
- `thread` reads by exact agent id. A holder sees name mail through
  `inbox`, and `thread` shows it only if the ruling extends `thread`.
- A crash delays delivery by up to one lease. A shorter lease shortens
  the delay and costs more renewals. A restart under the same key, a
  lapse, a config change and a secret rotation cause no lease wait.

## Open rulings

1. The reserved `svc:` claim namespace, or separate service verbs and a
   table.
2. Whether the per-name secret is required for every name, or only for a
   name whose `holders` contain `*`.
3. The default and bounds of `lease_minutes`.
4. Whether a holder may send as `svc:<name>` so replies reach whoever
   holds the producer's name, or whether that waits for a `reply_to`
   field.
5. Whether `thread` includes name mail for the current holder.
6. Whether `service_held` is returned only to bound senders, so an
   unbound client cannot see whether a name is held.
