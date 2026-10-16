use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::files::{Files, collect_tree, json_bytes, jsonl_bytes, parse_json, parse_jsonl};

pub const FORMATS: [&str; 2] = ["claude", "clawcodex"];
const MESSAGE_TYPES: [&str; 5] = ["user", "assistant", "system", "progress", "attachment"];
const COMMON_FIELDS: [&str; 24] = [
    "uuid",
    "timestamp",
    "isMeta",
    "isVirtual",
    "isCompactSummary",
    "origin",
    "toolUseResult",
    "sourceToolAssistantUUID",
    "permissionMode",
    "imagePasteIds",
    "summarizeMetadata",
    "requestId",
    "apiError",
    "error",
    "errorDetails",
    "isApiErrorMessage",
    "subtype",
    "level",
    "toolUseID",
    "parentToolUseID",
    "data",
    "progress",
    "preventContinuation",
    "attachments",
];

#[derive(Clone, Debug)]
pub struct Session {
    pub format: String,
    pub session_id: String,
    pub cwd: String,
    pub records: Vec<Value>,
    pub files: Files,
    pub transcript: String,
    pub metadata: Value,
}

pub fn uuid_string(value: &str) -> Result<String> {
    let parsed =
        Uuid::parse_str(value).context("セッションIDには小文字の標準UUIDを指定してください")?;
    ensure!(
        parsed.hyphenated().to_string() == value,
        "セッションIDには小文字の標準UUIDを指定してください"
    );
    Ok(value.to_owned())
}

fn valid_claw_id(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
}

fn string<'a>(value: &'a Value, name: &str) -> Result<&'a str> {
    value
        .get(name)
        .and_then(Value::as_str)
        .with_context(|| format!("{name}には文字列が必要です"))
}

fn cwd(metadata: &Value) -> Result<String> {
    match metadata.get("cwd") {
        None => Ok(String::new()),
        Some(Value::String(value)) => Ok(value.clone()),
        Some(_) => bail!("cwdには文字列が必要です"),
    }
}

fn companions(path: &Path, files: &mut Files) -> Result<()> {
    let related = path.with_extension("");
    if related.exists() {
        let prefix = related
            .file_name()
            .and_then(|name| name.to_str())
            .context("入力ファイル名がUTF-8ではありません")?;
        for (name, data) in collect_tree(&related)? {
            if !name.starts_with(".cc2cc/") {
                files.insert(format!("{prefix}/{name}"), data);
            }
        }
    }
    Ok(())
}

