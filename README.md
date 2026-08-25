<div align="center">

# ⚡ Akiro

**High-Performance, Memory-Safe Distributed Code Execution Engine & Online Judge Sandbox**

[![Rust](https://img.shields.io/badge/Rust-2021_Edition-orange?logo=rust)](https://www.rust-lang.org/)
[![Docker Image](https://img.shields.io/badge/Container-ghcr.io%2Fbarunaniket%2Fakiro-blue?logo=docker)](https://github.com/barunaniket/akiro/pkgs/container/akiro)
[![License: MIT](https://img.shields.io/badge/License-MIT-green.svg)](LICENSE)
[![Cgroups v2](https://img.shields.io/badge/Isolation-Cgroups_v2_%2B_Seccomp_BPF-red)](https://man7.org/linux/man-pages/man7/cgroups.7.html)
[![Throughput](https://img.shields.io/badge/Throughput-76%2B_tests%2Fsec-success)](#-live-performance-benchmarks)

*An ultra-low overhead, production-ready online judge sandbox built in Rust. Designed to execute untrusted code securely with nanosecond-level kernel isolation, sub-millisecond execution overhead, adaptive backpressure defense, and horizontal multi-node cluster scaling via Redis Streams.*

</div>

---

## 📑 Table of Contents

- [Key Architecture & Safeguards](#-key-architecture--safeguards)
- [Quick Start](#-quick-start)
- [Supported Language Stack](#-supported-language-stack)
- [Distributed Cluster Mesh](#-distributed-cluster-mesh)
- [REST API & WebSocket Specification](#-rest-api--websocket-specification)
- [Live Performance Benchmarks](#-live-performance-benchmarks)
- [Documentation Index](#-documentation-index)

---

## 🛡️ Key Architecture & Safeguards

Akiro employs a multi-layered **Defense-in-Depth** kernel isolation model matching the security standards of IOI/ICPC contest environments:

```
                      +----------------------------------------+
                      |         Untrusted Submission           |
                      +----------------------------------------+
                                           |
                                           v
                      +----------------------------------------+
                      |         Axum REST / WS Gateway         |
                      |  - Dual-Token Auth (JUDGE_SECRET)      |
                      |  - DefaultBodyLimit: 2 MB              |
                      |  - Bounded Tokio Channel: 128 (503)    |
                      +----------------------------------------+
                                           |
                                           v
                      +----------------------------------------+
                      |     Distributed Redis Stream Queue     |
                      |  - Topic: judge:jobs (Consumer Groups) |
                      |  - Active 20s TTL Worker Heartbeats    |
                      +----------------------------------------+
                                           |
                                           v
                      +----------------------------------------+
                      |       Worker Pool Execution Mesh       |
                      |  - Adaptive Concurrency (CPU Cores)    |
                      +----------------------------------------+
                                           |
     +-------------------------------------+-------------------------------------+
     |                                     |                                     |
     v                                     v                                     v
+------------------------+    +------------------------+    +------------------------+
|    Linux Namespaces    |    |       Cgroups v2       |    |   Seccomp & RLIMITs    |
| - CLONE_NEWNET (No net)|    | - memory.max = 256MB   |    | - RLIMIT_FSIZE (10MB)  |
| - CLONE_NEWPID (Hidden)|    | - pids.max (anti-fork) |    | - RLIMIT_CPU (Hard kill|
| - CLONE_NEWNS  (Mount) |    | - cgroup.kill cleanup  |    | - MS_RDONLY Rootfs     |
| - pivot_root isolation |    | - memory.swap.max = 0  |    | - Strict Syscall Filter|
+------------------------+    +------------------------+    +------------------------+
```

### 1. Filesystem & Escape Isolation (`pivot_root` + `MS_RDONLY`)
- Per-submission isolated root filesystem created inside `/tmp/judge-fs/<uuid>`.
- `pivot_root` changes the real root to the sandbox directory, unmounting the host filesystem.
- The entire root is mounted read-only (`MS_RDONLY`); only a temporary in-memory `tmpfs` is writable for standard output and scratch files.

### 2. Network Isolation (`CLONE_NEWNET` & Egress Filter)
- Every submission executes in an unshared network namespace (`CLONE_NEWNET`) with no loopback interface (`lo` down). Socket creations and network egress fail instantly.
- Blocks access to cloud metadata endpoints (`169.254.169.254`).

### 3. Resource & Fork-Bomb Protection (`Cgroups v2` + `pids.max`)
- Sandboxed processes are placed in dedicated cgroups under `/sys/fs/cgroup/judge/<uuid>`.
- Strict memory caps (`memory.max = 256MB`, `memory.swap.max = 0`) trigger immediate kernel-level OOM killing on over-allocation.
- Thread/process count is strictly capped (`pids.max = 2` for C/C++/Python, `12` for Bun/Java), preventing fork-bomb DoS attacks.
- Atomic cgroup cleanup via `cgroup.kill` guarantees zero zombie or orphan child processes remain after execution.

### 4. Output Limit Enforcement (`RLIMIT_FSIZE`)
- Hard file size and output limit enforced via `RLIMIT_FSIZE` (default: 10 MB).
- Infinite output floods (`while(1) printf("A");`) trigger immediate `SIGXFSZ` and `RuntimeError` within 20ms.

---

## 🚀 Quick Start

### Option 1: Run Pre-Built Container via GHCR (Recommended)

#### Start as Standalone Gateway + Worker:
```bash
docker run -d --name akiro --privileged -p 8080:8080 --restart unless-stopped \
  ghcr.io/barunaniket/akiro:latest
```

#### Join as a Distributed Worker Node:
```bash
docker run -d --name akiro-worker --privileged --restart unless-stopped \
  ghcr.io/barunaniket/akiro:latest --mode worker \
  --redis "redis://:CLUSTER_TOKEN@LEADER_IP:6379" --workers auto
```

### Option 2: Build from Source

```bash
# Prerequisites: Rust 1.75+, Linux with Cgroups v2 enabled, libseccomp-dev
git clone https://github.com/barunaniket/akiro.git
cd akiro
cargo build --release
sudo ./target/release/akiro --mode all --port 8080
```

---

## 🌐 Supported Language Stack

All runtimes are pre-warmed and compiled with competitive programming optimizations:

| Language | Identifier | Compiler / Engine | Version | Optimizations Applied |
| :--- | :---: | :--- | :--- | :--- |
| **C++** | `cpp` | G++ | 12.2+ (C++20) | Precompiled `<bits/stdc++.h>` (PCH) + **AtCoder Library (ACL)** |
| **C** | `c` | GCC | 12.2+ | `-O3 -march=x86-64` |
| **Python** | `python` | CPython | 3.11+ | Precompiled standard library bytecode (`.pyc`) |
| **Java** | `java` | OpenJDK | 17 LTS | Pre-dumped **Java CDS (Class Data Sharing)** archive |
| **JavaScript** | `javascript` | Bun | 1.1+ | Fast V8-compatible runtime engine |
| **TypeScript** | `typescript` | Bun TS | 1.1+ | Native zero-transpile JIT execution |
| **SQL** | `sql` | SQLite3 | 3.40+ | In-memory relational query execution with CSV table output parsing |

---

## ⚡ Distributed Cluster Mesh

Akiro features a **Dual-Token Architecture** allowing effortless scaling from 1 machine to hundreds of distributed nodes:

```
[ Frontend / Next.js ] 
        | (HTTPS + JUDGE_SECRET)
        v
[ Akiro Leader Node (Azure) ] <--- Port 6379 (CLUSTER_TOKEN) ---> [ Laptop Worker 1 (8 Cores) ]
                               <--- Port 6379 (CLUSTER_TOKEN) ---> [ Laptop Worker 2 (16 Cores) ]
                               <--- Port 6379 (CLUSTER_TOKEN) ---> [ Cloud VM Worker N ... ]
```

* **`JUDGE_SECRET`**: Protects the public HTTP API (`POST /api/v1/submit` and `GET /health`).
* **`CLUSTER_TOKEN`**: Authenticates distributed worker nodes directly to the Redis Streams queue.
* **Active 20s TTL Heartbeats**: Connected worker nodes automatically report CPU capacity to `judge:heartbeat:<consumer>` without dropping when idle.

---

## 📡 REST API & WebSocket Specification

### Health Check (`GET /health`)
```bash
curl -s https://<HOST>/health
```
```json
{
  "total_workers": 18,
  "idle_workers": 18,
  "busy_workers": 0,
  "queued_jobs": 0,
  "uptime_secs": 3600
}
```

### Submit Code (`POST /api/v1/submit`)
```bash
curl -X POST https://<HOST>/api/v1/submit \
  -H "Content-Type: application/json" \
  -H "X-Judge-Secret: <JUDGE_SECRET>" \
  -d '{
    "job_id": "job-101",
    "language": "cpp",
    "time_limit_ms": 1000,
    "memory_limit_bytes": 134217728,
    "source_code": "#include <iostream>\nint main() { int a, b; if (std::cin >> a >> b) std::cout << a + b << std::endl; return 0; }",
    "test_cases": [
      {"input": "15 27\n", "expected_output": "42\n"},
      {"input": "100 200\n", "expected_output": "300\n"}
    ]
  }'
```

### Response Schema:
```json
{
  "job_id": "job-101",
  "verdict": "Accepted",
  "total_cpu_time_ms": 8,
  "peak_memory_kb": 6864,
  "compile_output": null,
  "test_results": [
    {
      "test_case_index": 0,
      "status": "Accepted",
      "cpu_time_ms": 4,
      "memory_kb": 6864
    },
    {
      "test_case_index": 1,
      "status": "Accepted",
      "cpu_time_ms": 4,
      "memory_kb": 6864
    }
  ]
}
```

### Verdict Definitions:
* `Accepted`: All test cases matched within resource limits.
* `WrongAnswer`: Standard output mismatched expected output.
* `TimeLimitExceeded`: Process exceeded CPU or wall-clock execution ceiling.
* `MemoryLimitExceeded`: Process breached cgroup memory threshold.
* `RuntimeError`: Non-zero exit code or fatal signal (`SIGSEGV`, `SIGFPE`, `SIGXFSZ`).
* `CompilationError`: Compiler syntax/linker failure (diagnostics returned in `compile_output`).

---

## 📊 Live Performance Benchmarks

### 1. 200-Submission Multi-Language Round-Robin Stress Test
*Evaluated across all 7 languages on an 18-worker distributed cluster:*

```
================================================================
  • Total Submissions:      200 jobs
  • Total Wall Time:        11.88 seconds
  • Throughput:             16.84 submissions / second
  • Success Rate:           200 / 200 (100.0% Accepted)
  • Mean Latency:           932.1 ms
  • Median Latency (p50):   579.7 ms
================================================================
```

### 2. Heavy 3,000-Sandbox Algorithmic Stress Test (100 Tests / Submission)
*Evaluated with Kadane's Dynamic Programming Algorithm (randomized arrays) with 100 test cases per job:*

```
================================================================
  • Heavy Submissions:      30 jobs
  • Total Test Cases Run:   3,000 isolated sandboxes
  • Total Wall Time:        39.23 seconds
  • Test Execution Rate:    76.47 testcases / second
  • Individual Test Pass:   3,000 / 3,000 (100.0% Passed)
  • Cluster Status:         Zero memory leaks, zero hanging queues
================================================================
```

---

## 📚 Documentation Index

Detailed architectural and operational documentation can be found in the [`docs/`](docs/) directory:

- 📖 [`docs/DUAL_TOKEN_ARCHITECTURE.md`](docs/DUAL_TOKEN_ARCHITECTURE.md): Cluster scaling, tokens, and volunteer joining guide.
- 📖 [`docs/HORIZONTAL_SCALING.md`](docs/HORIZONTAL_SCALING.md): Redis Streams worker mesh topology.
- 📖 [`docs/COLAB_SETUP.md`](docs/COLAB_SETUP.md): Google Colab GPU/CPU worker node integration.
- 📖 [`docs/PHASE2_IMPLEMENTATION.md`](docs/PHASE2_IMPLEMENTATION.md) – [`docs/PHASE5_IMPLEMENTATION.md`](docs/PHASE5_IMPLEMENTATION.md): Sandbox milestones and implementation chronicles.
- 📖 [`docs/VALIDATION_REPORT.md`](docs/VALIDATION_REPORT.md): Security audit and attack resilience verification.

---

## 📜 License

Distributed under the **MIT License**. See `LICENSE` for more information.

Developed with ❤️ by **[Aniket Barun](https://github.com/barunaniket)** for **CodeChef PESU-ECC Chapter**.
