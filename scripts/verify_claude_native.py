#!/usr/bin/env python3
"""Check synthetic history in the real Claude Code UI with OS networking disabled.

No prompts are submitted. Only known setup dialogs and the history expansion
shortcut receive input. All configuration and history live in a temporary directory.
"""

from __future__ import annotations

import argparse
import contextlib
import fcntl
import json
import os
import pty
import re
import select
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time
from pathlib import Path
from uuid import uuid4

USER_MARKER = "CC2CC_SYNTHETIC_USER"
ASSISTANT_MARKER = "CC2CC_SYNTHETIC_ASSISTANT"
RESULT_MARKER = "CC2CC_SYNTHETIC_TOOL_RESULT"
STAMP = "2026-10-16T00:00:00Z"


def _json_command(arguments: list[str], env: dict[str, str], cwd: Path) -> dict:
    result = subprocess.run(
        arguments,
        check=False,
        env=env,
        cwd=cwd,
        capture_output=True,
        text=True,
        timeout=15,
    )
    if result.returncode:
        raise RuntimeError("conversion command failed")
    value = json.loads(result.stdout)
    if not isinstance(value, dict):
        raise TypeError("conversion command did not return an object")
    return value


def _clean(data: bytes) -> str:
    text = data.decode("utf-8", errors="replace")
    text = re.sub(r"\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)", "", text)
    text = re.sub(r"\x1b\[[0-?]*[ -/]*[@-~]", "", text)
    return re.sub(r"\x1b[78]", "", text)


def _stop(process: subprocess.Popen) -> None:
    if process.poll() is not None:
        return
    with contextlib.suppress(ProcessLookupError):
        os.killpg(process.pid, signal.SIGTERM)
    try:
        process.wait(timeout=3)
    except subprocess.TimeoutExpired:
        with contextlib.suppress(ProcessLookupError):
            os.killpg(process.pid, signal.SIGKILL)
        process.wait(timeout=3)


def _read_ui(
    arguments: list[str], env: dict[str, str], cwd: Path, timeout: float
) -> dict:
    master, slave = pty.openpty()
    process = None
    output = bytearray()
    expanded_at = None
    answered = set()
    try:
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 45, 130, 0, 0))
        process = subprocess.Popen(
            arguments,
            cwd=cwd,
            env=env,
            stdin=slave,
            stdout=slave,
            stderr=slave,
            start_new_session=True,
        )
        os.close(slave)
        slave = -1
        start = time.monotonic()
        while time.monotonic() - start < timeout:
            if select.select([master], [], [], 0.2)[0]:
                try:
                    chunk = os.read(master, 65536)
                except OSError:
                    break
                if not chunk:
                    break
                output.extend(chunk)
            text = _clean(output)
            if (
                "Detected a custom API key in your environment" in text
                and "Enter y/n:" in text
                and "key" not in answered
            ):
                os.write(master, b"y\r")
                answered.add("key")
            theme_prompts = (
                "Choose the text style",
                "Choose a theme",
                "Let’s get started",
                "Let's get started",
            )
            if (
                any(prompt in text for prompt in theme_prompts)
                and "theme" not in answered
            ):
                os.write(master, b"\r")
                answered.add("theme")
            if (
                "Do you trust the files in this folder?" in text
                and "trust" not in answered
            ):
                os.write(master, b"\r")
                answered.add("trust")
            loaded = USER_MARKER in text and ASSISTANT_MARKER in text and "Read" in text
            if loaded and expanded_at is None:
                os.write(
                    master, b"\x0f"
                )  # Ctrl+O expands recorded tool output; it does not send a prompt.
                expanded_at = time.monotonic()
            if loaded and RESULT_MARKER in text:
                return {
                    "user_visible": True,
                    "assistant_visible": True,
                    "tool_visible": True,
                    "tool_result_visible": True,
                }
            if (
                loaded
                and expanded_at is not None
                and time.monotonic() - expanded_at > 3
            ):
                return {
                    "user_visible": True,
                    "assistant_visible": True,
                    "tool_visible": True,
                    "tool_result_visible": False,
                }
            if process.poll() is not None:
                break
        raise RuntimeError("synthetic history did not appear in the native UI")
    finally:
        if process is not None:
            _stop(process)
        os.close(master)
        if slave >= 0:
            os.close(slave)


