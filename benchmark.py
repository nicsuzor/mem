import os
import time
import json
import urllib.request
import urllib.error

url = os.environ.get("PKB_MCP_URL")
if not url:
    print("PKB_MCP_URL not set")
    exit(1)

# Ensure the URL is just the base, e.g., http://host:port/mcp
if not url.startswith("http"):
    url = f"http://{url}"
if not url.endswith("/mcp"):
    url = f"{url.rstrip('/')}/mcp"

# Initialize Session
init_req_data = json.dumps({
    "jsonrpc": "2.0",
    "id": 1,
    "method": "initialize",
    "params": {
        "protocolVersion": "2024-11-05",
        "capabilities": {},
        "clientInfo": {"name": "bench", "version": "1.0"}
    }
}).encode("utf-8")

init_req = urllib.request.Request(url, data=init_req_data, headers={
    "Content-Type": "application/json",
    "Accept": "application/json, text/event-stream"
})

session_id = None
try:
    with urllib.request.urlopen(init_req) as response:
        session_id = response.getheader("Mcp-Session-Id")
        if not session_id:
            print("Failed to get Mcp-Session-Id")
            exit(1)
except Exception as e:
    print(f"Failed to initialize session: {e}")
    exit(1)

print(f"Session established: {session_id}")

# Send initialized notification
notif_req_data = json.dumps({
    "jsonrpc": "2.0",
    "method": "notifications/initialized"
}).encode("utf-8")

notif_req = urllib.request.Request(url, data=notif_req_data, headers={
    "Content-Type": "application/json",
    "Accept": "application/json, text/event-stream",
    "Mcp-Session-Id": session_id
})
try:
    urllib.request.urlopen(notif_req).read()
except Exception as e:
    pass

def bench(tool_name, args={}):
    req_data = json.dumps({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": tool_name,
            "arguments": args
        }
    }).encode("utf-8")
    
    req = urllib.request.Request(url, data=req_data, headers={
        "Content-Type": "application/json",
        "Accept": "application/json, text/event-stream",
        "Mcp-Session-Id": session_id
    })
    
    start_time = time.time()
    try:
        with urllib.request.urlopen(req) as response:
            response.read()
            status = response.status
    except urllib.error.HTTPError as e:
        status = e.code
    except Exception as e:
        print(f"{tool_name:20} -> Error: {e}")
        return
        
    duration = (time.time() - start_time) * 1000
    print(f"{tool_name:20} -> {duration:>8.2f} ms (HTTP {status})")

print(f"Benchmarking {url}...")
print("-" * 45)

bench("graph_stats")
bench("task_summary")
bench("list_tasks", {"limit": 10})
bench("pkb_orphans", {"limit": 10})

# Top level method test
req_data = json.dumps({"jsonrpc": "2.0", "id": 3, "method": "tools/list"}).encode("utf-8")
req = urllib.request.Request(url, data=req_data, headers={
    "Content-Type": "application/json",
    "Accept": "application/json, text/event-stream",
    "Mcp-Session-Id": session_id
})
start = time.time()
try:
    with urllib.request.urlopen(req) as res:
        res.read()
        status = res.status
    dur = (time.time() - start) * 1000
    print(f"{'tools/list (direct)':20} -> {dur:>8.2f} ms (HTTP {status})")
except Exception as e:
    pass

print("-" * 45)