pub fn load_session(path: &Path, format: &str) -> Result<Session> {
    ensure!(FORMATS.contains(&format), "未対応の形式です: {format}");
    ensure!(
        !path.is_symlink(),
        "入力にシンボリックリンクは使用できません"
    );
    if format == "clawcodex" && path.is_dir() {
        let files = collect_tree(path)?;
        let metadata = parse_json(
            files
                .get("metadata.json")
                .context("metadata.jsonが必要です")?,
        )?;
        let sid = string(&metadata, "session_id")?.to_owned();
        ensure!(valid_claw_id(&sid), "ClawCodexのsession_idが不正です");
        let records = parse_jsonl(
            files
                .get("transcript.jsonl")
                .context("transcript.jsonlが必要です")?,
        )?;
        return Ok(Session {
            format: format.into(),
            session_id: sid,
            cwd: cwd(&metadata)?,
            records,
            files,
            transcript: "transcript.jsonl".into(),
            metadata,
        });
    }
    ensure!(
        path.is_file(),
        "入力は通常のセッションファイルである必要があります"
    );
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("入力ファイル名がUTF-8ではありません")?
        .to_owned();
    let raw = fs::read(path).context("入力セッションを読み込めません")?;
    let mut files = Files::new();
    files.insert(name.clone(), raw.clone());
    if format == "clawcodex" {
        let metadata = parse_json(&raw)?;
        let records = metadata
            .get("conversation")
            .and_then(|value| value.get("messages"))
            .and_then(Value::as_array)
            .context("ClawCodexのconversation.messages配列が必要です")?
            .clone();
        ensure!(
            !records.is_empty() && records.iter().all(Value::is_object),
            "conversation.messagesには空でないメッセージ配列が必要です"
        );
        let sid = string(&metadata, "session_id")?.to_owned();
        ensure!(valid_claw_id(&sid), "ClawCodexのsession_idが不正です");
        companions(path, &mut files)?;
        return Ok(Session {
            format: format.into(),
            session_id: sid,
            cwd: cwd(&metadata)?,
            records,
            files,
            transcript: name,
            metadata,
        });
    }
    ensure!(
        path.extension().and_then(|value| value.to_str()) == Some("jsonl"),
        "Claude Codeには生のセッションJSONLを指定してください"
    );
    companions(path, &mut files)?;
    let records = parse_jsonl(&raw)?;
    let ids: BTreeSet<_> = records
        .iter()
        .filter_map(|record| record.get("sessionId").and_then(Value::as_str))
        .collect();
    ensure!(
        ids.len() == 1,
        "Claude Code履歴には単一のsessionIdが必要です"
    );
    let sid = uuid_string(ids.first().context("sessionIdがありません")?)?;
    let cwd = records
        .iter()
        .rev()
        .find_map(|record| record.get("cwd").and_then(Value::as_str))
        .unwrap_or("")
        .to_owned();
    Ok(Session {
        format: format.into(),
        session_id: sid,
        cwd,
        records,
        files,
        transcript: name,
        metadata: json!({}),
    })
}