def verify(binary: Path, claude: str, timeout: float) -> dict:
    if sys.platform != "darwin" or not Path("/usr/bin/sandbox-exec").is_file():
        raise RuntimeError("macOS sandbox-exec is required")
    binary = binary.expanduser().resolve(strict=True)
    claude_executable = shutil.which(claude)
    if not claude_executable:
        raise FileNotFoundError("Claude Code executable is required")
    with tempfile.TemporaryDirectory(prefix="cc2cc-claude-native-") as temporary:
        base = Path(temporary).resolve()
        os.chmod(base, 0o700)
        config, temp = base / "claude-config", base / "tmp"
        for directory in (config, temp):
            directory.mkdir(mode=0o700)
        env = {
            key: os.environ[key]
            for key in ("PATH", "HOME", "USER", "SHELL", "LANG")
            if key in os.environ
        }
        env.update(
            TMPDIR=str(temp),
            CLAUDE_CONFIG_DIR=str(config),
            ANTHROPIC_API_KEY="cc2cc-offline-synthetic-key",
            ANTHROPIC_BASE_URL="http://127.0.0.1:1",
            DISABLE_TELEMETRY="1",
            DISABLE_ERROR_REPORTING="1",
            CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC="1",
            TERM="xterm-256color",
        )
        profile = (
            "(version 1) (allow default) (deny network*) (deny file-write*) "
            f"(allow file-write* (subpath {json.dumps(str(base))}) "
            '(literal "/dev/null") (literal "/dev/tty") (regex #"^/dev/ttys[0-9]+$"))'
        )
        sandbox = ["/usr/bin/sandbox-exec", "-p", profile]
        version_run = subprocess.run(
            [*sandbox, claude_executable, "--version"],
            check=False,
            env=env,
            cwd=base,
            capture_output=True,
            text=True,
            timeout=10,
        )
        if version_run.returncode:
            raise RuntimeError("sandboxed Claude Code version check failed")
        version_match = re.search(r"\b\d+\.\d+\.\d+\b", version_run.stdout)
        if not version_match:
            raise ValueError("Claude Code version is missing")
        user_id, tool_id, result_id, assistant_id = (str(uuid4()) for _ in range(4))
        messages = [
            {
                "uuid": user_id,
                "timestamp": STAMP,
                "type": "user",
                "role": "user",
                "content": USER_MARKER,
            },
            {
                "uuid": tool_id,
                "timestamp": STAMP,
                "type": "assistant",
                "role": "assistant",
                "model": "claude-sonnet-4-6",
                "stop_reason": "tool_use",
                "content": [
                    {
                        "type": "tool_use",
                        "id": "synthetic_read",
                        "name": "Read",
                        "input": {"file_path": "/synthetic/cc2cc-native-check.txt"},
                    }
                ],
            },
            {
                "uuid": result_id,
                "timestamp": STAMP,
                "type": "user",
                "role": "user",
                "content": [
                    {
                        "type": "tool_result",
                        "tool_use_id": "synthetic_read",
                        "content": RESULT_MARKER,
                        "is_error": False,
                    }
                ],
            },
            {
                "uuid": assistant_id,
                "timestamp": STAMP,
                "type": "assistant",
                "role": "assistant",
                "model": "claude-sonnet-4-6",
                "stop_reason": "end_turn",
                "content": ASSISTANT_MARKER,
            },
        ]
        claw_config = base / "claw-config"
        (claw_config / "sessions").mkdir(parents=True)
        env["CLAWCODEX_CONFIG_DIR"] = str(claw_config)
        source = claw_config / "sessions" / "synthetic_claw_native.json"
        source.write_text(
            json.dumps(
                {
                    "session_id": "synthetic_claw_native",
                    "cwd": str(base),
                    "conversation": {"messages": messages, "max_history": 2000},
                }
            ),
            encoding="utf-8",
        )
        converted = _json_command(
            [str(binary), "convert", "synthetic_claw_native", "--to", "claude"],
            env,
            base,
        )
        installed = converted
        native = Path(installed["path"]).resolve(strict=True)
        if not native.is_relative_to(config):
            raise ValueError("installed history is outside the temporary configuration")
        (config / "settings.json").write_text("{}", encoding="utf-8")
        (config / ".claude.json").write_text(
            json.dumps(
                {
                    "hasCompletedOnboarding": True,
                    "theme": "dark",
                    "projects": {
                        str(base): {"hasTrustDialogAccepted": True, "allowedTools": []}
                    },
                }
            ),
            encoding="utf-8",
        )
        visible = _read_ui(
            [
                *sandbox,
                claude_executable,
                "--bare",
                "--safe-mode",
                "--strict-mcp-config",
                "--setting-sources",
                "",
                "--ax-screen-reader",
                "--resume",
                str(native),
            ],
            env,
            base,
            timeout,
        )
        return {
            "ok": True,
            "claude_version": version_match.group(),
            "message_count": converted["message_count"],
            "network_blocked": True,
            "user_files_write_blocked": True,
            "prompts_submitted": 0,
            **visible,
        }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--binary",
        type=Path,
        default=Path(__file__).resolve().parents[1] / "target/debug/cc2cc",
    )
    parser.add_argument("--claude", default="claude")
    parser.add_argument("--timeout", type=float, default=30)
    args = parser.parse_args()
    try:
        if args.timeout <= 0 or args.timeout > 60:
            raise ValueError("timeout must be between 0 and 60 seconds")
        result = verify(args.binary, args.claude, args.timeout)
    except (
        OSError,
        ValueError,
        TypeError,
        RuntimeError,
        subprocess.SubprocessError,
    ) as exc:
        # Native errors and captured terminal output can contain session data.
        print(
            json.dumps({"ok": False, "error_type": type(exc).__name__}), file=sys.stderr
        )
        return 1
    print(json.dumps(result, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
