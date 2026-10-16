# 開発

Rust 1.90以降とPython 3.10以降を使います。
Pythonはテスト用で、cc2ccの実行には不要です。

```sh
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked --bin cc2cc --examples
python3 -m unittest discover -s tests -v
```

`tests/driver/main.rs` は保存形式のテストに使う補助プログラムです。
利用者向けのコマンドは `src/main.rs` にあります。
実際のClaude Codeで表示を確認する場合は、macOSで `python3 scripts/verify_claude_native.py` を実行します。
ClawCodex本体での確認には `scripts/verify_claw_native.py --help` を参照してください。

# リリース

0.xの間は、機能の追加や使い方が変わる更新では2番目の数字を、不具合や文書の修正では3番目の数字を上げます。

Cargo.tomlのバージョンと一致する `vX.Y.Z` タグをpushすると、GitHub Actionsが両OSでテストとビルドを行います。
両方が成功した後に、アーカイブ、SHA-256の一覧、install.shをGitHub Releasesへ公開します。
Macは `aarch64-apple-darwin`、Linuxは静的リンクの `x86_64-unknown-linux-musl` です。

インストーラーの配置先は `CC2CC_INSTALL_DIR`、バージョンは `CC2CC_VERSION` で指定できます。
スキルの配置先は `CLAUDE_CONFIG_DIR` と `CLAWCODEX_CONFIG_DIR` に従います。
同じインストーラーを再実行すると本体と管理対象のスキルを更新します。
自作の同名スキルは上書きしません。
