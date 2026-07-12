# Compose E2E (preferred SPEC acceptance)

Full-stack tests on **local Docker** for SPEC §20.2–§20.3 / §21. No real Tailscale required: agent uses simulated `100.64.0.2`, `proxy_listen = 0.0.0.0:8088`, `status_listen = 0.0.0.0:8089` (test-only overrides).

## Stack

| Service | Role |
|---------|------|
| **controller** | DNS `:5353` + API `:8080`, SQLite (short lease TTL) |
| **agent** | Docker socket, discovery, proxy `:8088`, status `:8089` |
| **whoami** / multi / alias / shadow / bad | Labeled backends for §20.2 |
| **tester** | DNS, HTTP Host, admin API, stop/start lifecycle |

## Run

```bash
./deploy/compose/e2e/run-e2e.sh
# or
./scripts/test-all.sh
```

`run-e2e.sh` also:

1. Runs in-container tester scenarios  
2. Restarts **agent** (no re-enrollment)  
3. Restarts **controller** (SQLite persistence)  
4. Re-checks DNS + HTTP  

## Host ports

| Port | Service |
|------|---------|
| `127.0.0.1:18080` | Controller API |
| `127.0.0.1:15353` | Controller DNS |
| `127.0.0.1:18088` | Agent HTTP proxy |
| `127.0.0.1:18089` | Agent `/health` `/ready` |

## Helpers

- `scripts/lib.sh` — `wait_for_route`, `wait_for_dns`, `wait_for_no_dns`, `proxy_get`, …
- `scripts/run-tests-in-compose.sh` — scenario matrix body

## Scenario matrix (SPEC §20.2)

| # | Case | Status |
|---|------|--------|
| 1 | Bridge container via agent | compose whoami |
| 2 | Published localhost port | unit resolve |
| 3 | Host-network container | unit resolve |
| 4 | Explicit backend | whoami-alias |
| 5 | Multiple hostnames | whoami-multi |
| 6 | Duplicate hostname conflict | in-process |
| 7 | Container stop removes DNS | compose (&lt;10s) |
| 8 | Lease expiry | in-process purge + short TTL |
| 9 | Unknown domain upstream | compose dig |
| 10 | WebSocket | in-process 101 upgrade |
| 11 | SSE | in-process event-stream |
| 12 | Large streaming bodies | in-process |
| 13 | Docker event reconnect | agent reconnect loop |
| 14 | Agent restart no re-enroll | `run-e2e.sh` restart agent |
| 15 | Tailscale IP change | LocalAPI priority + override |
| 16 | Controller restart | `run-e2e.sh` restart controller |
| 17 | Public-domain shadowing | example.org |
| 18 | Invalid labels isolated | whoami-bad |
| — | **static_routes** admin UI host | `admin.e2e.internal` → controller |
| — | Admin dashboard + 401 without token | tester |

## Security

- Docker socket = root-equivalent on the host. Trusted machines only.
- Shadowing scenarios only affect the e2e network.
