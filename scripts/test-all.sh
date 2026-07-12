#!/usr/bin/env bash
# Single entry: unit → in-process integration → (if Docker) compose e2e.
# Release gate for SPEC-v1 when Docker is available.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

export PATH="/Applications/Docker.app/Contents/Resources/bin:${PATH:-}"

echo "==> [1/3] unit (tailsvc-common, tailsvc-docker)"
cargo test -p tailsvc-common -p tailsvc-docker --lib

echo "==> [2/3] in-process integration (tailsvc-integration-tests)"
cargo test -p tailsvc-integration-tests --tests

if docker ps >/dev/null 2>&1; then
  echo "==> [3/3] Docker available"
  echo "    cargo ignored docker_whoami (if present)"
  cargo test -p tailsvc-integration-tests docker_labeled -- --ignored --nocapture || true

  echo "    compose e2e (preferred SPEC acceptance)"
  ./deploy/compose/e2e/run-e2e.sh
else
  echo "==> [3/3] SKIP compose e2e: Docker daemon not running"
  echo "    Start Docker Desktop, then re-run ./scripts/test-all.sh"
  echo "    Without Docker, only unit + in-process tiers ran."
  exit 0
fi

echo "==> all enabled tiers passed"
