# 📡 Akiro REST & WebSocket API Reference

The **Akiro API** allows frontend platforms, contest management systems, and automated evaluation pipelines to submit code, receive structured verdicts, and monitor cluster health.

---

## 🔐 Authentication

If Akiro is launched with a `JUDGE_SECRET` configured, every HTTP request must include the secret token via either:

1. **Header (Recommended)**:
   ```http
   X-Judge-Secret: <YOUR_SECRET_TOKEN>
   ```
2. **Bearer Authorization Header**:
   ```http
   Authorization: Bearer <YOUR_SECRET_TOKEN>
   ```

---

## 📌 Endpoints Overview

| Method | Endpoint | Description | Auth Required |
| :--- | :--- | :--- | :---: |
| `GET` | `/health` | Cluster status, active workers, queue depth | Optional |
| `GET` | `/metrics` | Prometheus telemetry metrics | Optional |
| `POST` | `/api/v1/submit` | Execute code against test cases synchronously (blocks until verdict) | Yes (if secret set) |
| `POST` | `/api/v1/submit/async` | Enqueue a job and return its `job_id` immediately (`202`) | Yes (if secret set) |
| `GET` | `/api/v1/result/{job_id}` | Fetch a result: `200` done / `202` pending / `404` unknown | Yes (if secret set) |
| `GET` | `/api/v1/ws/result/{job_id}` | Push the final result over WebSocket the instant it's ready | Yes (if secret set) |
| `GET` | `/api/v1/ws/execute` | Real-time WebSocket streaming execution (local pool) | Yes (if secret set) |

---

## 1. Submit Code (`POST /api/v1/submit`)

Executes a single source code submission against an array of test cases inside isolated Cgroups v2 sandboxes.

### Request Headers
```http
Content-Type: application/json
X-Judge-Secret: <YOUR_SECRET_TOKEN>
```

### Request Body Schema

```json
{
  "job_id": "string",
  "language": "string",
  "source_code": "string",
  "time_limit_ms": 1000,
  "memory_limit_bytes": 134217728,
  "test_cases": [
    {
      "input": "string",
      "expected_output": "string"
    }
  ]
}
```

#### Field Details:
| Field | Type | Required | Description |
| :--- | :---: | :---: | :--- |
| `job_id` | `string` | Yes | Unique identifier for tracking the submission |
| `language` | `string` | Yes | Language identifier (e.g. `cpp`, `python`, `java`, `rust`, `go`, `pypy`, `kotlin`, `csharp`, `zig`, `ruby`, `php`, `haskell`, `dart`, `scala`, `javascript`, `typescript`, `sql`, `c`) |
| `source_code` | `string` | Yes | Raw source code string |
| `time_limit_ms` | `integer` | No | Per-test execution timeout in milliseconds (default: `1000`) |
| `memory_limit_bytes` | `integer` | No | Per-test memory ceiling in bytes (default: `134217728` = 128 MB) |
| `test_cases` | `array` | Yes | List of input and expected output pairs |

---

### Response Body Schema

```json
{
  "job_id": "test-job-001",
  "verdict": "Accepted",
  "total_cpu_time_ms": 12,
  "peak_memory_kb": 8420,
  "compile_output": null,
  "test_results": [
    {
      "test_case_index": 0,
      "status": "Accepted",
      "cpu_time_ms": 6,
      "memory_kb": 8420
    },
    {
      "test_case_index": 1,
      "status": "Accepted",
      "cpu_time_ms": 6,
      "memory_kb": 8100
    }
  ]
}
```

---

---

## 2. Async Submit & Result Retrieval (the "buzzer")

For high throughput, don't hold a connection open for the whole job. **Submit async, then fetch the result** by polling or WebSocket push. The gateway no longer polls Redis — a worker rings a "bell" (`PUBLISH`) the instant a result lands. *(Requires the Redis cluster; on a local-only instance these return `503`.)*

### 2a. `POST /api/v1/submit/async`
Same request body as `/api/v1/submit`. Returns immediately:

```json
// 202 Accepted
{ "job_id": "py-001", "status": "queued", "result_url": "/api/v1/result/py-001" }
```
- `409 Conflict` — a job with this `job_id` is already in flight. **Use a unique `job_id` per submission** (a UUID, or a monotonic id) — reusing an id that's still running is rejected.

### 2b. `GET /api/v1/result/{job_id}`
```json
// 200 OK  -> the full JobResult (same schema as the sync response)
// 202 Accepted -> { "job_id": "py-001", "status": "pending" }   (still running)
// 404 Not Found -> { "error": "unknown or expired job_id", "job_id": "py-001" }
```
Idempotent — safe to poll repeatedly. Results are retained for `JUDGE_RESULT_TTL_SECS` (default **1800s**); fetch within that window.

### 2c. `GET /api/v1/ws/result/{job_id}` (push)
Open a WebSocket; the server sends the final `JobResult` as one JSON text frame the moment it's ready, then closes. No polling. If the result already exists it's sent instantly; an unknown id gets an error frame and close.

