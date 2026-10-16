use anyhow::{Context, Result, bail};
use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};

pub type Files = BTreeMap<String, Vec<u8>>;

pub fn digest(data: &[u8]) -> String {
    format!("{:x}", Sha256::digest(data))
}

pub fn json_bytes(value: &Value) -> Result<Vec<u8>> {
    let mut data = serde_json::to_vec_pretty(value)?;
    data.push(b'\n');
    Ok(data)
}

struct StrictValue(Value);

impl<'de> Deserialize<'de> for StrictValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct StrictVisitor;
        impl<'de> Visitor<'de> for StrictVisitor {
            type Value = StrictValue;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("重複キーと非有限数を含まないJSON")
            }

            fn visit_bool<E: de::Error>(self, value: bool) -> std::result::Result<Self::Value, E> {
                Ok(StrictValue(Value::Bool(value)))
            }

            fn visit_i64<E: de::Error>(self, value: i64) -> std::result::Result<Self::Value, E> {
                Ok(StrictValue(Value::Number(value.into())))
            }

            fn visit_u64<E: de::Error>(self, value: u64) -> std::result::Result<Self::Value, E> {
                Ok(StrictValue(Value::Number(value.into())))
            }

            fn visit_f64<E: de::Error>(self, value: f64) -> std::result::Result<Self::Value, E> {
                Number::from_f64(value)
                    .map(|number| StrictValue(Value::Number(number)))
                    .ok_or_else(|| E::custom("不正な数値です"))
            }

            fn visit_str<E: de::Error>(self, value: &str) -> std::result::Result<Self::Value, E> {
                Ok(StrictValue(Value::String(value.to_owned())))
            }

            fn visit_string<E: de::Error>(
                self,
                value: String,
            ) -> std::result::Result<Self::Value, E> {
                Ok(StrictValue(Value::String(value)))
            }

            fn visit_unit<E: de::Error>(self) -> std::result::Result<Self::Value, E> {
                Ok(StrictValue(Value::Null))
            }

            fn visit_none<E: de::Error>(self) -> std::result::Result<Self::Value, E> {
                self.visit_unit()
            }

            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                while let Some(value) = seq.next_element::<StrictValue>()? {
                    values.push(value.0);
                }
                Ok(StrictValue(Value::Array(values)))
            }

            fn visit_map<A: MapAccess<'de>>(
                self,
                mut map: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut values = Map::new();
                while let Some(key) = map.next_key::<String>()? {
                    if values.contains_key(&key) {
                        return Err(de::Error::custom(format!(
                            "JSONのキーが重複しています: {key}"
                        )));
                    }
                    values.insert(key, map.next_value::<StrictValue>()?.0);
                }
                Ok(StrictValue(Value::Object(values)))
            }
        }
        deserializer.deserialize_any(StrictVisitor)
    }
}

pub fn parse_json(data: &[u8]) -> Result<Value> {
    let mut deserializer = serde_json::Deserializer::from_slice(data);
    StrictValue::deserialize(&mut deserializer).context("JSONを解析できません")?;
    deserializer.end().context("JSONの末尾が不正です")?;
    reject_reserved_number_key(data)?;
    // arbitrary_precisionの数値を標準Numberとして保持するため、検証後に読み直す。
    serde_json::from_slice(data).context("JSONを解析できません")
}

fn reject_reserved_number_key(data: &[u8]) -> Result<()> {
    let mut index = 0;
    while index < data.len() {
        if data[index] != b'"' {
            index += 1;
            continue;
        }
        let start = index;
        index += 1;
        while index < data.len() {
            if data[index] == b'\\' {
                index += 2;
            } else if data[index] == b'"' {
                index += 1;
                break;
            } else {
                index += 1;
            }
        }
        let end = index;
        while index < data.len() && data[index].is_ascii_whitespace() {
            index += 1;
        }
        if data.get(index) == Some(&b':')
            && serde_json::from_slice::<String>(&data[start..end])?
                == "$serde_json::private::Number"
        {
            bail!("JSONに使用できない予約キーです: $serde_json::private::Number");
        }
    }
    Ok(())
}

pub fn read_json(path: &Path) -> Result<Value> {
    parse_json(
        &fs::read(path).with_context(|| format!("JSONを読み込めません: {}", path.display()))?,
    )
}

pub fn parse_jsonl(data: &[u8]) -> Result<Vec<Value>> {
    let mut records = Vec::new();
    for (index, line) in data.split(|byte| *byte == b'\n').enumerate() {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let value = parse_json(line).with_context(|| format!("JSONLの{}行目", index + 1))?;
        if !value.is_object() {
            bail!("JSONLの{}行目がオブジェクトではありません", index + 1);
        }
        records.push(value);
    }
    if records.is_empty() {
        bail!("履歴が空です");
    }
    Ok(records)
}

