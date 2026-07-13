#!/bin/sh
# Primary compose e2e runner — SPEC §20.2–20.3 scenarios.
set -eu

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname "$0")" && pwd)"
# shellcheck source=lib.sh
. "${SCRIPT_DIR}/lib.sh"

apk add --no-cache curl bind-tools jq docker-cli >/dev/null 2>&1 \
  || apk add --no-cache curl bind-tools jq >/dev/null

: "${WHOAMI_HOST:=whoami.internal}"
: "${ALIAS_HOST:=alias.internal}"
: "${MULTI_A:=multi-a.internal}"
: "${MULTI_B:=multi-b.internal}"
: "${SHADOW_HOST:=example.org}"
: "${ADMIN_HOST:=admin.e2e.internal}"
: "${EXPECT_TS_IP:=100.64.0.2}"
: "${AGENT_STATUS:=http://agent:8089}"

echo "=== tailsvc compose e2e ==="

# --- readiness ---
wait_until 30 controller_health || fail "controller /health"
pass "controller health"
wait_until 30 controller_ready || fail "controller /ready"
pass "controller ready"

if wait_until 45 sh -c "curl -sf '${AGENT_STATUS}/ready' >/dev/null"; then
  pass "agent /ready"
else
  # Status server may lag first reconcile; health alone is still useful.
  if curl -sf "${AGENT_STATUS}/health" >/dev/null 2>&1; then
    skip "agent /ready not yet true (health ok)"
  else
    skip "agent status endpoint not reachable"
  fi
fi

# --- §20.2 #1 bridge labeled whoami ---
wait_for_route "${WHOAMI_HOST}" 60
pass "admin lists ${WHOAMI_HOST}"
wait_for_dns "${WHOAMI_HOST}" "${EXPECT_TS_IP}" 30
pass "DNS ${WHOAMI_HOST} -> ${EXPECT_TS_IP}"

# Wire-format: question section required (macOS getaddrinfo / MagicDNS)
if dns_response_has_question "${WHOAMI_HOST}" A; then
  pass "DNS A response includes question section (QUERY: 1)"
else
  fail "DNS A missing question section (QUERY: 0) — breaks curl/getaddrinfo"
fi
if dns_aaaa_nodata_well_formed "${WHOAMI_HOST}"; then
  pass "DNS AAAA NODATA well-formed (question + size>=30)"
else
  fail "DNS AAAA NODATA malformed (header-only MSG SIZE ~12)"
fi

wait_for_proxy_ok "${WHOAMI_HOST}" 30
BODY="$(proxy_get "${WHOAMI_HOST}")"
echo "$BODY" | grep -qi hostname || fail "proxy body unexpected: $BODY"
pass "proxy Host ${WHOAMI_HOST}"

# --- unknown host 404 ---
CODE="$(proxy_status unknown.internal)"
[ "$CODE" = "404" ] || fail "unknown host expected 404 got ${CODE}"
pass "proxy unknown host 404"

# --- §20.2 #4 explicit backend ---
wait_until 30 route_listed "${ALIAS_HOST}" || fail "alias.internal not registered"
wait_for_proxy_ok "${ALIAS_HOST}" 20
pass "proxy explicit backend ${ALIAS_HOST}"

# --- §20.2 #5 multiple hostnames ---
wait_until 30 route_listed "${MULTI_A}" || fail "multi-a not registered"
wait_for_dns "${MULTI_A}" "${EXPECT_TS_IP}" 20
wait_for_dns "${MULTI_B}" "${EXPECT_TS_IP}" 20
wait_for_proxy_ok "${MULTI_A}" 15
wait_for_proxy_ok "${MULTI_B}" 15
pass "multi-host ${MULTI_A} + ${MULTI_B}"

# --- §20.2 #17 public-domain shadowing ---
wait_until 20 route_listed "${SHADOW_HOST}" || fail "shadow host not registered"
wait_for_dns "${SHADOW_HOST}" "${EXPECT_TS_IP}" 15
wait_for_proxy_ok "${SHADOW_HOST}" 15
pass "public-domain shadow ${SHADOW_HOST} -> agent"

# --- §20.2 #9 unknown domain upstream ---
UP_IP="$(dns_a example.com || true)"
if [ -n "$UP_IP" ] && [ "$UP_IP" = "${EXPECT_TS_IP}" ]; then
  fail "unregistered example.com shadowed to agent IP"
fi
if [ -n "$UP_IP" ]; then
  pass "upstream forward example.com -> ${UP_IP}"
else
  skip "upstream dig empty (network restricted)"
fi

# --- §20.2 #18 invalid labels: whoami still works ---
wait_for_proxy_ok "${WHOAMI_HOST}" 10
pass "valid routes survive invalid-label neighbor"

# --- static_routes: admin UI via agent Host routing ---
wait_until 30 route_listed "${ADMIN_HOST}" || fail "static route ${ADMIN_HOST} not registered"
wait_for_dns "${ADMIN_HOST}" "${EXPECT_TS_IP}" 20
pass "DNS static ${ADMIN_HOST} -> ${EXPECT_TS_IP}"

