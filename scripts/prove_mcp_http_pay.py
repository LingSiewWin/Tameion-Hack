#!/usr/bin/env python3
"""MCP over HTTP, end to end, with the standard library only.

    prove_mcp_http_pay.py <service-url> <policy> <payee> <amount> <token-file> [<operation-id>]

Talks to the service's POST /mcp (Streamable HTTP, MCP 2025-06-18): initialize,
notifications/initialized, tools/list, tools/call treasury_inbox, then tools/call
pay twice with one operation id (the second must answer with the first payment).
The agent key is read from the file and never printed: any occurrence in a reply
is shown as [token]. Without an operation id, a new one is made.
"""
import json
import sys
import urllib.error
import urllib.request
import uuid


def main():
    if len(sys.argv) not in (6, 7):
        sys.exit(__doc__)
    url, policy, payee, amount, token_file = sys.argv[1:6]
    op = sys.argv[6] if len(sys.argv) == 7 else f"op_{uuid.uuid4()}"
    with open(token_file) as f:
        token = f.read().strip()
    endpoint = url.rstrip("/") + "/mcp"
    ids = iter(range(1, 100))

    def post(msg, version=True):
        headers = {
            "Content-Type": "application/json",
            "Accept": "application/json, text/event-stream",
            "Authorization": f"Bearer {token}",
        }
        if version:
            headers["MCP-Protocol-Version"] = "2025-06-18"
        req = urllib.request.Request(endpoint, json.dumps(msg).encode(), headers, method="POST")
        try:
            with urllib.request.urlopen(req, timeout=180) as r:
                status, head, body = r.status, dict(r.headers), r.read().decode()
        except urllib.error.HTTPError as e:
            status, head, body = e.code, dict(e.headers), e.read().decode()
        return status, head, body.replace(token, "[token]")

    def rpc(method, params=None, version=True):
        msg = {"jsonrpc": "2.0", "id": next(ids), "method": method}
        if params is not None:
            msg["params"] = params
        status, head, body = post(msg, version)
        return status, head, json.loads(body) if body else None

    def show(title, v):
        print(f"$ {title}")
        print(json.dumps(v, indent=1, sort_keys=True))
        print()

    status, head, init = rpc(
        "initialize",
        {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "prove_mcp_http_pay.py", "version": "1"}},
        version=False,
    )
    sessions = [k for k in head if k.lower() == "mcp-session-id"]
    show(
        f"POST {endpoint} initialize -> HTTP {status}, Content-Type {head.get('Content-Type')}, Mcp-Session-Id {'present' if sessions else 'absent'}",
        {k: init["result"][k] for k in ("protocolVersion", "serverInfo", "capabilities")},
    )
    status, _, body = post({"jsonrpc": "2.0", "method": "notifications/initialized"})
    print(f"$ POST {endpoint} notifications/initialized -> HTTP {status}, body {body!r}\n")

    status, _, listed = rpc("tools/list")
    names = [t["name"] for t in listed["result"]["tools"]]
    pay_tool = next(t for t in listed["result"]["tools"] if t["name"] == "pay")
    show(
        f"tools/list -> HTTP {status}, {len(names)} tools",
        {"names": names, "pay.annotations": pay_tool["annotations"], "pay.required": pay_tool["inputSchema"]["required"]},
    )

    status, _, inbox = rpc("tools/call", {"name": "treasury_inbox", "arguments": {"policy": policy}})
    sc = inbox["result"]["structuredContent"]
    show(
        f"tools/call treasury_inbox -> HTTP {status}, isError {inbox['result']['isError']}",
        {
            "ok": sc.get("ok"),
            "contractVersion": sc.get("contractVersion"),
            "mode": sc.get("mode"),
            "caps": sc.get("caps"),
            "payees": sc.get("payees"),
            "waitingCount": sc.get("waitingCount"),
        },
    )

    args = {"policy": policy, "payee": payee, "amount": amount, "why": "S16 proof: one direct payment to the approved payee", "operationId": op}
    for n in (1, 2):
        status, _, paid = rpc("tools/call", {"name": "pay", "arguments": args})
        show(
            f"tools/call pay #{n} (operationId {op}) -> HTTP {status}, isError {paid['result']['isError']}",
            paid["result"]["structuredContent"],
        )


if __name__ == "__main__":
    main()
