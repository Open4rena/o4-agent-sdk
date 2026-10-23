"""A minimal MCP server for the `mcp_tools` example.

It speaks MCP over stdin/stdout, one JSON-RPC message per line, and offers a
single tool, `echo`, which returns its `text` argument. It uses only the
Python standard library and touches no files or network.
"""

import json
import sys

ECHO = {
    "name": "echo",
    "description": "Return the given text unchanged.",
    "inputSchema": {
        "type": "object",
        "properties": {"text": {"type": "string", "description": "Text to return"}},
        "required": ["text"],
    },
}


def answer(request):
    method = request.get("method")
    params = request.get("params") or {}
    if method == "initialize":
        return {
            "protocolVersion": params.get("protocolVersion", "2024-11-05"),
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "echo-example", "version": "1.0.0"},
        }
    if method == "tools/list":
        return {"tools": [ECHO]}
    if method == "tools/call" and params.get("name") == "echo":
        text = str((params.get("arguments") or {}).get("text", ""))
        return {"content": [{"type": "text", "text": text}]}
    if method == "ping":
        return {}
    return None


def main():
    for line in sys.stdin:
        if not line.strip():
            continue
        request = json.loads(line)
        if "id" not in request:
            # A notification; MCP expects no reply.
            continue
        result = answer(request)
        if result is None:
            reply = {
                "jsonrpc": "2.0",
                "id": request["id"],
                "error": {"code": -32601, "message": "method not found"},
            }
        else:
            reply = {"jsonrpc": "2.0", "id": request["id"], "result": result}
        sys.stdout.write(json.dumps(reply) + "\n")
        sys.stdout.flush()


if __name__ == "__main__":
    main()
