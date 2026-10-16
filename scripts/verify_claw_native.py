#!/usr/bin/env python3
"""Verify a generated session with ClawCodex's real loaders without side effects.

Run with the ClawCodex virtual environment's Python interpreter:
    python scripts/verify_claw_native.py --source /path/to/clawcodex /path/to/bundle
"""

from __future__ import annotations

import argparse
import asyncio
import contextlib
import hashlib
import io
import json
import os
import sys
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import MagicMock, patch


def _read_only(event: str, arguments: tuple) -> None:
    if event == "open":
        _, mode, flags = arguments
        write_flags = os.O_WRONLY | os.O_RDWR | os.O_CREAT | os.O_TRUNC | os.O_APPEND
        if (
            isinstance(mode, str) and any(char in mode for char in "wax+")
        ) or flags & write_flags:
            raise PermissionError("検証中のファイル書き込みは禁止しています")
    if event in {
        "os.mkdir",
        "os.remove",
        "os.rmdir",
        "os.rename",
        "os.link",
        "os.symlink",
        "os.chmod",
        "os.chown",
        "os.truncate",
        "os.utime",
        "os.system",
        "os.posix_spawn",
        "os.exec",
        "os.spawn",
        "subprocess.Popen",
        "socket.connect",
        "socket.bind",
        "socket.sendto",
        "socket.sendmsg",
    }:
        raise PermissionError(
            "検証中のファイル変更、通信、プロセス起動は禁止しています"
        )


def _native_path(path: Path) -> Path:
    if path.is_dir() and (path / "manifest.json").is_file():
        manifest = json.loads((path / "manifest.json").read_text(encoding="utf-8"))
        if manifest.get("target_format") != "clawcodex":
            raise ValueError("ClawCodexを出力先にしたbundleを指定してください")
        relative = manifest.get("native_transcript")
        if not isinstance(relative, str) or "\\" in relative:
            raise ValueError("native_transcriptが不正です")
        native = path / "native" / relative
        if Path(relative).is_absolute() or not native.resolve().is_relative_to(
            (path / "native").resolve()
        ):
            raise ValueError(
                "native_transcriptがnativeディレクトリの外を参照しています"
            )
        path = native
    elif path.is_dir():
        candidates = list(path.glob("*.json"))
        if len(candidates) != 1:
            raise ValueError("nativeディレクトリにはJSONが1件必要です")
        path = candidates[0]
    if not path.is_file() or path.suffix != ".json":
        raise ValueError("現行ClawCodexのセッションJSONが必要です")
    return path.resolve()


