#!/usr/bin/env bash
# Sync repo to a remote host and bring up controller + agent + whoami.
# Requires operator-supplied host and secrets — no personal defaults in-repo.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
HOST="${TAILSVC_DEPLOY_HOST:?set TAILSVC_DEPLOY_HOST (e.g. user@host)}"
REMOTE_DIR="${TAILSVC_REMOTE_DIR:-/opt/tailroute}"
TS_IP="${TAILSVC_TS_IP:?set TAILSVC_TS_IP (this host Tailscale IPv4)}"
ADMIN_TOKEN="${TAILSVC_ADMIN_TOKEN:?set TAILSVC_ADMIN_TOKEN}"

# Local admin_token for rsync (gitignored)
echo -n "$ADMIN_TOKEN" > "${ROOT}/deploy/compose/primary/admin_token"

echo "==> rsync to ${HOST}:${REMOTE_DIR}"
rsync -az --delete \
  --exclude target \
  --exclude .git \
  --exclude data \
  --exclude '**/target' \
  "${ROOT}/" "${HOST}:${REMOTE_DIR}/"

echo "==> build + up on ${HOST}"
ssh -o BatchMode=yes -o ConnectTimeout=15 "${HOST}" bash -s <<EOF
set -euo pipefail
cd ${REMOTE_DIR}/deploy/compose/primary
chmod +x agent-entrypoint.sh
export DOCKER_BUILDKIT=1
export TAILSVC_ADMIN_TOKEN='${ADMIN_TOKEN}'
export TAILSVC_CONTROLLER='http://${TS_IP}:18080'
docker compose build controller
docker compose up -d --remove-orphans
echo "==> status"
docker compose ps
sleep 2
wget -q -O - --timeout=3 "http://${TS_IP}:18080/health" || true
echo
wget -q -O - --timeout=3 "http://${TS_IP}:18089/health" || true
echo
docker compose logs --tail 20 controller agent || true
EOF

echo "==> done"
echo "  curl -s http://${TS_IP}:18080/health"
echo "  dig @${TS_IP} whoami.services.example A"
