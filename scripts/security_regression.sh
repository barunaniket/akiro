#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# Akiro — security & isolation regression test.
#
# Boots the built image in a hardened, resource-capped container and asserts the
# containment properties the whole project exists to guarantee:
#   • untrusted code cannot reach the network, the host filesystem, or fork-bomb
#   • resource abuse is classified (MLE / TLE / OLE), never crashes the judge
#   • the API rejects malformed / oversized / unauthorized requests
#   • the control plane (/health) stays responsive and the judge never dies
#
# These properties are enforced by the kernel (cgroups v2 + seccomp + namespaces)
# and CANNOT be exercised by `cargo test` in ordinary CI (which is unprivileged),
# so this container test is the real regression guard. Run it before every release.
#
# Usage:   scripts/security_regression.sh [IMAGE] [CPUS] [MEM]
#   IMAGE  docker image to test          (default: akiro:latest)
#   CPUS   vCPU cap, emulates the VM      (default: 2)
#   MEM    memory cap, emulates the VM    (default: 1g)
#
# Exit code 0 = all assertions passed; non-zero = at least one regression.
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail

IMAGE="${1:-akiro:latest}"
CPUS="${2:-2}"
MEM="${3:-1g}"
NAME="akiro-sectest-$$"
PORT="${SECTEST_PORT:-18099}"
SECRET="sectest-$(date +%s)"
DOCKER=(docker)
# Honor an explicit context (this repo builds on the native engine, not Docker Desktop).
[ -n "${DOCKER_CONTEXT_OVERRIDE:-}" ] && DOCKER=(docker --context "$DOCKER_CONTEXT_OVERRIDE")

cleanup() { "${DOCKER[@]}" rm -f "$NAME" >/dev/null 2>&1 || true; }
trap cleanup EXIT

echo "== Akiro security regression =="
echo "   image=$IMAGE  cpus=$CPUS  mem=$MEM  port=$PORT"

"${DOCKER[@]}" rm -f "$NAME" >/dev/null 2>&1 || true
"${DOCKER[@]}" run -d --name "$NAME" --privileged \
  --cpus "$CPUS" --memory "$MEM" --memory-swap "$MEM" \
  -p "$PORT:8080" \
  -e JUDGE_SECRET="$SECRET" -e JUDGE_REQUIRE_AUTH=1 -e JUDGE_MODE=all \
  -e JUDGE_WORKERS="$CPUS" -e JUDGE_MAX_QUEUE=128 \
  -e JUDGE_MEM_BUDGET_BYTES=768m -e JUDGE_MAX_MEMORY_BYTES=512m -e JUDGE_COMPILE_MEMORY_BYTES=512m \
  "$IMAGE" --mode all --workers "$CPUS" >/dev/null || { echo "FATAL: container failed to start"; exit 2; }

# Wait for health (up to 60s).
for i in $(seq 1 60); do
  curl -sf -m 3 "http://localhost:$PORT/health" >/dev/null 2>&1 && break
  sleep 1
  [ "$i" = 60 ] && { echo "FATAL: health never came up"; "${DOCKER[@]}" logs "$NAME" 2>&1 | tail -20; exit 2; }
done
echo "   container healthy, running assertions…"
echo

BASE="http://localhost:$PORT" SECRET="$SECRET" python3 - <<'PY'
import json, os, sys, urllib.request, urllib.error
BASE=os.environ["BASE"]; SECRET=os.environ["SECRET"]
passed=failed=0

def call(path, body=None, secret=SECRET, raw=None, timeout=60):
    data = raw if raw is not None else (json.dumps(body).encode() if body is not None else None)
    h={"content-type":"application/json"}
    if secret is not None: h["X-Judge-Secret"]=secret
    req=urllib.request.Request(BASE+path, data=data, headers=h)
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            return r.status, r.read()
    except urllib.error.HTTPError as e:
        return e.code, e.read()
    except Exception as e:
        return None, str(e).encode()

def submit(lang, src, inp="", exp="X", tl=2000, ml=None, secret=SECRET):
    b={"language":lang,"source_code":src,"test_cases":[{"input":inp,"expected_output":exp}],"time_limit_ms":tl}
    if ml: b["memory_limit_bytes"]=ml
    st,body=call("/api/v1/submit", b, secret=secret)
    try: j=json.loads(body)
    except Exception: j=None
    return st, j

def verdict(j):
    return j.get("verdict") if isinstance(j,dict) else None
def stdout_of(j):
    try: return bytes(j["test_results"][0]["stdout"]).decode(errors="replace")
    except Exception: return ""

def check(name, ok, detail=""):
    global passed, failed
    if ok: passed+=1; print(f"  PASS  {name}  {detail}")
    else:  failed+=1; print(f"  FAIL  {name}  {detail}")

