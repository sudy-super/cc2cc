# cc2cc

Claude Codeの会話をClawCodexで再開するためのCLI。逆方向にも使えます。
変換後も、元と同じ作業ディレクトリの会話として保存します。
元の履歴は書き換えません。

## インストール

Apple Silicon Macとx86_64 Linuxに対応しています。Rustは不要です。

```sh
curl -fsSL https://github.com/sudy-super/cc2cc/releases/latest/download/install.sh | sh
```

実体: `~/.local/bin/cc2cc` 
Skill:  `~/.claude/skills/cc2cc` , `~/.clawcodex/skills/cc2cc` 
コマンドが見つからない場合は `~/.local/bin` をPATHに追加してください。
Claude CodeとClawCodexを再起動すると、`/cc2cc` から変換を依頼できます。

## 会話を移す

```sh
# 特定のディレクトリから移したい会話を見る
cc2cc list /hoge/fuga

# Claude Codeの会話をClawCodexへ移す
cc2cc convert SESSION_ID --to clawcodex

# ClawCodexの会話をClaude Codeへ移す
cc2cc convert SESSION_ID --to claude
```

会話が分岐している場合は、最後のメッセージが最も新しい分岐を使います。

## 注意点

- 操作前に対象の会話を閉じてください。
- 移行先で追加した会話も元のツールへ戻せますが、移行した時点の履歴を編集、削除、圧縮すると、元のツールへの変換はエラーになります。
- 設定、認証情報、作業ファイル、実行中のツールやエージェントは移行しません。

保存先は `CLAUDE_CONFIG_DIR` と `CLAWCODEX_CONFIG_DIR` に従います。
省略時は `~/.claude` と `~/.clawcodex` を使います。

[保存形式と検証](docs/storage.md) / [開発とリリース](docs/development.md) / [MIT License](LICENSE)
