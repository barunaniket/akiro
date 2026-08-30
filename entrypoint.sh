#!/bin/bash
set -e

# ── Enable cgroup v2 controller delegation for per-job memory/PID limits ──
# The sandbox now FAILS CLOSED: if these controllers are not delegated, every job is
# rejected rather than run without limits. So this must succeed on a healthy host.
#
# The subtlety the old best-effort version missed: cgroup v2's "no internal process"
# constraint forbids enabling controllers in a cgroup that still has member processes.
# Under a private cgroup namespace the container's PIDs sit in the namespace-root cgroup,
# so `echo +memory > cgroup.subtree_control` fails with EBUSY. Fix: move every process
# into a child "init" leaf first, then delegate controllers from the (now empty) root.
CGROUP_ROOT="/sys/fs/cgroup"

# Delegate controllers into a cgroup's subtree_control, most-capable first. memory is the
# only REQUIRED controller (the sandbox writes memory.max / pids.max); cpu is best-effort.
# Order matters on WSL2: the atomic "+memory +cpu +pids" fails wholesale because WSL2 rejects
# +cpu, AND delegating "+memory" ALONE also fails there — but "+memory +pids" TOGETHER works.
# So cascade: all three -> memory+pids -> per-controller.
delegate_controllers() {
    _sc="$1/cgroup.subtree_control"
    echo "+memory +cpu +pids" > "$_sc" 2>/dev/null && return 0
    echo "+memory +pids"      > "$_sc" 2>/dev/null && return 0
    for _ctrl in memory pids cpu; do
        echo "+$_ctrl" > "$_sc" 2>/dev/null || true
    done
}

enable_cgroup_delegation() {
    if [ ! -f "$CGROUP_ROOT/cgroup.controllers" ]; then
        echo "⚠  cgroup v2 unified hierarchy not found at $CGROUP_ROOT — per-job limits cannot be enforced."
        return 1
    fi

    # Vacate the root cgroup so controllers can be enabled (no-internal-process rule). Move
    # every pid into /init and RETRY until root is empty. TWO cgroupfs gotchas handled here:
    #   1. Do NOT gate on `[ -s cgroup.procs ]` — cgroupfs reports st_size 0 even when the file
    #      is populated, so `-s` is ALWAYS false and would skip the vacate entirely (this was a
    #      latent bug: on WSL2 the root was never emptied, so memory delegation always failed).
    #      Check the CONTENT (`[ -n "$(cat ...)" ]`) instead.
    #   2. Re-snapshot each pass: writing pids into /init mutates cgroup.procs, so a single
    #      streaming read leaves stragglers. Loop until empty (or give up after 10 tries).
    mkdir -p "$CGROUP_ROOT/init"
    _tries=0
    while [ -n "$(cat "$CGROUP_ROOT/cgroup.procs" 2>/dev/null)" ] && [ "$_tries" -lt 10 ]; do
        for _pid in $(cat "$CGROUP_ROOT/cgroup.procs"); do
            echo "$_pid" > "$CGROUP_ROOT/init/cgroup.procs" 2>/dev/null || true
        done
        _tries=$((_tries + 1))
    done

    delegate_controllers "$CGROUP_ROOT"
    if ! grep -qw memory "$CGROUP_ROOT/cgroup.subtree_control" 2>/dev/null; then
        echo "⚠  Failed to delegate the memory controller at $CGROUP_ROOT/cgroup.subtree_control."
        echo "   Run the container with --privileged (or --cgroupns=host). Jobs will be rejected."
        return 1
    fi
    mkdir -p "$CGROUP_ROOT/judge"
    delegate_controllers "$CGROUP_ROOT/judge"

    # Verify: the per-job cgroups live at /judge/<uuid>, so memory must be enabled in
    # /judge's *subtree_control* (governs children) — not merely present in its controllers.
    if grep -qw memory "$CGROUP_ROOT/judge/cgroup.subtree_control" 2>/dev/null; then
        echo "✓ cgroup v2 controllers (memory[,cpu],pids) delegated to /judge subtree"
        return 0
    fi
    echo "⚠  /judge/cgroup.subtree_control is missing the memory controller — per-job limits will NOT enforce; jobs will be rejected."
    return 1
}

enable_cgroup_delegation || echo "⚠  Continuing startup, but sandbox jobs will fail closed until cgroup delegation works."

# Block cloud metadata service IP (Azure / AWS / GCP) from container egress
iptables -A OUTPUT -d 169.254.169.254 -j DROP 2>/dev/null || true

# Sweep any stale per-job jail root dirs left by a previous run that was killed mid-job.
# On the happy path the judge parent rmdir's each one deterministically after the child
# exits; this only mops up crash leftovers so /tmp doesn't accrete empty directories.
rm -rf /tmp/judge_root_* 2>/dev/null || true

# Start embedded Redis daemon if enabled (default: true)
ENABLE_REDIS="${ENABLE_EMBEDDED_REDIS:-true}"
if [ "$ENABLE_REDIS" = "true" ]; then
    REDIS_PASSWORD="${CLUSTER_TOKEN:-${JUDGE_SECRET:-}}"
    if [ -n "$REDIS_PASSWORD" ]; then
        redis-server --daemonize yes \
            --bind 0.0.0.0 \
            --port 6379 \
            --requirepass "$REDIS_PASSWORD" \
            --maxmemory 64mb \
            --maxmemory-policy noeviction \
            --save "" --appendonly no
        if [ -z "$JUDGE_REDIS" ]; then
            export JUDGE_REDIS="redis://:${REDIS_PASSWORD}@127.0.0.1:6379"
        fi
    else
        redis-server --daemonize yes \
            --bind 127.0.0.1 \
            --port 6379 \
            --maxmemory 64mb \
            --maxmemory-policy noeviction \
            --save "" --appendonly no
        if [ -z "$JUDGE_REDIS" ]; then
            export JUDGE_REDIS="redis://127.0.0.1:6379"
        fi
    fi
    echo " Embedded Redis daemon started on :6379 (maxmemory: 64MB, noeviction) ✓"
fi

echo "----------------------------------------------"
echo " Akiro Sandbox starting..."
echo " Mode:    ${JUDGE_MODE:-all}"
echo " Port:    ${JUDGE_PORT:-8080}"
echo " Workers: ${JUDGE_WORKERS:-auto}"
echo " Redis:   ${JUDGE_REDIS:-none}"
echo " Memory:  budget ${JUDGE_MEM_BUDGET_BYTES:-768m} · job≤${JUDGE_MAX_MEMORY_BYTES:-512m} · compile≤${JUDGE_COMPILE_MEMORY_BYTES:-512m}"
echo "----------------------------------------------"

# If JUDGE_WORKERS is set to "auto", unset it so clap defaults to CPU core count
if [ "$JUDGE_WORKERS" = "auto" ]; then
    unset JUDGE_WORKERS
fi

exec akiro "$@"