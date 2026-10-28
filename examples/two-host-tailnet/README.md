# Two hosts on a tailnet

> **Tailnet only.** `swb` has no authentication in any release, v0.3.0
> included: anyone who can reach the broker's port can register as any agent
> id, read that agent's inbox and send as it. Run the broker only where every
> peer is a device you trust, such as a Tailscale tailnet. Never publish it
> to a LAN, a public IP, a LoadBalancer or an Ingress. See
> [SECURITY.md](../../SECURITY.md).

One broker, reachable on the tailnet as `swb`, and agent sessions on two
hosts that both point at it. Brokers do not federate: every host in a team
points its `SWB_BROKER_URL` at the same broker.

```text
 host-a (tailnet)                 broker (tailnet name: swb)            host-b (tailnet)
 Claude Code ── hooks, channel ─▶ swb serve :8080  /mcp  /v1/*  ◀─ MCP, CLI ── Pi / Codex / shell
                                  :9090 /metrics (not on the tailnet)
```

## 1. Run the broker

Pick one. Both need a v0.3.0 image digest: v0.3.0 is unreleased, so build
the image on a linux/amd64 build host with `just image` and use the digest it
prints. The approved v0.2.2 digest in
[approved-broker.json](../../docs/releases/approved-broker.json) is for
v0.2.2 clients only; broker and clients must run the same minor version.

- **Docker Compose**, [compose.yaml](compose.yaml): the broker shares a
  Tailscale sidecar's network namespace, binds `127.0.0.1:8080` there and
  publishes no host port. The sidecar's `tailscale serve`
  ([serve.json](serve.json)) forwards tailnet port 8080 to it, so the Docker
  host and other containers on the bridge network cannot reach the broker. Pass
  `TS_AUTHKEY` from your shell or an untracked `.env`; never commit it.
- **Kubernetes**, [k8s.yaml](k8s.yaml): one replica, a PVC, a non-root pod
  (UID 65532) and a ClusterIP Service that the
  [Tailscale Kubernetes operator](https://tailscale.com/kb/1236/kubernetes-operator)
  exposes on the tailnet (`tailscale.com/expose`). The Service carries only
  the API port. A ClusterIP Service is reachable from every pod in the
  cluster, so the manifest's NetworkPolicy admits that port only from the
  operator's proxy namespace (`tailscale`); it needs a CNI that enforces
  NetworkPolicy.

In both, set `SWB_MCP_ALLOWED_HOSTS` to exactly the names clients use (the
short MagicDNS name and the full `*.ts.net` name). A request for any other
`Host` is refused with 403. Leave `SWB_ALLOWED_ORIGINS` empty.

## 2. Point each host at it

On every host, start from [client.env.example](client.env.example):

```sh
export SWB_BROKER_URL=http://swb.your-tailnet.ts.net:8080   # http, host:port, no path
export SWB_HOST=host-a                                       # host-b on the other host
swb doctor --register      # one ok/skip/FAIL line per check; exits 3 on a failure
swb peers                  # sessions on every host
swb peers --host host-b    # just one host
```

Then wire the harnesses as on one host: Claude Code through
[integrations/claude-code/](../../integrations/claude-code/) (hooks plus the
channel), Codex and Pi through their snippets (unproven, R-C949), or any
shell through the CLI. [one-host/run.sh](../one-host/run.sh) shows the
launcher pattern that exports `SWB_SESSION_PID` and `SWB_PROC_START`.

## v0.3.0 end tokens

- `register` (and the SessionStart hook) returns a per-session end token to
  the registering session only. `SessionEnd` spends it; `POST /v1/end` with
  any other token answers `{"state":"unknown"}` and ends nothing.
- `swb hook` keeps tokens under `SWB_STATE_DIR/sessions/` (directory 0700,
  files 0600) and the CLI never prints one. Keep `SWB_STATE_DIR` local to
  each host and user: do not share it over NFS or sync it between hosts.
  From v0.5.0 a session started with `SWB_BIND=1` also keeps
  its session token under `SWB_STATE_DIR/bindings/` (ADR-0003); the same
  rule applies.
- Over MCP, the `register` result carries the token into the caller's
  context. An adapter that ends sessions itself should keep it and strip it
  before the model sees it.
- The token stops a caller ending a session it did not register. It is not
  authentication: anyone can re-register an agent id and get a fresh token.

The scripted demo exercises this cross-host flow on one machine, with two
processes as two hosts: [docs/demo/](../../docs/demo/).
