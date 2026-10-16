//! Exact native loader/serializer projection for ClawCodex 17c950b9.

use anyhow::{Context, Result, bail, ensure};
use serde_json::{Map, Value, json};

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64() != Some(0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
}

fn string_field<'a>(
    object: &'a Map<String, Value>,
    name: &str,
    default: &'a str,
) -> Result<&'a str> {
    match object.get(name) {
        None => Ok(default),
        Some(Value::String(value)) => Ok(value),
        Some(_) => bail!("{name}には文字列が必要です"),
    }
}

fn block(value: &Value) -> Result<Value> {
    let object = value
        .as_object()
        .context("contentの各ブロックにはオブジェクトが必要です")?;
    let kind = string_field(object, "type", "")?;
    Ok(match kind {
        "text" => json!({"type": kind, "text": string_field(object, "text", "")?}),
        "tool_use" => json!({
            "type": kind,
            "id": string_field(object, "id", "")?,
            "name": string_field(object, "name", "")?,
            "input": object.get("input").filter(|value| value.is_object()).cloned().unwrap_or(json!({})),
        }),
        "tool_result" => {
            let content = object.get("content").cloned().unwrap_or(json!(""));
            ensure!(
                content.is_string() || content.is_array(),
                "tool_result.contentの型が不正です"
            );
            // The native loader does not hydrate dictionaries inside a result.
            json!({
                "type": kind,
                "tool_use_id": string_field(object, "tool_use_id", "")?,
                "content": content,
                "is_error": object.get("is_error").is_some_and(truthy),
            })
        }
        "thinking" => {
            let mut result =
                json!({"type": kind, "thinking": string_field(object, "thinking", "")?});
            if let Some(signature) = object.get("signature").filter(|value| value.is_string()) {
                result["signature"] = signature.clone();
            }
            result
        }
        "redacted_thinking" => json!({"type": kind, "data": string_field(object, "data", "")?}),
        "image" | "document" => {
            let source = object.get("source").cloned().unwrap_or(json!({}));
            let source = if source.is_object() {
                source
            } else {
                json!({"value": source})
            };
            json!({"type": kind, "source": source})
        }
        _ => value.clone(),
    })
}

fn optional(result: &mut Value, name: &str, value: Option<&Value>) {
    if let Some(value) = value.filter(|value| !value.is_null() && **value != Value::Bool(false)) {
        result[name] = value.clone();
    }
}

