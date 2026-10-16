`cc2cc list DIRECTORY` で、指定した作業ディレクトリの会話だけを表示できるようになりました。
引数を省略すると全件を表示します。

```sh
cc2cc list .
cc2cc list /hoge/fuga --from claude
```

相対パスも使えます。
子ディレクトリの会話は含みません。

Apple Silicon Macとx86_64 Linuxのバイナリを配布しています。
次のコマンドで本体と両ツール用のスキルを更新できます。

```sh
curl -fsSL https://github.com/sudy-super/cc2cc/releases/latest/download/install.sh | sh
```
