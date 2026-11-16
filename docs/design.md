# andon 設計

## 名前の契約

> Devin セッション群の状態を一覧し、人間への判断要求を中継する。
> セッションの実行制御(起動・停止・コード生成)は含まない。

出典は工場の行灯(andon)。ラインの状態を一覧表示する「板」と、作業者が判断を仰ぐ「紐」の二要素が、セッションダッシュボードと判断要求の中継に対応する。

## 何をするものか

複数の Devin セッションの状態をカードとして一覧し、判断待ち(`waiting_for_user`)を選択肢ボタンつきのカードとしてスマホに届ける。人間は図と選択肢を見てタップし、回答はメッセージとしてセッションへ送られる。文字を読まずに判断だけするための層。

### 使い方の流れ

1. 常時稼働マシンで `andon serve` を起動する(Tailscale 等でスマホから到達させる)。
2. `andon send "タスク"` でセッションを作成する。カード形式の指示(knowledge note)とタグ `andon` が自動で付く。
3. スマホで PWA を開くとセッションがカード一覧で表示される。
4. セッションが `waiting_for_user` になると ntfy で push 通知される。
5. カードを開き、図解と選択肢を見てタップする。回答が `POST messages` でセッションへ返る。

## 接続経路

| adapter | 対象 | 仕組み | 優先度 |
|---|---|---|---|
| `adapter/devin_cloud` | Devin Cloud | v3 API ポーリングで状態検出、messages API で回答 | MVP |
| `adapter/devin_hooks` | Devin CLI / Desktop | ライフサイクル hook (`andon hook` コマンド) | 後回し |

### `devin_cloud` の検出と回答

- `GET /v3/organizations/{org}/sessions?tags=andon` を定期ポーリング(デフォルト 10 秒間隔)。
- `status == "running" && status_detail == "waiting_for_user"` → 判断カード化。最新の `source == "devin"` メッセージを本文に使う。
- `status_detail == "waiting_for_approval"` → 通知カード。**API から承認できない**ため、カードはセッション URL へのリンクを持つだけにする。`andon send` 時に `bypass_approval: true` を付けてこの状態自体を避ける選択肢も設定で持つ。
- `working` / `finished` / `suspended` → 状態カード(表示のみ)。
- 回答は `POST /v3/organizations/{org}/sessions/{devin_id}/messages` に `{ "message": ... }`。suspended 中でも自動 resume される。
- 認証は `Authorization: Bearer cog_...`。`GET /v3/self` で起動時に検証する。

### 判断カードの材料(2 経路)

1. **カード指令(主)**: セッションに「人間の判断が必要なときは、メッセージ末尾に `ANDON_JSON{...}` を付ける」という規約を knowledge note として注入する。`andon send` で作成したセッションには `knowledge_ids` で自動添付する。
2. **フォールバック**: 指令がないメッセージは、最終メッセージ抜粋 + 汎用選択肢(承認 / 却下 / 方針を変えて / 自由入力)でカード化する。

`ANDON_JSON` ペイロード案:

```json
{
  "title": "実装方針の選択",
  "summary": "……",
  "diagram": { "type": "mermaid", "source": "graph LR; A-->B" },
  "options": [
    { "id": "a", "label": "案A" },
    { "id": "b", "label": "案B" }
  ]
}
```

図解はセッション側が mermaid で記述し、andon はパースと描画だけを行う。

### `structured_output` の扱い

`POST sessions` に `structured_output_schema`(JSON Schema Draft 7)を渡すと、Devin が検証済みの構造データを `provide_structured_output` で返し、`session.structured_output` に入る。カードの公式チャネルとして有望だが、以下が未検証のため MVP では採用しない。

- ターン途中の中間出力(`is_final=false`)が API から読めるか
- `waiting_for_user` 中に最新出力が更新されるか

実機で確認できた場合、`ANDON_JSON` より正規の経路として昇格させる。

## API 調査メモ(Devin API v3)

