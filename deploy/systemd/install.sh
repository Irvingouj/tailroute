#!/usr/bin/env bash
# Install tailsvc as systemd services on a Linux host (deployment mode 2).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
PREFIX="${PREFIX:-/usr/local}"
CONFIG_DIR="${CONFIG_DIR:-/etc/tailsvc}"
DATA_DIR="${DATA_DIR:-/var/lib/tailsvc-controller}"
AGENT_DATA="${AGENT_DATA:-/var/lib/tailsvc-agent}"

if [[ "$(id -u)" -ne 0 ]]; then
  echo "run as root" >&2
  exit 1
fi

echo "==> build release binaries (no dev-defaults)"
cd "$ROOT"
# default features already include backup
cargo build --release -p tailsvc-controller
cargo build --release -p tailsvc-agent

echo "==> install binaries"
install -m 0755 target/release/tailsvc-controller "$PREFIX/bin/tailsvc-controller"
install -m 0755 target/release/tailsvc-agent "$PREFIX/bin/tailsvc-agent"

echo "==> user/dirs"
id tailsvc >/dev/null 2>&1 || useradd --system --home "$DATA_DIR" --shell /usr/sbin/nologin tailsvc
mkdir -p "$CONFIG_DIR" "$DATA_DIR/backups" "$AGENT_DATA"
if [[ ! -f "$CONFIG_DIR/admin_token" ]]; then
  umask 077
  openssl rand -hex 24 >"$CONFIG_DIR/admin_token"
  echo "wrote $CONFIG_DIR/admin_token"
fi
chown -R tailsvc:tailsvc "$DATA_DIR"
chmod 0750 "$DATA_DIR" "$DATA_DIR/backups"
chmod 0640 "$CONFIG_DIR/admin_token"
chown root:tailsvc "$CONFIG_DIR/admin_token"

if [[ ! -f "$CONFIG_DIR/controller.toml" ]]; then
  cat >"$CONFIG_DIR/controller.toml" <<'EOF'
[dns]
listen = "0.0.0.0:53"
upstreams = ["1.1.1.1:53", "8.8.8.8:53"]
service_ttl_seconds = 5
upstream_timeout_ms = 2000
positive_cache_max_seconds = 300
negative_cache_max_seconds = 30
query_timeout_ms = 5000

[api]
listen = "0.0.0.0:18080"
admin_token_file = "/etc/tailsvc/admin_token"

[storage]
sqlite_path = "/var/lib/tailsvc-controller/controller.db"

[leases]
heartbeat_interval_seconds = 20
agent_ttl_seconds = 90

[backup]
enabled = true
dir = "/var/lib/tailsvc-controller/backups"
interval_seconds = 3600
keep = 48

[security]
# Uncomment for safer global Tailscale DNS:
# allowed_suffixes = [".irvingou.com", ".internal"]
# blocked_names = ["github.com", "googleapis.com", "login.microsoftonline.com"]
EOF
  echo "wrote $CONFIG_DIR/controller.toml — edit listen IPs to Tailscale"
fi

if [[ ! -f "$CONFIG_DIR/agent.toml" ]]; then
  cat >"$CONFIG_DIR/agent.toml" <<'EOF'
controller_url = "http://127.0.0.1:18080"
docker_socket = "/var/run/docker.sock"
state_dir = "/var/lib/tailsvc-agent"
proxy_port = 80
# Set these on the host:
# tailscale_ipv4 = "100.x.y.z"
# proxy_listen = "100.x.y.z:80"
# status_listen = "100.x.y.z:18089"
connect_timeout_ms = 2000
response_timeout_seconds = 300
tcp_probe_on_register = true
EOF
  echo "wrote $CONFIG_DIR/agent.toml — set controller_url and Tailscale bind"
fi

echo "==> systemd units"
install -m 0644 deploy/systemd/tailsvc-controller.service /etc/systemd/system/
install -m 0644 deploy/systemd/tailsvc-agent.service /etc/systemd/system/
systemctl daemon-reload
systemctl enable tailsvc-controller.service
systemctl enable tailsvc-agent.service

echo "==> start controller"
systemctl restart tailsvc-controller.service
sleep 1
systemctl --no-pager --full status tailsvc-controller.service || true

echo "Next:"
echo "  1. Edit $CONFIG_DIR/controller.toml and agent.toml (Tailscale IPs)"
echo "  2. systemctl restart tailsvc-controller tailsvc-agent"
echo "  3. journalctl -u tailsvc-controller -f"
echo "Admin token: $CONFIG_DIR/admin_token"
