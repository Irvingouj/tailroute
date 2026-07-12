# SPEC-v1 Acceptance Bar & Test Pyramid

Frozen gate for calling v1 complete. Product requirements: [SPEC-v1.md](../SPEC-v1.md) §20–§21.

## Test pyramid

| Tier | Purpose | Command | Docker? |
|------|---------|---------|---------|
| **Unit** | Hostnames, labels, backend resolution, Tailscale parse | `cargo test -p tailsvc-common -p tailsvc-docker --lib` | No |
| **In-process integration** | Controller API, DNS, proxy (Host/XFF/stream/WS/SSE/errors), leases | `cargo test -p tailsvc-integration-tests --tests` | No |
| **Compose e2e (preferred)** | Real discovery, multi-label matrix, stop&lt;10s, agent/controller restart | `./deploy/compose/e2e/run-e2e.sh` | Yes |

**Single entrypoint:** `./scripts/test-all.sh`

### Rules

1. E2E is source of truth for user-visible routing.
2. No false greens — public surfaces only.
3. `proxy_listen` / simulated `100.x` / `status_listen` only in test configs.
4. Ship only when unit + in-process are green; release when compose e2e is green too.

## SPEC §21 checklist

| # | Criterion | Proof |
|---|-----------|-------|
| 1 | Controller DNS | compose dig A |
| 2 | Unknown DNS upstream | compose dig example.com |
| 3 | Arbitrary hostname | compose `example.org` shadow |
| 4 | One Agent per host | compose |
| 5 | Tailscale IPv4 discovery | LocalAPI→CLI→ifaces; e2e override |
| 6 | Bind proxy to TS IP / test override | agent bind + e2e proxy_listen |
| 7 | Label discovery | whoami |
| 8 | Multi-hostname | whoami-multi |
| 9 | Explicit backends | whoami-alias |
| 10 | Published ports | unit resolve matrix |
| 11 | Host-network | unit resolve |
| 12 | Bridge IP | compose whoami |
| 13 | Multi-network selection | unit |
| 14 | Ownership conflicts | in-process conflict |
| 15 | Lease expiry | in-process purge |
| 16 | No Controller data path | architecture |
| 17 | WS / SSE / streaming | in-process proxy tests |
| 18 | Agent restart no re-enroll | `run-e2e.sh` |
| 19 | Controller restart state | `run-e2e.sh` |
| 20 | No public domain/certs | design |
| 21 | No public bind by default (prod) | docs |
| 22 | Automated primary paths | `test-all.sh` |
| 23 | Stop → DNS gone &lt;~10s | compose (measured ~1s) |

## Last green run (local)

- Unit + in-process: green  
- Compose e2e: green including stop/DNS, multi-host, shadow, agent restart, controller restart, agent `/ready`
