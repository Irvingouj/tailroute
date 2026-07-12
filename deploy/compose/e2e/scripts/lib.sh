#!/bin/sh
# Shared helpers for compose e2e scenarios. Source from scenario scripts.
# shellcheck shell=sh

: "${CONTROLLER:?CONTROLLER required}"
: "${AGENT_PROXY:?AGENT_PROXY required}"
: "${DNS_SERVER:?DNS_SERVER required}"
: "${DNS_PORT:?DNS_PORT required}"
: "${ADMIN_TOKEN:?ADMIN_TOKEN required}"

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

pass() {
  echo "PASS: $*"
}

skip() {
  echo "SKIP: $*"
}

# poll until command succeeds or timeout (seconds)
wait_until() {
  _timeout="${1:?}"
  shift
  _i=0
  while [ "$_i" -lt "$_timeout" ]; do
    if "$@"; then
      return 0
    fi
    sleep 1
    _i=$((_i + 1))
  done
  return 1
}

controller_health() {
  curl -sf "${CONTROLLER}/health" >/dev/null
}

controller_ready() {
  curl -sf "${CONTROLLER}/ready" >/dev/null
}

# admin_routes_json → stdout
admin_routes() {
  curl -sf -H "Authorization: Bearer ${ADMIN_TOKEN}" "${CONTROLLER}/v1/admin/routes"
}

# hostname present in admin routes?
route_listed() {
  _host="${1:?}"
  admin_routes | jq -e --arg h "$_host" '.[] | select(.hostname==$h)' >/dev/null 2>&1
}

route_absent() {
  _host="${1:?}"
  ! route_listed "$_host"
}

# dig A records; first answer on stdout
dns_a() {
  _host="${1:?}"
  dig +short @"${DNS_SERVER}" -p "${DNS_PORT}" "${_host}" A | head -1 | tr -d '[:space:]'
}

dns_a_equals() {
  _host="${1:?}"
  _want="${2:?}"
  _got="$(dns_a "$_host")"
  [ "$_got" = "$_want" ]
}

dns_a_empty() {
  _host="${1:?}"
  _got="$(dns_a "$_host")"
  [ -z "$_got" ]
}

# HTTP GET via agent with Host header; body on stdout
proxy_get() {
  _host="${1:?}"
  _path="${2:-/}"
  curl -sf -H "Host: ${_host}" "${AGENT_PROXY}${_path}"
}

proxy_status() {
  _host="${1:?}"
  _path="${2:-/}"
  curl -s -o /dev/null -w "%{http_code}" -H "Host: ${_host}" "${AGENT_PROXY}${_path}"
}

wait_for_route() {
  _host="${1:?}"
  _timeout="${2:-30}"
  wait_until "$_timeout" route_listed "$_host" \
    || fail "route ${_host} not listed within ${_timeout}s"
}

wait_for_dns() {
  _host="${1:?}"
  _want="${2:?}"
  _timeout="${3:-30}"
  wait_until "$_timeout" dns_a_equals "$_host" "$_want" \
    || fail "DNS ${_host} != ${_want} within ${_timeout}s (got '$(dns_a "$_host")')"
}

wait_for_no_dns() {
  _host="${1:?}"
  _timeout="${2:-30}"
  wait_until "$_timeout" dns_a_empty "$_host" \
    || fail "DNS ${_host} still present within ${_timeout}s (got '$(dns_a "$_host")')"
}

wait_for_proxy_ok() {
  _host="${1:?}"
  _timeout="${2:-30}"
  wait_until "$_timeout" sh -c "curl -sf -H 'Host: ${_host}' '${AGENT_PROXY}/' >/dev/null" \
    || fail "proxy Host ${_host} not OK within ${_timeout}s"
}

# RFC 1035: response must echo question. dig prints QUERY:0 when broken (macOS curl hangs).
dns_response_has_question() {
  _host="${1:?}"
  _type="${2:-A}"
  _out=$(dig +time=2 +tries=1 @"${DNS_SERVER}" -p "${DNS_PORT}" "${_host}" "${_type}" 2>/dev/null || true)
  echo "$_out" | grep -q "status: NOERROR" || return 1
  # Accept QUERY: 1 (correct). Fail on QUERY: 0 (header-only / missing question).
  echo "$_out" | grep -E "QUERY: 1," >/dev/null 2>&1
}

dns_aaaa_nodata_well_formed() {
  _host="${1:?}"
  _out=$(dig +time=2 +tries=1 @"${DNS_SERVER}" -p "${DNS_PORT}" "${_host}" AAAA 2>/dev/null || true)
  echo "$_out" | grep -q "status: NOERROR" || return 1
  echo "$_out" | grep -E "QUERY: 1," >/dev/null 2>&1 || return 1
  # Must not be header-only (~12 bytes)
  _sz=$(echo "$_out" | sed -n 's/.*MSG SIZE  rcvd: \([0-9]*\).*/\1/p' | head -1)
  [ -n "$_sz" ] || return 1
  [ "$_sz" -ge 30 ]
}
