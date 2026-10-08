# 🖥️ Worker Nodes — Joining Machines to the Cluster

Akiro splits into two roles:

- **Leader (control plane + gateway):** one always-on node that exposes the HTTP/WebSocket API, holds the Redis broker, and hands out jobs. This is the box your frontend talks to.
- **Workers:** any number of machines that connect to the leader's Redis, pull submissions off the queue, execute them in the sandbox, and publish results back.

During a contest you scale execution capacity simply by **connecting more worker machines**. The leader coordinates; the workers do the heavy lifting. Each worker you add contributes its cores to the pool, so the more machines join, the shorter everyone's wait — especially the tail (the last person in a burst).

---

## ⚡ One-Line Join (Linux, macOS, Windows)

With Docker installed and running (Docker Engine on Linux, Docker Desktop on macOS/Windows), run this **one line** — identical in a Linux shell, macOS Terminal, Windows PowerShell or `cmd`:

```bash
docker run -d --name akiro-worker --privileged --restart unless-stopped --pull always ghcr.io/barunaniket/akiro:latest --mode worker --redis rediss://:<CLUSTER_TOKEN>@redis.172-198-71-80.sslip.io:443
```

Replace `<CLUSTER_TOKEN>` with the cluster token. That's the whole setup:

- `rediss://` connects over **TLS on standard port 443** (`redis.172-198-71-80.sslip.io:443`), bypassing strict university/corporate firewall port restrictions while keeping all communications encrypted in transit.
- Worker mode auto-tunes to the machine: one job slot per CPU core, a memory budget of ~75% of RAM (on macOS/Windows that is the RAM given to Docker Desktop — raise it in Docker Desktop → Settings → Resources), and no embedded Redis.
- `--pull always` fetches the right image for the CPU (amd64, or arm64 on Apple Silicon); `--restart unless-stopped` rejoins after reboots and network drops.

On Linux there is also a script that installs Docker first if it's missing:

```bash
curl -fsSL https://raw.githubusercontent.com/barunaniket/akiro/main/scripts/join-worker.sh | bash -s -- <CLUSTER_TOKEN>
```

The script:

1. **Checks Docker** is installed and running (offers to install it on Linux via `get.docker.com`; on macOS/Windows, start Docker Desktop first).
2. **Pulls** the public worker image `ghcr.io/barunaniket/akiro:latest` (falls back to a cached copy if the registry is unreachable).
3. **Auto-tunes to the machine** — one worker per CPU core (`nproc`), and a memory budget of ~75% of RAM. *(This budget is the single most important knob: left at its conservative default a 16-core box runs at ~12% utilisation; the script sets it for you so the machine is actually used.)*
4. **Starts** the worker with `--restart unless-stopped` and prints how to view logs / stop it.

Re-running the command just **replaces** the existing worker, so it's safe to run again.

---

## ✅ Verify It Connected

The worker logs should show it registered with the broker:

```bash
docker logs akiro-worker | grep "consumer started"
# → Redis concurrent consumer started: judge:jobs ... (capacity: N workers)
```

Or check the leader's health — `total_workers` should rise by the number this machine added:

```bash
curl -sk https://<leader-host>/health
# {"total_workers": 18, "idle_workers": 18, ...}
```

> Heartbeats have a ~20 s TTL, so after you stop a worker the count drops back within 20 seconds.

---

## 🔧 Managing a Worker

```bash
docker logs -f akiro-worker     # live logs
docker stats akiro-worker       # CPU / memory usage
docker rm -f akiro-worker       # disconnect + remove
```

The worker survives reboots (`--restart unless-stopped`). To leave the cluster, remove the container.

---

## ⚙️ Overrides

The token is the only required argument. Host and port default to the leader and can be overridden positionally; auto-detection can be overridden with env vars.

```bash
# custom leader host / port
... | bash -s -- <CLUSTER_TOKEN> <HOST> <PORT>

# pin worker count / memory budget / image explicitly
AKIRO_WORKERS=8 AKIRO_BUDGET=12g AKIRO_IMAGE=ghcr.io/barunaniket/akiro:latest \
  bash join-worker.sh <CLUSTER_TOKEN>
```

| Variable | Default | Meaning |
| :--- | :--- | :--- |
| `<CLUSTER_TOKEN>` (arg 1) | *(required)* | Cluster token; authenticates to the leader's Redis. |
| `<HOST>` (arg 2) | `172-198-71-80.sslip.io` | Leader host the worker connects to (must match the TLS certificate). |
| `<PORT>` (arg 3) | `6380` | Leader's TLS Redis front (stunnel → Redis `6379`). |
| `AKIRO_REDIS_SCHEME` | `rediss` | `redis` for plain TCP, e.g. through your own SSH tunnel. |
| `AKIRO_WORKERS` | `nproc` | Concurrent job slots (≈ cores). |
| `AKIRO_BUDGET` | ~75% RAM | `JUDGE_MEM_BUDGET_BYTES` — gates how many sandboxes run at once. Set close to available RAM to use all cores. |
| `AKIRO_IMAGE` | GHCR `:latest` | Worker image to run. |

---

## 🔒 Security Notes

- **The token is a shared secret.** Anyone holding it can join as a worker, which means they can **receive and read submitted source code** and **publish results — including forged verdicts**. Treat it like a password and only give it to machines you trust.
- **The token appears on the command line** (shell history, process list). Acceptable for trusted organiser machines; **rotate `CLUSTER_TOKEN` after each event.**
- **`curl | bash` runs code as root** (the worker needs a privileged container for sandbox isolation). Only run it on machines you control — don't hand this line to untrusted participants.
- **Expose only the TLS front.** Open `6380` (stunnel, TLS) in the Azure NSG — never the plaintext Redis port `6379`, which stays bound to the VM's localhost. If your workers have fixed IPs, restrict `6380` to them; the token protects the broker either way.
- **Timing differs between machines.** The same submission runs faster on a fast laptop than on the leader VM, so a solution near the time limit can pass on one worker and TLE on another. Use generous limits, or join machines of similar speed.

---

## 🧠 How Many Workers Do I Need?

Execution scales roughly linearly with worker machines. As a rough guide from load testing on a small leader:

- A **single well-tuned worker machine** (16 threads) alongside the 2-core leader cleared **20,000 heavy executions** with the last person answered in ~140 s under a 200-way simultaneous burst.
- For larger events, add more worker machines — the last-person wait drops roughly in proportion, and the leader stops being the straggler.

For a big hackathon, also raise the leader's queue and Redis limits (`JUDGE_MAX_QUEUE`, Redis `maxmemory`) — see the main deployment tuning table in the [README](../README.md).
