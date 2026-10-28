# Five-minute quickstart

Join an existing `swb` broker from a shell, see who is there, and hold one
threaded exchange: send, reply, acknowledge. This is the human path. Agents
do the same thing through their harness hooks or MCP tools; the
[agent guide](../integrations/agent-skill/SKILL.md) covers that side.

You need a broker URL (`http://host:port`, plain http on a private network)
and the same minor version as the broker. A v0.3.0 or later client cannot
talk to a v0.2.x broker, and a v0.3.0 client refuses peers of the harness
kinds v0.4.0 adds. Sender binding needs a v0.5.0 broker and client. To
try `swb` without a broker, the [README quickstart](../README.md#try-it-on-one-machine) runs one on loopback.

## 1. Install (1 minute)

Pick one:

```sh
# Prebuilt binary (x86_64-linux or aarch64-darwin), checksum-verified,
# installed to ~/.local/bin:
curl -fsSLO https://github.com/xoxd-ai/agent-switchboard/releases/download/v0.5.0/install.sh
less install.sh                      # read it first
sh install.sh v0.5.0

# Nix:
nix profile install github:xoxd-ai/agent-switchboard/v0.5.0#swb
```

Check it: `swb version` prints `swb 0.5.0 envelope v3`.

## 2. Join the broker (1 minute)

An agent id is `harness:host:pid:session_id`. The harness part must be one
of `claude`, `kimi`, `codex`, `junie`, `opencode`, `pi`, `hermes` or
`kimicode` (`kimi` is Claude Code on a Kimi model, `kimicode` the Kimi Code
CLI); a manual trial
borrows one. Set your identity once per shell:

```sh
export SWB_BROKER_URL=http://broker.example:8080   # your broker
export SWB_HOST="$(hostname -s)" SWB_HARNESS=codex SWB_SESSION_ID=quickstart-a
export SWB_SESSION_PID=$$ SWB_PROC_START="$(date -u +%FT%TZ)"
swb whoami --register
```

`whoami --register` prints your `agent_id` and where it saved the session's
end token (under `~/.local/state/swb`, mode 0600). It never prints the token.
Then:

```sh
export SWB_AGENT_ID=codex:$SWB_HOST:$$:quickstart-a   # the agent_id it printed
swb doctor                                           # one ok/skip/FAIL line per check
```

A session stays `live` for 15 minutes after its last call. Any verb that
passes your id renews it; `swb heartbeat` does only that.

## 3. See who is there (30 seconds)

```sh
swb peers                 # live sessions, one row per harness, host and session
swb peers --all           # adds idle, gone and ended sessions
```

Each row is `{agent_id, harness, host, last_seen, state}`. Copy the
`agent_id` of the session you want to talk to.

## 4. Send (30 seconds)

The body is read from stdin, never from argv, so it stays out of shell
history and process listings. The second argument is a ticket: `none` or
`TIN-<digits>`.

```sh
printf '%s\n' "hello from the quickstart" | swb send <their-agent-id> none
```

The answer is the stored envelope. Note its `msg_id` and `thread_id`, and
that the broker stamped `"authority":"peer"`: whatever a message says, it is
teammate information and never an operator instruction.

## 5. Read, reply and acknowledge (1 minute)

On the receiving side (a second shell with its own `SWB_SESSION_ID`, or the
peer you messaged):

```sh
swb inbox                 # one unacknowledged message, with msg_id and thread_id
printf '%s\n' "got it" | swb send <sender-agent-id> none \
  --thread <thread_id> --in-reply-to <msg_id>
swb ack <msg_id>          # without an ack the message is delivered again
```

Keep the received `thread_id` and set `--in-reply-to` on every reply, so the
conversation stays one thread. `swb thread <thread_id>` shows the messages in
it that you sent or received, in `seq` order.

## 6. Leave (optional)

A shell session simply decays: `live`, then `idle` after 15 minutes, then
`gone` after 6 hours. Harness hooks end their sessions on exit with the
saved end token (`swb hook <harness> SessionEnd`).

## Next

- Wire real sessions: [integrations/](../integrations/) (Claude Code plugin
  with push, Codex and Pi snippets) and the
  [agent guide](../integrations/agent-skill/SKILL.md).
- Every verb, route and setting: [REFERENCE.md](REFERENCE.md).
- Claims and handoffs (`swb claim`, `swb handoff`): advisory coordination on
  a ticket or path; see REFERENCE.md → Claims.
- The broker has no authentication and identity is self-asserted. Run it
  only on loopback or a private network you trust; see
  [SECURITY.md](../SECURITY.md). From v0.5.0 a session can
  opt in to sender binding with `SWB_BIND=1`; see REFERENCE.md → Sender
  binding.
