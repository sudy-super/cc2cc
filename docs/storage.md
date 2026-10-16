# ClawCodex の保存形式と参照ファイルの調査記録

`cc2cc verify ID` は、変換時に保存した元ファイルのSHA-256、変換済みのメッセージの内容と順序、ツール呼び出しと結果の対応を調べる。
元ファイルの破損や、変換後に既存のメッセージを編集した場合はエラーを返す。
会話を実際のモデルへ送信する試験は行わない。

確認日: 2026-10-16。
公開 GitHub main と ローカルのClawCodex の HEAD は、ともに `17c950b9a59478dd867c49c5b2584ce435234b1e`。
GitHub API でコミットを確認し、`src/server/agent_server.py` のローカルと公開ファイルの Git blob が一致することも確認した。
調査と動作検証では公開ソースと合成会話だけを使用した。

## 現行 UI の保存・再開形式

現行の TUI、Desktop、agent-server、headless は `<config>/sessions/<ID>.json` を使用する。
`<config>` は `$CLAWCODEX_CONFIG_DIR`、未指定なら `~/.clawcodex`。
標準の変換出力はこの単一 JSON とし、入力は単一 JSON と後述の旧 JSONL ディレクトリ形式の両方を受け付ける。

| 根拠 | 確認した処理 |
|---|---|
| [clawcodex_dirs.py](https://github.com/agentforce314/clawcodex/blob/17c950b9a59478dd867c49c5b2584ce435234b1e/src/utils/clawcodex_dirs.py) | sessions と transcripts の保存先、設定ディレクトリの環境変数 |
| [agent_server.py の _save_session](https://github.com/agentforce314/clawcodex/blob/17c950b9a59478dd867c49c5b2584ce435234b1e/src/server/agent_server.py#L1712) | 現行セッション JSON の書込み |
| [agent_server.py の _do_resume](https://github.com/agentforce314/clawcodex/blob/17c950b9a59478dd867c49c5b2584ce435234b1e/src/server/agent_server.py#L3396) | 単一 JSON を Conversation.from_dict で再開 |
| [desktop_sessions.py](https://github.com/agentforce314/clawcodex/blob/17c950b9a59478dd867c49c5b2584ce435234b1e/src/server/desktop_sessions.py) | *.json の一覧、保存会話の表示、provider/model の読込み |
| [desktop_gateway_methods.py](https://github.com/agentforce314/clawcodex/blob/17c950b9a59478dd867c49c5b2584ce435234b1e/src/server/desktop_gateway_methods.py#L1538) | 保存会話を再開する実行用セッションの作成 |
| [agent/session.py](https://github.com/agentforce314/clawcodex/blob/17c950b9a59478dd867c49c5b2584ce435234b1e/src/agent/session.py) | headless の同形式での保存と再開 |

現行 JSON の主な属性は session_id、provider、model、cwd、updated_at、message_count、preview、name、mode、turns、conversation。
conversation は `{messages: [...], max_history: 2000}`。
cost、goal、scheduled_tasks が入る場合もある。
agent-server の updated_at は epoch 秒、headless の Session.save は ISO 文字列を保存する。
TUI の履歴一覧は現在の作業ディレクトリと cwd が一致するセッションに絞る。
provider/model は再開するモデルの選択に使われる。
セッション ID は英数字、`_`、`-` のみを許可する。

## 旧 JSONL ディレクトリ形式

[session_storage.py](https://github.com/agentforce314/clawcodex/blob/17c950b9a59478dd867c49c5b2584ce435234b1e/src/services/session_storage.py) は `<config>/sessions/<ID>/metadata.json`、`transcript.jsonl`、`content/` を扱う別 API。
[session_resume.py](https://github.com/agentforce314/clawcodex/blob/17c950b9a59478dd867c49c5b2584ce435234b1e/src/services/session_resume.py) で読めるが、現行 UI の一覧と再開はこの API を使わない。

metadata は session_id、start_time、model、cwd、title、total_cost、message_count、last_updated。
transcript はメッセージ辞書を1行ずつ保存する。
tool_result.content の文字数が10,000を超えると content/<UUID>.txt に分け、元ブロックへ _content_ref を付ける。
本体の read_messages と resume_session はその参照を解決しない。
変換時に本文を取得してインライン化し、元 JSONL と外部本文も原本として保存する必要がある。
参照先の欠落やディレクトリ外への参照は明示的に失敗させる。

## 子エージェント履歴の収集対象

[agent/transcript.py](https://github.com/agentforce314/clawcodex/blob/17c950b9a59478dd867c49c5b2584ce435234b1e/src/agent/transcript.py) は `<config>/transcripts/<agent_id>.jsonl` を保存する。
主セッションごとのフォルダーではなく、ユーザーの設定ディレクトリで共有される。
この共有ディレクトリを丸ごと自動収集すると無関係な会話まで取り込むため、原本保存では構造化された参照をたどって対象を限定する。

直接の子は、親メッセージの `toolUseResult = {type: "agent", agent_id: "...", ...}` から特定する。
根拠は [agent_server.py の _persisted_tool_use_result と _agent_display_envelope](https://github.com/agentforce314/clawcodex/blob/17c950b9a59478dd867c49c5b2584ce435234b1e/src/server/agent_server.py#L6757)。
現行の主セッションでは、この表示用データが保存された Agent 呼び出しと子履歴を結ぶリンクになる。

子から孫への参照は、主セッションと同じ場所だけを探しても不足する。
[tool_execution.py](https://github.com/agentforce314/clawcodex/blob/17c950b9a59478dd867c49c5b2584ce435234b1e/src/services/tool_execution/tool_execution.py#L440) は、子の toolUseResult を None にする一方、tool_result ブロックの `metadata.tool_output` に元のツール出力辞書を持たせる。
TranscriptWriter は dataclass.asdict で保存するので、この辞書が子 JSONL に残る。
[Agent ツールの出力](https://github.com/agentforce314/clawcodex/blob/17c950b9a59478dd867c49c5b2584ce435234b1e/src/tool_system/tools/agent.py#L766) は agent_id と status を含む。

参照収集の仕様は以下とする。

- 主セッションの toolUseResult と、子 JSONL 内の toolUseResult および tool_result.metadata.tool_output を調べる。
- `type == "agent"`、または status が completed、interrupted、async_launched、teammate_spawned の辞書にある agent_id を対象とする。
- agent_id は本体と同じく、空不可、英数字と `_` と `-`、最大64文字で検証する。
- 入力が既知の `<config>/sessions/` 以下にあるとき、その `<config>/transcripts/` の該当 JSONL だけを収集する。
- 収集した JSONL の構造化された参照を再帰的にたどり、既読 ID の集合で重複と循環を防ぐ。
- 本文中の agent_id や output_file の文字列、任意の絶対パスは自動収集の根拠にしない。
- 参照先がない場合は、原本から復元できない関連ファイルとして明示的に報告する。欠落を黙って完了扱いにしない。
- コピーした単一 JSON など設定ディレクトリを特定できない入力や、認識できない関連資料は `--attachments` で明示的に追加する。
- 自動収集・明示追加ともシンボリックリンクを拒否し、読み取り前後のファイル状態とハッシュを検査する。

子 JSONL の保存は履歴の保全であり、Claude Code 側のネイティブな子セッションへの変換や、実行中の子エージェントの再開を保証しない。
[resume_agent.py](https://github.com/agentforce314/clawcodex/blob/17c950b9a59478dd867c49c5b2584ce435234b1e/src/agent/resume_agent.py) はメモリ内の agent_continuations と実行用の restart 関数も必要とする。

## 分岐と圧縮の限界

[branch](https://github.com/agentforce314/clawcodex/blob/17c950b9a59478dd867c49c5b2584ce435234b1e/src/server/agent_server.py#L3331) は全会話を新しい単一 JSON にコピーする。
親 ID や parentUuid による分岐の木は保存しない。
[rewind](https://github.com/agentforce314/clawcodex/blob/17c950b9a59478dd867c49c5b2584ce435234b1e/src/server/agent_server.py#L3621) は現在の会話配列の末尾を削除する。
[圧縮処理](https://github.com/agentforce314/clawcodex/blob/17c950b9a59478dd867c49c5b2584ce435234b1e/src/compact_service/service.py) も会話を要約へ置き換える。
既に本体で削られた圧縮前・巻き戻し前の会話は、残ったセッション JSON だけから復元できない。
[FileHistory](https://github.com/agentforce314/clawcodex/blob/17c950b9a59478dd867c49c5b2584ce435234b1e/src/utils/file_history.py) のチェックポイントはメモリ内で管理され、セッションファイルへの保存処理はない。

## 本体の再保存で落ちる属性

[messages.py](https://github.com/agentforce314/clawcodex/blob/17c950b9a59478dd867c49c5b2584ce435234b1e/src/types/messages.py) と [content_blocks.py](https://github.com/agentforce314/clawcodex/blob/17c950b9a59478dd867c49c5b2584ce435234b1e/src/types/content_blocks.py) を全文確認した。
未知のメッセージ属性と、既知ブロックの追加属性は型への読込みで落ちる。
requestId は読めるが再保存時に落ちる。
tool_result の metadata と _content_ref も保存されない。
thinking.signature、redacted_thinking.data、画像・文書の source、未知のブロック型の辞書は保持される。

原本は変更せず保存する。
本体の再保存と同じ変更だけを検出するため、src/claw_normalize.rs で message_from_dict → message_to_dict を投影する。
比較は「元の出力を投影した結果」と「読み戻した実ファイル」を厳密に照合する。
読み戻したファイルまで投影すると、ユーザーが追加・変更した属性も消えて編集の検知を妨げるため、その比較方法は使わない。

## 検証済みの範囲

- 公開 main とローカル実装が同一コミットであること。
- 合成3メッセージを本体の Conversation.from_dict と _AgentSession._do_resume で読み込み、API 向け会話も生成できること。外部 API やユーザー設定へ触れる周辺処理だけ置き換えた。
- 合成した旧 JSONL の read_messages と resume_session が外部本文を自動復元しないこと。
- 旧 resume_session が最後の isCompactSummary より前を除き、未完了の tool_use に結果を合成すること。
- Rust版は正常な合成1,000例で本体の loader → serializer と一致した。移植前の比較基準は、合成5,076例で本体と一致していた。不正な型はRust側で明示的に拒否する。

生成した bundle の現行セッション JSON を使い、Conversation、Desktop の一覧と会話読込み、本体 _do_resume が成功することも確認した。
合成した変換元の会話2件から生成された native の SHA-256 は `1fb30874dc3a6c947da1ebb8c204ce4455d167d7de31a19fc56724a304a9c36a` で、検証前後に変化しなかった。
再現用の `scripts/verify_claw_native.py` は `--source` に本体のソースルートを指定し、bundle、native ディレクトリ、セッション JSON のいずれかを入力する。
本体の依存パッケージが入った Python で実行し、通常の tests には本体への依存を持ち込まない。
スクリプトはファイルへの書込み、通信、外部プロセスの起動を audit hook で禁止し、本体の設定保存先も未使用の検証用パスへ分離する。
実プロバイダーへ次のメッセージを送信する試験、プロバイダー間での thinking 署名や画像の受理、実行状態や子セッションの完全な移植は、この読込み試験だけでは確認できない。

## CLI と Desktop 起動時のモデル選択

現行の [cli.py の _build_parser](https://github.com/agentforce314/clawcodex/blob/17c950b9a59478dd867c49c5b2584ce435234b1e/src/cli.py#L396) と [tui_launcher.py](https://github.com/agentforce314/clawcodex/blob/17c950b9a59478dd867c49c5b2584ce435234b1e/src/entrypoints/tui_launcher.py) に `--resume` はない。
実際の _build_parser でも `--resume <ID>` が拒否されることを確認した。
起動後の `/resume <ID>` または UI の履歴一覧を使用する。

[DesktopServeState.spawn_for](https://github.com/agentforce314/clawcodex/blob/17c950b9a59478dd867c49c5b2584ce435234b1e/src/server/desktop_serve.py#L74) は provider/model が両方空で effort の指定もない場合、既定の spawn_agent を使う。
これも実メソッドで確認した。
その場合はユーザー設定とサーバー起動時の設定が使われる。
一方、保存 JSON の model だけが非空なら、そのモデルを既定の provider へ渡して新しい実行用セッションを起動する。
履歴中のモデルが既定 provider で使える保証はないため、変換器がモデルと provider の有効な組を確定できない場合は、ルートの provider/model を両方空とし、履歴中の assistant.model に元のモデル名を残す。


## Rust CLIとしての最終実装

変換器はRustへ移植した。
Python製の変換処理は削除し、実行時のPython依存をなくした。
テスト用のPython比較基準は tests/claw_normalize_oracle.py に残し、統合テストはRust実行ファイルを呼び出す。
正常な合成例で、本体との再保存の投影比較1,000件、移植前との変換比較64件、現行JSON生成比較2件が一致した。
数値は serde_json の任意精度設定で保持し、重複キーと不正なJSONを拒否する。
JSONライブラリの予約キー `$serde_json::private::Number` は、オブジェクトを誤って数値として扱うことを防ぐため明示的に拒否する。

## Claude Codeの確認結果

一次ソース: [Claude Code公式のセッション管理](https://code.claude.com/docs/en/sessions)。
確認したローカルCLIは2.1.301。
公式資料は、JSONLの絶対パスを指定する `claude --resume`、projectごとの履歴保存、設定ディレクトリの変更を説明している。
履歴JSONL自体は公開された固定の変換仕様ではないため、将来のバージョンへの互換性を保証しない。

ローカルの履歴では型とキーだけを調べ、attachmentにもuuid/parentUuidがあること、renderedの配列に再開用の文脈が含まれること、systemにlogicalParentUuid/compactMetadataがあることを確認した。
分岐の末端判定にはlogicalParentUuidも参照するが、圧縮前の内容をAPI向け会話へ再注入しないため、実際の会話列はparentUuidだけでたどる。
複数の分岐がある場合は、末尾メッセージの日時が最も新しい分岐を自動で選ぶ。
日時が同じ場合は、ファイル内で末尾メッセージが後ろに記録された分岐を選ぶ。
別の分岐を使う場合は `--leaf UUID` で指定できる。
別分岐、圧縮前の行、独自イベントは原本に残す。
UUID重複や解決できない参照は、推測で結合せず停止する。

macOSのsandbox-execでネットワークを禁止し、隔離したCLAUDE_CONFIG_DIR、ダミーAPIキー、合成会話だけを使ってClaude Code本体を起動した。
生成JSONLの絶対パスを指定すると、ユーザー発言、Readのツール呼び出し、アシスタント発言を履歴として表示できた。
プロンプトは送信していないため、実プロバイダーへの次のターンの成功を確認したわけではない。
作業ディレクトリがシンボリックリンクの別名を経由する場合にID指定の探索差があったため、installのresume案内はJSONLの絶対パスを使う。
英数字以外を置換した作業パスが200文字を超える場合のClaude Code登録名は未実装とし、installを停止して絶対パスによる再開を案内する。
