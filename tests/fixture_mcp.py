"""Local MCP fixture. All state and diagnostics stay in its temporary directory."""

import base64
import json
import os
from pathlib import Path
import subprocess
import sys
import time


DIRECTORY = Path(os.environ["MCP_DOZE_TEST_DIR"])
EVENTS = DIRECTORY / "events.jsonl"
CONTROL = DIRECTORY / "control.json"
VALUE = 0
NEXT_SERVER_ID = 0


def record(event, **fields):
    with EVENTS.open("a") as stream:
        stream.write(json.dumps({"event": event, "pid": os.getpid(), **fields}) + "\n")


def control():
    return json.loads(CONTROL.read_text()) if CONTROL.exists() else {}


def send(item):
    print(json.dumps(item, separators=(",", ":")), flush=True)


def result(request_id, value):
    send({"jsonrpc": "2.0", "id": request_id, "result": value})


def tool_result(request_id, value):
    result(request_id, {"content": [{"type": "text", "text": json.dumps(value)}],
                        "isError": False, "_meta": {"fixture/extension": {"kept": True}}})


def tools():
    names = ["increment", "state", "duplex", "media", "spawn_child", "crash", "malformed"]
    items = [{"name": name, "description": f"fixture {name} version {control().get('catalog_version', 1)}",
              "inputSchema": {"type": "object", "additionalProperties": True},
              "_meta": {"fixture/tool": name}} for name in names]
    schema = {"type": "object", "additionalProperties": False,
              "properties": {name: {"type": "string", "minLength": 1} for name in
                             ["hook_event_name", "session_id", "turn_id"]},
              "required": ["hook_event_name", "session_id", "turn_id"]}
    if control().get("invalid_cleanup_schema"):
        schema["required"] = ["hook_event_name"]
    items.append({"name": "turn_ended", "description": "fixture cleanup", "inputSchema": schema,
                  "annotations": {"idempotentHint": True}})
    return items


def client_request(method, params):
    global NEXT_SERVER_ID
    NEXT_SERVER_ID += 1
    request_id = "fixture-server-" + str(NEXT_SERVER_ID)
    send({"jsonrpc": "2.0", "id": request_id, "method": method, "params": params,
          "_meta": {"fixture/server-request": "preserve"}})
    for line in sys.stdin:
        message = json.loads(line)
        if message.get("id") == request_id and "method" not in message:
            record("client_response", method=method, result=message.get("result"))
            return message.get("result")
        if message.get("method") == "notifications/cancelled":
            record("cancelled", request_id=message.get("params", {}).get("requestId"))
    raise EOFError("client disconnected during reverse request")


def handle(request):
    global VALUE
    method = request.get("method")
    params = request.get("params", {})
    request_id = request.get("id")
    if method == "initialize":
        record("initialize", protocol=params.get("protocolVersion"),
               capabilities=params.get("capabilities"), client_info=params.get("clientInfo"),
               meta=params.get("_meta"))
        time.sleep(control().get("initialize_delay", 0))
        if error := control().get("initialize_error"):
            send({"jsonrpc": "2.0", "id": request_id, "error": error})
            return
        result(request_id, {"protocolVersion": params["protocolVersion"],
                            "capabilities": {"tools": {"listChanged": True},
                                             "resources": {"subscribe": True, "listChanged": True},
                                             "prompts": {"listChanged": True}},
                            "serverInfo": {"name": "mcp-doze-fixture", "version": "0.1.0"},
                            "instructions": "Keep the fixture instructions intact.",
                            "_meta": {"fixture/initialize": True}})
    elif method == "notifications/initialized":
        record("initialized")
    elif method == "tools/list":
        items = tools()
        cursor = params.get("cursor")
        if cursor:
            assert cursor == "page-two-" + str(os.getpid())
            result(request_id, {"tools": items[4:], "_meta": {"fixture/catalog": True}})
        else:
            result(request_id, {"tools": items[:4], "nextCursor": "page-two-" + str(os.getpid()),
                                "_meta": {"fixture/catalog": True}})
    elif method == "resources/list":
        result(request_id, {"resources": [{"uri": "fixture://one", "name": "one"}]})
    elif method == "resources/templates/list":
        result(request_id, {"resourceTemplates": [{"uriTemplate": "fixture://{id}", "name": "template"}]})
    elif method == "prompts/list":
        result(request_id, {"prompts": [{"name": "fixture-prompt", "description": "fixture prompt"}]})
    elif method == "resources/read":
        record("operation", name=method)
        result(request_id, {"contents": [{"uri": "fixture://one", "text": "resource contents"}]})
    elif method == "prompts/get":
        record("operation", name=method)
        result(request_id, {"messages": [{"role": "user", "content": {"type": "text", "text": "prompt contents"}}]})
    elif method == "ping":
        result(request_id, {})
    elif method == "notifications/cancelled":
        record("cancelled", request_id=params.get("requestId"))
    elif method == "tools/call":
        name = params["name"]
        args = params.get("arguments", {})
        record("operation", name=name, arguments=args, meta=params.get("_meta"))
        if name == "increment":
            VALUE += args.get("delta", 1)
            tool_result(request_id, {"value": VALUE, "pid": os.getpid()})
        elif name == "state":
            tool_result(request_id, {"value": VALUE, "pid": os.getpid()})
        elif name == "turn_ended":
            result(request_id, {"content": [{"type": "text", "text": "{}"}], "isError": False})
        elif name == "duplex":
            roots = client_request("roots/list", {})
            answer = client_request("elicitation/create", {"message": "fixture question",
                                      "requestedSchema": {"type": "object", "properties": {"name": {"type": "string"}}}})
            send({"jsonrpc": "2.0", "method": "notifications/progress", "params": {
                "progressToken": params.get("_meta", {}).get("progressToken", "fixture-progress"),
                "progress": 1, "total": 1, "_meta": {"fixture/progress": "preserve"}}})
            tool_result(request_id, {"roots": roots, "answer": answer})
        elif name == "media":
            image = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aJVQAAAAASUVORK5CYII="
            audio = base64.b64encode(b"RIFF" + b"\0" * 40).decode()
            result(request_id, {"content": [{"type": "image", "mimeType": "image/png", "data": image},
                                            {"type": "audio", "mimeType": "audio/wav", "data": audio}],
                                "structuredContent": {"value": "x" * 1048576},
                                "_meta": {"fixture/unknown": [1, {"preserve": True}]}})
        elif name == "spawn_child":
            child = subprocess.Popen([sys.executable, "-c",
                "import signal,time; signal.signal(signal.SIGTERM,signal.SIG_IGN); time.sleep(600)"],
                stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            time.sleep(.1)
            record("child", child_pid=child.pid)
            tool_result(request_id, {"child_pid": child.pid})
        elif name == "crash":
            os._exit(37)
        elif name == "malformed":
            print("this is not JSON-RPC", flush=True)
        else:
            send({"jsonrpc": "2.0", "id": request_id, "error": {"code": -32601, "message": "unknown fixture tool"}})
    elif request_id is not None:
        result(request_id, {"echo": request, "_meta": {"fixture/unknown-method": True}})


if __name__ == "__main__":
    record("start", ppid=os.getppid(), pgid=os.getpgrp())
    try:
        for line in sys.stdin:
            handle(json.loads(line))
        record("stdin_eof")
    except BrokenPipeError:
        pass
