"""Codex manifest wrapping preserves registration data and follows plugin updates."""
import argparse
import fcntl
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

BINARY = None


class CodexIntegration(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="mcp-doze-codex-")
        self.home = Path(self.temp.name)
        self.addCleanup(self.temp.cleanup)

    def manifest(self, version, servers):
        path = self.home / "plugins/cache/example/plugin" / version / ".mcp.json"
        path.parent.mkdir(parents=True)
        path.write_text(json.dumps({"mcpServers": servers, "unknown": {"preserved": True}}))
        return path

    def sync(self, success=True):
        result = subprocess.run([BINARY, "codex", "sync-plugins", "--codex-home", str(self.home)],
                                text=True, capture_output=True)
        self.assertEqual(result.returncode == 0, success, result.stderr)
        return json.loads(result.stdout) if success else result

    def test_wrap_preserves_metadata_is_idempotent_and_applies_to_updates(self):
        original = {"command": "node", "args": ["./server.mjs", "argument with spaces"],
                    "env": {"TOKEN": "test-only-token"}, "cwd": "./mcp", "authPolicy": "ON_USE",
                    "other": {"identity": "unchanged"}}
        http = {"url": "https://example.invalid/mcp", "auth": {"type": "oauth2"}}
        disabled = {"command": "node", "enabled": False}
        path = self.manifest("1.0.0", {"cua_repl": original, "http": http, "disabled": disabled})
        self.assertEqual(self.sync(), {"manifests": 1, "servers": 1})
        value = json.loads(path.read_text())
        wrapped = value["mcpServers"]["cua_repl"]
        self.assertEqual(wrapped["command"], BINARY)
        self.assertEqual(wrapped["args"], ["run", "--auto-refresh", "--timeout-ms", "30000",
                                            "--repl-cleanup", "--", "node", *original["args"]])
        self.assertEqual({k: v for k, v in wrapped.items() if k not in {"command", "args"}},
                         {k: v for k, v in original.items() if k not in {"command", "args"}})
        self.assertEqual(value["mcpServers"]["http"], http)
        self.assertEqual(value["mcpServers"]["disabled"], disabled)
        self.assertEqual(value["unknown"], {"preserved": True})
        before = path.read_bytes()
        self.assertEqual(self.sync(), {"manifests": 0, "servers": 0})
        self.assertEqual(path.read_bytes(), before)
        updated = self.manifest("1.0.1", {"data": {"command": "./new-launch", "args": []}})
        self.assertEqual(self.sync(), {"manifests": 1, "servers": 1})
        self.assertEqual(json.loads(updated.read_text())["mcpServers"]["data"]["args"][-1], "./new-launch")
        backups = self.home / "backups/mcp-doze/plugins"
        self.assertEqual(backups.stat().st_mode & 0o777, 0o700)
        for backup in backups.glob("*.json"):
            self.assertEqual(backup.stat().st_mode & 0o777, 0o600)
            record = json.loads(backup.read_text())
            self.assertIn(record["path"], [str(path), str(updated)])
            self.assertNotEqual(record["original"]["mcpServers"].get("data", {}).get("command"), BINARY)

    def test_existing_temporary_file_is_not_removed(self):
        path = self.manifest("1", {"data": {"command": "node"}})
        before = path.read_bytes()
        backups = self.home / "backups/mcp-doze/plugins"
        backups.mkdir(parents=True, mode=0o700)
        lock_path = backups / ".sync.lock"
        lock_path.touch(mode=0o600)
        with lock_path.open("r+") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            child = subprocess.Popen([BINARY, "codex", "sync-plugins", "--codex-home", str(self.home)],
                                     text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            existing = path.with_name(f".mcp-doze-{child.pid}.tmp")
            existing.write_text("belongs to another operation")
            fcntl.flock(lock, fcntl.LOCK_UN)
            _, error = child.communicate(timeout=10)
        self.assertNotEqual(child.returncode, 0, error)
        self.assertEqual(existing.read_text(), "belongs to another operation")
        self.assertEqual(path.read_bytes(), before)

    def test_symlink_manifest_is_rejected_without_modifying_other_files(self):
        regular = self.manifest("1", {"data": {"command": "node"}})
        before = regular.read_bytes()
        linked = self.home / "plugins/cache/example/plugin/2/.mcp.json"
        linked.parent.mkdir()
        target = self.home / "outside.json"
        target.write_bytes(before)
        linked.symlink_to(target)
        self.sync(success=False)
        self.assertEqual(regular.read_bytes(), before)
        self.assertEqual(target.read_bytes(), before)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True)
    args, remaining = parser.parse_known_args()
    BINARY = str(Path(args.binary).resolve())
    unittest.main(argv=[__file__, *remaining])
