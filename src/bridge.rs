use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::claw_normalize::normalize_claw;
use crate::files::{
    Files, collect_tree, digest, inventory, json_bytes, jsonl_bytes, parse_json, parse_jsonl,
    read_json, safe_path, write_file, write_tree,
};
use crate::formats::{
    Session, build_files, load_session, resolve_content, select_claude, timestamp_seconds,
    to_claude, to_claw, uuid_string, validate_messages,
};

const SCHEMA: &str = "cc2cc/1";
const SIDECAR: &str = ".cc2cc";

#[derive(Default)]
pub struct ConvertOptions {
    pub source_format: Option<String>,
    pub leaf: Option<String>,
    pub provenance: Option<PathBuf>,
    pub cwd: Option<String>,
    pub session_id: Option<String>,
    pub transcripts_dir: Option<PathBuf>,
    pub attachments: Vec<PathBuf>,
    pub dry_run: bool,
}

#[derive(Default)]
pub struct InstallOptions {
    pub home: Option<PathBuf>,
    pub session_id: Option<String>,
    pub dry_run: bool,
}

fn field<'a>(value: &'a Value, name: &str) -> Result<&'a str> {
    value
        .get(name)
        .and_then(Value::as_str)
        .with_context(|| format!("{name}がありません"))
}

fn absolute(path: &Path) -> Result<PathBuf> {
    let expanded = if path.starts_with("~") {
        PathBuf::from(std::env::var_os("HOME").context("HOMEが設定されていません")?)
            .join(path.strip_prefix("~")?)
    } else {
        path.to_path_buf()
    };
    Ok(if expanded.is_absolute() {
        expanded
    } else {
        std::env::current_dir()?.join(expanded)
    })
}

fn manifest(root: &Path) -> Result<Value> {
    ensure!(
        !root.is_symlink(),
        "変換記録にシンボリックリンクは使用できません"
    );
    let data = read_json(&safe_path(root, "manifest.json")?)?;
    ensure!(data["schema"] == SCHEMA, "未対応のcc2cc保存形式です");
    for key in ["source_format", "target_format"] {
        ensure!(
            matches!(field(&data, key)?, "claude" | "clawcodex"),
            "変換記録の形式が不正です"
        );
    }
    for key in ["source_transcript", "native_transcript"] {
        safe_path(root, field(&data, key)?)?;
    }
    Ok(data)
}

fn verified_archive(root: &Path, meta: &Value) -> Result<(Files, Vec<Value>)> {
    ensure!(
        meta["archive"].as_object().is_some_and(|o| !o.is_empty()),
        "原本一覧がありません"
    );
    let files = collect_tree(&root.join("archive"))?;
    ensure!(
        inventory(&files) == meta["archive"],
        "原本のSHA-256またはファイル一覧が一致しません"
    );
    let baseline = fs::read(safe_path(root, "baseline.jsonl")?)?;
    ensure!(
        digest(&baseline) == field(meta, "baseline_sha256")?,
        "変換記録のSHA-256が一致しません"
    );
    Ok((files, parse_jsonl(&baseline)?))
}

fn find_provenance(path: &Path) -> Option<PathBuf> {
    let sidecar = if path.is_dir() {
        path.join(SIDECAR)
    } else {
        path.with_extension("").join(SIDECAR)
    };
    let parent = path.parent()?;
    let bundle = if parent.file_name().is_some_and(|v| v == "native") {
        parent.parent()?.to_path_buf()
    } else {
        parent.to_path_buf()
    };
    [sidecar, bundle]
        .into_iter()
        .find(|p| p.join("manifest.json").is_file())
}

fn prefix(source: &Session, baseline: &[Value]) -> Result<(Vec<Value>, bool)> {
    ensure!(
        source.records.len() >= baseline.len(),
        "変換済み履歴の先頭が削除されています。原本復元を中止しました"
    );
    let mut resaved = false;
    for (index, (current, previous)) in source.records.iter().zip(baseline).enumerate() {
        if current == previous {
            continue;
        }
        if source.format == "clawcodex" && current == &normalize_claw(previous)? {
            resaved = true;
            continue;
        }
        bail!(
            "変換済み履歴の{}件目が変更されています。原本復元を中止しました",
            index + 1
        );
    }
    Ok((source.records[baseline.len()..].to_vec(), resaved))
}