def verify(source: Path, path: Path) -> dict:
    sys.dont_write_bytecode = True
    sys.addaudithook(_read_only)
    source = source.resolve()
    if not (source / "src" / "server" / "agent_server.py").is_file():
        raise ValueError("ClawCodex本体のソースディレクトリが必要です")
    path = _native_path(path.resolve())
    sys.path.insert(0, str(source))
    # Imports must never read or initialize the user's real configuration.
    os.environ["CLAWCODEX_CONFIG_DIR"] = str(
        path.parent / ".cc2cc-verification-unused-config"
    )
    from src.agent.conversation import Conversation
    from src.cli import _build_parser
    from src.server import agent_server as server
    from src.server.desktop_serve import DesktopServeState
    from src.server.desktop_sessions import (
        list_session_rows,
        load_session_messages,
        load_session_meta,
    )

    before = path.read_bytes()
    payload = json.loads(before)
    sid = payload["session_id"]
    if path.stem != sid or not server._SESSION_ID_RE.fullmatch(sid):
        raise ValueError("ファイル名とセッションIDが不一致か、IDが不正です")
    records = payload["conversation"]["messages"]
    if not isinstance(records, list) or not all(
        isinstance(record, dict) for record in records
    ):
        raise ValueError("メッセージがオブジェクトの配列ではありません")
    conversation = Conversation.from_dict(payload["conversation"])
    if len(conversation.messages) != len(records):
        raise ValueError("本体の読み込みでメッセージが欠落しました")
    api_messages = conversation.get_messages()
    stored = load_session_messages(path.parent, sid)
    if stored is None or stored["message_count"] != len(records):
        raise ValueError("Desktopの会話読み込みに失敗しました")
    rows = list_session_rows(path.parent, limit=0)["sessions"]
    if not any(row["id"] == sid for row in rows):
        raise ValueError("Desktopの履歴一覧に表示されません")

    loop = asyncio.new_event_loop()
    runtime = server._AgentSession(
        "cc2cc-verification-runtime",
        payload.get("cwd") or str(path.parent),
        server.AgentServerConfig(single_session=False),
        loop,
        asyncio.Queue(),
    )
    runtime.session = SimpleNamespace(conversation=None)
    runtime._discard_pending_files = lambda files: None
    runtime._rebuild_system_prompt = lambda: None
    runtime._push_cron_state = lambda *arguments: None
    runtime.cron_scheduler = MagicMock()
    runtime.cron_scheduler.list_jobs.return_value = []
    runtime.cron_scheduler.restore.return_value = 0
    goal = MagicMock()
    goal.restore.return_value = None
    runtime._goal_manager = lambda: goal
    replies = []
    runtime._reply = lambda request_id, result: replies.append(result)
    try:
        with (
            patch.object(server, "_sessions_dir", return_value=path.parent),
            patch.object(server, "_cost_snapshot", return_value={}),
        ):
            runtime._do_resume("cc2cc-verification", sid)
        if not replies or replies[-1].get("ok") is not True:
            raise ValueError("本体の_do_resumeに失敗しました")
        if runtime.session.conversation.to_dict() != conversation.to_dict():
            raise ValueError("本体の再開結果がConversationの読み込み結果と一致しません")
    finally:
        loop.close()

    parser = _build_parser()
    resume_flag = any("--resume" in action.option_strings for action in parser._actions)
    with contextlib.redirect_stderr(io.StringIO()):
        try:
            parser.parse_args(["--resume", sid])
            accepts_resume_flag = True
        except SystemExit:
            accepts_resume_flag = False

    def sentinel(*arguments):
        return None

    state = DesktopServeState(
        token="verification",
        workspace=str(path.parent),
        manager=None,
        spawn_agent=sentinel,
        protocol_version="verification",
        agent_config=server.AgentServerConfig(),
    )
    if state.spawn_for("", "", None) is not sentinel:
        raise ValueError("Desktopの空provider/modelの選択が想定と異なります")
    meta = load_session_meta(path.parent, sid)
    if path.read_bytes() != before:
        raise ValueError("検証中に入力が変更されました")
    return {
        "ok": True,
        "native_sha256": hashlib.sha256(before).hexdigest(),
        "message_count": len(records),
        "api_message_count": len(api_messages),
        "desktop_listed": True,
        "desktop_loaded": True,
        "native_resume_loaded": True,
        "cli_has_resume_option": resume_flag,
        "cli_accepts_resume_option": accepts_resume_flag,
        "empty_provider_model_uses_default_spawn": True,
        "saved_provider_empty": not meta["provider"],
        "saved_model_empty": not meta["model"],
        "mixed_model_without_provider": bool(meta["model"] and not meta["provider"]),
        "input_unchanged": True,
        "filesystem_mutations_and_network_blocked": True,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--source", required=True, type=Path, help="ClawCodex本体のソースルート"
    )
    parser.add_argument(
        "input", type=Path, help="bundle、nativeディレクトリ、または現行セッションJSON"
    )
    args = parser.parse_args()
    try:
        result = verify(args.source, args.input)
    except Exception as exc:
        # Native exception messages may contain conversation text.
        print(
            json.dumps({"ok": False, "error_type": type(exc).__name__}), file=sys.stderr
        )
        return 1
    print(json.dumps(result, ensure_ascii=False, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