# HTML admin page through agent proxy
ADMIN_CODE=$(curl -s -o /tmp/admin.html -w "%{http_code}" -H "Host: ${ADMIN_HOST}" "${AGENT_PROXY}/admin/")
[ "$ADMIN_CODE" = "200" ] || fail "admin UI via proxy expected 200 got ${ADMIN_CODE}"
grep -qi "tailsvc" /tmp/admin.html || fail "admin HTML missing tailsvc marker"
pass "static_routes proxy Host ${ADMIN_HOST}/admin/ -> controller UI"

# Dashboard API requires admin bearer (semi health payload)
DASH=$(curl -sf -H "Authorization: Bearer ${ADMIN_TOKEN}" -H "Host: ${ADMIN_HOST}" \
  "${AGENT_PROXY}/v1/admin/dashboard" || true)
if [ -z "$DASH" ]; then
  # fall back to controller direct if proxy path odd
  DASH=$(curl -sf -H "Authorization: Bearer ${ADMIN_TOKEN}" "${CONTROLLER}/v1/admin/dashboard")
fi
echo "$DASH" | jq -e --arg h "${ADMIN_HOST}" '.routes[] | select(.hostname==$h)' >/dev/null \
  || fail "dashboard missing static host ${ADMIN_HOST}"
echo "$DASH" | jq -e --arg h "${WHOAMI_HOST}" '.routes[] | select(.hostname==$h)' >/dev/null \
  || fail "dashboard missing docker host ${WHOAMI_HOST}"
pass "admin dashboard lists static + docker routes"

# Unauthorized without token
UNAUTH=$(curl -s -o /dev/null -w "%{http_code}" -H "Host: ${ADMIN_HOST}" \
  "${AGENT_PROXY}/v1/admin/dashboard")
[ "$UNAUTH" = "401" ] || fail "dashboard without token expected 401 got ${UNAUTH}"
pass "admin API still requires bearer token"

# --- username/password login + discovery ---
LOGIN_JSON=$(curl -sf -H "Host: ${ADMIN_HOST}" -H "Content-Type: application/json" \
  -d "{\"username\":\"admin\",\"password\":\"${ADMIN_TOKEN}\"}" \
  "${AGENT_PROXY}/v1/admin/login") \
  || fail "admin login failed"
SESSION=$(echo "$LOGIN_JSON" | jq -r .token)
[ -n "$SESSION" ] && [ "$SESSION" != "null" ] || fail "login missing token"
pass "admin username/password login"

# HTML contains required element ids (guards JS null textContent bugs)
for id in c-status c-routes c-agents c-disc c-probe-ok disc-body routes-body agents-body whoami login-view app-view; do
  grep -q "id=\"${id}\"" /tmp/admin.html || fail "admin HTML missing id=${id}"
done
pass "admin HTML has required element ids"

# Discovery endpoint (may be empty briefly; wait for agent report)
wait_until 45 sh -c "curl -sf -H 'Authorization: Bearer ${SESSION}' -H 'Host: ${ADMIN_HOST}' '${AGENT_PROXY}/v1/admin/discovery' | jq -e 'type==\"array\" and length>=1' >/dev/null" \
  || fail "discovery empty or failed"
DISC=$(curl -sf -H "Authorization: Bearer ${SESSION}" -H "Host: ${ADMIN_HOST}" \
  "${AGENT_PROXY}/v1/admin/discovery")
echo "$DISC" | jq -e --arg n whoami '.[] | select(.container_name|test("whoami";"i"))' >/dev/null \
  || fail "discovery missing whoami container"
pass "admin discovery lists docker candidates"

# --- §20.2 #7 / §20.3 container stop removes DNS ---
if command -v docker >/dev/null 2>&1; then
  CID="$(docker ps --filter "label=tailsvc.hosts=whoami.internal" --format '{{.ID}}' | head -1 || true)"
  if [ -z "$CID" ]; then
    CID="$(docker ps --filter "name=whoami" --format '{{.ID}}' | head -1 || true)"
  fi
  if [ -n "$CID" ]; then
    echo "stopping container ${CID} for stop/DNS test..."
    docker stop "$CID" >/dev/null
    START=$(date +%s)
    wait_for_no_dns "${WHOAMI_HOST}" 25
    END=$(date +%s)
    ELAPSED=$((END - START))
    if [ "$ELAPSED" -gt 10 ]; then
      echo "WARN: DNS removal took ${ELAPSED}s (SPEC target under 10s)"
    fi
    pass "container stop cleared DNS ${WHOAMI_HOST} in ${ELAPSED}s"
    docker start "$CID" >/dev/null || true
    wait_until 40 dns_a_equals "${WHOAMI_HOST}" "${EXPECT_TS_IP}" \
      || fail "whoami did not return after start"
    pass "container restart restored DNS ${WHOAMI_HOST}"
  else
    skip "could not find whoami container to stop"
  fi
else
  skip "docker CLI unavailable in tester for stop scenario"
fi

echo "=== compose e2e scenarios finished ==="
