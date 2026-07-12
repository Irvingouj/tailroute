# Example: host-network deploy on a Tailscale node

This directory is a **template**. Replace placeholder Tailscale IPs and DNS names before use. Do not commit real tokens or personal hostnames.

## Placeholders

| Placeholder | Meaning |
|-------------|---------|
| `100.64.0.1` | This machine’s Tailscale IPv4 |
| `.services.example` | Your split-DNS zone (e.g. what you configure in Tailscale admin) |
| `admin.services.example` | Hostname for the controller admin UI via agent static route |

## What runs

| Service | Bind |
|---------|------|
| Controller DNS | `<TS_IP>:53` |
| Controller API + admin UI | `<TS_IP>:18080` |
| Agent proxy | `<TS_IP>:80` |
| Agent status | `<TS_IP>:18089` |
| whoami demo | `whoami.services.example` |

Admin UI is exposed only if `agent.toml` `[[static_routes]]` points at the controller API (domain names live in config, not in Rust source).

## Deploy

```bash
export TAILSVC_DEPLOY_HOST=user@your-host
export TAILSVC_TS_IP=100.x.y.z
export TAILSVC_ADMIN_TOKEN="$(openssl rand -hex 24)"
export TAILSVC_CONTROLLER="http://${TAILSVC_TS_IP}:18080"

# Edit controller.toml / agent.toml: set listen addresses to $TAILSVC_TS_IP
# and allowed_suffixes / static_routes hosts to your zone.

./sync-and-up.sh
```

Or on the host:

```bash
echo "$TAILSVC_ADMIN_TOKEN" > admin_token
export TAILSVC_ADMIN_TOKEN TAILSVC_CONTROLLER
docker compose up -d --build
```

## Test (from a tailnet client)

```bash
curl -s http://$TAILSVC_TS_IP:18080/health
dig @$TAILSVC_TS_IP whoami.services.example A
curl -H "Host: whoami.services.example" http://$TAILSVC_TS_IP/
# With split DNS configured:
# curl http://whoami.services.example/
# open http://admin.services.example/admin/
```

## Static routes

```toml
[[static_routes]]
hosts = ["admin.services.example"]
backend = "http://100.64.0.1:18080"
```
