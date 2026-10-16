"""Projection of ClawCodex 17c950b9 message loading followed by serialization.

Keep this projection separate from conversion: it identifies the exact changes
the native loader makes without treating unrelated edits as a native re-save.
"""

from __future__ import annotations

from collections.abc import Mapping
from copy import deepcopy
from typing import Any


def _block(block: Any) -> dict:
    if not isinstance(block, Mapping):
        return {"type": "text", "text": str(block)}
    kind = str(block.get("type", ""))
    if kind == "text":
        return {"type": kind, "text": str(block.get("text", ""))}
    if kind == "tool_use":
        raw_input = block.get("input", {})
        return {
            "type": kind,
            "id": str(block.get("id", "")),
            "name": str(block.get("name", "")),
            "input": dict(raw_input) if isinstance(raw_input, Mapping) else {},
        }
    if kind == "tool_result":
        content = block.get("content", "")
        return {
            "type": kind,
            "tool_use_id": str(block.get("tool_use_id", "")),
            # Nested tool-result dictionaries are not hydrated by the loader.
            "content": content if isinstance(content, (str, list)) else str(content),
            "is_error": bool(block.get("is_error", False)),
        }
    if kind == "thinking":
        result = {"type": kind, "thinking": str(block.get("thinking", ""))}
        if isinstance(block.get("signature"), str):
            result["signature"] = block["signature"]
        return result
    if kind == "redacted_thinking":
        return {"type": kind, "data": str(block.get("data", ""))}
    if kind in ("image", "document"):
        source = block.get("source", {})
        return {
            "type": kind,
            "source": dict(source)
            if isinstance(source, Mapping)
            else {"value": source},
        }
    return dict(block)


def normalize_claw(record: dict) -> dict:
    """Match ``message_to_dict(message_from_dict(record))`` for JSON records.

    UUID and timestamp must be supplied; the native loader generates missing
    identities, which cannot be predicted and must not broaden edit matching.
    """
    if not isinstance(record, dict):
        raise ValueError("ClawCodexのメッセージがオブジェクトではありません")
    if "uuid" not in record or "timestamp" not in record:
        raise ValueError("ClawCodexの正規化にはuuidとtimestampが必要です")
    data = deepcopy(record)
    role = str(data.get("role", "user"))
    kind = str(data.get("type", role))
    content = data.get("content", "")
    if isinstance(content, list):
        content = [_block(block) for block in content]
    elif not isinstance(content, str):
        content = str(content)
    result = {
        "role": role,
        "content": content,
        "type": kind,
        "uuid": str(data["uuid"]),
        "timestamp": str(data["timestamp"]),
        "isMeta": bool(data.get("isMeta", data.get("_is_internal", False))),
        "isVirtual": bool(data.get("isVirtual", False)),
        "isCompactSummary": bool(data.get("isCompactSummary", False)),
    }
    optional: dict[str, Any] = {"origin": data.get("origin")}

    if kind == "assistant" or role == "assistant":
        result.update(role="assistant", type="assistant")
        optional["isApiErrorMessage"] = bool(data.get("isApiErrorMessage", False))
        for name in ("stop_reason", "model", "errorDetails"):
            if isinstance(data.get(name), str):
                optional[name] = data[name]
        for name in ("usage", "apiError", "error"):
            if isinstance(data.get(name), dict):
                optional[name] = data[name]
    elif kind == "progress":
        result.update(
            role="system", type="progress", isVirtual=False, isCompactSummary=False
        )
        optional.update(
            toolUseID=str(data.get("toolUseID", "")),
            parentToolUseID=str(data.get("parentToolUseID", "")),
            data=data.get("data"),
        )
        if isinstance(data.get("progress"), str):
            optional["progress"] = data["progress"]
    elif kind == "system" or role == "system":
        result.update(role="system", type="system")
        optional["preventContinuation"] = bool(data.get("preventContinuation", False))
        for name in ("subtype", "level", "toolUseID"):
            if isinstance(data.get(name), str):
                optional[name] = data[name]
    elif kind == "attachment" or role == "user":
        raw_attachments = data.get("attachments")
        attachments = (
            [dict(item) for item in raw_attachments]
            if isinstance(raw_attachments, list)
            else []
        )
        if kind == "attachment" or attachments:
            result.update(role="user", type="attachment")
            if attachments:
                result["attachments"] = attachments
        else:
            result.update(role="user", type="user")
            for name in (
                "toolUseResult",
                "sourceToolAssistantUUID",
                "permissionMode",
                "imagePasteIds",
                "summarizeMetadata",
            ):
                optional[name] = data.get(name)
    else:
        # The base Message dataclass defaults type to user even for other roles.
        result["type"] = "user"

    for name, value in optional.items():
        if value is not None and value is not False:
            result[name] = value
    return result