fn valid_agent_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn agent_ids(value: &Value, found: &mut BTreeSet<String>) {
    if let Some(object) = value.as_object() {
        if let Some(id) = object.get("agent_id").and_then(Value::as_str)
            && valid_agent_id(id)
            && (value["type"] == "agent"
                || matches!(
                    value["status"].as_str(),
                    Some("completed" | "interrupted" | "async_launched" | "teammate_spawned")
                ))
        {
            found.insert(id.to_string());
        }
        for key in ["toolUseResult", "metadata", "tool_output", "content"] {
            if let Some(child) = object.get(key) {
                agent_ids(child, found);
            }
        }
    } else if let Some(array) = value.as_array() {
        for child in array {
            agent_ids(child, found);
        }
    }
}

fn collect_agents(
    source: &mut Session,
    path: &Path,
    explicit: Option<&Path>,
    notes: &mut Vec<String>,
) -> Result<()> {
    if source.format != "clawcodex" {
        return Ok(());
    }
    let mut agents = BTreeSet::new();
    for record in &source.records {
        agent_ids(record, &mut agents);
    }
    if agents.is_empty() {
        return Ok(());
    }
    let parent = path.parent().context("履歴の親ディレクトリがありません")?;
    let sibling = parent.join("transcripts");
    let root = match explicit {
        Some(root) => root.to_path_buf(),
        None if sibling.is_dir() => sibling,
        None => parent
            .parent()
            .context("子履歴の保存先を指定してください")?
            .join("transcripts"),
    };
    let mut seen = BTreeSet::new();
    while let Some(id) = agents.difference(&seen).next().cloned() {
        seen.insert(id.clone());
        let name = format!("{id}.jsonl");
        let child = safe_path(&root, &name)?;
        ensure!(
            child.is_file(),
            "参照された子履歴がありません。--transcripts-dirで指定してください: {id}"
        );
        let data = fs::read(child)?;
        for record in parse_jsonl(&data)? {
            agent_ids(&record, &mut agents);
        }
        source.files.insert(format!("transcripts/{name}"), data);
    }
    notes.push(format!(
        "子エージェント履歴{}件を原本に保存しました。実行状態は移植しません",
        seen.len()
    ));
    Ok(())
}

fn source_from_archive(files: Files, meta: &Value) -> Result<Session> {
    let transcript = field(meta, "source_transcript")?.to_string();
    let data = files.get(&transcript).context("原本の主履歴がありません")?;
    let format = field(meta, "source_format")?.to_string();
    let (metadata, records) = if format == "clawcodex" && transcript.ends_with(".json") {
        let native = parse_json(data)?;
        let rows = native["conversation"]["messages"]
            .as_array()
            .context("原本のmessagesが不正です")?
            .clone();
        (native, rows)
    } else {
        let metadata = if format == "clawcodex" {
            parse_json(
                files
                    .get("metadata.json")
                    .context("原本のmetadataがありません")?,
            )?
        } else {
            json!({})
        };
        (metadata, parse_jsonl(data)?)
    };
    Ok(Session {
        format,
        session_id: field(meta, "source_id")?.to_string(),
        cwd: field(meta, "source_cwd")?.to_string(),
        records,
        files,
        transcript,
        metadata,
    })
}

struct Restored {
    files: Files,
    records: Vec<Value>,
    id: String,
    cwd: String,
    notes: Vec<String>,
    target_leaf: Option<String>,
}

