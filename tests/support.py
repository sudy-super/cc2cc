"""Exercise the Rust executable without importing application internals."""

from __future__ import annotations

import json
import os
import subprocess
from pathlib import Path


class BridgeError(ValueError):
    pass


def binary_path() -> str:
    return os.environ.get(
        "CC2CC_CORE_BIN",
        str(Path(__file__).resolve().parents[1] / "target/debug/examples/core-driver"),
    )


def command(*arguments: object) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [binary_path(), *(str(value) for value in arguments)],
        capture_output=True,
        text=True,
        timeout=60,
    )


def _invoke(arguments: list[object]) -> dict:
    result = command(*arguments)
    if result.returncode:
        raise BridgeError(
            result.stderr.strip() or f"cc2cc exited with status {result.returncode}"
        )
    value = json.loads(result.stdout)
    if not isinstance(value, dict):
        raise AssertionError("cc2cc stdout must contain one JSON object")
    return value


def convert(
    source_path,
    target,
    output,
    *,
    source_format=None,
    leaf=None,
    provenance=None,
    cwd=None,
    session_id=None,
    transcripts_dir=None,
    dry_run=False,
    attachments=None,
):
    arguments = ["convert", source_path, "--to", target, "--output", output]
    for option, value in (
        ("--from", source_format),
        ("--leaf", leaf),
        ("--provenance", provenance),
        ("--cwd", cwd),
        ("--session-id", session_id),
        ("--transcripts-dir", transcripts_dir),
    ):
        if value is not None:
            arguments.extend([option, value])
    for attachment in attachments or []:
        arguments.extend(["--attachments", attachment])
    if dry_run:
        arguments.append("--dry-run")
    return _invoke(arguments)


def verify(bundle):
    return _invoke(["verify", bundle])


def install(bundle, *, home=None, session_id=None, dry_run=False):
    arguments = ["install", bundle]
    for option, value in (("--home", home), ("--session-id", session_id)):
        if value is not None:
            arguments.extend([option, value])
    if dry_run:
        arguments.append("--dry-run")
    return _invoke(arguments)
