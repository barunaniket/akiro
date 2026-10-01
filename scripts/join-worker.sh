#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# Akiro — one-line worker join.
#
#   curl -fsSL https://raw.githubusercontent.com/barunaniket/akiro/main/scripts/join-worker.sh | bash -s -- <CLUSTER_TOKEN>
#
# Optional positional args:  <CLUSTER_TOKEN> [HOST] [PORT]
# Optional env override:     AKIRO_IMAGE=<image>  AKIRO_WORKERS=<n>  AKIRO_BUDGET=<e.g. 12g>
#                            AKIRO_REDIS_SCHEME=redis   (plain TCP, e.g. through your own tunnel)
#
# Connects over TLS (rediss://) to the leader's stunnel front on :6380, so the token and the
# submissions it carries are encrypted in transit.
#
# Auto-detects cores + RAM and tunes the worker for this machine. Idempotent:
# re-running replaces the existing worker.
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

TOKEN="${1:-${CLUSTER_TOKEN:-}}"
HOST="${2:-172-198-71-80.sslip.io}"
PORT="${3:-6380}"
SCHEME="${AKIRO_REDIS_SCHEME:-rediss}"
IMAGE="${AKIRO_IMAGE:-ghcr.io/barunaniket/akiro:latest}"
NAME="akiro-worker"

say(){ printf '\033[1;36m==>\033[0m %s\n' "$*"; }
die(){ printf '\033[1;31mError:\033[0m %s\n' "$*" >&2; exit 1; }

[ -n "$TOKEN" ] || die "Missing cluster token.  Usage: ... | bash -s -- <CLUSTER_TOKEN>"

# 1. Ensure Docker is available (offer to install on Linux; Mac/Win need Docker Desktop).
if ! command -v docker >/dev/null 2>&1; then
  if [ "$(uname -s)" = "Linux" ]; then
    say "Docker not found — installing via get.docker.com (needs sudo)…"
    curl -fsSL https://get.docker.com | sh || die "Docker install failed; install Docker manually and re-run."
  else
    die "Docker not found. Install Docker Desktop, then re-run."
  fi
fi
docker info >/dev/null 2>&1 || die "Docker daemon not reachable. Start Docker (or add your user to the 'docker' group / use sudo)."

# 2. Auto-tune to this machine: 1 worker per core, memory budget ~75% of RAM.
WORKERS="${AKIRO_WORKERS:-$(nproc 2>/dev/null || echo 4)}"
if [ -n "${AKIRO_BUDGET:-}" ]; then
  BUDGET="$AKIRO_BUDGET"
else
  RAM_KB="$(awk '/MemTotal/{print $2}' /proc/meminfo 2>/dev/null || echo 4194304)"
  G="$(( RAM_KB * 3 / 4 / 1024 / 1024 ))"; [ "$G" -lt 1 ] && G=1
  BUDGET="${G}g"
fi

say "Pulling worker image ($IMAGE)…"
docker pull "$IMAGE" || docker image inspect "$IMAGE" >/dev/null 2>&1 || die "Could not pull $IMAGE and no local copy found."

say "Starting worker → ${SCHEME}://${HOST}:${PORT}   (${WORKERS} workers, ${BUDGET} memory budget)"
docker rm -f "$NAME" >/dev/null 2>&1 || true
docker run -d --name "$NAME" --privileged --restart unless-stopped \
  -e ENABLE_EMBEDDED_REDIS=false \
  -e JUDGE_MEM_BUDGET_BYTES="$BUDGET" \
  "$IMAGE" --mode worker --redis "${SCHEME}://:${TOKEN}@${HOST}:${PORT}" --workers "$WORKERS" >/dev/null

# 3. Confirm connection.
sleep 4
if docker logs "$NAME" 2>&1 | grep -q "consumer started"; then
  say "✓ Connected — this machine is now an Akiro judge worker (${WORKERS} workers)."
else
  say "Started, but connection not confirmed yet. If it doesn't pick up jobs, check the logs."
fi
echo "    live logs :  docker logs -f $NAME"
echo "    stop      :  docker rm -f $NAME"