Base: `https://api.devin.ai/v3`。認証は service user(`cog_`)または personal access token。必要権限は `ViewOrgSessions` + `ManageOrgSessions`、セッション作成を含めるなら `UseDevinSessions`。

| 用途 | エンドポイント |
|---|---|
| 認証確認 | `GET /v3/self` |
| セッション一覧 | `GET /v3/organizations/{org}/sessions`(cursor ページング、`tags`/`is_archived` 等で絞込) |
| セッション詳細 | `GET .../sessions/{devin_id}` |
| メッセージ一覧 | `GET .../sessions/{devin_id}/messages`(cursor、`source: devin|user`) |
| メッセージ送信 | `POST .../sessions/{devin_id}/messages` → `{message, attachment_urls}` |
| セッション作成 | `POST .../sessions` → `prompt` + `knowledge_ids`/`tags`/`bypass_approval`/`structured_output_schema`/`devin_mode` 等 |
| 洞察 | `GET .../sessions/{devin_id}/insights` → `analysis.timeline`/`issues`/`action_items`、AI生成のためオンデマンド(`POST .../insights/generate`) |
| knowledge | `POST .../knowledge/notes` → `{name, trigger, body}` |

### `status` / `status_detail` の対応

- `status`: `new` `claimed` `running` `exit` `error` `suspended` `resuming`
- `status_detail`(running 時): `working` `waiting_for_user` `waiting_for_approval` `finished`
- `status_detail`(suspended 時): `inactivity` `user_request` `usage_limit_exceeded` 等の理由

### その他の事実

- v1 API は非推奨。v3 を使う。
- Playbooks API も非推奨で Skills へ移行中(2026-10-24 以降作成アカウントは 410)。注入経路は knowledge notes が安定。
- 承認(waiting_for_approval)を解消する API は存在しない。
- `parent_session_id` / `child_session_ids` がある。Dynamic Workflows の子セッションを木構造で一覧できる。
- List Sessions の `qs` クエリパラメータは spec 上 required だが実態は未確認。実装時に検証する。
- レート制限は 429 で返る。ポーリングはバックオフを入れる(公式例は 10 秒間隔)。

## andon 自身の構成(実装済み)

```
src/
  main.rs                 起動のみ
  lib.rs                  モジュール宣言・Error/Result 再輸出
  cli.rs                  clap サブコマンド振り分け
  error.rs                Error 集約(thiserror)
  config.rs               環境変数の読み込み・検証
  types.rs                Session/Decision/CardOption 等の型
  card.rs                 ANDON_JSON パース・フォールバック選択肢・回答文組立
  store.rs                SQLite: sessions + decisions + meta
  answer.rs               回答実行(devin 送信 + カード answered 化)
  server.rs               axum: REST・SSE・認証 middleware・PWA 配信
  notify.rs               ntfy.sh 通知
  adapter/mod.rs          adapter 名前空間
  adapter/devin_cloud.rs  v3 API クライアント・ポーラー・カード検出
static/
  index.html  app.js      PWA(バニラ JS + mermaid CDN)
```

CLI: `serve` / `send` / `decide` / `setup` / `sessions` / `poll` / `ping`。
HTTP: `GET /` `GET /app.js` `GET /api/state` `POST /api/decisions/{id}/answer` `GET /api/events` `POST /api/poll`。
認証: `ANDON_TOKEN` があれば Bearer または `?token=`(SSE 用)を照合、なければ全通過。

## 技術スタック(確定)

Rust。edition 2024、依存は `=` で完全固定(oci/containers の流儀に合わせる)。

- axum + tokio(oci に先行例あり)+ tokio-stream(SSE 用)
- ureq で Devin API クライアント(同期・依存が薄い。ポーラーは専用スレッド、handler は spawn_blocking)
- rusqlite(bundled)+ Mutex<Connection> で状態保存
- clap(derive)、serde、thiserror
- フロントは `include_str!` で埋め込む単一バイナリ構成。描画に mermaid.js(CDN)

## MVP 範囲

実装済み:

