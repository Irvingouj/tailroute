# Deployments

Two supported modes. Both use the same binaries; differ in process supervisor and packaging.

## 1) Docker (compose)

- Image: `deploy/docker/Dockerfile`
  - Built **without** `dev-defaults` (admin token file required)
  - Built **with** `backup` feature
- Stack examples:
  - `deploy/compose/primary/` — host network + Tailscale on a real host
  - `deploy/compose/e2e/` — CI/dev e2e
- Restart: `restart: unless-stopped` in compose
- Health: container `HEALTHCHECK` + app `/health` `/ready`

```bash
./deploy/compose/primary/sync-and-up.sh
```

## 2) systemd service

- Units: `deploy/systemd/*.service`
- Installer: `sudo ./deploy/systemd/install.sh`
- `Restart=always`, `RestartSec=2`
- Controller runs as `tailsvc` user with `CAP_NET_BIND_SERVICE`
- Agent typically needs docker group / root for socket + :80
- SQLite backups under `/var/lib/tailsvc-controller/backups` (hourly, keep 48)

```bash
sudo ./deploy/systemd/install.sh
# edit /etc/tailsvc/*.toml
sudo systemctl restart tailsvc-controller tailsvc-agent
```

## Compile flags (features)

| Feature | Crate | Default | Production | Purpose |
|---------|-------|---------|------------|---------|
| `backup` | controller | **on** | **on** | Periodic SQLite `VACUUM INTO` |
| `dev-defaults` | controller | **off** | **off** | Missing admin token → `dev-admin-token` |

```bash
# production / normal build (backup already in default features)
cargo build --release -p tailsvc-controller

# local dev only if you want missing-token fallback
cargo build -p tailsvc-controller --features dev-defaults
```

## Agent static routes (non-Docker / admin UI on a domain)

Declare in `agent.toml` (domain names belong in deploy config only):

```toml
[[static_routes]]
hosts = ["admin.example.com"]
backend = "http://127.0.0.1:18080"
```

Merged with Docker label discovery on each reconcile. **Static wins** if a hostname collides with a container route.

## Controller resilience

- Panic hook logs; process does not rely on panic for control flow
- `CatchPanicLayer` on HTTP — one bad request cannot kill the API
- DNS task **auto-restarts** with backoff if bind/run fails
- Lease cleanup loop isolated
- Backup failures are logged; process keeps serving
- `/ready` requires storage + DNS up flag
- Optional `[security] allowed_suffixes` / `blocked_names` for safer global DNS
