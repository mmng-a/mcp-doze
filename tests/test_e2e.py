"""End-to-end stdio checks; uses only the Python standard library and local fixtures."""

import argparse
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import threading
import time
import unittest


ROOT = Path(__file__).resolve().parent.parent
FIXTURE = Path(__file__).with_name("fixture_mcp.py")
BINARY = ROOT / "target/debug/mcp-doze"
INIT = {"protocolVersion": "2025-11-25", "capabilities": {"roots": {"listChanged": True}, "elicitation": {}},
        "clientInfo": {"name": "mcp-doze-e2e", "version": "1"}, "_meta": {"fixture/client": "keep"}}
HOOK = {"hook_event_name": "Stop", "session_id": "fixture-session", "turn_id": "fixture-turn"}


def group_members(pgid):
    members = []
    for line in subprocess.check_output(["ps", "-axo", "pid=,pgid="], text=True).splitlines():
        pid, group = map(int, line.split())
        if group == pgid:
            members.append(pid)
    return members


def until(predicate, timeout=6):
    deadline = time.monotonic() + timeout
    while not predicate():
        if time.monotonic() >= deadline:
            raise TimeoutError("condition timed out")
        time.sleep(.025)


class MCPError(Exception):
    pass


class Client:
    def __init__(self, command, environment, cwd):
        self.stderr_file = tempfile.TemporaryFile(mode="w+t")
        self.process = subprocess.Popen(command, env=environment, cwd=cwd, stdin=subprocess.PIPE,
            stdout=subprocess.PIPE, stderr=self.stderr_file, text=True, bufsize=1, start_new_session=True)
        self.condition = threading.Condition()
        self.writer = threading.Lock()
        self.responses = {}
        self.received = []
        self.parse_errors = []
        self.closed = False
        self.cleaned = False
        self.stderr_text = ""
        self.sequence = 0
        self.reader = threading.Thread(target=self.read, daemon=True)
        self.reader.start()

    def read(self):
        try:
            for line in self.process.stdout:
                try:
                    message = json.loads(line)
                except json.JSONDecodeError:
                    with self.condition:
                        self.parse_errors.append(line)
                        self.condition.notify_all()
                    continue
                with self.condition:
                    self.received.append(message)
                if "method" in message and "id" in message:
                    if message["method"] == "roots/list":
                        answer = {"roots": [{"uri": "file:///fixture", "name": "fixture"}]}
                    elif message["method"] == "elicitation/create":
                        answer = {"action": "accept", "content": {"name": "fixture answer"}}
                    elif message["method"] == "ping":
                        answer = {}
                    else:
                        answer = {"fixture": "answer"}
                    self.send({"jsonrpc": "2.0", "id": message["id"], "result": answer})
                elif "id" in message:
                    with self.condition:
                        self.responses[message["id"]] = message
                        self.condition.notify_all()
        finally:
            with self.condition:
                self.closed = True
                self.condition.notify_all()

    def send(self, message):
        with self.writer:
            try:
                self.process.stdin.write(json.dumps(message) + "\n")
                self.process.stdin.flush()
            except BrokenPipeError as error:
                raise MCPError("transport closed") from error

    def begin(self, method, params):
        self.sequence += 1
        request_id = self.sequence
        self.send({"jsonrpc": "2.0", "id": request_id, "method": method, "params": params})
        return request_id

    def wait(self, request_id, timeout=6):
        deadline = time.monotonic() + timeout
        with self.condition:
            while request_id not in self.responses:
                if self.parse_errors:
                    raise MCPError("invalid JSON-RPC received")
                if self.closed:
                    raise MCPError("transport closed: " + self.stderr())
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise TimeoutError("MCP request timed out")
                self.condition.wait(remaining)
            response = self.responses.pop(request_id)
        if "error" in response:
            raise MCPError(str(response["error"]))
        return response["result"]

    def call(self, method, params=None, timeout=6):
        return self.wait(self.begin(method, params or {}), timeout)

    def initialize(self):
        result = self.call("initialize", INIT, timeout=3)
        self.send({"jsonrpc": "2.0", "method": "notifications/initialized"})
        return result

    def tool(self, name, arguments=None, meta=None):
        params = {"name": name, "arguments": arguments or {}}
        if meta is not None:
            params["_meta"] = meta
        result = self.call("tools/call", params)
        if result.get("isError"):
            raise MCPError(str(result))
        return result

    def stderr(self):
        if self.stderr_file.closed:
            return self.stderr_text
        self.stderr_file.flush()
        self.stderr_file.seek(0)
        return self.stderr_file.read()

    def close(self):
        if self.cleaned:
            return
        if not self.process.stdin.closed:
            self.process.stdin.close()
        try:
            self.process.wait(timeout=4)
        except subprocess.TimeoutExpired:
            os.killpg(self.process.pid, signal.SIGKILL)
            self.process.wait(timeout=2)
        # This is a test-owned session created by start_new_session, never a user group.
        if group_members(self.process.pid):
            os.killpg(self.process.pid, signal.SIGKILL)
            until(lambda: not group_members(self.process.pid))
        self.reader.join(timeout=1)
        self.process.stdout.close()
        self.stderr_text = self.stderr()
        self.stderr_file.close()
        self.cleaned = True


