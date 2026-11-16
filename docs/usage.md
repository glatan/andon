# andon 利用ガイド

andon は Devin セッション群の「行灯」である。各セッションの状態をカードで一覧し、人間への判断要求(`waiting_for_user`)を選択肢つきのカードとして届ける。人間のやることは、届いたカードの図を見て、選択肢をタップすることだけである。

## 全体像

```mermaid
flowchart LR
    subgraph cloud[Devin Cloud]
        S1[session A<br/>waiting_for_user]
        S2[session B<br/>working]
        S3[session C<br/>finished]
    end

    subgraph home[常時稼働マシン]
        P[poller<br/>10s ごと] --> ST[(SQLite)]
        SRV[server + PWA]
    end

    N[ntfy.sh]
    PH[スマホのブラウザ]

    cloud <-->|v3 API| P
    P --> SRV
    P -->|判断待ちを検出| N
    N --> PH
    PH <-->|SSE / REST| SRV
    PH -->|選択肢をタップ| SRV
    SRV -->|POST messages| cloud
```

andon が行うのは三段である。検出(ポーリング)→ 提示(カード・通知)→ 中継(回答をメッセージとして送信)。セッションの実行そのものには関与しない。

## セットアップ

必要なものは Devin API キーと組織 ID だけである。

```mermaid
flowchart TD
    A[Settings &gt; Devin API で<br/>service user を作成] --> B[環境変数を設定]
    B --> C[andon ping で認証確認]
    C --> D[andon setup で<br/>knowledge note 作成]
    D --> E[andon serve で起動]
```

1. Devin の **Settings > Devin API** で service user を作り、API キー(`cog_` 始まり)を取得する。必要な権限は `ViewOrgSessions`・`ManageOrgSessions`・`UseDevinSessions`。
2. 環境変数を設定する。

   ```sh
   export DEVIN_API_KEY=cog_...
   export DEVIN_ORG_ID=org-...     # Settings > Devin API の上部に表示される
   export ANDON_TOKEN=なにか長い文字列   # PWA/API の認証。推奨
   ```

3. `andon ping` で認証が通るか確認する。principal の JSON が返ればよい。
4. `andon setup` を実行する。カード規約(ANDON_JSON)を教える knowledge note が組織に作成され、ID がローカル DB に保存される。以後 `andon send` で作るセッションに自動添付される。
5. `andon serve` で起動する。既定では `127.0.0.1:8787` で待つ。

スマホから見る場合は、常時稼働マシンを Tailscale 等で到達可能にしておく(`ANDON_BIND=0.0.0.0:8787` + `ANDON_TOKEN` 必須、または tailscale アドレスにバインド)。

## 日常の使い方

```mermaid
sequenceDiagram
    participant U as 人間(スマホ)
    participant A as andon
    participant D as Devin Cloud
    participant N as ntfy

    U->>A: andon send "タスク"
    A->>D: POST /sessions (タグ andon + note)
    D-->>U: セッション開始(作業中)
    D->>D: 判断が必要になる<br/>status_detail=waiting_for_user
    A->>D: ポーリングで検出
    A->>A: 最終メッセージからカード作成<br/>(ANDON_JSON があれば選択肢を採用)
    A->>N: push 通知
    N->>U: 通知到着
    U->>A: カードを開き選択肢をタップ
    A->>D: POST messages "b: 案B"
    D->>D: 作業再開(status_detail=working)
    A->>A: カードを answered に更新
```

### セッションを作る

```sh
andon send "containers リポジトリの builder を調査して報告して"
```

`andon` タグが付き、ポーリング対象になる。Devin 側の UI や Slack から作ったセッションは管理対象外である(タグがないため)。既存セッションを対象に入れたい場合は Devin アプリで `andon` タグを付けるか、タグ絞り込みを外す運用を検討する(現状はタグ必須)。

### 判断が届く

