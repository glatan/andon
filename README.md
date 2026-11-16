# andon

Devin セッション群の状態を一覧し、人間への判断要求を中継するダッシュボード。
工場の行灯(andon)が名前の由来。セッションの実行そのものは Devin が行い、andon は「見る」と「選ぶ」だけを担当する。

複数セッションをカードで一覧し、判断待ち(`waiting_for_user`)を選択肢つきカードとしてスマホに届ける。文字を読まず、図を見て、タップで判断する。

## セットアップ

### Devin Cloud(API 経由)

```sh
export DEVIN_API_KEY=cog_...      # service user または personal access token
export DEVIN_ORG_ID=org-...       # Settings > Devin API に表示される org ID

# カード規約(ANDON_JSON)の knowledge note を作成して保存
andon setup
```

### ローカル Devin(CLI/Desktop)

```sh
andon serve --source local
```

API キー不要。`~/.local/share/devin/cli/sessions.db` を監視し、回答注入・セッション起動・権限承認は `devin acp`(stdio JSON-RPC)経由で行う。認証は credentials.toml の API キーを自動で読むため `devin auth login` は不要(ACP 不可時の `devin -r` フォールバックでは必要)。`--source both` で Cloud と併用もできる。

## 使い方

```sh
# 常時稼働マシンで起動(スマホからは Tailscale 等で到達)
andon serve                     # cloud
andon serve --source local      # ローカル Devin のみ
andon serve --source both       # 併用

# セッションを作る(カード規約の note と管理タグが自動で付く)
andon send "containers リポジトリの builder を調査して報告して"

# ローカルセッションを作る(devin -p で起動。--cwd で作業dir指定)
andon send --local --cwd ~/Projects/containers "builder を調査して報告して"
```

セッションが人間の入力待ちになるとカードが作成され(ntfy 設定時は push 通知)、選択肢をタップすると回答がセッションへ返る。PWA の「+ 新規」ボタンからもセッションを起動できる(ACP 経由・auth login 不要)。ACP 接続中は Devin の権限要求(許可/拒否)もカードとして届く。

## コマンド

| コマンド | 内容 |
|---|---|
| `andon serve [--source cloud\|local\|both]` | デーモン起動。ポーリング + Web UI(`ANDON_BIND`、既定 `127.0.0.1:8787`) |
| `andon send "タスク"` | Cloud セッション作成 |
| `andon send --local [--cwd dir]` | ローカルセッション起動(`devin -p`) |
| `andon decide <id> "回答"` | カードに CLI から回答(local/cloud 自動判別) |
| `andon setup` | knowledge note のプロビジョン |
| `andon sessions [--local]` | 対象セッション一覧(検証用) |
| `andon poll [--local]` | ポーリング 1 回実行(検証用) |
| `andon ping` | `GET /v3/self` 認証確認 |

## 環境変数

| 変数 | 必須 | 内容 |
|---|---|---|
| `DEVIN_API_KEY` | cloud 時 | `cog_` API キー。権限: `ViewOrgSessions` + `ManageOrgSessions` + `UseDevinSessions` |
| `DEVIN_ORG_ID` | cloud 時 | 組織 ID |
| `ANDON_TOKEN` | | PWA/API の Bearer トークン(未設定なら認証なし) |
| `ANDON_BIND` | | バインドアドレス(既定 `127.0.0.1:8787`) |
| `ANDON_DB` | | SQLite パス(既定 `./andon.db`) |
| `ANDON_POLL_INTERVAL` | | ポーリング秒間隔(既定 10) |
| `ANDON_NTFY_TOPIC` | | ntfy.sh トピック(設定時のみ通知) |
| `ANDON_NTFY_BASE` | | ntfy サーバ(既定 `https://ntfy.sh`) |
| `ANDON_BYPASS_APPROVAL` | | `1` でセッション作成時に safe mode 承認を回避 |
| `ANDON_KNOWLEDGE_NOTE_ID` | | note ID の上書き(通常は `setup` が DB に保存) |
| `ANDON_DEVIN_BIN` | | `devin` 実行ファイル(既定 `devin`) |
| `ANDON_DEVIN_HOME` | | Devin CLI データdir(既定 `~/.local/share/devin/cli`) |
| `ANDON_DEVIN_CREDENTIALS` | | credentials.toml(ACP 認証用。既定 `~/.local/share/devin/credentials.toml`) |
| `ANDON_LOCAL_DAYS` | | ローカル一覧の対象期間・日(既定 30) |
| `DEVIN_API_BASE` | | API ベース URL(既定 `https://api.devin.ai`) |

ローカルモードの注意: Desktop/TUI がロックを握っているセッションは一覧のみ(注入不可)で `in_terminal` と表示される。andon 起動・回答が通ったセッションのみがカード化対象になる。PWA は manifest 対応で「ホーム画面に追加」から standalone で開ける。

## ドキュメント

- [docs/usage.md](docs/usage.md) — 利用ガイド(セットアップ・使い方・図解)
- [docs/design.md](docs/design.md) — 設計・Devin API v3 調査結果・MVP 範囲