pub fn jsonl_bytes(records: &[Value]) -> Result<Vec<u8>> {
    let mut data = Vec::new();
    for record in records {
        if !record.is_object() {
            bail!("JSONLの各行はオブジェクトである必要があります");
        }
        serde_json::to_writer(&mut data, record)?;
        data.push(b'\n');
    }
    Ok(data)
}

fn reject_symlink(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            bail!("シンボリックリンクは使用できません: {}", path.display())
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub fn safe_path(root: &Path, relative: &str) -> Result<PathBuf> {
    if relative.is_empty()
        || relative.contains('\\')
        || relative.split('/').any(|part| matches!(part, "." | ".."))
        || Path::new(relative)
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        bail!("不正な相対パスです: {relative}");
    }
    reject_symlink(root)?;
    let mut path = root.to_owned();
    for component in Path::new(relative).components() {
        path.push(component.as_os_str());
        reject_symlink(&path)?;
    }
    Ok(path)
}

fn unchanged(before: &fs::Metadata, after: &fs::Metadata) -> Result<bool> {
    let equal = before.len() == after.len() && before.modified()? == after.modified()?;
    #[cfg(unix)]
    let equal = equal && before.ino() == after.ino() && before.dev() == after.dev();
    Ok(equal && before.is_file() && after.is_file())
}

pub fn collect_tree(root: &Path) -> Result<Files> {
    reject_symlink(root)?;
    if !root.is_dir() {
        bail!("入力がディレクトリではありません: {}", root.display());
    }
    fn walk(root: &Path, directory: &Path, files: &mut Files) -> Result<()> {
        let mut entries = fs::read_dir(directory)?.collect::<std::io::Result<Vec<_>>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            if entry.file_name() == ".cc2cc" {
                continue;
            }
            let path = entry.path();
            let before = fs::symlink_metadata(&path)?;
            if before.file_type().is_symlink() {
                bail!("シンボリックリンクは保存できません: {}", path.display());
            }
            if before.is_dir() {
                walk(root, &path, files)?;
            } else if before.is_file() {
                let mut file = File::open(&path)?;
                let opened = file.metadata()?;
                let mut data = Vec::new();
                file.read_to_end(&mut data)?;
                let after = fs::symlink_metadata(&path)?;
                if !unchanged(&before, &opened)?
                    || !unchanged(&before, &after)?
                    || data.len() as u64 != after.len()
                {
                    bail!("読み込み中に入力が変更されました。使用中のセッションを閉じてください");
                }
                let name = path
                    .strip_prefix(root)?
                    .to_str()
                    .context("ファイル名がUTF-8ではありません")?
                    .replace(std::path::MAIN_SEPARATOR, "/");
                files.insert(name, data);
            } else {
                bail!("通常のファイルではありません: {}", path.display());
            }
        }
        Ok(())
    }
    let mut files = Files::new();
    walk(root, root, &mut files)?;
    Ok(files)
}

pub fn write_file(path: &Path, data: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .context("出力先の親ディレクトリがありません")?;
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    builder.mode(0o700);
    builder.create(parent)?;
    reject_symlink(parent)?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(path)
        .with_context(|| format!("出力ファイルを作成できません: {}", path.display()))?;
    if let Err(error) = file.write_all(data).and_then(|()| file.sync_all()) {
        drop(file);
        fs::remove_file(path).context("未完了の出力ファイルを削除できません")?;
        return Err(error.into());
    }
    Ok(())
}

pub fn write_tree(root: &Path, files: &Files) -> Result<()> {
    for (relative, data) in files {
        write_file(&safe_path(root, relative)?, data)?;
    }
    Ok(())
}