セッションが入力待ちになると、最大 `ANDON_POLL_INTERVAL`(既定 10 秒)後にカードが現れ、ntfy 設定時は push 通知が飛ぶ。通知のリンクまたは andon の画面を開くと、ページ先頭に判断待ちカードが並ぶ。

### カードを読んでタップする

カードの構成は次の通りである。

```
┌─────────────────────────────────────┐
│ セッション名 · 3m 前                 │  ← 出自と経過時間
│ 実装方針の選択                        │  ← title
│ ────────────────────────────────    │
│ API の認証方式を2案で検討した。        │  ← summary / 最終メッセージ
│ [mermaid 図解]                       │  ← diagram(あれば)
│ ┌─────────────────────────────────┐ │
│ │ a: JWT セッショントークンで実装   │ │  ← 選択肢ボタン
│ ├─────────────────────────────────┤ │
│ │ b: パスキーで実装                │ │
│ └─────────────────────────────────┘ │
│ ┌──────────────────────────┐ ┌────┐ │
│ │ 自由入力で回答…           │ │送信│ │  ← 自由入力
│ └──────────────────────────┘ └────┘ │
└─────────────────────────────────────┘
```

- Devin が ANDON_JSON 指令を出したカードは、件名・要約・図解・選択肢がそのまま反映される。
- 指令がない質問は、最終メッセージの抜粋と汎用選択肢(承認 / 却下 / 方針を変えて)で作られる。
- 選択肢でも自由入力でも、回答はそのまま Devin へのメッセージとして送られる。
- `承認待ち (safe mode)` のカードには回答欄がなく、代わりに「Devin で開く」リンクが出る。safe mode の承認は API から行えないため、Devin アプリ側で操作する。

## 状態の読みかた

セッション一覧のドットとカード状態は次の対応である。

```mermaid
stateDiagram-v2
    working --> waiting_for_user : 入力待ち
    working --> waiting_for_approval : 承認待ち
    waiting_for_user --> pending : カード作成
    waiting_for_approval --> pending : カード作成
    pending --> answered : 回答をタップ
    pending --> expired : 待ち状態を抜けた
    waiting_for_user --> working : 回答後に再開
    working --> finished : 完了
```

| 表示 | 意味 | 要操作 |
|---|---|---|
| 橙ドット `working` | 作業中 | なし |
| 赤ドット `waiting_for_user` | 入力待ち | カードで回答 |
| 赤ドット `waiting_for_approval` | safe mode 承認待ち | Devin アプリで承認 |
| 緑ドット `finished` | 完了 | なし |
| 灰ドット | suspended / error 等 | Devin アプリで確認 |

`expired` になったカードは、Devin 側の UI で直接回答した場合など、andon 経由でなく待ち状態が解消されたものである。

## 通知(ntfy)

```sh
export ANDON_NTFY_TOPIC=andon-あなたの秘密のトピック名
```

ntfy.sh の場合、同じトピック名をスマホの ntfy アプリで Subscribe すると、判断待ちと承認待ちが push で届く。通知のクリック先はセッションの Devin URL である。自前の ntfy サーバを使う場合は `ANDON_NTFY_BASE` で差し替える。

## トラブルシューティング

- **カードが現れない**: `andon poll` を手で回す。`sessions` タブに対象セッション自体が出ないなら `andon` タグが付いているか確認する。`status_detail` が `waiting_for_user` でも最終メッセージに Devin 発言がないケースではカードは作られない。
- **回答が送れない**: セッションが suspended に変わった可能性がある。メッセージ送信は自動 resume するはずだが、失敗時は Devin アプリ側で確認する。
- **ポーリングが失敗し続ける**: serve の標準エラーに `[andon] ポーリング失敗:` と出る。API キー・org ID・権限を `andon ping` で切り分ける。
- **選択肢が出ない・文字だけの質問になる**: Devin が ANDON_JSON を付けていない。`andon setup` 済みか、そのセッションに note が添付されているか(`andon send` 経由か)を確認する。
