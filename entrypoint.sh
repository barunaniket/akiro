#!/bin/bash
set -e

# Enable required cgroup controllers for the judge subtree
CGROUP_ROOT="/sys/fs/cgroup"

# Ensure the judge cgroup parent exists
mkdir -p "$CGROUP_ROOT/judge" 2>/dev/null || true

# Try to enable memory, cpu, and pids controllers
if [ -f "$CGROUP_ROOT/cgroup.subtree_control" ]; then
    echo "+memory +cpu +pids" > "$CGROUP_ROOT/cgroup.subtree_control" 2>/dev/null || true
fi

# Block cloud metadata service IP (Azure / AWS / GCP) from container egress
iptables -A OUTPUT -d 169.254.169.254 -j DROP 2>/dev/null || true

# Start embedded Redis daemon if enabled (default: true)
ENABLE_REDIS="${ENABLE_EMBEDDED_REDIS:-true}"
if [ "$ENABLE_REDIS" = "true" ]; then
    REDIS_PASSWORD="${JUDGE_SECRET:-}"
    if [ -n "$REDIS_PASSWORD" ]; then
        redis-server --daemonize yes \
            --bind 0.0.0.0 \
            --port 6379 \
            --requirepass "$REDIS_PASSWORD" \
            --maxmemory 64mb \
            --maxmemory-policy allkeys-lru
        if [ -z "$JUDGE_REDIS" ]; then
            export JUDGE_REDIS="redis://:${REDIS_PASSWORD}@127.0.0.1:6379"
        fi
    else
        redis-server --daemonize yes \
            --bind 127.0.0.1 \
            --port 6379 \
            --maxmemory 64mb \
            --maxmemory-policy allkeys-lru
        if [ -z "$JUDGE_REDIS" ]; then
            export JUDGE_REDIS="redis://127.0.0.1:6379"
        fi
    fi
    echo " Embedded Redis daemon started on :6379 (maxmemory: 64MB) ✓"
fi

echo "----------------------------------------------"
echo " Akiro Sandbox starting..."
echo " Mode:    ${JUDGE_MODE:-all}"
echo " Port:    ${JUDGE_PORT:-8080}"
echo " Workers: ${JUDGE_WORKERS:-auto}"
echo " Redis:   ${JUDGE_REDIS:-none}"
echo "----------------------------------------------"

exec akiro "$@"