fn restore(source: &Session, root: &Path, meta: &Value) -> Result<Restored> {
    ensure!(
        source.format == field(meta, "target_format")?
            && source.session_id == field(meta, "target_id")?,
        "入力セッションと変換記録の形式またはIDが一致しません"
    );
    let (files, baseline) = verified_archive(root, meta)?;
    let original = source_from_archive(files, meta)?;
    let (extra, resaved) = prefix(source, &baseline)?;
    let mut notes = Vec::new();
    if resaved {
        notes.push("ClawCodexの通常の再保存を検出し、欠落した原本の属性を復元しました".into());
    }
    let mut target_leaf = meta["source_leaf"].as_str().map(str::to_string);
    if extra.is_empty() {
        return Ok(Restored {
            files: original.files,
            records: original.records,
            id: original.session_id,
            cwd: original.cwd,
            notes,
            target_leaf,
        });
    }
    notes.push(format!(
        "変換後に追加された会話{}件を元の履歴の末尾へ変換しました",
        extra.len()
    ));
    let mapped = if original.format == "claude" {
        let mut parent = target_leaf.clone();
        let mut mapped = Vec::new();
        for record in &extra {
            let resolved = resolve_content(record, &source.files)?;
            let next = to_claude(
                &resolved,
                &original.session_id,
                &original.cwd,
                parent.as_deref(),
            )?;
            parent = Some(field(&next, "uuid")?.to_string());
            mapped.push(next);
        }
        let (mut chain, _) = select_claude(&original.records, target_leaf.as_deref())?;
        chain.extend(mapped.clone());
        validate_messages(&chain, "claude")?;
        target_leaf = parent;
        mapped
    } else {
        let (selected, _) = select_claude(&source.records, None)?;
        let (baseline_chain, _) = select_claude(&baseline, meta["target_leaf"].as_str())?;
        ensure!(
            selected.len() >= baseline_chain.len()
                && selected
                    .iter()
                    .take(baseline_chain.len())
                    .map(|r| &r["uuid"])
                    .eq(baseline_chain.iter().map(|r| &r["uuid"])),
            "追加会話の分岐が元の履歴につながっていません"
        );
        let tail = &selected[baseline_chain.len()..];
        if extra.len() != tail.len() {
            notes.push(format!(
                "追加された独自イベント等{}件は今回の原本に保存しました",
                extra.len().saturating_sub(tail.len())
            ));
        }
        let mapped = tail.iter().map(to_claw).collect::<Result<Vec<_>>>()?;
        let mut chain = original.records.clone();
        chain.extend(mapped.clone());
        validate_messages(&chain, "clawcodex")?;
        mapped
    };
    let mut records = original.records.clone();
    records.extend(mapped.clone());
    let mut files = original.files;
    if original.format == "claude" || original.transcript == "transcript.jsonl" {
        let raw = files
            .get_mut(&original.transcript)
            .context("原本の主履歴がありません")?;
        if !mapped.is_empty() {
            if !raw.is_empty() && !raw.ends_with(b"\n") {
                raw.push(b'\n');
            }
            raw.extend(jsonl_bytes(&mapped)?);
        }
        if original.format == "clawcodex" {
            let mut metadata = original.metadata;
            metadata["message_count"] = json!(records.len());
            metadata["last_updated"] = json!(timestamp_seconds(field(
                records.last().context("空の履歴")?,
                "timestamp"
            )?)?);
            files.insert("metadata.json".into(), json_bytes(&metadata)?);
        }
    } else {
        let mut updated = build_files(
            "clawcodex",
            &original.session_id,
            &original.cwd,
            &records,
            Some(&original.metadata),
        )?;
        let data = updated
            .remove(&format!("{}.json", original.session_id))
            .context("変換履歴がありません")?;
        files.insert(original.transcript, data);
    }
    Ok(Restored {
        files,
        records,
        id: original.session_id,
        cwd: original.cwd,
        notes,
        target_leaf,
    })
}