def text_value(result):
    return json.loads(result["content"][0]["text"])


class EndToEnd(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="mcp-doze-e2e-")
        self.directory = Path(self.temporary.name)
        self.environment = dict(os.environ, MCP_DOZE_TEST_DIR=str(self.directory))
        self.cache_dir = self.directory / "cache"
        self.clients = []
        self.write_control()

    def tearDown(self):
        for client in self.clients:
            client.close()
        self.temporary.cleanup()

    def write_control(self, **values):
        (self.directory / "control.json").write_text(json.dumps(values))

    def events(self):
        p = self.directory / "events.jsonl"
        return [json.loads(line) for line in p.read_text().splitlines()] if p.exists() else []

    def starts(self):
        return [e for e in self.events() if e["event"] == "start"]

    def operations(self):
        return [e for e in self.events() if e["event"] == "operation"]

    def command(self, mode, flags=(), fixture=FIXTURE):
        return [str(BINARY), *mode, "--cache-dir", str(self.cache_dir), *flags,
                "--", sys.executable, str(fixture)]

    def refresh(self, flags=(), fixture=FIXTURE, expected_success=True):
        result = subprocess.run(self.command(["cache", "refresh"], flags, fixture), cwd=ROOT,
            env=self.environment, capture_output=True, text=True, timeout=12, start_new_session=True)
        if expected_success:
            self.assertEqual(result.returncode, 0, result.stderr)
        (self.directory / "events.jsonl").write_text("")
        return result

    def client(self, flags=(), fixture=FIXTURE):
        client = Client(self.command(["run"], flags, fixture), self.environment, ROOT)
        self.clients.append(client)
        return client

    def test_discovery_and_cleanup_stay_cold(self):
        self.refresh(["--repl-cleanup"])
        client = self.client(["--repl-cleanup"])
        init = client.initialize()
        self.assertEqual(init["instructions"], "Keep the fixture instructions intact.")
        self.assertEqual(init["_meta"], {"fixture/initialize": True})
        tools = client.call("tools/list")
        self.assertEqual(len(tools["tools"]), 8)
        self.assertFalse(tools.get("nextCursor"))
        self.assertEqual(client.call("resources/list")["resources"][0]["uri"], "fixture://one")
        self.assertEqual(len(client.call("resources/templates/list")["resourceTemplates"]), 1)
        self.assertEqual(len(client.call("prompts/list")["prompts"]), 1)
        self.assertEqual(client.call("ping"), {})
        self.assertEqual(client.tool("turn_ended", HOOK),
                         {"content": [{"type": "text", "text": "{}"}], "isError": False})
        self.assertEqual(self.starts(), [])
        self.assertEqual(self.operations(), [])
        for path in self.cache_dir.glob("*.json"):
            self.assertEqual(path.stat().st_mode & 0o777, 0o600)
        self.assertEqual(self.cache_dir.stat().st_mode & 0o777, 0o700)

    def test_state_isolation_reuse_and_negotiated_version(self):
        self.refresh()
        a, b = self.client(), self.client()
        negotiated = a.initialize()["protocolVersion"]
        b.initialize()
        first = a.tool("increment", {"delta": 41}, {"fixture/request": "unchanged"})
        second = a.tool("increment")
        other = b.tool("state")
        self.assertEqual(text_value(first)["value"], 41)
        self.assertEqual(text_value(second)["value"], 42)
        self.assertEqual(text_value(other)["value"], 0)
        self.assertEqual(text_value(first)["pid"], text_value(second)["pid"])
        self.assertNotEqual(text_value(first)["pid"], text_value(other)["pid"])
        self.assertEqual(second["_meta"], {"fixture/extension": {"kept": True}})
        inits = [e for e in self.events() if e["event"] == "initialize"]
        self.assertEqual(len(inits), 2)
        self.assertTrue(all(e["protocol"] == negotiated for e in inits))
        self.assertTrue(all(e["client_info"] == INIT["clientInfo"] for e in inits))
        self.assertTrue(all(e["capabilities"] == INIT["capabilities"] for e in inits))
        self.assertEqual(self.operations()[0]["meta"], {"fixture/request": "unchanged"})
        a.close()
        self.assertEqual(text_value(b.tool("increment"))["value"], 1)

    def test_duplex_media_extensions_and_resource_use(self):
        self.refresh()
        client = self.client()
        client.initialize()
        self.assertIn("resource contents", client.call("resources/read", {"uri": "fixture://one"})["contents"][0]["text"])
        result = text_value(client.tool("duplex", meta={"progressToken": "preserve-token"}))
        self.assertEqual(result["roots"]["roots"][0]["uri"], "file:///fixture")
        self.assertEqual(result["answer"]["content"]["name"], "fixture answer")
        notifications = [m for m in client.received if m.get("method") == "notifications/progress"]
        self.assertEqual(notifications[0]["params"]["progressToken"], "preserve-token")
        self.assertEqual(notifications[0]["params"]["_meta"], {"fixture/progress": "preserve"})
        media = client.tool("media")
        self.assertEqual([c["type"] for c in media["content"]], ["image", "audio"])
        self.assertEqual(len(media["structuredContent"]["value"]), 1048576)
        self.assertEqual(media["_meta"], {"fixture/unknown": [1, {"preserve": True}]})
        self.assertEqual(client.call("prompts/get", {"name": "fixture-prompt"})["messages"][0]["content"]["text"], "prompt contents")
        echo = client.call("fixture/unknown", {"_meta": {"keep": [1, 2, 3]}})
        self.assertEqual(echo["echo"]["params"]["_meta"], {"keep": [1, 2, 3]})
        self.assertEqual(len(self.starts()), 1)

    def test_cancel_during_initialization_does_not_execute(self):
        self.refresh()
        self.write_control(initialize_delay=.5)
        client = self.client()
        client.initialize()
        cancelled_id = client.begin("tools/call", {"name": "increment", "arguments": {"delta": 100}})
        until(lambda: len(self.starts()) == 1)
        client.send({"jsonrpc": "2.0", "method": "notifications/cancelled",
                     "params": {"requestId": cancelled_id, "reason": "fixture cancellation"}})
        self.assertEqual(text_value(client.tool("state"))["value"], 0)
        self.assertFalse(any(e.get("arguments", {}).get("delta") == 100 for e in self.operations()))

    def test_eof_and_term_reap_term_resistant_descendants(self):
        self.refresh()
        for cause in ["eof", "term"]:
            with self.subTest(cause=cause):
                client = self.client()
                client.initialize()
                child = text_value(client.tool("spawn_child"))["child_pid"]
                self.assertIn(child, group_members(client.process.pid))
                if cause == "eof":
                    client.process.stdin.close()
                else:
                    os.kill(client.process.pid, signal.SIGTERM)
                client.process.wait(timeout=5)
                until(lambda: not group_members(client.process.pid))

    def test_missing_corrupt_and_changed_launch_cache_fail(self):
        missing = self.client()
        with self.assertRaises(MCPError):
            missing.initialize()
        self.assertEqual(self.starts(), [])
        self.refresh()
        cache_files = list(self.cache_dir.glob("*.json"))
        self.assertTrue(cache_files)
        cache_files[0].write_text("broken JSON")
        corrupt = self.client()
        with self.assertRaises(MCPError):
            corrupt.initialize()
        self.assertEqual(self.starts(), [])
        copied = self.directory / "fixture-copy.py"
        copied.write_bytes(FIXTURE.read_bytes())
        self.refresh(fixture=copied)
        copied.write_bytes(copied.read_bytes() + b"\n# changed entrypoint\n")
        changed = self.client(fixture=copied)
        with self.assertRaises(MCPError):
            changed.initialize()
        self.assertEqual(self.starts(), [])

    def test_changed_live_catalog_never_executes_operation(self):
        self.refresh()
        self.write_control(catalog_version=2)
        client = self.client()
        client.initialize()
        with self.assertRaises(MCPError):
            client.tool("increment")
        self.assertEqual(len(self.starts()), 1)
        self.assertEqual(self.operations(), [])

    def test_backend_failure_never_restarts_or_replays(self):
        self.refresh()
        for name in ["crash", "malformed"]:
            with self.subTest(tool=name):
                (self.directory / "events.jsonl").write_text("")
                client = self.client()
                client.initialize()
                with self.assertRaises(MCPError):
                    client.tool(name)
                time.sleep(.1)
                self.assertEqual(len(self.starts()), 1)
                self.assertEqual([e["name"] for e in self.operations()], [name])
                self.assertEqual(client.parse_errors, [])
                client.close()

    def test_cleanup_is_explicit_and_invalid_schema_is_not_faked(self):
        self.refresh()
        client = self.client()
        client.initialize()
        client.tool("turn_ended", HOOK)
        self.assertEqual([e["name"] for e in self.operations()], ["turn_ended"])
        client.close()
        self.write_control(invalid_cleanup_schema=True)
        refreshed = self.refresh(["--repl-cleanup"], expected_success=False)
        if refreshed.returncode != 0:
            self.assertTrue(refreshed.stderr.strip())
            return
        client = self.client(["--repl-cleanup"])
        try:
            client.initialize()
            client.tool("turn_ended", HOOK)
        except MCPError:
            return
        self.assertTrue(self.starts(), "changed cleanup schema must not receive a fake cold acknowledgement")
        self.assertEqual([e["name"] for e in self.operations()], ["turn_ended"])


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=BINARY)
    args, remaining = parser.parse_known_args()
    BINARY = args.binary.resolve()
    unittest.main(argv=[sys.argv[0], *remaining])