- `adapter/devin_cloud`: polling → カード化 → 回答送信
- `choice` / `approve` カード(`status` 型は定義のみ)
- `ANDON_JSON` パース + フォールバック選択肢 + 自由入力
- knowledge note のプロビジョン(`andon setup`)
- ntfy.sh 通知、Bearer 認証、SSE ライブ更新、429 相当の指数バックオフ

含めない(後回し):

- `devin_hooks` adapter・`PreToolUse` 承認待機
- insights によるタイムライン図
- Dynamic Workflows の親子ツリー表示
- Web Push(ntfy 以外)、マルチユーザー、PR 表示、PWA manifest

## 未決事項(実機検証が必要)

- ~~`qs` パラメータの実際の形~~ **解決済み**: flattened パラメータ(`first`/`tags`/`is_archived`)で実 API から正常応答を確認。`qs` JSON フォールバックは残してある
- `structured_output` の中間更新が読めるか
- `waiting_for_user` 中のメッセージ送信が即 unblock になるか(送ると `status_detail` が `working` に戻るか)
- ポーリング間隔と 429 の実際の挙動
- knowledge note の `trigger` 条件をどう書けば判断要求時にだけ効くか

## ローカル Devin(CLI/Desktop)連携 — 調査結果

Cloud API とは別に、ローカルマシンで動く Devin と連携する経路を調査した。

### 利用できる資格情報

`~/.local/share/devin/credentials.toml` の `windsurf_api_key`(`devin-` 始まり)は **v3 API で有効**(`GET /v3/self` で 200、`principal_type: windsurf_session`)。org_id もここから得られる。つまり `devin auth login` なしでも Cloud API 側は動く。
一方 `devin -p` や `devin acp` は CLI 側の `devin auth login` を要求する(Desktop の認証とは別経路)。

### sessions.db — ローカルセッションの監視

`~/.local/share/devin/cli/sessions.db` にローカル(CLI/Desktop)の全セッションが入る。

- `sessions`: id・title・working_directory・agent_mode・last_activity_at
- `message_nodes`: `chat_message` JSON(role=assistant/user/tool、content、tool_calls)。最終 assistant 発言はここから取れる
- `tool_call_state`: ACP ToolCall/ToolCallUpdate の JSON。`tool_call_update_json IS NULL` = 実行中ツール
- `live_row_times`: 実行中の tool/thought 行
- `session_locks/<id>.lock`: 中身は PID。ファイル存在=そのセッションがプロセスを握っている

`waiting_for_user` 相当の検出ヒューリスティック: ロック有り(プロセス生存)+ 最終メッセージが assistant 発言 + 未完了 tool_call 無し。精度は要検証。

### `devin -p` / `-r` — ヘッドレス実行と回答注入

- `devin -p "タスク"` は非対話で 1 ターン走って終了する
- `devin -r <id> -p "回答"` で既存セッションにメッセージ注入して再開できる(回答チャネル)
- TUI がロックを握っているセッションへの注入は不可 → andon が spawn して所有するヘッドレス運用前提
- 要 `devin auth login`

### `devin acp` — 双方向プロトコル

- stdio JSON-RPC。実機でサーバ起動を確認
- `request_permission` がホストに返るので「スマホで承認」が実現できる(Cloud API にはない機能)
- ACP host が `authenticate` で資格情報を渡す設計。こちらも CLI 認証が必要

### 設計への反映案

```
adapter/
  devin_cloud.rs   既存(API ポーリング)
  devin_local.rs   sessions.db 監視 + devin -p/-r 注入
```

- `andon send --local` → 指定 cwd で `devin -p` を spawn、sessions.db から新 id を拾う
- ポーラーは sessions.db を読み、ロック中=実行中、最終 assistant メッセージ+ANDON_JSON→カード化
- 回答 → `devin -r <id> -p "回答"` を spawn
- Desktop/対話 CLI のセッションは監視のみ(回答不可)

### ローカル連携の制約

- `devin auth login` が一度必要(現在は Desktop のみ認証済み)
- TUI で開いているセッションへの注入は不可
- ACP adapter は permission 承認カードまで取れる本命だが JSON-RPC クライアント実装が要る
