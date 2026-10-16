# cc2cc

Claude Codeの会話をClawCodexで再開するためのCLIです。逆方向にも使えます。
変換後も、元と同じ作業ディレクトリの会話として保存します。
元の履歴は書き換えません。

## インストール

Apple Silicon Macとx86_64 Linuxに対応しています。Rustは不要です。

```sh
curl -fsSL https://github.com/sudy-super/cc2cc/releases/latest/download/install.sh | sh
```

本体を `~/.local/bin/cc2cc` に、スキルを `~/.claude/skills/cc2cc` と `~/.clawcodex/skills/cc2cc` に置きます。
コマンドが見つからない場合は `~/.local/bin` をPATHに追加してください。
Claude CodeとClawCodexを再起動すると、`/cc2cc` から変換を依頼できます。

## 会話を移す

保存されている会話の一覧を出します。

```sh
cc2cc list
```

移したい会話を選び、一覧の `id` の値を次の `SESSION_ID` と置き換えます。

```sh
# Claude Codeの会話をClawCodexへ移す
cc2cc convert SESSION_ID --to clawcodex

# ClawCodexの会話をClaude Codeへ移す
cc2cc convert SESSION_ID --to claude
```

変換後に表示されるコマンドで、移行先のツールから会話を再開します。
移行先で会話を続けた後に元のツールへ戻す場合も、同じ操作で変換します。

## 注意点

- 操作前に対象の会話を閉じてください。
- 移行後に追加した会話は戻せますが、元からある部分を編集、削除、圧縮した場合は停止します。
- 設定、認証情報、作業ファイル、実行中のツールやエージェントは移行しません。

保存先は `CLAUDE_CONFIG_DIR` と `CLAWCODEX_CONFIG_DIR` に従います。
省略時は `~/.claude` と `~/.clawcodex` を使います。

[保存形式と検証](docs/storage.md) / [開発とリリース](docs/development.md) / [MIT License](LICENSE)
