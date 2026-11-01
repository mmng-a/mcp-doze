"""Auto-refresh checks using only test-owned servers and process groups."""

import argparse
import json
import os
from pathlib import Path
import signal
import sys
import time
import unittest

import test_e2e as support


FLAGS = ("--auto-refresh", "--timeout-ms", "15000")


class AutoRefresh(unittest.TestCase):
    def setUp(self):
        self.fixture = support.EndToEnd("test_discovery_and_cleanup_stay_cold")
        self.fixture.setUp()
        self.addCleanup(self.fixture.tearDown)

    def client(self, flags=(), fixture=support.FIXTURE):
        return self.fixture.client([*FLAGS, *flags], fixture=fixture)

    def initialize(self, client):
        result = client.call("initialize", support.INIT, timeout=20)
        client.send({"jsonrpc": "2.0", "method": "notifications/initialized"})
        return result

    def assert_collector_stopped(self):
        starts = self.fixture.starts()
        self.assertTrue(starts)
        for group in {event["pgid"] for event in starts}:
            self.assertEqual(support.group_members(group), [], "collector survived initialization response")

    def test_first_connection_collects_once_then_later_connections_stay_cold(self):
        first = self.client(["--repl-cleanup"])
        self.initialize(first)
        self.assertEqual(len(self.fixture.starts()), 1)
        self.assert_collector_stopped()
        self.assertEqual(len(first.call("tools/list")["tools"]), 8)
        first.tool("turn_ended", support.HOOK)
        self.assertEqual(self.fixture.operations(), [])
        self.assertEqual(len(self.fixture.starts()), 1)

        self.assertEqual(support.text_value(first.tool("increment", {"delta": 2}))["value"], 2)
        self.assertEqual(len(self.fixture.starts()), 2)
        first.close()
        warm = self.client(["--repl-cleanup"])
        self.initialize(warm)
        self.assertEqual(len(self.fixture.starts()), 2, "warm connection recollected the catalog")
        self.assertTrue(list(self.fixture.cache_dir.glob("*.json")))

    def test_changed_entrypoint_recollects_without_manual_refresh(self):
        copied = self.fixture.directory / "server.py"
        copied.write_bytes(support.FIXTURE.read_bytes())
        first = self.client(fixture=copied)
        self.initialize(first)
        self.assert_collector_stopped()
        first.close()
        copied.write_bytes(copied.read_bytes() + b"\n# a new server version\n")
        second = self.client(fixture=copied)
        self.initialize(second)
        self.assertEqual(len(self.fixture.starts()), 2)
        self.assert_collector_stopped()
        self.assertEqual(self.fixture.operations(), [])

    def test_simultaneous_connections_share_only_one_catalog_collection(self):
        self.fixture.write_control(initialize_delay=.3)
        a, b = self.client(), self.client()
        aid = a.begin("initialize", support.INIT)
        bid = b.begin("initialize", support.INIT)
        a.wait(aid, timeout=20)
        b.wait(bid, timeout=20)
        for client in [a, b]:
            client.send({"jsonrpc": "2.0", "method": "notifications/initialized"})
            self.assertEqual(len(client.call("tools/list")["tools"]), 8)
        self.assertEqual(len(self.fixture.starts()), 1)
        self.assert_collector_stopped()
        self.assertEqual(self.fixture.operations(), [])

    def test_failed_collection_is_reported_without_retry(self):
        failed = self.fixture.directory / "failed.py"
        failed.write_text(
            "import json,os,sys\n"
            "from pathlib import Path\n"
            "p=Path(os.environ['MCP_DOZE_TEST_DIR'])/'events.jsonl'\n"
            "with p.open('a') as f:f.write(json.dumps({'event':'start','pid':os.getpid(),'pgid':os.getpgrp()})+'\\n')\n"
            "for line in sys.stdin:\n"
            " request=json.loads(line)\n"
            " if request.get('method')=='initialize':\n"
            "  print(json.dumps({'jsonrpc':'2.0','id':request['id'],'error':{'code':-32001,'message':'fixture authorization required'}}),flush=True)\n"
            "  break\n"
        )
        client = self.client(fixture=failed)
        with self.assertRaises(support.MCPError) as caught:
            self.initialize(client)
        self.assertIn("fixture authorization required", str(caught.exception))
        client.process.wait(timeout=8)
        time.sleep(.1)
        self.assertEqual(len(self.fixture.starts()), 1)
        self.assertEqual(self.fixture.operations(), [])
        self.assertEqual(list(self.fixture.cache_dir.glob("*.json")), [])
        self.assert_collector_stopped()

    def test_bootstrap_disconnect_and_signals_recover_every_owned_group(self):
        hanging = self.fixture.directory / "hanging.py"
        hanging.write_text(
            "import json,os,subprocess,sys,time\n"
            "from pathlib import Path\n"
            "p=Path(os.environ['MCP_DOZE_TEST_DIR'])/'events.jsonl'\n"
            "def record(event,**values):\n"
            " with p.open('a') as f:f.write(json.dumps({'event':event,'pid':os.getpid(),'pgid':os.getpgrp(),**values})+'\\n')\n"
            "child=subprocess.Popen([sys.executable,'-c','import signal,time;signal.signal(signal.SIGTERM,signal.SIG_IGN);time.sleep(600)'],stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)\n"
            "time.sleep(.1)\n"
            "record('start',ppid=os.getppid())\n"
            "record('child',child_pid=child.pid)\n"
            "for line in sys.stdin:\n"
            " if json.loads(line).get('method')=='initialize':\n"
            "  record('initialize')\n"
            "  time.sleep(600)\n"
        )
        for cause in ["eof", "term", "kill"]:
            with self.subTest(cause=cause):
                (self.fixture.directory / "events.jsonl").write_text("")
                client = self.client(fixture=hanging)
                client.begin("initialize", support.INIT)
                groups = {client.process.pid}
                try:
                    support.until(lambda: any(e["event"] == "initialize" for e in self.fixture.events()))
                    groups.update(event["pgid"] for event in self.fixture.starts())
                    child = next(e["child_pid"] for e in self.fixture.events() if e["event"] == "child")
                    self.assertTrue(any(child in support.group_members(group) for group in groups))
                    if cause == "eof":
                        client.process.stdin.close()
                    else:
                        os.kill(client.process.pid, signal.SIGTERM if cause == "term" else signal.SIGKILL)
                    client.process.wait(timeout=8)
                    support.until(lambda: not any(support.group_members(group) for group in groups), timeout=10)
                    self.assertEqual(self.fixture.operations(), [])
                finally:
                    # These groups are created by this case's wrapper and fixture only.
                    client.process.poll()
                    groups.update(event["pgid"] for event in self.fixture.starts())
                    for group in groups:
                        if support.group_members(group):
                            os.killpg(group, signal.SIGKILL)
                    support.until(lambda: not any(support.group_members(group) for group in groups))
                    client.close()


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=support.BINARY)
    args, remaining = parser.parse_known_args()
    support.BINARY = args.binary.resolve()
    unittest.main(argv=[sys.argv[0], *remaining])
