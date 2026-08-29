# 🚀 Akiro Quickstart Guide

This guide walks you through building, launching, and verifying the **Akiro Code Execution Engine** on your machine in under 2 minutes.

---

## 📋 Prerequisites

* **Docker**: Docker Engine 24.0+ or Docker Desktop with Linux containers enabled.
* **Operating System**: Linux (Ubuntu, Debian, Fedora, Arch) or Windows (WSL2) or macOS (Apple Silicon M1/M2/M3/M4 or Intel).
* **Root / Sudo privileges**: Required for container namespace isolation (`--privileged` or `CAP_SYS_ADMIN`).

---

## ⚡ 1. Clone and Build

Akiro compiles natively on your host machine to ensure binary instructions are tuned specifically for your CPU architecture (AMD64 / ARM64):

```bash
# 1. Clone the repository
git clone https://github.com/barunaniket/akiro.git
cd akiro

# 2. Build the container image locally
docker build -t akiro .
```

*Note: The initial build downloads and caches all 18 language toolchains (GCC, OpenJDK, Rust, Go, Python, PyPy, Kotlin, Bun, Haskell GHC, Scala, Dart, Mono, Zig, Ruby, PHP, SQLite). Subsequent builds take only seconds.*

---

## 🏃 2. Run Akiro

### Mode A: Standalone Mode (API Gateway + Local Workers)
Runs the HTTP REST API on port `8080` and handles submissions locally:

```bash
docker run -d \
  --name akiro \
  --privileged \
  -p 8080:8080 \
  --restart unless-stopped \
  akiro
```

### Mode B: Secure Production Mode (With Authentication Secret)
Sets a secret token (`X-Judge-Secret`) to prevent unauthorized API requests:

```bash
docker run -d \
  --name akiro \
  --privileged \
  -p 8080:8080 \
  -e JUDGE_SECRET="my-super-secret-token" \
  --restart unless-stopped \
  akiro
```

### Mode C: Custom Language Whitelist Mode
Restricts the judge to only accept specific permitted languages:

```bash
docker run -d \
  --name akiro \
  --privileged \
  -p 8080:8080 \
  -e ENABLED_LANGUAGES="cpp,python,java,rust" \
  --restart unless-stopped \
  akiro
```

---

## 🧪 3. Verify Your Setup

### Step 1: Health Check
Run a health query against the engine:

```bash
curl -s http://localhost:8080/health
```

**Expected JSON Response:**
```json
{
  "total_workers": 8,
  "idle_workers": 8,
  "busy_workers": 0,
  "queued_jobs": 0,
  "uptime_secs": 15
}
```

---

### Step 2: Submit a Test Job (C++)
Submit a sample C++ program computing the sum of two integers:

```bash
curl -X POST http://localhost:8080/api/v1/submit \
  -H "Content-Type: application/json" \
  -d '{
    "job_id": "test-job-001",
    "language": "cpp",
    "time_limit_ms": 1000,
    "memory_limit_bytes": 134217728,
    "source_code": "#include <iostream>\nint main() { int a, b; if (std::cin >> a >> b) std::cout << (a + b) << std::endl; return 0; }",
    "test_cases": [
      {"input": "10 20\n", "expected_output": "30\n"},
      {"input": "123 456\n", "expected_output": "579\n"}
    ]
  }'
```

**Expected Verdict:**
```json
{
  "job_id": "test-job-001",
  "verdict": "Accepted",
  "total_cpu_time_ms": 4,
  "peak_memory_kb": 6864,
  "compile_output": null,
  "test_results": [
    {"test_case_index": 0, "status": "Accepted", "cpu_time_ms": 2, "memory_kb": 6864},
    {"test_case_index": 1, "status": "Accepted", "cpu_time_ms": 2, "memory_kb": 6864}
  ]
}
```

---

## ⚙️ Configuration & CLI Options

Akiro can be customized via command-line flags or environment variables:

| Argument | Environment Variable | Default | Description |
| :--- | :--- | :--- | :--- |
| `--mode <MODE>` | `JUDGE_MODE` | `all` | `all` (Gateway + Worker), `gateway` (API only), `worker` (Execution only) |
| `--port <PORT>` | `JUDGE_PORT` | `8080` | TCP port for the REST and WebSocket gateway |
| `--workers <NUM>`| `JUDGE_WORKERS`| `auto` | Number of parallel execution workers (default: matching CPU cores) |
| `--secret <TOKEN>`| `JUDGE_SECRET` | `""` | Dual-token authentication token for HTTP requests (`X-Judge-Secret`) |
| `--languages <LIST>`| `ENABLED_LANGUAGES`| `""` (all 18) | Comma-separated language whitelist (e.g. `cpp,python,java,rust`) |
| `--redis <URL>` | `REDIS_URL` | `""` | Distributed Redis Streams endpoint for cluster mesh worker scaling |
| `--cluster-token <TOKEN>` | `CLUSTER_TOKEN` | `""` | Redis auth password for distributed worker nodes |

---

## 🛠️ Stopping & Restarting

```bash
# View live container logs
docker logs -f akiro

# Stop the container
docker stop akiro

# Restart the container
docker start akiro

# Remove the container
docker rm -f akiro
```
