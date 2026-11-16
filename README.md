# andon

Devin セッション群の状態を一覧し、人間への判断要求を中継するダッシュボード。
工場の行灯(andon)が名前の由来。セッションの実行そのものは Devin が行い、andon は「見る」と「選ぶ」だけを担当する。

複数セッションをカードで一覧し、判断待ち(`waiting_for_user`)を選択肢つきカードとしてスマホに届ける。文字を読まず、図を見て、タップで判断する。

## セットアップ

```sh
export DEVIN_API_KEY=cog_...      # service user または personal access token
export DEVIN_ORG_ID=org-...       # Settings > Devin API に表示される org ID

# カード規約(ANDON_JSON)の knowledge note を作成して保存
andon setup
```

## 使い方

```sh
# 常時稼働マシンで起動(スマホからは Tailscale 等で到達)
andon serve

# セッションを作る(カード規約の note と管理タグが自動で付く)
andon send "containers リポジトリの builder を調査して報告して"
```

セッションが人間の入力待ちになるとカードが作成され(ntfy 設定時は push 通知)、選択肢をタップすると回答がメッセージとしてセッションへ返る。

## コマンド

| コマンド | 内容 |
|---|---|
| `andon serve` | デーモン起動。ポーリング + Web UI(`ANDON_BIND`、既定 `127.0.0.1:8787`) |
| `andon send "タスク"` | セッション作成 |
| `andon decide <id> "回答"` | カードに CLI から回答 |
| `andon setup` | knowledge note のプロビジョン |
| `andon sessions` | 管理対象セッション一覧(検証用) |
| `andon poll` | ポーリング 1 回実行(検証用) |
| `andon ping` | `GET /v3/self` 認証確認 |

## 環境変数

| 変数 | 必須 | 内容 |
|---|---|---|
| `DEVIN_API_KEY` | ✓ | `cog_` API キー。権限: `ViewOrgSessions` + `ManageOrgSessions` + `UseDevinSessions` |
| `DEVIN_ORG_ID` | ✓ | 組織 ID |
| `ANDON_TOKEN` | | PWA/API の Bearer トークン(未設定なら認証なし) |
| `ANDON_BIND` | | バインドアドレス(既定 `127.0.0.1:8787`) |
| `ANDON_DB` | | SQLite パス(既定 `./andon.db`) |
| `ANDON_POLL_INTERVAL` | | ポーリング秒間隔(既定 10) |
| `ANDON_NTFY_TOPIC` | | ntfy.sh トピック(設定時のみ通知) |
| `ANDON_NTFY_BASE` | | ntfy サーバ(既定 `https://ntfy.sh`) |
| `ANDON_BYPASS_APPROVAL` | | `1` でセッション作成時に safe mode 承認を回避 |
| `ANDON_KNOWLEDGE_NOTE_ID` | | note ID の上書き(通常は `setup` が DB に保存) |
| `DEVIN_API_BASE` | | API ベース URL(既定 `https://api.devin.ai`) |

## ドキュメント

- [docs/usage.md](docs/usage.md) — 利用ガイド(セットアップ・使い方・図解)
- [docs/design.md](docs/design.md) — 設計・Devin API v3 調査結果・MVP 範囲