pub fn inventory(files: &Files) -> Value {
    Value::Object(
        files
            .iter()
            .map(|(name, data)| {
                (
                    name.clone(),
                    serde_json::json!({"sha256": digest(data), "bytes": data.len()}),
                )
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let base = std::env::temp_dir().canonicalize().unwrap();
            let root = base.join(format!(
                "cc2cc-files-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).unwrap();
            Self(root)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn rejects_duplicates_and_invalid_numbers() {
        for text in [
            r#"{"a":1,"a":2}"#,
            r#"{"nested":[{"a":1,"\u0061":2}]}"#,
            "NaN",
            "Infinity",
            "-Infinity",
            "{}{}",
        ] {
            assert!(parse_json(text.as_bytes()).is_err(), "{text}");
        }
        assert_eq!(
            parse_json(br#"{"a":[null,true,1,-2,0.5]}"#).unwrap()["a"][2],
            1
        );
        assert!(parse_json(&[0xff]).is_err());
    }

    #[test]
    fn preserves_arbitrary_precision_numbers() {
        for (number, normalized) in [
            (
                "184467440737095516160000000000000000000000000000000000001",
                "184467440737095516160000000000000000000000000000000000001",
            ),
            (
                "-922337203685477580900000000000000000000000000000000000001",
                "-922337203685477580900000000000000000000000000000000000001",
            ),
            ("1e1000", "1e+1000"),
            (
                "0.123456789012345678901234567890123456789012345678901234567890",
                "0.123456789012345678901234567890123456789012345678901234567890",
            ),
        ] {
            let data = format!(r#"{{"nested":[{{"number":{number}}}]}}"#);
            let value = parse_json(data.as_bytes()).unwrap();
            assert!(value["nested"][0]["number"].is_number());
            assert_eq!(value["nested"][0]["number"].to_string(), normalized);
            assert_eq!(
                String::from_utf8(jsonl_bytes(std::slice::from_ref(&value)).unwrap()).unwrap(),
                format!("{{\"nested\":[{{\"number\":{normalized}}}]}}\n")
            );
            assert_eq!(parse_json(&json_bytes(&value).unwrap()).unwrap(), value);
        }
        assert!(parse_json(br#"{"n":1e1000,"n":2}"#).is_err());
        assert!(parse_json(br#"{"a":[{"n":0.12345678901234567890,"n":1}]}"#).is_err());
    }

    #[test]
    fn rejects_reserved_number_keys_without_rejecting_string_values() {
        for data in [
            br#"{"$serde_json::private::Number":"123"}"#.as_slice(),
            br#"{"nested":[{"\u0024serde_json::private::Number":"123"}]}"#.as_slice(),
            br#"{"first":true,"$serde_json::private::Number":"123"}"#.as_slice(),
        ] {
            assert!(parse_json(data).is_err());
        }
        let value = parse_json(br#"{"text":"$serde_json::private::Number"}"#).unwrap();
        assert_eq!(value["text"], "$serde_json::private::Number");
    }

    #[test]
    fn jsonl_requires_nonempty_objects_and_preserves_unicode() {
        assert!(parse_jsonl(b"\n \n").is_err());
        assert!(parse_jsonl(b"{}\n[]\n").is_err());
        let records = vec![serde_json::json!({"text":"日本語", "nested":{"x":1}})];
        let bytes = jsonl_bytes(&records).unwrap();
        assert_eq!(parse_jsonl(&bytes).unwrap(), records);
        assert!(bytes.ends_with(b"\n"));
        assert!(json_bytes(&records[0]).unwrap().ends_with(b"\n"));
    }

    #[test]
    fn tree_roundtrip_excludes_internal_files_and_prevents_overwrite() {
        let temp = Temp::new();
        let mut files = Files::new();
        files.insert("nested/a.json".into(), b"{}\n".to_vec());
        files.insert("binary".into(), vec![0, 255, 13]);
        write_tree(&temp.0, &files).unwrap();
        write_file(&temp.0.join(".cc2cc/private"), b"excluded").unwrap();
        assert_eq!(collect_tree(&temp.0).unwrap(), files);
        assert!(write_tree(&temp.0, &files).is_err());
        assert_eq!(fs::read(temp.0.join("binary")).unwrap(), vec![0, 255, 13]);
        assert_eq!(inventory(&files)["binary"]["bytes"], 3);
        assert_eq!(
            digest(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(temp.0.join("nested"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(temp.0.join("binary"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn rejects_paths_outside_root() {
        let temp = Temp::new();
        for path in ["", "/absolute", "..", "a/../b", "./a", "a/./b", "a\\b"] {
            assert!(safe_path(&temp.0, path).is_err(), "{path}");
        }
        assert_eq!(
            safe_path(&temp.0, "nested/file").unwrap(),
            temp.0.join("nested/file")
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_files_directories_and_roots() {
        use std::os::unix::fs::symlink;
        let temp = Temp::new();
        let outside = Temp::new();
        symlink(&outside.0, temp.0.join("linked")).unwrap();
        assert!(safe_path(&temp.0, "linked/file").is_err());
        assert!(safe_path(&temp.0.join("linked"), "file").is_err());
        assert!(collect_tree(&temp.0).is_err());
        assert!(collect_tree(&temp.0.join("linked")).is_err());
    }
}
