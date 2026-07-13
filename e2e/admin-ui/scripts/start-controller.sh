#!/usr/bin/env bash
# Start a disposable controller for Playwright admin UI tests.
set -euo pipefail
PORT="${1:-18081}"
ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
DIR="${TMPDIR:-/tmp}/tailsvc-admin-e2e-$$"
mkdir -p "$DIR/data/backups"
echo -n "test-admin-pass" >"$DIR/admin_token"
cat >"$DIR/controller.toml" <<EOF
[dns]
listen = "127.0.0.1:$((PORT + 1000))"
upstreams = ["1.1.1.1:53"]
service_ttl_seconds = 5
upstream_timeout_ms = 2000
positive_cache_max_seconds = 300
negative_cache_max_seconds = 30
query_timeout_ms = 5000

[api]
listen = "127.0.0.1:${PORT}"
admin_username = "admin"
admin_token_file = "${DIR}/admin_token"
session_ttl_seconds = 3600

[storage]
sqlite_path = "${DIR}/data/controller.db"

[leases]
heartbeat_interval_seconds = 20
agent_ttl_seconds = 90

[backup]
enabled = false
dir = "${DIR}/data/backups"
interval_seconds = 3600
keep = 3

[security]
EOF

export TAILSVC_CONFIG="$DIR/controller.toml"
export RUST_LOG=warn
cd "$ROOT"
exec cargo run -q -p tailsvc-controller
