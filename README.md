# tailroute (tailsvc)

[![CI](https://github.com/Irvingouj/tailroute/actions/workflows/ci.yml/badge.svg)](https://github.com/Irvingouj/tailroute/actions/workflows/ci.yml)

Tailnet Docker service router — v1 per [SPEC-v1.md](./SPEC-v1.md).

Automatically exposes Docker services to devices on the same Tailscale tailnet via arbitrary DNS names. Application traffic never goes through the Controller.

## Binaries

- `tailsvc-controller` — DNS + control-plane API (no app traffic)
- `tailsvc-agent` — Docker discovery, route registration, HTTP reverse proxy on Tailscale IPv4

## Build

```bash
cargo build --release
```

## Local dev config

See `config/controller.toml` and `config/agent.toml`. Defaults use loopback and non-privileged ports for dev (DNS `5353`, proxy `8088`). Production: bind controller DNS/API and agent proxy to Tailscale IPs on port 53/80.

## Quick start

1. Create enrollment token: `curl -X POST -H "Authorization: Bearer dev-admin-token" http://127.0.0.1:8080/v1/admin/enrollment-tokens`
2. Run controller: `TAILSVC_CONFIG=config/controller.toml cargo run -p tailsvc-controller`
3. Run agent (with token + Docker): `TAILSVC_ENROLLMENT_TOKEN=... TAILSVC_CONFIG=config/agent.toml cargo run -p tailsvc-agent`

Label containers per spec (`tailsvc.enable`, `tailsvc.hosts`, `tailsvc.port`).

## Security

- **Docker socket access is equivalent to root** on the Docker host. The agent mounts the socket only to discover labels and inspect containers; it never exposes the Docker API remotely, never mutates containers based on labels, and treats label values as untrusted input.
- **Arbitrary domain shadowing is intentional** inside the tailnet (e.g. registering `google.com`). Shadowing authentication, package registry, update, or Tailscale control-plane domains can break client behavior. The system may warn but will not block registration.

## Tests (pyramid)

| Tier | What | Command | Docker? |
|------|------|---------|---------|
| **Unit** | Hostname/labels/backend resolution pure logic | `cargo test -p tailsvc-common -p tailsvc-docker --lib` | No |
| **In-process integration** | Controller API, DNS UDP, proxy Host routing against local backends | `cargo test -p tailsvc-integration-tests --tests` | No (optional ignored Docker test) |
| **Compose e2e (preferred acceptance)** | Full controller + agent + labeled services + DNS/HTTP/WS scenarios | `./deploy/compose/e2e/run-e2e.sh` | Yes |

**Single entrypoint (release gate when Docker is available):**

```bash
./scripts/test-all.sh
```

CI (GitHub Actions) runs `fmt` + `clippy` + unit/integration tests, Docker image build, and compose e2e on every PR/push to `main`. Tag `v*` publishes binaries + GHCR image.

- Always runs unit + in-process integration.
- If `docker ps` works: also runs ignored Docker cargo test + compose e2e.
- **v1 is complete only when compose e2e covers SPEC §20.2–§20.3 and all §21 acceptance criteria are green.**

See [docs/ACCEPTANCE-v1.md](./docs/ACCEPTANCE-v1.md) for the frozen §21 checklist and [deploy/compose/e2e/README.md](./deploy/compose/e2e/README.md) for the scenario matrix and helpers.

### Acceptance bar (SPEC §21)

Compose e2e (with short lease TTLs in e2e config) must demonstrate:

1. Controller serves DNS; unknown names forward upstream
2. Agent discovers Tailscale IPv4 (or e2e override), binds proxy only to that address (or explicit test override)
3. Label discovery: multi-host, explicit backend, published port, bridge IP; multi-network requires selection
4. Hostname ownership conflicts are deterministic
5. Agent leases expire stale DNS; container stop removes routes within ~10s
6. Traffic path is client → agent → backend (never controller)
7. WebSocket, SSE, and streaming HTTP work through the agent
8. Agent restart without re-enrollment; controller restart preserves identity/routes
9. Public-domain shadowing allowed; invalid labels do not break valid routes

## Deploy notes

Production examples use host networking and Tailscale IPs (see SPEC §16). Compose e2e uses `proxy_listen` / `tailscale_ipv4` **only for testing** without a real tailnet.
