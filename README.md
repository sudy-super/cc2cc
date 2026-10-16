# cc2cc

Claude CodeとClawCodexの会話履歴を相互に変換するRust CLIです。
セッションIDを指定すると、元の作業ディレクトリの会話として登録します。
元の履歴は変更せず保存するので、移行先で会話を続けてから戻すこともできます。

## インストール

Apple Silicon Macとx86_64 Linuxに対応しています。Rustは不要です。

```sh
curl -fsSL https://github.com/sudy-super/cc2cc/releases/latest/download/install.sh | sh
```

本体を `~/.local/bin/cc2cc` に、スキルを `~/.claude/skills/cc2cc` と `~/.clawcodex/skills/cc2cc` に置きます。
コマンドが見つからない場合は `~/.local/bin` をPATHに追加してください。
Claude CodeとClawCodexを再起動すると、`/cc2cc` から変換を依頼できます。

## 使い方

```sh
# セッションIDを確認する
cc2cc list

# Claude Codeの会話をClawCodexへ移す
cc2cc convert SESSION_ID --to clawcodex

# ClawCodexの会話をClaude Codeへ移す
cc2cc convert SESSION_ID --to claude
```

変換後に表示されるコマンドで会話を再開してください。
たとえば `/hoge/fuga` の会話をClaude Codeへ移すと、`~/.claude/projects/-hoge-fuga/` に新しいIDで保存します。
変換前に確認したい場合は `--dry-run` を付けます。
複数の分岐がある会話は、表示された末端のUUIDを `--leaf` に指定してください。

## 保存するもの

本文、ツール呼び出しと結果、思考、画像、文書、モデル情報、トークン使用量を扱います。
変換先で使えない情報と関連ファイルも原本として保存します。
`cc2cc verify SESSION_ID` で、変換した履歴と原本を検証できます。

操作前に対象の会話を閉じてください。
移行後に追加した会話は戻せますが、既存部分を編集、削除、圧縮した履歴は自動では戻しません。
設定、認証情報、作業ファイル、実行中のツールやエージェントは移行しません。
画像や思考の情報をAPIが受け付けるかどうかは、使用するモデルによります。

保存先は `CLAUDE_CONFIG_DIR` と `CLAWCODEX_CONFIG_DIR` に従います。
省略時は `~/.claude` と `~/.clawcodex` を使います。

[保存形式と検証](docs/storage.md) / [開発とリリース](docs/development.md) / [MIT License](LICENSE)
