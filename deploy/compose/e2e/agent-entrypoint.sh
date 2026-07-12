#!/bin/sh
set -eu
ADMIN_TOKEN="${TAILSVC_ADMIN_TOKEN:-e2e-admin-token}"
CONTROLLER="${TAILSVC_CONTROLLER:-http://controller:8080}"
STATE="/var/lib/tailsvc-agent/agent.json"

if [ ! -f "$STATE" ]; then
  echo "waiting for controller..."
  i=0
  while [ "$i" -lt 60 ]; do
    if wget -q -O - "${CONTROLLER}/health" >/dev/null 2>&1; then
      break
    fi
    i=$((i + 1))
    sleep 1
  done
  if [ "$i" -ge 60 ]; then
    echo "controller not ready" >&2
    exit 1
  fi
  echo "creating enrollment token..."
  TOKEN=$(wget -q -O - \
    --header="Authorization: Bearer ${ADMIN_TOKEN}" \
    --header="Content-Type: application/json" \
    --post-data="" \
    "${CONTROLLER}/v1/admin/enrollment-tokens" \
    | sed -n 's/.*"token":"\([^"]*\)".*/\1/p')
  if [ -z "$TOKEN" ]; then
    echo "failed to parse enrollment token" >&2
    exit 1
  fi
  export TAILSVC_ENROLLMENT_TOKEN="$TOKEN"
fi

exec /usr/local/bin/tailsvc-agent