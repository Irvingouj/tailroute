#!/bin/sh
set -eu
# Require secrets from the environment — no default homelab tokens in-repo.
ADMIN_TOKEN="${TAILSVC_ADMIN_TOKEN:?set TAILSVC_ADMIN_TOKEN}"
CONTROLLER="${TAILSVC_CONTROLLER:?set TAILSVC_CONTROLLER}"
STATE="/var/lib/tailsvc-agent/agent.json"

if [ ! -f "$STATE" ]; then
  echo "waiting for controller at ${CONTROLLER}..."
  i=0
  while [ "$i" -lt 90 ]; do
    if wget -q -O - "${CONTROLLER}/health" >/dev/null 2>&1; then
      break
    fi
    i=$((i + 1))
    sleep 1
  done
  if [ "$i" -ge 90 ]; then
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
  echo "enrolling agent with one-time token"
fi

exec /usr/local/bin/tailsvc-agent