fn publish(output: &Path, files: &Files) -> Result<()> {
    ensure!(
        !output.is_symlink(),
        "出力にシンボリックリンクは使用できません"
    );
    fs::create_dir_all(output.parent().context("出力先の親がありません")?)?;
    fs::create_dir(output)
        .with_context(|| format!("出力先は既に存在するか作成できません: {}", output.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(error) = fs::set_permissions(output, fs::Permissions::from_mode(0o700)) {
            let _ = fs::remove_dir(output);
            return Err(error.into());
        }
    }
    let result = (|| -> Result<()> {
        let payload: Files = files
            .iter()
            .filter(|(key, _)| key.as_str() != "manifest.json")
            .map(|(key, data)| (key.clone(), data.clone()))
            .collect();
        write_tree(output, &payload)?;
        // The manifest is the commit marker for a complete bundle.
        if let Some(data) = files.get("manifest.json") {
            write_file(&output.join("manifest.json"), data)?;
        }
        Ok(())
    })();
    if result.is_err() {
        fs::remove_dir_all(output).context("未完了の出力を削除できません")?;
    }
    result
}

pub fn convert(
    source_path: &Path,
    target: &str,
    output: &Path,
    options: &ConvertOptions,
) -> Result<Value> {
    ensure!(
        matches!(target, "claude" | "clawcodex"),
        "未対応の変換先です"
    );
    let mut path = absolute(source_path)?;
    let output = absolute(output)?;
    ensure!(
        !path.is_symlink(),
        "入力にシンボリックリンクは使用できません"
    );
    let mut source_format = options.source_format.clone();
    let mut provenance = options.provenance.clone();
    if path.is_dir() && path.join("manifest.json").is_file() {
        let root = path.clone();
        let meta = manifest(&root)?;
        let format = field(&meta, "target_format")?;
        ensure!(
            source_format.as_deref().is_none_or(|v| v == format),
            "--fromと変換記録の形式が一致しません"
        );
        source_format = Some(format.into());
        path = safe_path(&root.join("native"), field(&meta, "native_transcript")?)?;
        if path.file_name().is_some_and(|v| v == "transcript.jsonl") {
            path.pop();
        }
        provenance = provenance.or(Some(root));
    }
    let format =
        source_format.context("生履歴には--from claudeまたは--from clawcodexを指定してください")?;
    let mut source = load_session(&path, &format)?;
    ensure!(target != source.format, "変換元と変換先が同じ形式です");
    ensure!(
        !(path.is_dir() && output.starts_with(&path)),
        "入力ディレクトリの中へ出力できません"
    );
    let mut notes = Vec::new();
    collect_agents(
        &mut source,
        &path,
        options.transcripts_dir.as_deref(),
        &mut notes,
    )?;
    for item in &options.attachments {
        let attachment = absolute(item)?;
        ensure!(
            !attachment.is_symlink(),
            "添付にシンボリックリンクは使用できません"
        );
        let name = attachment
            .file_name()
            .and_then(|v| v.to_str())
            .context("添付名が不正です")?;
        let (extra, prefix) = if attachment.is_dir() {
            (collect_tree(&attachment)?, format!("attachments/{name}/"))
        } else {
            (
                Files::from([(name.into(), fs::read(&attachment)?)]),
                "attachments/".into(),
            )
        };
        notes.push(format!(
            "指定された添付ファイル{}件を原本に保存しました",
            extra.len()
        ));
        for (name, data) in extra {
            let key = format!("{prefix}{name}");
            ensure!(
                !source.files.contains_key(&key),
                "添付ファイル名が重複しています: {key}"
            );
            source.files.insert(key, data);
        }
    }
    let root = match provenance {
        Some(root) => Some(absolute(&root)?),
        None => find_provenance(&path),
    };
    let prior = root.as_deref().map(manifest).transpose()?;
    let restored = prior.as_ref().is_some_and(|p| p["source_format"] == target);
    let mut source_leaf = None;
    if source.format == "claude" {
        let previous_leaf = prior
            .as_ref()
            .filter(|p| p["baseline_count"].as_u64() == Some(source.records.len() as u64))
            .and_then(|p| p["target_leaf"].as_str());
        let leaf = options.leaf.as_deref().or(previous_leaf);
        let (chain, _) = select_claude(&source.records, leaf)?;
        source_leaf = Some(field(chain.last().context("空の会話です")?, "uuid")?.to_string());
    }
    let (native, records, sid, cwd, transcript, target_leaf) = if restored {
        ensure!(
            options.leaf.is_none() && options.cwd.is_none() && options.session_id.is_none(),
            "原本復元では--leaf、--cwd、--session-idを指定できません。新しいIDへの登録にはinstallを使ってください"
        );
        let prior = prior.as_ref().context("変換記録がありません")?;
        let result = restore(&source, root.as_deref().context("原本がありません")?, prior)?;
        notes.extend(result.notes);
        (
            result.files,
            result.records,
            result.id,
            result.cwd,
            field(prior, "source_transcript")?.to_string(),
            result.target_leaf,
        )
    } else {
        ensure!(root.is_none(), "変換記録の原本形式が変換先と一致しません");
        let sid = options
            .session_id
            .as_deref()
            .map(uuid_string)
            .transpose()?
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let cwd = options.cwd.clone().unwrap_or_else(|| source.cwd.clone());
        ensure!(
            !cwd.is_empty(),
            "作業ディレクトリがありません。--cwdで指定してください"
        );
        let records = if source.format == "claude" {
            let (selected, warnings) = select_claude(&source.records, options.leaf.as_deref())?;
            notes.extend(warnings);
            validate_messages(&selected, "claude")?;
            selected.iter().map(to_claw).collect::<Result<Vec<_>>>()?
        } else {
            let selected = source
                .records
                .iter()
                .map(|r| resolve_content(r, &source.files))
                .collect::<Result<Vec<_>>>()?;
            validate_messages(&selected, "clawcodex")?;
            let mut parent = None;
            let mut rows = Vec::new();
            for record in &selected {
                let row = to_claude(record, &sid, &cwd, parent.as_deref())?;
                parent = Some(field(&row, "uuid")?.to_string());
                rows.push(row);
            }
            notes.push("ClawCodexの会話以外の状態と添付ファイルは原本に保存しました".into());
            rows
        };
        validate_messages(&records, target)?;
        let native = build_files(target, &sid, &cwd, &records, None)?;
        let transcript = format!(
            "{sid}.{}",
            if target == "claude" { "jsonl" } else { "json" }
        );
        let leaf = if target == "claude" {
            Some(field(records.last().context("空の会話")?, "uuid")?.to_string())
        } else {
            None
        };
        (native, records, sid, cwd, transcript, leaf)
    };
    if target == "clawcodex" {
        let mut changed = 0;
        for record in &records {
            if record != &normalize_claw(record)? {
                changed += 1;
            }
        }
        if changed > 0 {
            notes.push(format!("ClawCodexの読み込み・再保存で属性が変わるメッセージが{changed}件あります。原本は別途保持します"));
        }
    }
    notes.push(
        "思考の署名・画像・未知のブロックのAPI互換性は使用するプロバイダーに依存します".into(),
    );
    let report = json!({"tool":"cc2cc", "source_format":source.format, "target_format":target,
        "source_id":source.session_id, "target_id":sid, "message_count":records.len(),
        "restored_original":restored, "warnings":notes, "dry_run":options.dry_run,
        "output":output, "native_path":output.join("native").join(&transcript)});
    if options.dry_run {
        return Ok(report);
    }
    let baseline = jsonl_bytes(&records)?;
    let meta = json!({"schema":SCHEMA, "source_format":source.format, "source_id":source.session_id,
        "source_cwd":source.cwd, "source_transcript":source.transcript,"source_leaf":source_leaf,
        "target_format":target,"target_id":sid,"target_cwd":cwd,"native_transcript":transcript,
        "target_leaf":target_leaf,"baseline_count":records.len(),"archive":inventory(&source.files),
        "baseline_sha256":digest(&baseline)});
    let mut bundle: Files = native
        .into_iter()
        .map(|(name, data)| (format!("native/{name}"), data))
        .collect();
    bundle.extend(
        source
            .files
            .into_iter()
            .map(|(name, data)| (format!("archive/{name}"), data)),
    );
    bundle.insert("manifest.json".into(), json_bytes(&meta)?);
    bundle.insert("baseline.jsonl".into(), baseline);
    bundle.insert("report.json".into(), json_bytes(&report)?);
    publish(&output, &bundle)?;
    Ok(report)
}

fn native_source(root: &Path, meta: &Value) -> Result<(Session, PathBuf)> {
    let mut path = safe_path(&root.join("native"), field(meta, "native_transcript")?)?;
    if path.file_name().is_some_and(|v| v == "transcript.jsonl") {
        path.pop();
    }
    Ok((load_session(&path, field(meta, "target_format")?)?, path))
}

pub fn verify(bundle: &Path) -> Result<Value> {
    let root = absolute(bundle)?;
    let meta = manifest(&root)?;
    let (_, baseline) = verified_archive(&root, &meta)?;
    let (source, _) = native_source(&root, &meta)?;
    verify_loaded(&source, &meta, &baseline)
}

pub fn verify_installed(path: &Path, format: &str) -> Result<Value> {
    let source = load_session(path, format)?;
    let root = find_provenance(path).context("cc2ccの変換記録がありません")?;
    let meta = manifest(&root)?;
    let (_, baseline) = verified_archive(&root, &meta)?;
    verify_loaded(&source, &meta, &baseline)
}

fn verify_loaded(source: &Session, meta: &Value, baseline: &[Value]) -> Result<Value> {
    ensure!(
        source.session_id == field(meta, "target_id")?
            && source.format == field(meta, "target_format")?,
        "ネイティブ履歴のIDが変換記録と一致しません"
    );
    let (extra, resaved) = prefix(source, baseline)?;
    let selected = if source.format == "claude" {
        select_claude(
            &source.records,
            if extra.is_empty() {
                meta["target_leaf"].as_str()
            } else {
                None
            },
        )?
        .0
    } else {
        source.records.clone()
    };
    validate_messages(&selected, &source.format)?;
    Ok(
        json!({"ok":true,"archive_files":meta["archive"].as_object().context("原本一覧不正")?.len(),
        "message_count":source.records.len(),"appended_messages":extra.len(),"native_resaved":resaved}),
    )
}

fn reidentify(records: &[Value], old: &str, new: &str) -> Vec<Value> {
    let mut result = records.to_vec();
    for record in &mut result {
        for key in [
            "sessionId",
            "session_id",
            "parentSessionId",
            "parent_session_id",
        ] {
            if record[key] == old {
                record[key] = json!(new);
            }
        }
    }
    result
}

pub fn destination_path(format: &str, cwd: &str, sid: &str, config: &Path) -> Result<PathBuf> {
    if format == "clawcodex" {
        return Ok(config.join("sessions").join(format!("{sid}.json")));
    }
    ensure!(format == "claude", "未対応の形式です");
    ensure!(
        Path::new(cwd).is_absolute(),
        "作業ディレクトリには絶対パスが必要です"
    );
    let encoded: String = cwd
        .encode_utf16()
        .map(|c| {
            if c <= 127 && (c as u8).is_ascii_alphanumeric() {
                char::from(c as u8)
            } else {
                '-'
            }
        })
        .collect();
    ensure!(
        encoded.len() <= 200,
        "作業ディレクトリが長すぎます。このClaude Codeの保存名には未対応です"
    );
    Ok(config
        .join("projects")
        .join(encoded)
        .join(format!("{sid}.jsonl")))
}

pub fn install(bundle: &Path, options: &InstallOptions) -> Result<Value> {
    let root = absolute(bundle)?;
    verify(&root)?;
    let meta = manifest(&root)?;
    let format = field(&meta, "target_format")?;
    let (mut source, path) = native_source(&root, &meta)?;
    collect_agents(&mut source, &path, None, &mut Vec::new())?;
    let sid = options
        .session_id
        .as_deref()
        .map(uuid_string)
        .transpose()?
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let config = match &options.home {
        Some(home) => absolute(home)?,
        None => {
            let (key, default) = if format == "claude" {
                ("CLAUDE_CONFIG_DIR", ".claude")
            } else {
                ("CLAWCODEX_CONFIG_DIR", ".clawcodex")
            };
            match std::env::var_os(key) {
                Some(path) => absolute(Path::new(&path))?,
                None => PathBuf::from(std::env::var_os("HOME").context("HOMEがありません")?)
                    .join(default),
            }
        }
    };
    let (destination, records, native) = if format == "claude" {
        let dest = destination_path(format, &source.cwd, &sid, &config)?;
        let rows = reidentify(&source.records, &source.session_id, &sid);
        let data = jsonl_bytes(&rows)?;
        (dest, rows, data)
    } else {
        let dest = config.join("sessions").join(format!("{sid}.json"));
        let rows = source
            .records
            .iter()
            .map(|r| resolve_content(r, &source.files))
            .collect::<Result<Vec<_>>>()?;
        let mut metadata = source.metadata.clone();
        if let Some(object) = metadata.as_object_mut() {
            object.remove("goal");
            object.remove("scheduled_tasks");
        }
        let data = build_files(format, &sid, &source.cwd, &rows, Some(&metadata))?
            .remove(&format!("{sid}.json"))
            .context("履歴がありません")?;
        (dest, rows, data)
    };
    let sidecar_dir = destination.with_extension("");
    let mut report = json!({"format":format,"session_id":sid,"path":destination,"cwd":source.cwd,
        "resume":if format == "claude" {
            format!("claude --resume '{}'", destination.to_string_lossy().replace('\'', "'\"'\"'"))
        } else {format!("/resume {sid}")},"dry_run":options.dry_run});
    if format == "clawcodex"
        && (source.metadata.get("goal").is_some()
            || source.metadata.get("scheduled_tasks").is_some())
    {
        report["warnings"] =
            json!(["目標と予約処理は原本に保持し、新しいセッションでは自動実行しません"]);
    }
    if options.dry_run {
        return Ok(report);
    }
    ensure!(
        !destination.exists()
            && !destination.is_symlink()
            && !sidecar_dir.exists()
            && !sidecar_dir.is_symlink(),
        "登録先のセッションは既に存在します"
    );
    let mut sidecar = collect_tree(&root)?;
    sidecar.retain(|key, _| !key.starts_with("native/"));
    let mut provenance = meta.clone();
    provenance["target_id"] = json!(sid);
    provenance["native_transcript"] = json!(
        destination
            .file_name()
            .and_then(|v| v.to_str())
            .context("出力名不正")?
    );
    provenance["baseline_count"] = json!(records.len());
    let baseline = jsonl_bytes(&records)?;
    provenance["baseline_sha256"] = json!(digest(&baseline));
    sidecar.insert("manifest.json".into(), json_bytes(&provenance)?);
    sidecar.insert("baseline.jsonl".into(), baseline);
    let mut files: Files = sidecar
        .into_iter()
        .map(|(name, data)| (format!("{SIDECAR}/{name}"), data))
        .collect();
    if format == "claude" {
        let prefix = format!("{}/", source.transcript.trim_end_matches(".jsonl"));
        for (name, data) in &source.files {
            if let Some(relative) = name.strip_prefix(&prefix) {
                let bytes = if relative.ends_with(".jsonl") {
                    jsonl_bytes(&reidentify(&parse_jsonl(data)?, &source.session_id, &sid))?
                } else {
                    data.clone()
                };
                files.insert(relative.into(), bytes);
            }
        }
    }
    publish(&sidecar_dir, &files)?;
    let mut copied_children = Vec::new();
    let result = (|| -> Result<()> {
        if format == "clawcodex" {
            for (name, data) in &source.files {
                if !name.starts_with("transcripts/") {
                    continue;
                }
                let child = safe_path(&config, name)?;
                if child.exists() {
                    ensure!(
                        fs::read(&child)? == *data,
                        "既存の子履歴と内容が異なります。別の--homeで登録してください"
                    );
                } else {
                    write_file(&child, data)?;
                    copied_children.push(child);
                }
            }
        }
        write_file(&destination, &native)?;
        Ok(())
    })();
    if result.is_err() {
        for child in copied_children {
            fs::remove_file(child)?;
        }
        fs::remove_dir_all(&sidecar_dir)?;
    }
    result?;
    Ok(report)
}
