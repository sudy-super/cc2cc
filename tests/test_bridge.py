from __future__ import annotations

import copy
import json
import tempfile
import unittest
from pathlib import Path

from claw_normalize_oracle import normalize_claw
from support import BridgeError, command, convert, install, verify

SOURCE_ID = "11111111-1111-4111-8111-111111111111"
TARGET_ID = "22222222-2222-4222-8222-222222222222"
INSTALL_ID = "33333333-3333-4333-8333-333333333333"
STAMP = "2026-10-16T00:00:00Z"


def encode(value):
    return (json.dumps(value, ensure_ascii=False, indent=2) + "\n").encode()


def jsonl(records):
    return b"".join(
        (json.dumps(record, ensure_ascii=False) + "\n").encode() for record in records
    )


def read(path):
    return json.loads(path.read_bytes())


def claude_record(uid, parent, kind, content, **extra):
    record = dict(
        type=kind,
        uuid=uid,
        parentUuid=parent,
        sessionId=SOURCE_ID,
        cwd="/synthetic/project",
        timestamp=STAMP,
        isSidechain=False,
        message=dict(role=kind, content=content),
    )
    if kind == "assistant":
        record["message"].update(
            id="msg_" + uid,
            model="synthetic-model",
            usage={"input_tokens": 17, "output_tokens": 3},
            stop_reason="end_turn",
        )
    record.update(extra)
    return record


def rich_claude():
    image = {
        "type": "image",
        "source": {"type": "base64", "media_type": "image/png", "data": "c3ludGhldGlj"},
    }
    return [
        claude_record(
            "u1",
            None,
            "user",
            [{"type": "text", "text": "合成データ"}, image],
            unknown_attribute={"nested": [1, False, None]},
        ),
        claude_record(
            "a1",
            "u1",
            "assistant",
            [
                {
                    "type": "thinking",
                    "thinking": "synthetic thought",
                    "signature": "sig",
                    "unknown": 7,
                },
                {"type": "redacted_thinking", "data": "opaque"},
                {
                    "type": "tool_use",
                    "id": "call1",
                    "name": "Read",
                    "input": {"file_path": "/synthetic/file"},
                },
            ],
        ),
        claude_record(
            "u2",
            "a1",
            "user",
            [
                {
                    "type": "tool_result",
                    "tool_use_id": "call1",
                    "content": [{"type": "text", "text": "result"}, image],
                }
            ],
        ),
        claude_record(
            "a2",
            "u2",
            "assistant",
            [
                {
                    "type": "text",
                    "text": "回答",
                    "citations": [
                        {
                            "type": "char_location",
                            "document_index": 0,
                            "start_char_index": 0,
                            "end_char_index": 2,
                        }
                    ],
                },
                {"type": "vendor_extension", "payload": {"unknown": True}},
            ],
        ),
        {
            "type": "file-history-snapshot",
            "snapshot": {"trackedFileBackups": {}},
            "uuid": "event1",
        },
    ]


def claw_record(uid, kind, content, **extra):
    result = dict(uuid=uid, type=kind, role=kind, content=content, timestamp=STAMP)
    result.update(extra)
    return result


class BridgeTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)

    def tearDown(self):
        self.temp.cleanup()

    def claude(self, records=None, companions=False):
        path = self.root / (SOURCE_ID + ".jsonl")
        records = rich_claude() if records is None else records
        # Deliberate whitespace, CRLF and absent final newline must survive restoration.
        raw = b"\r\n".join(
            json.dumps(r, ensure_ascii=False, separators=(", ", ": ")).encode()
            for r in records
        )
        path.write_bytes(raw)
        if companions:
            side = path.with_suffix("")
            (side / "subagents").mkdir(parents=True)
            (side / "subagents" / "agent-synthetic.jsonl").write_bytes(
                jsonl(
                    [claude_record("child1", None, "user", "child", isSidechain=True)]
                )
            )
            (side / "attachment.bin").write_bytes(bytes(range(256)))
        return path

    def claw(self, legacy=False, records=None):
        records = records or [
            claw_record("c1", "user", "質問", future_field={"x": [1]}),
            claw_record(
                "c2",
                "assistant",
                [{"type": "text", "text": "答え"}],
                usage={"input_tokens": 3, "output_tokens": 1},
            ),
        ]
        metadata = dict(
            session_id="synthetic_claw",
            cwd="/synthetic/project",
            name="合成セッション",
            provider="synthetic",
            custom_state={"keep": True},
        )
        if legacy:
            path = self.root / "legacy"
            path.mkdir()
            (path / "metadata.json").write_bytes(encode(metadata))
            (path / "transcript.jsonl").write_bytes(jsonl(records))
            (path / "content").mkdir()
            (path / "content" / "payload.txt").write_bytes("外部保存テキスト".encode())
        else:
            path = self.root / "synthetic_claw.json"
            metadata["conversation"] = dict(
                messages=records, max_history=2000, future_flag=7
            )
            path.write_bytes(encode(metadata))
        return path

    def forward(self, source=None, format="claude", name="forward"):
        source = source or self.claude()
        bundle = self.root / name
        report = convert(
            source,
            "clawcodex" if format == "claude" else "claude",
            bundle,
            source_format=format,
            session_id=TARGET_ID,
        )
        return bundle, Path(report["native_path"])

    def assert_restore(self, source, format, companions=False):
        bundle, _ = self.forward(source, format)
        restored = self.root / "restored"
        report = convert(bundle, format, restored)
        self.assertTrue(report["restored_original"])
        self.assertEqual(
            (restored / "native" / source.name).read_bytes(), source.read_bytes()
        )
        if companions:
            original_side = source.with_suffix("")
            for file in original_side.rglob("*"):
                if file.is_file():
                    self.assertEqual(
                        (
                            restored
                            / "native"
                            / original_side.name
                            / file.relative_to(original_side)
                        ).read_bytes(),
                        file.read_bytes(),
                    )
        return restored

    def test_claude_byte_exact_roundtrip_with_unknown_events_and_companions(self):
        source = self.claude(companions=True)
        restored = self.assert_restore(source, "claude", companions=True)
        self.assertTrue(verify(restored)["ok"])

    def test_rich_blocks_and_usage_survive_forward_conversion(self):
        source = self.claude()
        bundle, native = self.forward(source)
        messages = read(native)["conversation"]["messages"]
        expected = rich_claude()[:4]
        self.assertEqual([r["uuid"] for r in messages], [r["uuid"] for r in expected])
        for current, original in zip(messages, expected):
            self.assertEqual(current["content"], original["message"]["content"])
        self.assertEqual(messages[1]["usage"], expected[1]["message"]["usage"])
        self.assertEqual(messages[1]["model"], "synthetic-model")
        self.assertEqual(read(native)["model"], "")
        self.assertEqual(read(native)["provider"], "")
        self.assertEqual(
            (bundle / "archive" / source.name).read_bytes(), source.read_bytes()
        )
        self.assertEqual(verify(bundle)["appended_messages"], 0)

    def test_current_claw_json_byte_exact_roundtrip(self):
        self.assert_restore(self.claw(), "clawcodex")

    def test_legacy_claw_files_byte_exact_roundtrip(self):
        source = self.claw(legacy=True)
        bundle, _ = self.forward(source, "clawcodex")
        output = self.root / "restored"
        convert(bundle, "clawcodex", output)
        for original in source.rglob("*"):
            if original.is_file():
                self.assertEqual(
                    (output / "native" / original.relative_to(source)).read_bytes(),
                    original.read_bytes(),
                )

    def test_legacy_external_content_is_loaded_without_modifying_original(self):
        source = self.claw(
            legacy=True, records=[claw_record("c1", "user", "", _content_ref="payload")]
        )
        original = (source / "transcript.jsonl").read_bytes()
        bundle, native = self.forward(source, "clawcodex")
        self.assertEqual(
            json.loads(native.read_text().splitlines()[0])["message"]["content"],
            "外部保存テキスト",
        )
        self.assertEqual((source / "transcript.jsonl").read_bytes(), original)
        convert(bundle, "clawcodex", self.root / "restore")
        self.assertEqual(
            (self.root / "restore/native/transcript.jsonl").read_bytes(), original
        )

    def test_branch_defaults_to_last_on_ties_and_other_branch_is_archived(self):
        records = [
            claude_record("u", None, "user", "question"),
            claude_record("a-left", "u", "assistant", "left"),
            claude_record("a-right", "u", "assistant", "right"),
        ]
        source = self.claude(records)
        automatic, native = self.forward(source)
        self.assertEqual(
            [r["uuid"] for r in read(native)["conversation"]["messages"]],
            ["u", "a-right"],
        )
        report = convert(automatic, "claude", self.root / "automatic-restore")
        self.assertEqual(Path(report["native_path"]).read_bytes(), source.read_bytes())
        bundle = self.root / "selected"
        report = convert(
            source, "clawcodex", bundle, source_format="claude", leaf="a-left"
        )
        self.assertEqual(
            [
                r["uuid"]
                for r in read(Path(report["native_path"]))["conversation"]["messages"]
            ],
            ["u", "a-left"],
        )
        convert(bundle, "claude", self.root / "restore")
        self.assertEqual(
            (self.root / "restore/native" / source.name).read_bytes(),
            source.read_bytes(),
        )

    def test_latest_branch_is_selected_and_appended_messages_return_to_it(self):
        source = self.claude(
            [
                claude_record("u", None, "user", "question"),
                claude_record(
                    "latest",
                    "u",
                    "assistant",
                    "latest",
                    timestamp="2026-10-16T00:00:03Z",
                ),
                claude_record(
                    "older", "u", "assistant", "older", timestamp="2026-10-16T00:00:02Z"
                ),
                claude_record(
                    "sidechain",
                    "u",
                    "assistant",
                    "sidechain",
                    isSidechain=True,
                    timestamp="2030-01-01T00:00:00Z",
                ),
                {
                    "type": "future-event",
                    "uuid": "event",
                    "timestamp": "2030-01-01T00:00:00Z",
                },
            ]
        )
        original = source.read_bytes()
        bundle, native = self.forward(source)
        data = read(native)
        self.assertEqual(
            [r["uuid"] for r in data["conversation"]["messages"]], ["u", "latest"]
        )
        self.assertEqual(read(bundle / "manifest.json")["source_leaf"], "latest")
        data["conversation"]["messages"].extend(
            [
                claw_record(
                    "follow-u", "user", "follow-up", timestamp="2026-10-16T00:00:04Z"
                ),
                claw_record(
                    "follow-a", "assistant", "answer", timestamp="2026-10-16T00:00:05Z"
                ),
            ]
        )
        native.write_bytes(encode(data))
        self.assertTrue(verify(bundle)["ok"])
        restored = self.root / "latest-restore"
        report = convert(bundle, "claude", restored)
        restored_bytes = Path(report["native_path"]).read_bytes()
        self.assertTrue(restored_bytes.startswith(original))
        rows = [json.loads(line) for line in restored_bytes.splitlines()]
        self.assertEqual(rows[-2]["parentUuid"], "latest")
        self.assertEqual(rows[-1]["parentUuid"], "follow-u")
        self.assertEqual(read(restored / "manifest.json")["target_leaf"], "follow-a")
        self.assertTrue(verify(restored)["ok"])
        self.assertEqual(source.read_bytes(), original)

    def test_latest_appended_claude_branch_returns_to_claw_and_archives_others(self):
        source = self.claw(
            records=[
                claw_record("c1", "user", "question"),
                claw_record("c2", "assistant", "answer"),
            ]
        )
        bundle, native = self.forward(source, "clawcodex")
        baseline = [json.loads(line) for line in native.read_bytes().splitlines()]
        parent = baseline[-1]["uuid"]
        extra = [
            claude_record(
                "new-u",
                parent,
                "user",
                "latest question",
                sessionId=TARGET_ID,
                timestamp="2026-10-16T00:00:04Z",
            ),
            claude_record(
                "new-a",
                "new-u",
                "assistant",
                "latest answer",
                sessionId=TARGET_ID,
                timestamp="2026-10-16T00:00:05Z",
            ),
            claude_record(
                "old-u",
                parent,
                "user",
                "older question",
                sessionId=TARGET_ID,
                timestamp="2026-10-16T00:00:02Z",
            ),
            claude_record(
                "old-a",
                "old-u",
                "assistant",
                "older answer",
                sessionId=TARGET_ID,
                timestamp="2026-10-16T00:00:03Z",
            ),
        ]
        with native.open("ab") as stream:
            stream.write(jsonl(extra))
        original = native.read_bytes()
        self.assertEqual(verify(bundle)["appended_messages"], 4)
        restored = self.root / "restore-latest-branch"
        report = convert(bundle, "clawcodex", restored)
        messages = read(Path(report["native_path"]))["conversation"]["messages"]
        self.assertEqual([r["uuid"] for r in messages], ["c1", "c2", "new-u", "new-a"])
        self.assertEqual((restored / "archive" / native.name).read_bytes(), original)
        self.assertTrue(verify(restored)["ok"])
        installed = install(
            restored, home=self.root / "latest-home", session_id=INSTALL_ID
        )
        self.assertEqual(
            read(Path(installed["path"]))["conversation"]["messages"][-1]["content"],
            "latest answer",
        )

    def test_appended_branch_before_imported_tail_is_rejected(self):
        source = self.claw(
            records=[
                claw_record("c1", "user", "question"),
                claw_record("c2", "assistant", "answer"),
            ]
        )
        original = source.read_bytes()
        bundle, native = self.forward(source, "clawcodex")
        baseline = [json.loads(line) for line in native.read_bytes().splitlines()]
        with native.open("ab") as stream:
            stream.write(
                jsonl(
                    [
                        claude_record(
                            "fork",
                            baseline[0]["uuid"],
                            "assistant",
                            "fork",
                            sessionId=TARGET_ID,
                            timestamp="2026-10-16T00:01:00Z",
                        )
                    ]
                )
            )
        with self.assertRaises(BridgeError):
            convert(bundle, "clawcodex", self.root / "invalid-fork")
        self.assertFalse((self.root / "invalid-fork").exists())
        self.assertEqual(source.read_bytes(), original)

    def test_claw_normal_resave_restores_original_rich_attributes(self):
        source = self.claude()
        bundle, native = self.forward(source)
        data = read(native)
        original_messages = copy.deepcopy(data["conversation"]["messages"])
        data["conversation"]["messages"] = [
            normalize_claw(r) for r in original_messages
        ]
        self.assertNotEqual(original_messages, data["conversation"]["messages"])
        native.write_bytes(encode(data))
        self.assertTrue(verify(bundle)["native_resaved"])
        convert(bundle, "claude", self.root / "restore")
        self.assertEqual(
            (self.root / "restore/native" / source.name).read_bytes(),
            source.read_bytes(),
        )

    def test_appended_claw_messages_return_to_claude_with_original_bytes_intact(self):
        source = self.claude()
        raw = source.read_bytes()
        bundle, native = self.forward(source)
        data = read(native)
        data["conversation"]["messages"] += [
            claw_record("new-u", "user", "追加質問"),
            claw_record("new-a", "assistant", "追加回答"),
        ]
        native.write_bytes(encode(data))
        self.assertEqual(verify(bundle)["appended_messages"], 2)
        report = convert(bundle, "claude", self.root / "restore")
        output = Path(report["native_path"]).read_bytes()
        self.assertTrue(output.startswith(raw + b"\n"))
        records = [json.loads(line) for line in output.splitlines()]
        self.assertEqual(records[-2]["parentUuid"], "a2")
        self.assertEqual(records[-1]["parentUuid"], "new-u")
        self.assertEqual(records[-1]["sessionId"], SOURCE_ID)
        self.assertEqual(records[-1]["message"]["content"], "追加回答")

    def test_appended_claude_messages_return_to_claw(self):
        source = self.claw()
        bundle, native = self.forward(source, "clawcodex")
        records = [json.loads(line) for line in native.read_text().splitlines()]
        extra = claude_record(
            "new-u", records[-1]["uuid"], "user", "追加", sessionId=TARGET_ID
        )
        with native.open("ab") as stream:
            stream.write(jsonl([extra]))
        self.assertEqual(verify(bundle)["appended_messages"], 1)
        report = convert(bundle, "clawcodex", self.root / "restore")
        messages = read(Path(report["native_path"]))["conversation"]["messages"]
        self.assertEqual(messages[:2], read(source)["conversation"]["messages"])
        self.assertEqual(messages[-1]["content"], "追加")
        restored_metadata = read(Path(report["native_path"]))
        self.assertEqual(
            restored_metadata["custom_state"], read(source)["custom_state"]
        )
        self.assertEqual(
            restored_metadata["conversation"]["future_flag"],
            read(source)["conversation"]["future_flag"],
        )

    def test_original_archive_tampering_or_removal_prevents_restore(self):
        source = self.claude()
        bundle, _ = self.forward(source)
        archive = bundle / "archive" / source.name
        original = archive.read_bytes()
        for mutation in ("tamper", "remove"):
            with self.subTest(mutation=mutation):
                if mutation == "tamper":
                    archive.write_bytes(original + b" ")
                else:
                    archive.unlink()
                with self.assertRaises((BridgeError, OSError)):
                    verify(bundle)
                with self.assertRaises((BridgeError, OSError)):
                    convert(bundle, "claude", self.root / ("restore-" + mutation))
                archive.write_bytes(original)

    def test_baseline_tampering_prevents_restore(self):
        bundle, _ = self.forward()
        (bundle / "baseline.jsonl").write_bytes(b"{}\n")
        with self.assertRaises(BridgeError):
            verify(bundle)

    def test_native_edit_deletion_or_reorder_prevents_restore(self):
        bundle, native = self.forward()
        original = read(native)
        for mutation in ("edit", "delete", "reorder"):
            with self.subTest(mutation=mutation):
                data = copy.deepcopy(original)
                messages = data["conversation"]["messages"]
                if mutation == "edit":
                    messages[0]["content"] = "改変"
                elif mutation == "delete":
                    messages.pop(0)
                else:
                    messages[0], messages[1] = messages[1], messages[0]
                native.write_bytes(encode(data))
                with self.assertRaises(BridgeError):
                    verify(bundle)
                with self.assertRaises(BridgeError):
                    convert(bundle, "claude", self.root / ("restore-" + mutation))

    def test_output_overwrite_is_rejected(self):
        source = self.claude()
        output = self.root / "existing"
        output.mkdir()
        marker = output / "marker"
        marker.write_bytes(b"keep")
        with self.assertRaises(BridgeError):
            convert(source, "clawcodex", output, source_format="claude")
        self.assertEqual(marker.read_bytes(), b"keep")
        self.assertEqual(list(output.iterdir()), [marker])

    def test_convert_dry_run_writes_nothing(self):
        source = self.claude()
        destination = self.root / "absent-parent" / "output"
        report = convert(
            source, "clawcodex", destination, source_format="claude", dry_run=True
        )
        self.assertTrue(report["dry_run"])
        self.assertFalse(destination.parent.exists())

    def test_cli_dry_run_and_invalid_input_exit_code(self):
        source = self.claude()
        destination = self.root / "cli-output"
        result = command(
            "convert",
            str(source),
            "--from",
            "claude",
            "--to",
            "clawcodex",
            "-o",
            str(destination),
            "--dry-run",
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(json.loads(result.stdout)["dry_run"])
        self.assertFalse(destination.exists())
        result = command("verify", self.root / "missing")
        self.assertEqual(result.returncode, 2)
        self.assertNotIn("Traceback", result.stderr)

    def test_install_both_formats_isolated_and_restorable(self):
        for format in ("claude", "clawcodex"):
            with self.subTest(format=format):
                source = self.claude() if format == "claude" else self.claw()
                bundle, _ = self.forward(source, format, name="forward-" + format)
                home = self.root / ("home-" + format)
                home.mkdir()
                marker = home / "existing-config"
                marker.write_bytes(b"keep")
                preview = install(
                    bundle, home=home, session_id=INSTALL_ID, dry_run=True
                )
                self.assertFalse(Path(preview["path"]).exists())
                self.assertEqual(list(home.iterdir()), [marker])
                report = install(bundle, home=home, session_id=INSTALL_ID)
                destination = Path(report["path"])
                self.assertTrue(destination.is_relative_to(home))
                self.assertTrue(destination.exists())
                self.assertEqual(marker.read_bytes(), b"keep")
                with self.assertRaises(BridgeError):
                    install(bundle, home=home, session_id=INSTALL_ID)
                restored = self.root / ("installed-restore-" + format)
                convert(destination, format, restored, source_format=report["format"])
                self.assertEqual(
                    (restored / "native" / source.name).read_bytes(),
                    source.read_bytes(),
                )
                if report["format"] == "claude":
                    self.assertTrue(
                        all(
                            r["sessionId"] == INSTALL_ID
                            for r in (
                                json.loads(line)
                                for line in destination.read_text().splitlines()
                            )
                        )
                    )
                else:
                    self.assertEqual(read(destination)["session_id"], INSTALL_ID)

    def test_symlink_input_and_companion_are_rejected(self):
        source = self.claude()
        link = self.root / "input.jsonl"
        link.symlink_to(source)
        with self.assertRaises(BridgeError):
            self.forward(link)
        companions = source.with_suffix("")
        companions.mkdir()
        (companions / "linked").symlink_to(source)
        with self.assertRaises(BridgeError):
            self.forward(source)

    def test_symlink_output_is_rejected(self):
        source = self.claude()
        destination = self.root / "real-output"
        destination.mkdir()
        link = self.root / "linked-output"
        link.symlink_to(destination, target_is_directory=True)
        with self.assertRaises(BridgeError):
            convert(source, "clawcodex", link, source_format="claude")
        self.assertEqual(list(destination.iterdir()), [])

    def test_archive_symlink_is_rejected(self):
        source = self.claude()
        bundle, _ = self.forward(source)
        archive = bundle / "archive" / source.name
        archive.unlink()
        archive.symlink_to(source)
        with self.assertRaises(BridgeError):
            verify(bundle)

    def test_bundle_root_symlink_is_rejected(self):
        bundle, _ = self.forward()
        link = self.root / "linked-bundle"
        link.symlink_to(bundle, target_is_directory=True)
        with self.assertRaises(BridgeError):
            verify(link)
        with self.assertRaises(BridgeError):
            convert(link, "claude", self.root / "restore-via-link")

    def test_subagent_transcripts_are_archived_recursively(self):
        source = self.claw(
            records=[
                claw_record(
                    "c1",
                    "user",
                    "合成",
                    toolUseResult={"status": "completed", "agent_id": "agent_parent"},
                )
            ]
        )
        transcripts = self.root / "synthetic-transcripts"
        transcripts.mkdir()
        parent = {"type": "agent", "agent_id": "agent_child"}
        child = {"type": "event", "opaque": {"future": 1}}
        (transcripts / "agent_parent.jsonl").write_bytes(jsonl([parent]))
        (transcripts / "agent_child.jsonl").write_bytes(jsonl([child]))
        bundle = self.root / "agents-forward"
        convert(
            source,
            "claude",
            bundle,
            source_format="clawcodex",
            transcripts_dir=transcripts,
        )
        for name in ("agent_parent", "agent_child"):
            self.assertEqual(
                (bundle / "archive/transcripts" / (name + ".jsonl")).read_bytes(),
                (transcripts / (name + ".jsonl")).read_bytes(),
            )
        self.assertTrue(verify(bundle)["ok"])

    def test_missing_referenced_subagent_fails_before_writing_output(self):
        source = self.claw(
            records=[
                claw_record(
                    "c1",
                    "user",
                    "合成",
                    toolUseResult={"status": "completed", "agent_id": "agent_missing"},
                )
            ]
        )
        output = self.root / "missing-agent-forward"
        with self.assertRaises(BridgeError):
            convert(
                source,
                "claude",
                output,
                source_format="clawcodex",
                transcripts_dir=self.root / "empty-transcripts",
            )
        self.assertFalse(output.exists())

    def test_content_ref_traversal_and_missing_content_are_rejected(self):
        source = self.claw(legacy=True)
        for reference in (
            "../secret",
            "/tmp/secret",
            "a/b",
            "a\\b",
            ".",
            "..",
            "missing",
        ):
            with self.subTest(reference=reference):
                (source / "transcript.jsonl").write_bytes(
                    jsonl([claw_record("c1", "user", "", _content_ref=reference)])
                )
                with self.assertRaises(BridgeError):
                    convert(
                        source,
                        "claude",
                        self.root / "traversal-output",
                        source_format="clawcodex",
                    )
                self.assertFalse((self.root / "traversal-output").exists())

    def test_orphan_tool_result_and_unfinished_call_are_rejected(self):
        for content, kind in (
            (
                [{"type": "tool_result", "tool_use_id": "missing", "content": "x"}],
                "user",
            ),
            (
                [{"type": "tool_use", "id": "pending", "name": "Read", "input": {}}],
                "assistant",
            ),
        ):
            with self.subTest(kind=kind):
                source = self.claude([claude_record("x", None, kind, content)])
                with self.assertRaises(BridgeError):
                    self.forward(source)

    def test_explicit_attachments_are_archived_and_restored_byte_exact(self):
        source = self.claude()
        attachment = self.root / "image.bin"
        attachment.write_bytes(bytes(range(256)))
        directory = self.root / "documents"
        (directory / "nested").mkdir(parents=True)
        (directory / "nested" / "reference.txt").write_bytes("合成添付".encode())
        bundle = self.root / "attached-forward"
        convert(
            source,
            "clawcodex",
            bundle,
            source_format="claude",
            attachments=[attachment, directory],
        )
        self.assertEqual(
            (bundle / "archive/attachments/image.bin").read_bytes(),
            attachment.read_bytes(),
        )
        self.assertEqual(
            (
                bundle / "archive/attachments/documents/nested/reference.txt"
            ).read_bytes(),
            (directory / "nested/reference.txt").read_bytes(),
        )
        self.assertTrue(verify(bundle)["ok"])
        convert(bundle, "claude", self.root / "attached-restore")
        self.assertEqual(
            (self.root / "attached-restore/native/attachments/image.bin").read_bytes(),
            attachment.read_bytes(),
        )

    def test_attachment_symlink_and_duplicate_basename_are_rejected(self):
        source = self.claude()
        file = self.root / "asset.bin"
        file.write_bytes(b"original")
        link = self.root / "linked.bin"
        link.symlink_to(file)
        with self.assertRaises(BridgeError):
            convert(
                source,
                "clawcodex",
                self.root / "attached",
                source_format="claude",
                attachments=[link],
            )
        sibling = self.root / "sibling"
        sibling.mkdir()
        duplicate = sibling / "asset.bin"
        duplicate.write_bytes(b"different")
        with self.assertRaises(BridgeError):
            convert(
                source,
                "clawcodex",
                self.root / "attached",
                source_format="claude",
                attachments=[file, duplicate],
            )
        self.assertFalse((self.root / "attached").exists())

    def test_cli_repeated_attachment_options(self):
        source = self.claude()
        first, second = self.root / "first.bin", self.root / "second.bin"
        first.write_bytes(b"one")
        second.write_bytes(b"two")
        output = self.root / "cli-attachments"
        result = command(
            "convert",
            str(source),
            "--from",
            "claude",
            "--to",
            "clawcodex",
            "-o",
            str(output),
            "--attachments",
            str(first),
            "--attachments",
            str(second),
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            (output / "archive/attachments/first.bin").read_bytes(), b"one"
        )
        self.assertEqual(
            (output / "archive/attachments/second.bin").read_bytes(), b"two"
        )

    def test_appended_claude_events_are_archived_while_conversation_returns_to_claw(
        self,
    ):
        source = self.claw()
        bundle, native = self.forward(source, "clawcodex")
        baseline = [json.loads(line) for line in native.read_text().splitlines()]
        extra = [
            {"type": "last-prompt", "lastPrompt": "追加質問", "sessionId": TARGET_ID},
            claude_record(
                "new-u", baseline[-1]["uuid"], "user", "追加質問", sessionId=TARGET_ID
            ),
            {"type": "ai-title", "title": "合成タイトル", "sessionId": TARGET_ID},
            claude_record(
                "new-a", "new-u", "assistant", "追加回答", sessionId=TARGET_ID
            ),
            {"type": "unknown-future-event", "payload": {"retain": [1, None]}},
        ]
        with native.open("ab") as stream:
            stream.write(jsonl(extra))
        appended_raw = native.read_bytes()
        self.assertEqual(verify(bundle)["appended_messages"], len(extra))
        restored = self.root / "restore-events"
        report = convert(bundle, "clawcodex", restored)
        messages = read(Path(report["native_path"]))["conversation"]["messages"]
        self.assertEqual(
            [record["uuid"] for record in messages], ["c1", "c2", "new-u", "new-a"]
        )
        self.assertEqual(messages[-1]["content"], "追加回答")
        self.assertEqual(
            (restored / "archive" / native.name).read_bytes(), appended_raw
        )
        self.assertTrue(
            any("独自イベント" in warning for warning in report["warnings"])
        )
        self.assertTrue(verify(restored)["ok"])

    def test_claude_attachment_descriptor_and_rendered_context_are_preserved(self):
        descriptor = {
            "type": "file",
            "filename": "/synthetic/file.txt",
            "future": {"keep": True},
        }
        attachment = dict(
            type="attachment",
            uuid="attach1",
            parentUuid="u1",
            sessionId=SOURCE_ID,
            cwd="/synthetic/project",
            timestamp=STAMP,
            attachment=descriptor,
            rendered=[
                {"content": "先頭"},
                {"content": [{"type": "text", "text": "末尾"}]},
            ],
        )
        source = self.claude(
            [
                claude_record("u1", None, "user", "質問"),
                attachment,
                claude_record("a1", "attach1", "assistant", "回答"),
            ]
        )
        original = source.read_bytes()
        bundle, native = self.forward(source)
        converted = read(native)["conversation"]["messages"][1]
        self.assertEqual(converted["type"], "attachment")
        self.assertEqual(converted["role"], "user")
        self.assertEqual(converted["attachments"], [descriptor])
        self.assertEqual(
            converted["content"],
            [{"type": "text", "text": "先頭"}, {"type": "text", "text": "末尾"}],
        )
        data = read(native)
        data["conversation"]["messages"] = [
            normalize_claw(record) for record in data["conversation"]["messages"]
        ]
        native.write_bytes(encode(data))
        self.assertTrue(verify(bundle)["ok"])
        report = convert(bundle, "claude", self.root / "restore-attachment")
        self.assertEqual(Path(report["native_path"]).read_bytes(), original)

    def test_claude_unrendered_attachment_is_virtual(self):
        attachment = dict(
            type="attachment",
            uuid="attach1",
            parentUuid="u1",
            sessionId=SOURCE_ID,
            cwd="/synthetic/project",
            timestamp=STAMP,
            attachment={"type": "file", "filename": "/synthetic/file.txt"},
        )
        source = self.claude(
            [
                claude_record("u1", None, "user", "質問"),
                attachment,
                claude_record("a1", "attach1", "assistant", "回答"),
            ]
        )
        _, native = self.forward(source)
        converted = read(native)["conversation"]["messages"][1]
        self.assertTrue(converted["isVirtual"])
        self.assertEqual(converted["content"], "")
        self.assertEqual(converted["attachments"], [attachment["attachment"]])

    def test_claw_attachment_becomes_claude_meta_user_and_roundtrips_exactly(self):
        attachment = claw_record(
            "attach1",
            "attachment",
            [{"type": "text", "text": "添付内容"}],
            role="user",
            attachments=[{"type": "file", "filename": "synthetic.txt"}],
        )
        source = self.claw(
            records=[
                claw_record("c1", "user", "質問"),
                attachment,
                claw_record("c2", "assistant", "回答"),
            ]
        )
        original = source.read_bytes()
        bundle, native = self.forward(source, "clawcodex")
        records = [json.loads(line) for line in native.read_text().splitlines()]
        converted = records[1]
        self.assertEqual(converted["type"], "user")
        self.assertTrue(converted["isMeta"])
        self.assertEqual(
            converted["message"], {"role": "user", "content": attachment["content"]}
        )
        self.assertEqual(records[2]["parentUuid"], "attach1")
        report = convert(bundle, "clawcodex", self.root / "restore-claw-attachment")
        self.assertEqual(Path(report["native_path"]).read_bytes(), original)

    def test_install_restored_claw_disables_automation_without_modifying_native(self):
        source = self.claw()
        data = read(source)
        data.update(
            goal={"status": "active", "objective": "synthetic"},
            scheduled_tasks=[{"id": "synthetic-task", "enabled": True}],
        )
        source.write_bytes(encode(data))
        bundle, _ = self.forward(source, "clawcodex")
        restored = self.root / "restore-automation"
        report = convert(bundle, "clawcodex", restored)
        native = Path(report["native_path"])
        original = native.read_bytes()
        self.assertEqual(original, source.read_bytes())
        installed = install(
            restored, home=self.root / "automation-home", session_id=INSTALL_ID
        )
        registered = read(Path(installed["path"]))
        self.assertNotIn("goal", registered)
        self.assertNotIn("scheduled_tasks", registered)
        self.assertEqual(registered["conversation"], data["conversation"])
        self.assertEqual(native.read_bytes(), original)
        self.assertTrue(installed["warnings"])

    def test_install_claw_subagents_share_identical_content_and_reject_collisions(self):
        source = self.claw(
            records=[
                claw_record(
                    "c1",
                    "user",
                    "質問",
                    toolUseResult={
                        "status": "completed",
                        "agent_id": "agent_synthetic",
                    },
                )
            ]
        )
        transcripts = self.root / "transcripts"
        transcripts.mkdir()
        raw = jsonl([{"type": "event", "payload": "synthetic child"}])
        (transcripts / "agent_synthetic.jsonl").write_bytes(raw)
        bundle = self.root / "agent-forward"
        convert(
            source,
            "claude",
            bundle,
            source_format="clawcodex",
            transcripts_dir=transcripts,
        )
        restored = self.root / "restore-agents"
        convert(bundle, "clawcodex", restored)
        home = self.root / "agent-home"
        first = install(restored, home=home, session_id=INSTALL_ID)
        installed_child = home / "transcripts/agent_synthetic.jsonl"
        self.assertEqual(installed_child.read_bytes(), raw)
        second_id = "44444444-4444-4444-8444-444444444444"
        second = install(restored, home=home, session_id=second_id)
        self.assertTrue(Path(first["path"]).exists())
        self.assertTrue(Path(second["path"]).exists())
        self.assertEqual(installed_child.read_bytes(), raw)
        installed_child.write_bytes(b"conflicting original")
        third_id = "55555555-5555-4555-8555-555555555555"
        with self.assertRaises(BridgeError):
            install(restored, home=home, session_id=third_id)
        self.assertEqual(installed_child.read_bytes(), b"conflicting original")
        self.assertFalse((home / "sessions" / (third_id + ".json")).exists())
        self.assertFalse((home / "sessions" / third_id).exists())

    def test_terminal_system_record_is_selected_as_leaf_and_roundtrips(self):
        records = [
            claude_record("u1", None, "user", "質問"),
            claude_record("a1", "u1", "assistant", "回答"),
            {
                "type": "system",
                "uuid": "terminal",
                "parentUuid": "a1",
                "sessionId": SOURCE_ID,
                "cwd": "/synthetic/project",
                "timestamp": STAMP,
                "subtype": "turn_duration",
                "durationMs": 15,
            },
        ]
        source = self.claude(records)
        bundle, native = self.forward(source)
        messages = read(native)["conversation"]["messages"]
        self.assertEqual(
            [record["uuid"] for record in messages], ["u1", "a1", "terminal"]
        )
        self.assertEqual(messages[-1]["type"], "system")
        report = convert(bundle, "claude", self.root / "restore-terminal")
        self.assertEqual(Path(report["native_path"]).read_bytes(), source.read_bytes())
        self.assertTrue(verify(self.root / "restore-terminal")["ok"])

    def test_compact_logical_parent_keeps_single_leaf_and_archives_earlier_history(
        self,
    ):
        records = [
            claude_record("u1", None, "user", "古い質問"),
            claude_record("a1", "u1", "assistant", "古い回答"),
            {
                "type": "system",
                "uuid": "compact",
                "parentUuid": None,
                "logicalParentUuid": "a1",
                "sessionId": SOURCE_ID,
                "cwd": "/synthetic/project",
                "timestamp": STAMP,
                "subtype": "compact_boundary",
                "content": "要約",
                "compactMetadata": {"preTokens": 100},
            },
            claude_record("u2", "compact", "user", "新しい質問"),
            claude_record("a2", "u2", "assistant", "新しい回答"),
        ]
        source = self.claude(records)
        bundle, native = self.forward(source)
        self.assertEqual(
            [record["uuid"] for record in read(native)["conversation"]["messages"]],
            ["compact", "u2", "a2"],
        )
        report = convert(bundle, "claude", self.root / "restore-compact")
        self.assertEqual(Path(report["native_path"]).read_bytes(), source.read_bytes())
        self.assertTrue(verify(self.root / "restore-compact")["ok"])

    def test_restored_multiple_branch_bundle_verifies_using_original_selected_leaf(
        self,
    ):
        source = self.claude(
            [
                claude_record("u", None, "user", "質問"),
                claude_record("left", "u", "assistant", "左"),
                claude_record(
                    "right", "u", "assistant", "右", timestamp="2026-10-16T00:01:00Z"
                ),
            ]
        )
        bundle = self.root / "branched-forward"
        convert(source, "clawcodex", bundle, source_format="claude", leaf="left")
        restored = self.root / "restored-branches"
        report = convert(bundle, "claude", restored)
        self.assertEqual(Path(report["native_path"]).read_bytes(), source.read_bytes())
        self.assertEqual(read(restored / "manifest.json")["target_leaf"], "left")
        self.assertTrue(verify(restored)["ok"])
        registered = install(
            restored, home=self.root / "branch-home", session_id=INSTALL_ID
        )
        native_records = [
            json.loads(line)
            for line in Path(registered["path"]).read_text().splitlines()
        ]
        self.assertEqual(
            [record["uuid"] for record in native_records], ["u", "left", "right"]
        )

    def test_legacy_install_inlines_external_content(self):
        source = self.claw(
            legacy=True, records=[claw_record("c1", "user", "", _content_ref="payload")]
        )
        original = (source / "transcript.jsonl").read_bytes()
        bundle, _ = self.forward(source, "clawcodex")
        restored = self.root / "restore-external"
        convert(bundle, "clawcodex", restored)
        installed = install(
            restored, home=self.root / "legacy-home", session_id=INSTALL_ID
        )
        message = read(Path(installed["path"]))["conversation"]["messages"][0]
        self.assertEqual(message["content"], "外部保存テキスト")
        self.assertNotIn("_content_ref", message)
        self.assertEqual((restored / "native/transcript.jsonl").read_bytes(), original)

    def test_registration_reidentifies_session_id_aliases_in_main_and_child_records(
        self,
    ):
        records = [
            claude_record(
                "u1",
                None,
                "user",
                "質問",
                session_id=SOURCE_ID,
                parentSessionId=SOURCE_ID,
                parent_session_id=SOURCE_ID,
            ),
            claude_record("a1", "u1", "assistant", "回答"),
        ]
        source = self.claude(records, companions=True)
        child = source.with_suffix("") / "subagents/agent-synthetic.jsonl"
        child_record = claude_record(
            "child",
            None,
            "user",
            "子",
            isSidechain=True,
            session_id=SOURCE_ID,
            parentSessionId=SOURCE_ID,
            parent_session_id=SOURCE_ID,
        )
        child.write_bytes(jsonl([child_record]))
        bundle, _ = self.forward(source)
        restored = self.root / "restore-aliases"
        convert(bundle, "claude", restored)
        installed = install(
            restored, home=self.root / "alias-home", session_id=INSTALL_ID
        )
        destination = Path(installed["path"])
        main_record = json.loads(destination.read_text().splitlines()[0])
        child_record = json.loads(
            (
                destination.with_suffix("") / "subagents/agent-synthetic.jsonl"
            ).read_text()
        )
        for record in (main_record, child_record):
            for key in (
                "sessionId",
                "session_id",
                "parentSessionId",
                "parent_session_id",
            ):
                self.assertEqual(record[key], INSTALL_ID)
        self.assertEqual(
            (restored / "native" / source.name).read_bytes(), source.read_bytes()
        )

    def test_unknown_claw_message_type_is_rejected_instead_of_dropped(self):
        source = self.claw(
            records=[claw_record("c1", "unknown_vendor_event", "opaque")]
        )
        with self.assertRaises(BridgeError):
            self.forward(source, "clawcodex")

    def test_invalid_json_duplicate_keys_and_nonfinite_numbers_are_rejected(self):
        source = self.root / "invalid.jsonl"
        for raw in (b"{", b'{"sessionId":"x","sessionId":"y"}\n', b'{"value":NaN}\n'):
            with self.subTest(raw=raw):
                source.write_bytes(raw)
                with self.assertRaises(BridgeError):
                    self.forward(source)


if __name__ == "__main__":
    unittest.main()
