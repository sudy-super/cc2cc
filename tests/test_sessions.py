from __future__ import annotations

import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from uuid import uuid4

STAMP = "2026-10-16T00:00:00Z"
BINARY = os.environ.get(
    "CC2CC_BIN", str(Path(__file__).resolve().parents[1] / "target/debug/cc2cc")
)


class SessionTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.claude = self.root / "claude"
        self.claw = self.root / "claw"
        self.env = dict(
            os.environ,
            CLAUDE_CONFIG_DIR=str(self.claude),
            CLAWCODEX_CONFIG_DIR=str(self.claw),
        )

    def tearDown(self):
        self.temporary.cleanup()

    def run_cli(self, *arguments, success=True):
        result = subprocess.run(
            [BINARY, *arguments],
            cwd=self.root,
            env=self.env,
            capture_output=True,
            text=True,
            timeout=15,
        )
        if success:
            self.assertEqual(result.returncode, 0, result.stderr)
            return json.loads(result.stdout)
        self.assertNotEqual(result.returncode, 0)
        return result

    def source(self, tool, sid=None):
        sid = sid or str(uuid4())
        messages = [
            dict(
                type="user",
                role="user",
                uuid="u1",
                timestamp=STAMP,
                content="synthetic question",
            ),
            dict(
                type="assistant",
                role="assistant",
                uuid="a1",
                timestamp=STAMP,
                content=[{"type": "text", "text": "synthetic answer"}],
            ),
        ]
        if tool == "clawcodex":
            path = self.claw / "sessions" / f"{sid}.json"
            payload = {
                "session_id": sid,
                "cwd": "/hoge/fuga",
                "provider": "synthetic",
                "custom_state": {"keep": True},
                "conversation": {"messages": messages, "max_history": 2000},
            }
            data = json.dumps(payload).encode()
        else:
            path = self.claude / "projects" / "-hoge-fuga" / f"{sid}.jsonl"
            rows = []
            for index, message in enumerate(messages):
                rows.append(
                    dict(
                        type=message["type"],
                        uuid=message["uuid"],
                        timestamp=STAMP,
                        parentUuid=None if index == 0 else "u1",
                        sessionId=sid,
                        cwd="/hoge/fuga",
                        unknown_field={"keep": True},
                        message={
                            "role": message["role"],
                            "content": message["content"],
                        },
                    )
                )
            rows.append(
                {"type": "future-event", "sessionId": sid, "payload": [1, 2, 3]}
            )
            data = b"\r\n".join(json.dumps(row).encode() for row in rows)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
        return sid, path, data

    def test_list_finds_both_tools(self):
        a, _, _ = self.source("claude")
        b, _, _ = self.source("clawcodex")
        result = self.run_cli("list")["sessions"]
        self.assertEqual({row["id"] for row in result}, {a, b})
        self.assertEqual({row["cwd"] for row in result}, {"/hoge/fuga"})
        self.assertEqual(len(self.run_cli("list", "--from", "claude")["sessions"]), 1)

    def test_convert_by_id_registers_under_original_project(self):
        sid, path, raw = self.source("clawcodex")
        result = self.run_cli("convert", sid, "--to", "claude")
        native = Path(result["path"])
        self.assertEqual(native.parent, self.claude / "projects" / "-hoge-fuga")
        self.assertEqual(result["cwd"], "/hoge/fuga")
        self.assertNotEqual(result["session_id"], sid)
        self.assertEqual(path.read_bytes(), raw)
        self.assertTrue(self.run_cli("verify", result["session_id"])["ok"])
        self.assertIn(
            result["session_id"],
            {row["id"] for row in self.run_cli("list")["sessions"]},
        )

    def test_roundtrip_preserves_unknown_claude_fields_and_events(self):
        sid, path, raw = self.source("claude")
        first = self.run_cli("convert", sid, "--to", "clawcodex")
        back = self.run_cli("convert", first["session_id"], "--to", "claude")
        rows = [
            json.loads(line) for line in Path(back["path"]).read_bytes().splitlines()
        ]
        self.assertTrue(back["restored_original"])
        self.assertEqual(rows[0]["unknown_field"], {"keep": True})
        self.assertEqual(rows[-1]["type"], "future-event")
        self.assertEqual(path.read_bytes(), raw)
        self.assertTrue(self.run_cli("verify", back["session_id"])["ok"])

    def test_roundtrip_preserves_claw_metadata(self):
        sid, path, raw = self.source("clawcodex")
        first = self.run_cli("convert", sid, "--to", "claude")
        back = self.run_cli("convert", first["session_id"], "--to", "clawcodex")
        self.assertEqual(
            json.loads(Path(back["path"]).read_bytes())["custom_state"], {"keep": True}
        )
        self.assertEqual(path.read_bytes(), raw)

    def test_appended_conversation_returns_by_id(self):
        sid, _, _ = self.source("claude")
        first = self.run_cli("convert", sid, "--to", "clawcodex")
        native = Path(first["path"])
        data = json.loads(native.read_bytes())
        data["conversation"]["messages"].append(
            dict(
                type="user",
                role="user",
                uuid="new-u",
                timestamp=STAMP,
                content="follow-up",
            )
        )
        native.write_text(json.dumps(data))
        back = self.run_cli("convert", first["session_id"], "--to", "claude")
        rows = [
            json.loads(line) for line in Path(back["path"]).read_bytes().splitlines()
        ]
        self.assertEqual(rows[-1]["message"]["content"], "follow-up")
        self.assertEqual(rows[-1]["parentUuid"], "a1")

    def test_dry_run_does_not_create_destination(self):
        sid, path, raw = self.source("clawcodex")
        report = self.run_cli("convert", sid, "--to", "claude", "--dry-run")
        self.assertTrue(report["dry_run"])
        self.assertFalse(self.claude.exists())
        self.assertEqual(path.read_bytes(), raw)

    def test_path_input_and_output_argument_are_rejected(self):
        sid, path, _ = self.source("clawcodex")
        self.run_cli("convert", str(path), "--to", "claude", success=False)
        self.run_cli(
            "convert",
            sid,
            "--to",
            "claude",
            "--output",
            str(self.root / "out"),
            success=False,
        )
        self.assertFalse(self.claude.exists())

    def test_duplicate_ids_are_rejected(self):
        sid, path, raw = self.source("claude")
        other = self.claude / "projects" / "-other" / path.name
        other.parent.mkdir()
        other.write_bytes(raw)
        self.run_cli("convert", sid, "--to", "clawcodex", success=False)
        self.assertFalse(self.claw.exists())

    def test_missing_id_does_not_write(self):
        self.run_cli("convert", str(uuid4()), "--to", "claude", success=False)
        self.assertFalse(self.claude.exists())
        self.assertFalse(self.claw.exists())

    def test_metadata_id_mismatch_is_rejected(self):
        sid, path, _ = self.source("clawcodex")
        data = json.loads(path.read_bytes())
        data["session_id"] = str(uuid4())
        path.write_text(json.dumps(data))
        self.run_cli("convert", sid, "--to", "claude", success=False)
        self.assertFalse(self.claude.exists())

    def test_archive_tampering_prevents_verify_and_return(self):
        sid, _, _ = self.source("clawcodex")
        first = self.run_cli("convert", sid, "--to", "claude")
        preservation = Path(first["path"]).with_suffix("") / ".cc2cc" / "archive"
        original = next(p for p in preservation.iterdir() if p.is_file())
        original.write_bytes(b"{}")
        self.run_cli("verify", first["session_id"], success=False)
        self.run_cli("convert", first["session_id"], "--to", "clawcodex", success=False)
        self.assertEqual(len(list((self.claw / "sessions").glob("*.json"))), 1)