/// Project a valid JSON message through ClawCodex's loader and serializer.
/// Missing identities and invalid message shapes are rejected, never invented.
pub fn normalize_claw(record: &Value) -> Result<Value> {
    let data = record
        .as_object()
        .context("ClawCodexのメッセージがオブジェクトではありません")?;
    let uuid = data
        .get("uuid")
        .and_then(Value::as_str)
        .context("uuidが必要です")?;
    let timestamp = data
        .get("timestamp")
        .and_then(Value::as_str)
        .context("timestampが必要です")?;
    let role = string_field(data, "role", "user")?;
    let kind = string_field(data, "type", role)?;
    ensure!(
        ["user", "assistant", "system"].contains(&role),
        "メッセージのroleが不正です"
    );
    ensure!(
        ["user", "assistant", "system", "progress", "attachment"].contains(&kind),
        "メッセージのtypeが不正です"
    );
    let content = match data.get("content") {
        None => json!(""),
        Some(Value::String(value)) => json!(value),
        Some(Value::Array(values)) => {
            Value::Array(values.iter().map(block).collect::<Result<_>>()?)
        }
        Some(_) => bail!("contentには文字列または配列が必要です"),
    };
    let mut result = json!({
        "role": role, "type": kind, "uuid": uuid, "timestamp": timestamp, "content": content,
        "isMeta": data.get("isMeta").or_else(|| data.get("_is_internal")).is_some_and(truthy),
        "isVirtual": data.get("isVirtual").is_some_and(truthy),
        "isCompactSummary": data.get("isCompactSummary").is_some_and(truthy),
    });
    optional(&mut result, "origin", data.get("origin"));
    if kind == "assistant" || role == "assistant" {
        result["role"] = json!("assistant");
        result["type"] = json!("assistant");
        if data.get("isApiErrorMessage").is_some_and(truthy) {
            result["isApiErrorMessage"] = json!(true);
        }
        for name in ["stop_reason", "model", "errorDetails"] {
            optional(
                &mut result,
                name,
                data.get(name).filter(|value| value.is_string()),
            );
        }
        for name in ["usage", "apiError", "error"] {
            optional(
                &mut result,
                name,
                data.get(name).filter(|value| value.is_object()),
            );
        }
    } else if kind == "progress" {
        result["role"] = json!("system");
        result["isVirtual"] = json!(false);
        result["isCompactSummary"] = json!(false);
        result["toolUseID"] = json!(string_field(data, "toolUseID", "")?);
        result["parentToolUseID"] = json!(string_field(data, "parentToolUseID", "")?);
        optional(&mut result, "data", data.get("data"));
        optional(
            &mut result,
            "progress",
            data.get("progress").filter(|value| value.is_string()),
        );
    } else if kind == "system" || role == "system" {
        result["role"] = json!("system");
        result["type"] = json!("system");
        if data.get("preventContinuation").is_some_and(truthy) {
            result["preventContinuation"] = json!(true);
        }
        for name in ["subtype", "level", "toolUseID"] {
            optional(
                &mut result,
                name,
                data.get(name).filter(|value| value.is_string()),
            );
        }
    } else {
        let attachments = match data.get("attachments") {
            Some(Value::Array(values)) => {
                ensure!(
                    values.iter().all(Value::is_object),
                    "attachmentsの各要素にはオブジェクトが必要です"
                );
                values.clone()
            }
            _ => Vec::new(),
        };
        result["role"] = json!("user");
        if kind == "attachment" || !attachments.is_empty() {
            result["type"] = json!("attachment");
            if !attachments.is_empty() {
                result["attachments"] = json!(attachments);
            }
        } else {
            result["type"] = json!("user");
            for name in [
                "toolUseResult",
                "sourceToolAssistantUUID",
                "permissionMode",
                "imagePasteIds",
                "summarizeMetadata",
            ] {
                optional(&mut result, name, data.get(name));
            }
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_native_losses_and_keeps_unknown_nested_results() {
        let record = json!({"uuid":"u", "timestamp":"t", "role":"assistant", "requestId":"req", "extra":1,
            "content":[{"type":"thinking","thinking":"reason","signature":"sig","cache_control":{}},
                {"type":"tool_result","tool_use_id":"tool","content":[{"type":"text","text":"x","extra":1}],
                    "metadata":{},"_content_ref":"external"}, {"type":"unknown","extra":true}]});
        let result = normalize_claw(&record).unwrap();
        assert!(result.get("requestId").is_none());
        assert!(result.get("extra").is_none());
        assert_eq!(
            result["content"][0],
            json!({"type":"thinking","thinking":"reason","signature":"sig"})
        );
        assert!(result["content"][1].get("metadata").is_none());
        assert!(result["content"][1].get("_content_ref").is_none());
        assert_eq!(
            result["content"][1]["content"],
            record["content"][1]["content"]
        );
        assert_eq!(result["content"][2], record["content"][2]);
    }

    #[test]
    fn handles_progress_attachment_defaults_and_false_identity() {
        let progress = normalize_claw(
            &json!({"uuid":"u","timestamp":"t","role":"system","type":"progress",
            "isVirtual":true,"isCompactSummary":true,"data":0,"origin":false}),
        )
        .unwrap();
        assert_eq!(progress["isVirtual"], false);
        assert_eq!(progress["isCompactSummary"], false);
        assert_eq!(progress["toolUseID"], "");
        assert_eq!(progress["data"], 0);
        assert!(progress.get("origin").is_none());
        let attachment = normalize_claw(
            &json!({"uuid":"u","timestamp":"t","role":"user","attachments":[{}],
            "toolUseResult":{"ignored":true}}),
        )
        .unwrap();
        assert_eq!(attachment["type"], "attachment");
        assert!(attachment.get("toolUseResult").is_none());
        assert!(normalize_claw(&json!({"timestamp":"t"})).is_err());
    }
}
