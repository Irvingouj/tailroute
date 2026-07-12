#!/usr/bin/env bash
# Build images, start stack, run tester, lifecycle restarts, tear down.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
cd "$ROOT/deploy/compose/e2e"

export PATH="/Applications/Docker.app/Contents/Resources/bin:${PATH:-}"

ADMIN_TOKEN="${ADMIN_TOKEN:-e2e-admin-token}"
API="http://127.0.0.1:18080"
PROXY="http://127.0.0.1:18088"
STATUS="http://127.0.0.1:18089"
WHOAMI_HOST="whoami.internal"

cleanup() {
  docker compose down -v >/dev/null 2>&1 || true
}
trap cleanup EXIT

wait_http() {
  local url="$1" want="${2:-200}" tries="${3:-40}"
  local i code
  for i in $(seq 1 "$tries"); do
    code=$(curl -m 2 -s -o /dev/null -w "%{http_code}" "$url" 2>/dev/null || echo 000)
    if [ "$code" = "$want" ]; then
      return 0
    fi
    sleep 1
  done
  return 1
}

wait_proxy_host() {
  local host="$1" tries="${2:-40}"
  local i code
  for i in $(seq 1 "$tries"); do
    code=$(curl -m 2 -s -o /dev/null -w "%{http_code}" -H "Host: ${host}" "${PROXY}/" 2>/dev/null || echo 000)
    if [ "$code" = "200" ]; then
      return 0
    fi
    sleep 1
  done
  return 1
}

wait_admin_route() {
  local host="$1" tries="${2:-40}"
  local i
  for i in $(seq 1 "$tries"); do
    if curl -m 2 -sf -H "Authorization: Bearer ${ADMIN_TOKEN}" "${API}/v1/admin/routes" 2>/dev/null \
      | grep -q "\"hostname\":\"${host}\""; then
      return 0
    fi
    sleep 1
  done
  return 1
}

# Ensure non-secret e2e admin token file exists (gitignored)
if [[ ! -f admin_token ]]; then
  cp -f admin_token.example admin_token
fi

echo "Building tailsvc image (first run may take several minutes)..."
docker compose build controller

echo "Starting controller, agent, labeled backends..."
docker compose up -d controller agent whoami whoami-alias whoami-multi whoami-shadow whoami-bad

echo "Running tester (compose profile test)..."
docker compose --profile test run --rm tester

# --- §20.2 #14 agent restart without re-enrollment (state volume) ---
echo "Restarting agent (must re-use persisted credentials)..."
docker compose restart agent
wait_http "${STATUS}/health" 200 40 || {
  echo "FAIL: agent status not up after restart" >&2
  docker compose logs agent --tail 60 || true
  exit 1
}
wait_admin_route "${WHOAMI_HOST}" 45 || {
  echo "FAIL: admin routes missing ${WHOAMI_HOST} after agent restart" >&2
  docker compose logs agent --tail 60 || true
  exit 1
}
wait_proxy_host "${WHOAMI_HOST}" 40 || {
  echo "FAIL: proxy not serving after agent restart" >&2
  exit 1
}
echo "PASS: agent restart preserved enrollment (admin route + proxy OK)"

# --- §20.2 #16 controller restart with persisted state ---
echo "Restarting controller (SQLite volume)..."
docker compose restart controller
wait_http "${API}/health" 200 40 || {
  echo "FAIL: controller health after restart" >&2
  exit 1
}
# Agent re-heartbeats; wait for route to reappear in admin + proxy
wait_admin_route "${WHOAMI_HOST}" 60 || {
  echo "FAIL: routes not restored after controller restart" >&2
  docker compose logs controller --tail 40 || true
  docker compose logs agent --tail 40 || true
  exit 1
}
wait_proxy_host "${WHOAMI_HOST}" 40 || {
  echo "FAIL: proxy after controller restart" >&2
  exit 1
}
echo "PASS: controller restart preserved routes after agent heartbeat"
echo "PASS: proxy still serves ${WHOAMI_HOST} after restarts"

echo "E2E compose stack succeeded."
# trap cleanup tears down unless TAILSVC_E2E_KEEP=1
if [ "${TAILSVC_E2E_KEEP:-}" = "1" ]; then
  trap - EXIT
  echo "Leaving stack running (TAILSVC_E2E_KEEP=1)."
fi