**Example (submit async → WebSocket push):**
```javascript
await fetch(BASE + "/api/v1/submit/async", { method: "POST", headers, body });
const ws = new WebSocket(`ws://host:8080/api/v1/ws/result/${jobId}`);
ws.onmessage = (ev) => console.log("verdict:", JSON.parse(ev.data).verdict);
```

> The legacy `POST /api/v1/submit` still works unchanged (blocks, returns the full result) — it's just now event-driven internally, so it no longer polls either.

---

## 🚦 HTTP Status Codes & Error Responses

| Status Code | Reason | Description |
| :--- | :--- | :--- |
| **`200 OK`** | Success | Code was executed; full verdict results returned in body. |
| **`202 Accepted`** | Queued / Pending | Async job enqueued (`submit/async`), or result not ready yet (`GET /result`). |
| **`400 Bad Request`** | Invalid Request | Missing test cases, invalid JSON payload, or unsupported language. |
| **`401 Unauthorized`** | Auth Failure | Missing or invalid `X-Judge-Secret` / Bearer token. |
| **`403 Forbidden`** | Language Disabled | The requested language is excluded by the server's `ENABLED_LANGUAGES` whitelist. |
| **`404 Not Found`** | Unknown job | `job_id` not found, or its result TTL expired. |
| **`409 Conflict`** | Duplicate | A job with this `job_id` is already in flight (async submit). |
| **`503 Service Unavailable`** | Backpressure | Server queue saturated, or async endpoint used on a local-only (no-cluster) instance. |
| **`504 Gateway Timeout`** | Slow result | Sync `/submit` gave up waiting for the cluster; fetch later via `GET /result/{job_id}`. |

#### Example 403 Forbidden (Language Disabled by Whitelist):
```json
{
  "error": "Language 'ruby' is disabled on this judge instance",
  "enabled_languages": ["c", "cpp", "java", "python", "rust"]
}
```

---

## 🏆 Verdict Definitions

| Verdict | Meaning |
| :--- | :--- |
| **`Accepted`** | All test cases executed within time and memory limits and outputs matched exactly. |
| **`WrongAnswer`** | Process terminated normally, but stdout output did not match the expected output. |
| **`TimeLimitExceeded`** | Process exceeded the allocated CPU or wall-clock timeout ceiling. |
| **`MemoryLimitExceeded`**| Process breached cgroup memory threshold (`memory.max`). |
| **`RuntimeError`** | Non-zero exit code or fatal signal (`SIGSEGV`, `SIGFPE`, `SIGXFSZ` output bomb). |
| **`CompilationError`** | Compilation syntax or linker failure (diagnostics returned in `compile_output`). |

---

## 💻 Code Examples

### Python (Requests)
```python
import requests

url = "http://localhost:8080/api/v1/submit"
headers = {
    "Content-Type": "application/json",
    "X-Judge-Secret": "my-super-secret-token"
}
payload = {
    "job_id": "py-001",
    "language": "python",
    "time_limit_ms": 1000,
    "memory_limit_bytes": 64 * 1024 * 1024,
    "source_code": "import sys\na, b = map(int, sys.stdin.read().split())\nprint(a + b)",
    "test_cases": [
        {"input": "5 7\n", "expected_output": "12\n"},
        {"input": "100 250\n", "expected_output": "350\n"}
    ]
}

response = requests.post(url, json=payload, headers=headers)
print(response.json())
```

---

### JavaScript / TypeScript (Fetch)
```typescript
const submitCode = async () => {
  const response = await fetch("http://localhost:8080/api/v1/submit", {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      "X-Judge-Secret": "my-super-secret-token"
    },
    body: JSON.stringify({
      job_id: "ts-001",
      language: "typescript",
      time_limit_ms: 1000,
      memory_limit_bytes: 128 * 1024 * 1024,
      source_code: `
        import * as readline from "readline";
        const rl = readline.createInterface({ input: process.stdin });
        rl.on("line", (line) => {
          const [a, b] = line.trim().split(" ").map(Number);
          if (!isNaN(a) && !isNaN(b)) console.log(a + b);
        });
      `,
      test_cases: [
        { input: "10 20\n", expected_output: "30\n" }
      ]
    })
  });

  const result = await response.json();
  console.log("Verdict:", result.verdict);
};
```

---

### cURL
```bash
curl -X POST http://localhost:8080/api/v1/submit \
  -H "Content-Type: application/json" \
  -H "X-Judge-Secret: my-super-secret-token" \
  -d '{
    "job_id": "cpp-001",
    "language": "cpp",
    "source_code": "#include <iostream>\nint main() { int a, b; if (std::cin >> a >> b) std::cout << a * b << std::endl; return 0; }",
    "test_cases": [
      {"input": "6 7\n", "expected_output": "42\n"}
    ]
  }'
```
