import subprocess, json, time, sys

proc = subprocess.Popen(
    ["uvx", "mcp-gaokao-rank"],
    stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
    text=True, encoding="utf-8", bufsize=1
)

def send(obj):
    line = json.dumps(obj) + "\n"
    proc.stdin.write(line)
    proc.stdin.flush()

def read_response():
    while True:
        line = proc.stdout.readline()
        if not line:
            break
        line = line.strip()
        if not line:
            continue
        try:
            obj = json.loads(line)
            # Filter specifically for messages that look like responses to our requests
            # (or at least valid JSON objects)
            if isinstance(obj, dict):
                return obj
        except:
            pass

# initialize
send({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}})
r = read_response()
print("init:", json.dumps(r, ensure_ascii=False))

send({"jsonrpc":"2.0","method":"notifications/initialized","params":{}})

# list tools
send({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}})
r = read_response()
print("tools:", json.dumps(r, ensure_ascii=False))

# get_categories 河南 2024
send({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"get_categories","arguments":{"province":"河南","year":"2024"}}})
r = read_response()
print("categories 河南 2024:", json.dumps(r, ensure_ascii=False))

# get_categories 上海 2024
send({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"get_categories","arguments":{"province":"上海","year":"2024"}}})
r = read_response()
print("categories 上海 2024:", json.dumps(r, ensure_ascii=False))

# get_rank 河南 2024 理科 500
send({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"get_rank","arguments":{"province":"河南","year":"2024","category":"理科","score":500}}})
r = read_response()
print("rank 河南 2024 理科 500:", json.dumps(r, ensure_ascii=False))

proc.terminate()
