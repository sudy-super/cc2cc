会話が分岐している場合に、最後のメッセージが最も新しい分岐を自動で選ぶようになりました。
日時が同じ場合は、履歴の後ろに記録された分岐を使います。
選ばれなかった分岐も原本に保存します。

```sh
cc2cc convert SESSION_ID --to clawcodex
```

別の分岐を使う場合は、引き続き `--leaf UUID` で指定できます。

Apple Silicon Macとx86_64 Linuxのバイナリを配布しています。
次のコマンドで本体と両ツール用のスキルを更新できます。

```sh
curl -fsSL https://github.com/sudy-super/cc2cc/releases/latest/download/install.sh | sh
```
