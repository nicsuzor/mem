import json
import os
import time
import urllib.error
import urllib.request

url = os.environ.get("PKB_MCP_URL")
if not url:
    print("PKB_MCP_URL not set")
    exit(1)

if not url.startswith("http"):
    url = f"http://{url}"
if not url.endswith("/mcp"):
    url = f"{url.rstrip('/')}/mcp"

PROTOCOL_VERSION = "2026-07-28"
SESSION_ID = os.environ.get("PKB_SESSION_ID", "mem-benchmark")


def protocol_meta():
    return {
        "io.modelcontextprotocol/protocolVersion": PROTOCOL_VERSION,
        "io.modelcontextprotocol/clientCapabilities": {},
        "io.modelcontextprotocol/clientInfo": {"name": "bench", "version": "1.0"},
    }


def post_mcp(method, params, tool_name=None, req_id=1):
    payload = {"jsonrpc": "2.0", "id": req_id, "method": method, "params": params}
    headers = {
        "Content-Type": "application/json",
        "Accept": "application/json, text/event-stream",
        "MCP-Protocol-Version": PROTOCOL_VERSION,
        "Mcp-Method": method,
    }
    if tool_name:
        headers["Mcp-Name"] = tool_name
    req = urllib.request.Request(
        url, data=json.dumps(payload).encode("utf-8"), headers=headers
    )
    start = time.time()
    try:
        with urllib.request.urlopen(req) as resp:
            resp.read()
            status = resp.status
    except urllib.error.HTTPError as e:
        status = e.code
    return (time.time() - start) * 1000, status


def bench(tool_name, args=None):
    call_args = dict(args or {})
    call_args.setdefault("session_id", SESSION_ID)
    try:
        duration, status = post_mcp(
            "tools/call",
            {
                "name": tool_name,
                "arguments": call_args,
                "_meta": protocol_meta(),
            },
            tool_name=tool_name,
            req_id=2,
        )
        print(f"{tool_name:20} -> {duration:>8.2f} ms (HTTP {status})")
    except Exception as e:
        print(f"{tool_name:20} -> Error: {e}")


print(f"Benchmarking {url} (MCP {PROTOCOL_VERSION})...")
print("-" * 45)

dur, status = post_mcp("server/discover", {"_meta": protocol_meta()}, req_id=1)
print(f"{'server/discover':20} -> {dur:>8.2f} ms (HTTP {status})")

bench("graph_stats")
bench("task_summary")
bench("list_tasks", {"limit": 10})
bench("pkb_orphans", {"limit": 10})

dur, status = post_mcp("tools/list", {"_meta": protocol_meta()}, req_id=3)
print(f"{'tools/list (direct)':20} -> {dur:>8.2f} ms (HTTP {status})")
print("-" * 45)