# 1. Baseline correctness
st,j=submit("python","print(int(input())+1)","41","42")
check("correctness/python", verdict(j)=="Accepted" and stdout_of(j).strip()=="42", f"verdict={verdict(j)}")

# 2. Network egress severed
netsrc=("import socket\n"
        "try:\n s=socket.socket();s.settimeout(2);s.connect(('1.1.1.1',80));print('NET_OK')\n"
        "except Exception:\n print('NET_BLOCKED')")
st,j=submit("python",netsrc,"","NET_BLOCKED")
check("isolation/network_severed", verdict(j)=="Accepted" and "NET_BLOCKED" in stdout_of(j), f"out={stdout_of(j).strip()!r}")

# 3. Host filesystem not reachable (no /etc/shadow in jail)
fssrc=("try:\n open('/etc/shadow').read(); print('FS_OPEN')\n"
       "except Exception as e:\n print('FS_BLOCKED')")
st,j=submit("python",fssrc,"","FS_BLOCKED")
check("isolation/filesystem_jailed", verdict(j)=="Accepted" and "FS_BLOCKED" in stdout_of(j), f"out={stdout_of(j).strip()!r}")

# 4. Fork bomb contained (must not crash judge; verdict is returned)
st,j=submit("python","import os\nwhile True:\n try: os.fork()\n except: break",tl=2000)
check("isolation/fork_bomb_contained", st==200 and verdict(j) is not None, f"verdict={verdict(j)}")

# 5. Memory bomb -> MLE
st,j=submit("python","a=bytearray(2_000_000_000)\nprint(len(a))",ml=256*1024*1024,tl=3000)
check("limits/memory_bomb_MLE", verdict(j)=="MemoryLimitExceeded", f"verdict={verdict(j)}")

# 6. Output flood -> TLE or OLE
st,j=submit("python","import sys\nwhile True: sys.stdout.write('A'*8192)",tl=2000)
check("limits/output_flood_bounded", verdict(j) in ("TimeLimitExceeded","OutputLimitExceeded"), f"verdict={verdict(j)}")

# 7. CPU spin -> TLE
st,j=submit("cpp","int main(){while(1){}}",tl=1000)
check("limits/cpu_spin_TLE", verdict(j)=="TimeLimitExceeded", f"verdict={verdict(j)}")

# 8. Oversized body (>2MB) -> 413
st,_=call("/api/v1/submit", {"language":"python","source_code":"print(1)",
    "test_cases":[{"input":"x"*(3*1024*1024),"expected_output":"1"}]})
check("api/oversized_body_rejected", st==413, f"status={st}")

# 9. Too many test cases -> 400
st,_=call("/api/v1/submit", {"language":"python","source_code":"print(1)",
    "test_cases":[{"input":"1","expected_output":"1"}]*500})
check("api/too_many_cases_rejected", st==400, f"status={st}")

# 10. Malformed JSON -> 400
st,_=call("/api/v1/submit", raw=b"{not valid json")
check("api/malformed_json_rejected", st==400, f"status={st}")

# 11. Wrong secret -> 401
st,j=submit("python","print(1)","1","1",secret="wrong-secret")
check("api/wrong_secret_rejected", st==401, f"status={st}")

# 12. Missing secret -> 401
st,_=call("/api/v1/submit", {"language":"python","source_code":"print(1)",
    "test_cases":[{"input":"1","expected_output":"1"}]}, secret=None)
check("api/missing_secret_rejected", st==401, f"status={st}")

# 13. Unknown language -> 400
st,j=submit("brainfuck","+++","","")
check("api/unknown_language_rejected", st==400, f"status={st}")

# 14. Control plane still alive after all abuse
st,body=call("/health", secret=None, timeout=5)
alive = st==200
try: alive = alive and json.loads(body).get("total_workers",0)>=1
except Exception: alive=False
check("liveness/health_after_abuse", alive, f"status={st}")

print()
print(f"== {passed} passed, {failed} failed ==")
sys.exit(1 if failed else 0)
PY
rc=$?

# 15. The judge process must not have restarted (proves no crash under abuse).
restarts=$("${DOCKER[@]}" inspect "$NAME" --format '{{.RestartCount}}' 2>/dev/null || echo "?")
state=$("${DOCKER[@]}" inspect "$NAME" --format '{{.State.Status}}' 2>/dev/null || echo "?")
echo "  judge process: state=$state restarts=$restarts (OOMKilled flag reflects per-job cgroup kills, expected)"
if [ "$restarts" != "0" ] || [ "$state" != "running" ]; then
  echo "  FAIL  liveness/no_restart  state=$state restarts=$restarts"
  rc=1
fi

echo
if [ "$rc" = 0 ]; then echo "RESULT: PASS — isolation & API guarantees intact"; else echo "RESULT: FAIL — a security/isolation regression was detected"; fi
exit "$rc"
