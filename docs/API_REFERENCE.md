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
| `POST` | `/api/v1/submit` | Execute code against test cases synchronously | Yes (if secret set) |
| `GET` | `/api/v1/ws` | Real-time WebSocket streaming execution | Yes (if secret set) |

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

## 🚦 HTTP Status Codes & Error Responses

| Status Code | Reason | Description |
| :--- | :--- | :--- |
| **`200 OK`** | Success | Code was executed; full verdict results returned in body. |
| **`400 Bad Request`** | Invalid Request | Missing test cases, invalid JSON payload, or unsupported language. |
| **`401 Unauthorized`** | Auth Failure | Missing or invalid `X-Judge-Secret` / Bearer token. |
| **`403 Forbidden`** | Language Disabled | The requested language is excluded by the server's `ENABLED_LANGUAGES` whitelist. |
| **`503 Service Unavailable`** | Backpressure | Server queue is saturated (tokio backpressure buffer full). |

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