pub fn resolve_content(record: &Value, files: &Files) -> Result<Value> {
    fn walk(value: &mut Value, files: &Files) -> Result<()> {
        match value {
            Value::Object(object) => {
                if let Some(reference) = object.get("_content_ref") {
                    let reference = reference.as_str().context("不正な_content_refです")?;
                    ensure!(
                        !reference.is_empty()
                            && !reference.contains(['/', '\\'])
                            && reference != "."
                            && reference != "..",
                        "不正な_content_refです"
                    );
                    let name = format!("content/{reference}.txt");
                    let data = files
                        .get(&name)
                        .with_context(|| format!("外部保存された内容がありません: {name}"))?;
                    let content = std::str::from_utf8(data)
                        .context("外部保存されたcontentはUTF-8ではありません")?;
                    object.insert("content".into(), json!(content));
                    object.remove("_content_ref");
                }
                for child in object.values_mut() {
                    walk(child, files)?;
                }
            }
            Value::Array(values) => {
                for child in values {
                    walk(child, files)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    let mut result = record.clone();
    walk(&mut result, files)?;
    Ok(result)
}

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

fn parent(record: &Value, name: &str) -> Result<Option<String>> {
    match record.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if value.is_empty() => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        _ => bail!("{name}には文字列かnullが必要です"),
    }
}

pub fn select_claude(records: &[Value], leaf: Option<&str>) -> Result<(Vec<Value>, Vec<String>)> {
    let mut nodes = BTreeMap::new();
    let mut order = Vec::new();
    for record in records {
        ensure!(record.is_object(), "履歴の各行にはオブジェクトが必要です");
        let kind = record.get("type").and_then(Value::as_str).unwrap_or("");
        if !MESSAGE_TYPES.contains(&kind)
            || record.get("isSidechain").is_some_and(truthy)
            || !record.get("uuid").is_some_and(truthy)
        {
            continue;
        }
        let uid = string(record, "uuid")?.to_owned();
        ensure!(
            !nodes.contains_key(&uid),
            "メッセージUUIDが重複しています: {uid}"
        );
        order.push(uid.clone());
        nodes.insert(uid, record);
    }
    ensure!(!nodes.is_empty(), "変換可能な会話がありません");
    let mut parents = BTreeSet::new();
    for record in nodes.values() {
        for name in ["parentUuid", "logicalParentUuid"] {
            if let Some(uid) = parent(record, name)? {
                parents.insert(uid);
            }
        }
    }
    let leaves: Vec<_> = order.iter().filter(|uid| !parents.contains(*uid)).collect();
    let automatic = leaf.is_none() && leaves.len() > 1;
    let leaf = match leaf {
        Some(leaf) => leaf.to_owned(),
        None => leaves
            .iter()
            .map(|uid| {
                let time = string(nodes[*uid], "timestamp")
                    .and_then(timestamp_datetime)
                    .with_context(|| format!("分岐の末端{uid}の日時が不正です"))?;
                Ok((time, *uid))
            })
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .max_by_key(|(time, _)| *time)
            .context("会話の末端がありません。親メッセージの参照を確認してください")?
            .1
            .clone(),
    };
    ensure!(
        nodes.contains_key(&leaf),
        "指定した--leafが会話にありません"
    );
    let mut cursor = Some(leaf.clone());
    let mut visited = BTreeSet::new();
    let mut chain = Vec::new();
    while let Some(uid) = cursor {
        ensure!(visited.insert(uid.clone()), "parentUuidが循環しています");
        let record = nodes
            .get(&uid)
            .with_context(|| format!("親メッセージがありません: {uid}"))?;
        chain.push((*record).clone());
        cursor = parent(record, "parentUuid")?;
    }
    chain.reverse();
    let excluded = records.len() - chain.len();
    let mut warnings = Vec::new();
    if automatic {
        warnings.push(format!("最も新しい分岐を選択しました (末端UUID: {leaf})"));
    }
    if excluded > 0 {
        warnings.push(format!(
            "選択した会話以外の{excluded}件 (別分岐、独自イベントなど)は原本に保存しました"
        ));
    }
    Ok((chain, warnings))
}

fn common_fields(record: &Value) -> Value {
    Value::Object(
        COMMON_FIELDS
            .iter()
            .filter_map(|name| {
                record
                    .get(*name)
                    .map(|value| ((*name).into(), value.clone()))
            })
            .collect(),
    )
}

pub fn to_claw(record: &Value) -> Result<Value> {
    let kind = string(record, "type")?;
    ensure!(
        MESSAGE_TYPES.contains(&kind),
        "未対応のメッセージ型です: {kind}"
    );
    let message = if ["user", "assistant"].contains(&kind) {
        let message = record.get("message").context("messageがありません")?;
        ensure!(
            message.get("content").is_some(),
            "{kind}メッセージにcontentがありません"
        );
        message
    } else {
        record
    };
    ensure!(message.is_object(), "messageがオブジェクトではありません");
    let default_role = if ["user", "assistant"].contains(&kind) {
        kind
    } else {
        "system"
    };
    let role = match message.get("role") {
        None => default_role,
        Some(value) => value.as_str().context("roleには文字列が必要です")?,
    };
    ensure!(
        ["user", "assistant", "system"].contains(&role),
        "roleが不正です"
    );
    let mut result = common_fields(record);
    result["type"] = json!(kind);
    result["role"] = json!(role);
    result["content"] = message.get("content").cloned().unwrap_or(json!(""));
    for name in ["model", "usage", "stop_reason", "requestId"] {
        if let Some(value) = message.get(name) {
            result[name] = value.clone();
        }
    }
    if kind == "attachment" {
        result["role"] = json!("user");
        if let Some(attachment) = record.get("attachment").filter(|value| value.is_object()) {
            result["attachments"] = json!([attachment]);
        }
        if let Some(rendered) = record.get("rendered").and_then(Value::as_array) {
            let mut blocks = Vec::new();
            for item in rendered {
                match item.get("content") {
                    Some(Value::String(text)) => blocks.push(json!({"type":"text", "text":text})),
                    Some(Value::Array(values)) => blocks.extend(values.clone()),
                    _ => bail!("attachment.renderedのcontentが不正です"),
                }
            }
            result["content"] = json!(blocks);
        }
        if !truthy(&result["content"]) {
            result["isVirtual"] = json!(true);
        }
    }
    Ok(result)
}

pub fn to_claude(record: &Value, sid: &str, cwd: &str, parent: Option<&str>) -> Result<Value> {
    let original_kind = string(record, "type")?;
    ensure!(
        MESSAGE_TYPES.contains(&original_kind),
        "未対応のClawCodexメッセージ型です: {original_kind}"
    );
    let kind = if original_kind == "attachment" {
        "user"
    } else {
        original_kind
    };
    let mut result = common_fields(record);
    let uuid = match record.get("uuid") {
        None | Some(Value::Null) => Uuid::new_v4().to_string(),
        Some(Value::String(value)) if value.is_empty() => Uuid::new_v4().to_string(),
        Some(Value::String(value)) => value.clone(),
        Some(_) => bail!("uuidには文字列が必要です"),
    };
    for (name, value) in [
        ("type", json!(kind)),
        ("uuid", json!(uuid)),
        ("parentUuid", json!(parent)),
        ("sessionId", json!(sid)),
        ("cwd", json!(cwd)),
        ("isSidechain", json!(false)),
        ("userType", json!("external")),
        ("version", json!("2.1.301")),
    ] {
        result[name] = value;
    }
    if original_kind == "attachment" {
        result["isMeta"] = json!(true);
    }
    let content = record.get("content").cloned().unwrap_or(json!(""));
    if ["user", "assistant"].contains(&kind) {
        let mut message = json!({"role":kind, "content":content});
        if kind == "assistant" {
            message["id"] = json!(format!("msg_{}", uuid.replace('-', "")));
            message["type"] = json!("message");
            message["stop_sequence"] = Value::Null;
            for name in ["model", "usage", "stop_reason"] {
                if let Some(value) = record.get(name) {
                    message[name] = value.clone();
                }
            }
            message
                .as_object_mut()
                .unwrap()
                .entry("model")
                .or_insert(json!("imported"));
            message
                .as_object_mut()
                .unwrap()
                .entry("stop_reason")
                .or_insert(json!("end_turn"));
        }
        result["message"] = message;
    } else {
        result["content"] = content;
    }
    Ok(result)
}

pub fn validate_messages(records: &[Value], format: &str) -> Result<()> {
    ensure!(FORMATS.contains(&format), "未対応の形式です: {format}");
    let mut uuids = BTreeSet::new();
    let mut pending = BTreeSet::new();
    let mut all_tools = BTreeSet::new();
    for record in records {
        let kind = string(record, "type")?;
        ensure!(
            MESSAGE_TYPES.contains(&kind),
            "未対応のメッセージ型です: {kind}"
        );
        let uid = string(record, "uuid")?;
        ensure!(
            !uid.is_empty() && uuids.insert(uid),
            "メッセージUUIDが未指定か重複しています"
        );
        string(record, "timestamp")?;
        let message = if format == "claude" && ["user", "assistant"].contains(&kind) {
            record.get("message").context("messageがありません")?
        } else {
            record
        };
        ensure!(message.is_object(), "messageがオブジェクトではありません");
        if let Some(role) = message.get("role") {
            let expected = if kind == "attachment" {
                "user"
            } else if kind == "progress" {
                "system"
            } else {
                kind
            };
            ensure!(
                role.as_str() == Some(expected),
                "roleとメッセージ型が一致しません"
            );
        }
        let empty = json!("");
        let content = message.get("content").unwrap_or(&empty);
        ensure!(
            content.is_string() || content.is_array(),
            "contentには文字列か配列が必要です"
        );
        let blocks = content.as_array().map(Vec::as_slice).unwrap_or(&[]);
        for block in blocks {
            ensure!(
                block.is_object() && block.get("type").is_some_and(Value::is_string),
                "contentの各要素にはtype付きオブジェクトが必要です"
            );
        }
        if !["user", "assistant"].contains(&kind) {
            continue;
        }
        let results: Vec<_> = blocks
            .iter()
            .filter(|block| block["type"] == "tool_result")
            .collect();
        ensure!(
            results.is_empty() || kind == "user",
            "tool_resultはuserメッセージに必要です"
        );
        for block in &results {
            let tid = string(block, "tool_use_id")?;
            ensure!(
                pending.remove(tid),
                "対応する呼び出しがないtool_resultです: {tid}"
            );
        }
        let calls: Vec<_> = blocks
            .iter()
            .filter(|block| block["type"] == "tool_use")
            .collect();
        ensure!(
            calls.is_empty() || kind == "assistant",
            "tool_useはassistantメッセージに必要です"
        );
        ensure!(
            kind != "assistant" || pending.is_empty(),
            "ツール結果が揃う前に次のassistantメッセージがあります"
        );
        for block in calls {
            let tid = string(block, "id")?;
            ensure!(
                !tid.is_empty() && all_tools.insert(tid.to_owned()),
                "tool_useのIDが未指定か重複しています"
            );
            ensure!(
                block.get("input").is_some_and(Value::is_object)
                    && block.get("name").is_some_and(Value::is_string),
                "tool_useにはnameとinputオブジェクトが必要です"
            );
            pending.insert(tid.to_owned());
        }
        ensure!(
            kind != "user" || pending.is_empty() || !results.is_empty(),
            "ツール結果が未完了のまま次のuserメッセージがあります"
        );
    }
    ensure!(
        pending.is_empty(),
        "未完了のツール呼び出しがあります: {}",
        pending.into_iter().collect::<Vec<_>>().join(", ")
    );
    Ok(())
}

fn timestamp_datetime(value: &str) -> Result<DateTime<Utc>> {
    if let Ok(time) = DateTime::parse_from_rfc3339(value) {
        ensure!(
            time.timestamp_subsec_nanos() < 1_000_000_000,
            "うるう秒のtimestampには対応していません"
        );
        return Ok(time.to_utc());
    }
    for format in [
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M",
    ] {
        if let Ok(time) = NaiveDateTime::parse_from_str(value, format) {
            ensure!(
                time.and_utc().timestamp_subsec_nanos() < 1_000_000_000,
                "うるう秒のtimestampには対応していません"
            );
            return Ok(time.and_utc());
        }
    }
    if let Ok(date) = NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        return Ok(date
            .and_hms_opt(0, 0, 0)
            .context("不正なtimestampです")?
            .and_utc());
    }
    bail!("不正なtimestampです")
}

pub fn timestamp_seconds(value: &str) -> Result<f64> {
    let time = timestamp_datetime(value)?;
    Ok(time.timestamp() as f64 + f64::from(time.timestamp_subsec_nanos()) / 1_000_000_000.0)
}

pub fn build_files(
    format: &str,
    sid: &str,
    cwd: &str,
    records: &[Value],
    metadata: Option<&Value>,
) -> Result<Files> {
    ensure!(FORMATS.contains(&format), "未対応の形式です: {format}");
    let mut files = Files::new();
    if format == "claude" {
        files.insert(format!("{sid}.jsonl"), jsonl_bytes(records)?);
        return Ok(files);
    }
    let mut meta = metadata.cloned().unwrap_or(json!({}));
    let object = meta
        .as_object_mut()
        .context("metadataにはオブジェクトが必要です")?;
    let mut conversation = object.get("conversation").cloned().unwrap_or(json!({}));
    let conv = conversation
        .as_object_mut()
        .context("conversationにはオブジェクトが必要です")?;
    let previous_max = match conv.get("max_history") {
        None => 0,
        Some(value) => value
            .as_u64()
            .context("max_historyには非負の整数が必要です")?,
    };
    let maximum = previous_max.max(2000).max(records.len() as u64 + 1000);
    conv.insert("messages".into(), json!(records));
    conv.insert("max_history".into(), json!(maximum));
    let last = records.last().context("会話が空です")?;
    object.insert("session_id".into(), json!(sid));
    object.insert("cwd".into(), json!(cwd));
    object.insert("message_count".into(), json!(records.len()));
    object.insert(
        "updated_at".into(),
        json!(timestamp_seconds(string(last, "timestamp")?)?),
    );
    object.insert("conversation".into(), conversation);
    for (name, value) in [
        ("name", "Imported conversation"),
        ("preview", "Imported conversation"),
        ("model", ""),
        ("provider", ""),
        ("mode", "normal"),
    ] {
        object.entry(name).or_insert(json!(value));
    }
    files.insert(format!("{sid}.json"), json_bytes(&meta)?);
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branches_default_to_last_on_ties_and_logical_parent_changes_leaf_detection() {
        let branches = vec![
            json!({"type":"user","uuid":"root","parentUuid":null,"timestamp":"2026-10-16T00:00:00Z"}),
            json!({"type":"assistant","uuid":"a","parentUuid":"root","timestamp":"2026-10-16T00:00:00Z"}),
            json!({"type":"assistant","uuid":"b","parentUuid":"root","timestamp":"2026-10-16T00:00:00Z"}),
        ];
        let (selected, warnings) = select_claude(&branches, None).unwrap();
        assert_eq!(selected.len(), 2);
        assert_eq!(selected[1]["uuid"], "b");
        assert_eq!(warnings.len(), 2);
        assert_eq!(
            select_claude(&branches, Some("a")).unwrap().0[1]["uuid"],
            "a"
        );
        let compacted = vec![
            branches[0].clone(),
            json!({"type":"system","uuid":"compact",
            "parentUuid":null,"logicalParentUuid":"root","timestamp":"2026-10-16T00:00:00Z"}),
        ];
        assert_eq!(
            select_claude(&compacted, None).unwrap().0[0]["uuid"],
            "compact"
        );
    }

    #[test]
    fn branches_compare_instants_without_losing_fractional_seconds() {
        for (left, right, expected) in [
            ("2026-10-16T00:01:00Z", "2026-10-16T00:00:00Z", "z-left"),
            (
                "2026-10-16T00:30:00Z",
                "2026-10-16T09:00:00+09:00",
                "z-left",
            ),
            (
                "2026-10-16T00:00:00.000000002Z",
                "2026-10-16T00:00:00.000000001Z",
                "z-left",
            ),
            (
                "2026-10-16T00:00:00Z",
                "2026-10-16T09:00:00+09:00",
                "a-right",
            ),
        ] {
            let records = vec![
                json!({"type":"user","uuid":"root","parentUuid":null}),
                json!({"type":"assistant","uuid":"z-left","parentUuid":"root","timestamp":left}),
                json!({"type":"assistant","uuid":"a-right","parentUuid":"root","timestamp":right}),
            ];
            assert_eq!(
                select_claude(&records, None).unwrap().0[1]["uuid"],
                expected
            );
            assert_eq!(
                select_claude(&records, Some("z-left")).unwrap().0[1]["uuid"],
                "z-left"
            );
        }
    }

    #[test]
    fn automatic_branch_selection_rejects_missing_or_invalid_timestamps() {
        for invalid in [
            Value::Null,
            json!(123),
            json!("invalid"),
            json!("2016-12-31T23:59:60Z"),
        ] {
            let mut records = vec![
                json!({"type":"user","uuid":"root","parentUuid":null}),
                json!({"type":"assistant","uuid":"bad","parentUuid":"root","timestamp":invalid}),
                json!({"type":"assistant","uuid":"valid","parentUuid":"root","timestamp":"2026-10-16T00:00:00Z"}),
            ];
            if records[1]["timestamp"].is_null() {
                records[1].as_object_mut().unwrap().remove("timestamp");
            }
            let error = select_claude(&records, None).unwrap_err();
            assert!(error.to_string().contains("末端bad"));
            assert!(select_claude(&records, Some("valid")).is_ok());
        }
    }

    #[test]
    fn automatic_branch_selection_rejects_cycles_and_broken_latest_branch() {
        let cycle = vec![
            json!({"type":"user","uuid":"a","parentUuid":"b"}),
            json!({"type":"assistant","uuid":"b","parentUuid":"a"}),
        ];
        assert!(select_claude(&cycle, None).is_err());
        assert!(select_claude(&cycle, Some("a")).is_err());
        let records = vec![
            json!({"type":"user","uuid":"root","parentUuid":null}),
            json!({"type":"assistant","uuid":"older","parentUuid":"root","timestamp":"2026-10-16T00:00:00Z"}),
            json!({"type":"assistant","uuid":"newer","parentUuid":"missing","timestamp":"2026-10-16T00:01:00Z"}),
        ];
        assert!(select_claude(&records, None).is_err());
    }

    #[test]
    fn resolves_external_content_and_rejects_missing_or_escaping_references() {
        let mut files = Files::new();
        files.insert("content/result.txt".into(), "complete".as_bytes().to_vec());
        let record = json!({"content":[{"type":"tool_result","_content_ref":"result","content":"placeholder"}]});
        let result = resolve_content(&record, &files).unwrap();
        assert_eq!(result["content"][0]["content"], "complete");
        assert!(result["content"][0].get("_content_ref").is_none());
        assert!(resolve_content(&json!({"_content_ref":"../result"}), &files).is_err());
        assert!(resolve_content(&json!({"_content_ref":"missing"}), &files).is_err());
    }

    #[test]
    fn attachments_preserve_rendered_context_and_do_not_create_empty_prompts() {
        let record = json!({"type":"attachment","uuid":"u","attachment":{"type":"test"},
            "rendered":[{"content":"context"},{"content":[{"type":"image","source":{}}]}]});
        let claw = to_claw(&record).unwrap();
        assert_eq!(claw["content"].as_array().unwrap().len(), 2);
        assert_eq!(claw["attachments"][0], record["attachment"]);
        let claude = to_claude(&claw, "session", "/tmp", None).unwrap();
        assert_eq!(claude["type"], "user");
        assert_eq!(claude["isMeta"], true);
        assert_eq!(claude["message"]["content"], claw["content"]);
        assert_eq!(
            to_claw(&json!({"type":"attachment"})).unwrap()["isVirtual"],
            true
        );
    }

    #[test]
    fn validates_pairing_without_synthesizing_tool_results() {
        let base = |uid: &str, kind: &str, content: Value| json!({"uuid":uid,"timestamp":"2026-10-16T00:00:00Z","type":kind,"role":kind,"content":content});
        let call = base(
            "a",
            "assistant",
            json!([{"type":"tool_use","id":"tool","name":"Read","input":{}}]),
        );
        let result = base(
            "b",
            "user",
            json!([{"type":"tool_result","tool_use_id":"tool","content":"ok"}]),
        );
        assert!(validate_messages(&[call.clone(), result.clone()], "clawcodex").is_ok());
        assert!(validate_messages(&[call], "clawcodex").is_err());
        assert!(validate_messages(&[result], "clawcodex").is_err());
    }

    #[test]
    fn builds_current_json_without_forcing_historical_model_on_default_provider() {
        let record = json!({"uuid":"u","timestamp":"2026-10-16T00:00:00.125Z","role":"assistant",
            "type":"assistant","content":"ok","model":"historical-model"});
        let files = build_files(
            "clawcodex",
            "id",
            "/tmp",
            std::slice::from_ref(&record),
            None,
        )
        .unwrap();
        let payload = parse_json(&files["id.json"]).unwrap();
        assert_eq!(payload["model"], "");
        assert_eq!(payload["provider"], "");
        assert_eq!(
            payload["conversation"]["messages"][0]["model"],
            "historical-model"
        );
        assert_eq!(payload["conversation"]["max_history"], 2000);
        let explicit = json!({"conversation":{"max_history":9000,"other":true},"provider":"configured","model":"configured-model"});
        let files = build_files("clawcodex", "id", "/tmp", &[record], Some(&explicit)).unwrap();
        let payload = parse_json(&files["id.json"]).unwrap();
        assert_eq!(payload["conversation"]["max_history"], 9000);
        assert_eq!(payload["conversation"]["other"], true);
        assert_eq!(payload["provider"], "configured");
    }

    #[test]
    fn parses_utc_offsets_and_rejects_noncanonical_uuid() {
        assert_eq!(
            timestamp_seconds("2026-10-16T09:00:00+09:00").unwrap(),
            timestamp_seconds("2026-10-16T00:00:00Z").unwrap()
        );
        assert_eq!(
            timestamp_seconds("2026-10-16T00:00:00").unwrap(),
            timestamp_seconds("2026-10-16T00:00:00Z").unwrap()
        );
        assert!(timestamp_seconds("invalid").is_err());
        assert!(timestamp_seconds("2016-12-31T23:59:60Z").is_err());
        assert!(uuid_string("a4353982-ffe7-4cf2-9eaa-3c1c78fa2458").is_ok());
        assert!(uuid_string("A4353982-FFE7-4CF2-9EAA-3C1C78FA2458").is_err());
    }

    #[test]
    fn loads_current_legacy_and_claude_without_internal_sidecar_recursion() {
        let temp = tempfile::tempdir().unwrap();
        let record = json!({"uuid":"u1", "timestamp":"2026-10-16T00:00:00Z", "type":"user",
            "role":"user", "content":"synthetic"});
        let native = temp.path().join("branch-b123456.json");
        fs::write(
            &native,
            serde_json::to_vec(&json!({"session_id":"branch-b123456","cwd":"/tmp",
            "conversation":{"messages":[record.clone()]}}))
            .unwrap(),
        )
        .unwrap();
        let companion = native.with_extension("");
        fs::create_dir_all(companion.join(".cc2cc")).unwrap();
        fs::write(companion.join(".cc2cc/manifest.json"), b"{}").unwrap();
        fs::write(companion.join("asset.bin"), [0, 255]).unwrap();
        let current = load_session(&native, "clawcodex").unwrap();
        assert_eq!(current.session_id, "branch-b123456");
        assert_eq!(current.records, vec![record.clone()]);
        assert!(current.files.contains_key("branch-b123456/asset.bin"));
        assert!(!current.files.keys().any(|name| name.contains(".cc2cc")));

        let legacy = temp.path().join("legacy");
        fs::create_dir(&legacy).unwrap();
        fs::write(
            legacy.join("metadata.json"),
            br#"{"session_id":"legacy","cwd":"/tmp"}"#,
        )
        .unwrap();
        fs::write(
            legacy.join("transcript.jsonl"),
            jsonl_bytes(&[record]).unwrap(),
        )
        .unwrap();
        assert_eq!(
            load_session(&legacy, "clawcodex").unwrap().transcript,
            "transcript.jsonl"
        );

        let sid = "a4353982-ffe7-4cf2-9eaa-3c1c78fa2458";
        let claude = temp.path().join(format!("{sid}.jsonl"));
        fs::write(
            &claude,
            jsonl_bytes(&[json!({"sessionId":sid,"cwd":"/tmp","type":"user",
            "uuid":"u1","message":{"role":"user","content":"synthetic"}})])
            .unwrap(),
        )
        .unwrap();
        assert_eq!(load_session(&claude, "claude").unwrap().session_id, sid);
    }
}
