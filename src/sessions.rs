use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::bridge::{
    ConvertOptions, InstallOptions, convert, destination_path, install, verify_installed,
};
use crate::files::{parse_json, safe_path};
use crate::formats::load_session;

fn config_dir(format: &str) -> Result<PathBuf> {
    let (key, name) = match format {
        "claude" => ("CLAUDE_CONFIG_DIR", ".claude"),
        "clawcodex" => ("CLAWCODEX_CONFIG_DIR", ".clawcodex"),
        _ => bail!("未対応の形式です: {format}"),
    };
    let path = match std::env::var_os(key) {
        Some(path) => PathBuf::from(path),
        None => PathBuf::from(std::env::var_os("HOME").context("HOMEがありません")?).join(name),
    };
    ensure!(path.is_absolute(), "{key}には絶対パスが必要です");
    ensure!(
        !path.is_symlink(),
        "設定ディレクトリにシンボリックリンクは使用できません"
    );
    Ok(path)
}

fn check_id(id: &str) -> Result<()> {
    ensure!(
        !id.is_empty()
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b)),
        "パスではなくセッションIDを指定してください"
    );
    Ok(())
}

fn entries(path: &Path) -> Result<Vec<PathBuf>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    ensure!(!path.is_symlink(), "履歴の保存先がシンボリックリンクです");
    let mut result = fs::read_dir(path)?
        .map(|entry| entry.map(|v| v.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    result.sort();
    Ok(result)
}

fn paths(format: &str) -> Result<Vec<PathBuf>> {
    let root = config_dir(format)?;
    let mut result = Vec::new();
    if format == "claude" {
        for project in entries(&root.join("projects"))? {
            ensure!(
                !project.is_symlink(),
                "プロジェクト履歴にシンボリックリンクがあります"
            );
            if !project.is_dir() {
                continue;
            }
            for path in entries(&project)? {
                if path.extension().is_some_and(|v| v == "jsonl") {
                    ensure!(!path.is_symlink(), "履歴がシンボリックリンクです");
                    result.push(path);
                }
            }
        }
    } else {
        for path in entries(&root.join("sessions"))? {
            if path.extension().is_some_and(|v| v == "json") || path.join("metadata.json").is_file()
            {
                ensure!(!path.is_symlink(), "履歴がシンボリックリンクです");
                result.push(path);
            }
        }
    }
    Ok(result)
}

pub fn find(id: &str, format: &str) -> Result<PathBuf> {
    check_id(id)?;
    let candidates: Vec<_> = paths(format)?
        .into_iter()
        .filter(|p| {
            if p.is_dir() {
                p.file_name().is_some_and(|v| v == id)
            } else {
                p.file_stem().is_some_and(|v| v == id)
            }
        })
        .collect();
    match candidates.as_slice() {
        [path] => Ok(path.clone()),
        [] => bail!("{format}のセッションが見つかりません: {id}"),
        _ => bail!("同じIDの履歴が複数あります: {id}"),
    }
}

pub fn list(format: Option<&str>) -> Result<Value> {
    let formats = match format {
        Some("claude") => vec!["claude"],
        Some("clawcodex") => vec!["clawcodex"],
        Some(other) => bail!("未対応の形式です: {other}"),
        None => vec!["claude", "clawcodex"],
    };
    let mut sessions = Vec::new();
    for format in formats {
        for path in paths(format)? {
            if format == "claude" {
                let id = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .context("履歴名が不正です")?;
                let mut cwd = String::new();
                for line in BufReader::new(fs::File::open(&path)?).lines() {
                    let line = line?;
                    if line.trim().is_empty() {
                        continue;
                    }
                    let row = parse_json(line.as_bytes())?;
                    if row["sessionId"] == id
                        && let Some(value) = row["cwd"].as_str()
                    {
                        cwd = value.to_owned();
                        break;
                    }
                }
                if !cwd.is_empty() {
                    sessions.push(json!({"id":id,"tool":format,"cwd":cwd}));
                }
            } else {
                let session = load_session(&path, format)?;
                sessions.push(json!({"id":session.session_id,"tool":format,"cwd":session.cwd}));
            }
        }
    }
    Ok(json!({"sessions":sessions}))
}

pub fn convert_id(id: &str, target: &str, leaf: Option<String>, dry_run: bool) -> Result<Value> {
    let source_format = match target {
        "claude" => "clawcodex",
        "clawcodex" => "claude",
        _ => bail!("未対応の変換先です"),
    };
    let path = find(id, source_format)?;
    let source = load_session(&path, source_format)?;
    ensure!(
        source.session_id == id,
        "ファイル名と履歴中のセッションIDが一致しません"
    );
    ensure!(
        Path::new(&source.cwd).is_absolute(),
        "元の履歴に作業ディレクトリの絶対パスがありません"
    );
    let config = config_dir(target)?;
    let target_id = Uuid::new_v4().to_string();
    let destination = destination_path(target, &source.cwd, &target_id, &config)?;
    let bundle = safe_path(&config, &format!(".cc2cc/archives/{target_id}"))?;
    let converted = convert(
        &path,
        target,
        &bundle,
        &ConvertOptions {
            source_format: Some(source_format.to_string()),
            dry_run,
            leaf,
            ..Default::default()
        },
    )?;
    if dry_run {
        return Ok(
            json!({"source_id":id,"session_id":target_id,"from":source_format,"to":target,
            "cwd":source.cwd,"path":destination,"message_count":converted["message_count"],
            "dry_run":true,"warnings":converted["warnings"]}),
        );
    }
    let installed = match install(
        &bundle,
        &InstallOptions {
            home: Some(config),
            session_id: Some(target_id),
            dry_run: false,
        },
    ) {
        Ok(result) => result,
        Err(error) => {
            fs::remove_dir_all(&bundle).context("未完了の変換を削除できません")?;
            return Err(error);
        }
    };
    let mut warnings = converted["warnings"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if let Some(notes) = installed["warnings"].as_array() {
        warnings.extend(notes.clone());
    }
    Ok(
        json!({"source_id":id,"session_id":installed["session_id"],"from":source_format,"to":target,
        "cwd":installed["cwd"],"path":installed["path"],"resume":installed["resume"],
        "message_count":converted["message_count"],"restored_original":converted["restored_original"],
        "archive":bundle,"warnings":warnings,"dry_run":false}),
    )
}

pub fn verify_id(id: &str, format: Option<&str>) -> Result<Value> {
    check_id(id)?;
    let format = match format {
        Some(value) => value.to_string(),
        None => {
            let mut found = Vec::new();
            for tool in ["claude", "clawcodex"] {
                let matches: Vec<_> = paths(tool)?
                    .into_iter()
                    .filter(|p| {
                        p.file_stem().is_some_and(|s| s == id)
                            || p.file_name().is_some_and(|s| s == id)
                    })
                    .collect();
                if !matches.is_empty() {
                    found.push(tool);
                }
            }
            ensure!(
                found.len() == 1,
                "セッションを一意に特定できません。--fromでツールを指定してください"
            );
            found[0].to_string()
        }
    };
    verify_installed(&find(id, &format)?, &format)
}
