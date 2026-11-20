<p align="center">
  <img src="resources/comp-icon.png" width="128" height="128" alt="comP Logo">
</p>

# comP — AI に、リポジトリの地図を持たせる

comP は、Claude Code や Codex、Cursor などの AI コーディングエージェントのための VS Code 拡張です。リポジトリのファイル・関数・呼び出し関係を手元で索引にして、エージェントに渡します。作業の最初に comP に聞けば、AI はどこに何があり、何が何に依存しているかを一度でつかめます。オープンソース（MIT）で、索引も作業の記録も、手元のマシンに残ります。

🌐 [公式サイト](https://tsucky230.github.io/comP/) ／ [English](README.md)

全体像は「作ったきっかけ」から「MAGATAMA」の節までで分かります。導入は「インストールと設定」から始めてください。それより後ろの節は、必要なときに引けば足ります。

---

## 作ったきっかけはトークン代でした

最初の目的は単純で、AI に読ませるトークンを減らすことでした。Claude Code に小さな修正を頼むたびに、AI は grep を何度も打ち、関係のないファイルまで開いて、ようやく本題にたどり着きます。その探索に毎回お金と待ち時間を払うのが、もったいなく感じていました。

そこで、リポジトリを一度だけ索引にしておき、作業の内容を伝えると関係のあるファイルとシンボル（関数やクラスの名前）だけを返す仕組みを作りました。トークンは確かに減りました。ただ、使い続けるうちに、それより大きな変化に気づきました。AI の仕事の質が変わったのです。

地図を持った AI は、最初から正しいファイルを開き、関数を変える前に呼び出し元を確かめ、前回のセッションで決めたことを踏まえて続きを始めることが増えました。目の前のファイルだけを見ていた視野が、リポジトリ全体に広がった感覚です。私は、この変化が comP のいちばんの価値だと考えています。これから AI に任せる仕事を増やしたい人に、ぜひ自分のリポジトリで試してほしいと思っています。

---

## 地図を持った AI は仕事の質が変わる

comP は MCP（AI エージェントが外部のツールを呼ぶための共通の仕組み）のサーバーとして動き、`run_pipeline` などのツールを AI に渡します。ふだんは、あなたが作業を頼むと AI が自分でこれらのツールを呼びます。

### 最初の一手で関係するファイルにたどり着く

やりたい作業を AI が一文で `run_pipeline` に渡すと、comP は索引を検索し、関係の深い順にファイルを返します。まだコミットしていない変更があるファイル（`git diff HEAD` の対象）は先頭に置き、依存関係で一歩つながるファイルも添えます。AI の読み始めがここになるので、見当違いのファイルを開き、誤った前提で実装して、やり直す、という回り道が起きにくくなります。頼む側は、いつもどおりに頼むだけです。日本語で頼んでも、AI がツールに渡すときに英語に直します。

```text
認証ミドルウェアの JWT 有効期限のバグを直して。まず comP の run_pipeline で関係するコードを探してから始めて。
```

マルチルートのワークスペースへの対応を頼んだときは、AI が最初の `run_pipeline` の結果から、既存の不具合に実装の前に気づきました。返ってきた `daemon/src/main.rs` を読み、`additional_paths` 機能がメインの索引を消してしまうことを見つけたのです。頼まれた場所の外にまで目が届くのは、地図があるからだと思います。

### 変える前に波及先が見える

AI に任せた変更で私がいちばん怖いのは、関数を一つ直したつもりで、別のモジュールの呼び出し元を壊してしまうことです。`get_impact_graph` は、指定したシンボルに依存している関数とファイルを、依存グラフをたどって列挙します。名前から依存元を引きたいときは `get_dependencies` も使えます。次は、comP 自身のパス検証の関数に対して実際に返ってきた結果です（一部）。

```text
get_dependencies({ "name": "validate_within_workspace", "direction": "in" })
- Depended on by `handle_compress_file` (function) via `function_call` in `daemon/src/mcp/mod.rs`
```

この検証を変えるなら、`compress_file` のふるまいとテストも確かめる必要がある、と手を入れる前に分かります。変更の前に AI がこれを呼べば、確かめるべき呼び出し元とテストを押さえたうえで編集できます。変更を重ねるたびに少しずつ品質が落ちていく、AI 開発でよくある劣化を、ここで食い止めたいと考えています。

### 昨日までの作業と判断を次のセッションが引き継ぐ

AI とのセッションは、閉じれば終わりです。翌日「なぜリトライを3回に制限したんだっけ」と聞いても、新しいセッションは何も知りません。

comP は、AI が `run_pipeline` と `get_context` で調べた内容（問い・見つかったファイル・シンボル）を、セッションをまたいで自動で記録します。判断まで残るのは、作業の区切りで AI が `session_log` に依頼と結果を書き残したときです。次のセッションで `session_recall` を呼べば、前回の続きから始められます。次は、comP 自身の開発で実際に返ってきた記録の抜粋です（書式と長い行を省いています）。

```text
session_recall({ "query": "stale Claude Code comp MCP paths" })
- 2026-10-19 13:23  Query: "Implement automatic repair of stale Claude Code comp MCP paths on VS Code startup, ..."
  - Outcome: Updated src/mcp/AgentSetup.ts to refresh versioned old comP extension binary paths
    even if old files exist, ... Did not alter live user-scope Claude configuration or remove
    duplicate registration.
  - Files: src/mcp/AgentSetup.ts, src/mcp/tests/AgentSetup.test.ts, docs/user/MCP_SETUP.md, CHANGELOG.md
```

comP の裏で動くデーモン（索引を管理する常駐プロセス）を再起動しても、Claude Code から Codex にエージェントを替えても、同じ記録を読めます。日本語の問いでも引けます。私の使っている範囲では、Claude や ChatGPT のメモリー機能は会話の要約や好みを覚えるのが得意で、コードの細部は要約の途中で落ちがちでした。comP の記録は、そのリポジトリでの技術的な判断に絞った索引です。覚えるものが違うので、両方を使い分けるのがおすすめです。

---

## 数字で見ると

効果は、リポジトリの大きさと作業の種類で大きく変わります。測った値を、条件と一緒に載せます。

| 場面 | 値 | 読み方 |
| --- | --- | --- |
| comP 自身のリポジトリ（索引上で約44万トークン分）で、上で書いたマルチルート対応の作業に `run_pipeline` が返した量 | 約8,000トークン（ファイル9件） | 既定の予算 8,000 トークンに合わせて圧縮した量で、リポジトリ全体の約1.8%にあたります。AI が実際にどれだけ読むかは作業によるので、ここから出る削減率は上限と考えてください |
| 後述の MAGATAMA のリポジトリで、概要をつかむ作業1件（MAGATAMA 側の計測） | 選んだソースの推定 145,978トークンに対し、comP の要約は 289トークン | 一度きりの計測です。どのプロジェクトでも同じ割合になるわけではありません |
| 数十ファイルほどの小さなリポジトリ | 差は小さい【未計測の見込み】 | 全部読んでも安く済むので、伸びしろが少なめです |

探索で迷った AI のやり直しが減ることは、表の数字に出にくい効果です。迷った AI は、違うファイルを読み、誤った前提で実装し、テストに落ちてやり直します。この往復は、1回で数千トークン【目安】を使います。私の使っている範囲では、comP を入れてからこの往復が目に見えて減り、安いモデルほどその差が大きく出ました。

---

## MAGATAMA と組むと変更前に確かめる場所を根拠つきで出せる

[MAGATAMA（勾玉）](https://github.com/tsucky230/MAGATAMA) は、comP の索引を読んで「このファイルを変えたら、どこを確認すればいいか」を答える、もう一つの MCP サーバーです。comP が地図を作り、MAGATAMA はその地図を変更前の点検に使います。

comP の `get_impact_graph` が、1つのシンボルから依存元をたどって並べるのに対し、MAGATAMA の `analyze_change_scope` は、変更するファイルを単位に受け取ります。そのファイルの中のシンボルすべてから依存関係を逆にたどり、波及候補ごとに、どの経路で影響が及ぶかを根拠として添えます。索引に手がかりが足りないところは、推測で埋めずに「unknown（要確認）」と書きます。

| 知りたいこと | 使う入口 |
| --- | --- |
| 最初に読む関数はどれか | comP の `run_pipeline` / `get_context` |
| このファイルを変えたら、どこを確認するか | MAGATAMA の `analyze_change_scope` |
| 前にこのファイルを触った理由は何か | MAGATAMA の履歴照会と、comP の `session_recall` |
| 前回から何が変わったか | MAGATAMA の patrol（留守の間の変更を確かめて次のセッションに残す機能）と、comP の `session_recall` |

`analyze_change_scope` に変更するファイルを渡すと、次の3つが返ります。そのファイルに含まれるシンボル、呼び出し元などの波及候補、そして各候補が挙がった根拠となる依存関係です。ファイル名があいまいなときや、依存関係が索引にないときは「unknown」になります。私はこの正直さを気に入っています。結果は確認の出発点で、実行時の影響をすべて証明するものではありません。

MAGATAMA の登録と更新は、comP に任せられます。[uv](https://docs.astral.sh/uv/) を入れ、設定 `comp.magatama.manage` を ON にして（またはコマンド **comP: MAGATAMA の更新を comP に任せる** を実行して）から、**comP: Setup Agents** を実行してください。comP は MAGATAMA を `uvx magatama@<版> serve` で起動するように登録し、版は comP が動作を確かめたものに固定します。comP が Marketplace から更新されると、MAGATAMA もそれに合わせて上がります。comP が書き換えるのは自分で書いた登録だけで、版を下げることはありません。MAGATAMA を手で登録している場合は、comP が起動時に版を確かめ、comP が動作を確かめた版より古ければお知らせします。手動の導入手順は MAGATAMA の README にあります。

---

## インストールと設定（3ステップ）

VS Code 1.91 以降で動きます。デーモンは Windows・macOS・Linux 用を同梱しています。git のリポジトリで使うと、未コミットの変更を優先する機能や、ブランチ切り替えの検知も働きます。

### 1. VS Code にインストールする

拡張機能ビュー（`Ctrl+Shift+X`）で「comP - Code Context Engine」を検索して、インストールします。

### 2. フォルダを開いて索引を作る

作業するリポジトリを開き、アクティビティバー（左端）の comP アイコンから **Start** を押します。索引はバックグラウンドで作られ、進み具合はステータスバーに出ます。2回目からは、変更のあったファイルだけを解析し直します。

```text
◈ comP: 12534 symbols | ✓ Ready
```

索引と作業の記録はリポジトリの `.comp/` に入ります。そのまま git に含めると、作業の記録や会話の本文までリモートに上がってしまうので、`.gitignore` に次の2行を足してください。チームで除外の設定（`.comp/ignore`）を共有したいときのために、2行目でこのファイルだけを残しています。

```gitignore
.comp/*
!.comp/ignore
```

### 3. AI エージェントにつなぐ

コマンドパレット（`Ctrl+Shift+P`）で `comP: Setup Agents` を実行し、使うエージェントを選びます。この PC で見つかったエージェントは、最初から選ばれています。comP は各エージェントが実際に読む設定ファイルへ書き込み、既存の MCP サーバーの設定はそのまま残します。

| エージェント | 書き込み先 |
| --- | --- |
| Claude Code | `.mcp.json`（任意で `claude mcp add --scope user` も実行）。会話の記録（β版）が ON なら、Stop フック（`.claude/settings.local.json`）も |
| Codex | `~/.codex/config.toml` と `.codex/config.toml` |
| GitHub Copilot | `.vscode/mcp.json` |
| Cursor | `.cursor/mcp.json` と `~/.cursor/mcp.json` |
| Cline | VS Code の globalStorage にある MCP 設定 |
| Windsurf | `~/.codeium/windsurf/mcp_config.json` |
| Continue.dev | `.continue/mcpServers/comp.yaml`（プロジェクトとホーム） |
| Antigravity（App・IDE・`agy` CLI・VS Code 拡張） | `~/.gemini/config/mcp_config.json` |
| Aider | `.aider.conf.yml` |
| Gemini CLI | `.gemini/settings.json` と `~/.gemini/settings.json`（有料の Gemini API キーか、Code Assist Standard / Enterprise のライセンスがある場合のみ。個人アカウントは 2026-06-18 に提供終了） |

書き込んだら、エージェントを再起動してください。どのエージェントも、起動している間は MCP の設定を読み直しません。VS Code 系のエディタなら、コマンドパレットの **Developer: Reload Window** で足ります。エージェントごとの手順は出力パネルの「comP Setup」に表示され、詳しくは [MCP 設定ガイド（英語）](docs/user/MCP_SETUP.md) にあります。

---

## AI に comP を使ってもらうコツ

comP を入れても、AI は自分の判断で grep やファイルの直読みに戻ることがあります。私の使っている範囲では、指示ファイル（Claude Code なら `CLAUDE.md`、Codex なら `AGENTS.md`、Copilot なら `.github/copilot-instructions.md`）にルールを書いておくと、ほとんど収まりました。`comP: Setup Agents` も最小限のルールを書き込みますが、次のプロンプトを AI に渡せば、プロジェクトに合わせて書き直してくれます。

```markdown
CLAUDE.md を更新してください。次を MUST / NEVER のルールとして追記し、理由を1行ずつ添えてください。
既存のルールと矛盾する箇所があれば指摘してください。

MUST:
- コードの調査は grep や find より先に、comP の run_pipeline を呼ぶ
- 既存のコードを変える前に、get_impact_graph で波及先を確かめる
- セッションの始めに、session_recall で過去の関連する判断を確かめる
- 作業の区切りで、session_log に依頼と結果を残す

NEVER:
- run_pipeline の結果にないファイルを、推測で前提にする
```

それでも直読みが起きたら、「なぜ run_pipeline を使わなかったのか、原因を1行で述べ、防ぐルールを1行提案して」と聞いてみてください。違反のたびにルールを1行ずつ育てると、私の場合は数回で落ち着きました。

---

## 設定

索引から外したいものは、プロジェクトのルートに `.comp/ignore` を作り、`.gitignore` と同じ書き方で並べます。`.` で始まるファイルとフォルダ、`.gitignore` に一致するパス、5 MiB を超えるファイル、`node_modules`・`venv`・`__pycache__`・`coverage`・`vendor`・`out` は、何もしなくても外れます。VS Code の設定 `comp.exclude` に並べたフォルダ名も、名前が一致すれば外れます（反映は Force Re-index の後です）。

```gitignore
dist/
build/
*.min.js
```

ほかのフォルダも同じ索引に入れたいときは、`.comp/config.json` の `additional_paths` に並べます。v0.12.0 からは、VS Code のワークスペースに複数のフォルダを開いている場合、comP が `.comp/` のある各フォルダの `.comp/config.json` に、ほかのフォルダのパスを `workspace_folders` として書き込みます。そのため、どのフォルダで起動したエージェントからも、全フォルダを検索できます（`comp.indexWorkspaceFolders`、既定 ON）。各フォルダがそれぞれ全体を索引するので、索引にかかる時間は増えます。

同じく v0.12.0 から、ホーム直下の隠しフォルダ（`.` で始まるフォルダ、`AppData`、`Library`。ただし `AppData\Local\Temp` は除く）は、リポジトリの設定ファイルに書かれていても索引しません（`comp.blockHiddenHomeFolders`、既定 ON）。この VS Code の設定が効くのは拡張機能が起動するデーモンで、エージェントが起動するデーモンで解除するには環境変数を使います。

`run_pipeline` が返す量や圧縮の度合いも、`.comp/config.json` で変えられます。

| キー | 意味 |
| --- | --- |
| `default_budget_tokens` | `run_pipeline` が返すトークン量の目安（既定 8000）。収まるように圧縮レベルを自動で選ぶ |
| `compression_rules` | ファイル名のパターン（`*.md` など）ごとの圧縮レベル（0 はそのまま、1 はコメントを除く、2 はシグネチャだけ） |
| `max_nodes` / `on_limit_exceeded` | 索引するシンボル数の上限（既定 200000）と、超えたときの動き（既定 `warn`。`stop` で止める） |

設定の一覧は [設定ガイド（英語）](docs/user/CONFIGURATION.md) にあります。

---

## β版の機能

β版の機能は、会話の記録とルール共有の2つです。どちらも既定は OFF です。

会話の記録は、Claude Code との往復を1回ずつ `.comp/history/` に自動で残し、`session_recall` から読めるようにします。VS Code の設定で `comp.conversationRecording.enabled` を ON にしてから `comP: Setup Agents` で Claude Code を選ぶと、記録用の Stop フックが入ります。いまのところ Claude Code だけが対象です。

ルール共有は、作業に関係する指示ファイルの節を、`run_pipeline` の結果に添えて返す機能です。節は、`CLAUDE.md` や `AGENTS.md` など、ほかのエージェント向けの指示ファイルから取り出します。`check_rule_conflicts` は、矛盾していそうな節の組を一覧にします。読むのは git で管理しているファイルだけです。

使い方は [β版の機能](docs/user/BETA_FEATURES_ja.md) を見てください。

---

## 仕組み

VS Code 拡張が Rust 製のデーモン（`comp-daemon`）を起動し、デーモンがリポジトリを読んで SQLite の索引（`.comp/index.db`）を作ります。AI エージェントはそれぞれ comp-daemon を MCP サーバーとして起動し、同じ索引を共有します。

```text
リポジトリのファイル
  ↓ tree-sitter で関数・クラスを取り出し、呼び出し関係を抽出
.comp/index.db（SQLite のグラフ）
  ↓ BM25・TF-IDF・依存グラフで検索し、予算に合わせて圧縮
MCP サーバー（comp-daemon）
  ↓
Claude Code / Codex / Cursor / Copilot ほか
```

関数・クラスと呼び出し関係を取り出せる言語は、Rust・TypeScript・JavaScript・Python・Go の5つです。HTML からは要素名だけを取り出します。全文検索の対象になる文書は、Markdown・Word・PowerPoint・Excel・PDF と、セッションの記録（JSONL）です。JSON は最上位のキー、XML は要素名、Parquet はスキーマの列名を、名前として索引します。ほかの言語のファイルからは、いまはシンボルを取り出しません。

保存したコードと Markdown などは、そのファイルだけを索引し直します。新しく作ったファイルは、次の起動か Force Re-index で索引に入ります。git でブランチを切り替えると変わるファイルが多すぎるので、comP はブランチの変化を検知して全体を索引し直します（`comp.autoIndex` で止められます）。

`session_recall` は、問いをそのまま含む記録を先に出し、残りを BM25 で並べます。英数字は単語単位、日本語は1〜2文字の組で照合し、新しい記録ほど上に来ます。MCP のツールは13個あり、一覧は [MCP ツール（英語）](docs/user/MCP_TOOLS.md) にあります。

---

## データは手元から出ない

索引・作業の記録・会話の記録は、すべてリポジトリの `.comp/` に保存します。comP のデーモンは外部と通信せず、拡張機能もテレメトリを送りません。エージェントに渡るのは、エージェントが comP のツールで求めた分だけです。その先でエージェントが LLM に何を送るかは、各エージェントの設定によります。

例外が1つあります。VS Code のチャットで `@comp` を使うと、依頼文と、添付したファイルを圧縮した内容が、VS Code で選んでいる言語モデルへ送られます。

---

## 困ったとき

### 索引が進まない → Force Re-index を実行する

ステータスバーで進み具合を確かめ、止まっていたらコマンドパレットから `comP: Force Re-index` を実行します。10万ファイルを超える大きなリポジトリでは、最初の1回に時間がかかります。2回目からは、変わったファイルだけを解析し直します。

### 更新したら MCP のツールが消えた → 起動時に自動で直る

VS Code は拡張機能をバージョンごとのフォルダに置き、更新すると古いフォルダを消します。comP は v0.9.4 から、エージェントの設定に残った古いデーモンのパスを、起動のたびに今のパスへ書き直します。修復の通知が出たら、エージェントを再起動してください。

### Claude Code に「Conflicting scopes」と出る → 残す登録を1つにする

`comp` がプロジェクト用（`.mcp.json`）と全プロジェクト共通（ユーザースコープ）の両方に、別のパスで登録されているという警告です。拡張機能の更新で古くなったパスは、v0.11.10 からは VS Code の起動時に直ります。開発用のバイナリを使っている場合など、2つのパスがどちらも有効で、しかも互いに違うときは、警告が残ります。

プロジェクトのフォルダで `claude mcp list` と `claude mcp get comp` を実行し、残す方を決めてください。プロジェクト用を残すなら `claude mcp remove comp --scope user` を実行します（ほかのプロジェクトからも comp の共通登録が消えます）。共通を残すなら `claude mcp remove comp --scope project` を実行します（このプロジェクトの `.mcp.json` から comp が消えます）。どちらか一方だけを実行して、Claude Code を再起動してください。判断の材料は [MCP 設定ガイド（英語）](docs/user/MCP_SETUP.md#resolve-a-claude-code-scope-conflict) にまとめました。

### AI が comP を使わずにファイルを読む → 指示ファイルにルールを足す

「AI に comP を使ってもらうコツ」のプロンプトを試してください。

### MCP につながらない → Setup Agents をやり直す

`comP: Setup Agents` をもう一度実行し、出力パネルで「comP Setup」を選んでログを確かめてください。ここに書いた方法で解決しないときは、[Issue](https://github.com/tsucky230/comP/issues/new) で教えてください。

---

## 対応エージェント

| エージェント | 状況 |
| --- | --- |
| Claude Code / GitHub Copilot | 対応・動作確認済み |
| Codex（CLI・VS Code 拡張） | 対応 |
| Cursor / Cline / Windsurf / Antigravity / Aider / Continue.dev | 対応 |
| Gemini CLI | 対応（使える条件は、上の Setup Agents の表を見てください） |

MCP 2024-11-05 に準拠したクライアントなら、たいてい動きます。動かなければ [Issue](https://github.com/tsucky230/comP/issues/new) で教えてください。

---

## これまでとこれから

| バージョン | 主な変更 |
| --- | --- |
| v0.1〜v0.8.5 | 索引と MCP サーバー、Office・PDF の索引、圧縮、大きなリポジトリ向けの最適化 |
| v0.8.6〜v0.9 | セッションをまたぐ記録（`session_log` / `session_recall`） |
| v0.10.0 | Setup Agents が、各エージェントの設定ファイルへ直接書き込むように |
| v0.11.0〜v0.11.6 | Codex 対応、日英の切り替え、ブランチ切り替え時の自動再索引、壊れた索引の自動復旧、正確なトークン数 |
| v0.11.7〜v0.11.8 | β版: 会話の記録、ルール共有 |
| v0.11.9〜v0.11.10 | エージェントの設定が壊れにくく（Codex の自動審査、更新後のパスの修復） |
| v0.11.11 | Antigravity 2.0 に対応、デーモンから zstd の脆弱性を除去 |
| v0.12.0 | マルチルートのワークスペース、ホーム直下の隠しフォルダの保護、フォルダの削除・移動・リネームを索引に反映、再索引後も依存エッジと検索結果が正しく保たれるように |
| v0.12.1 | 拡張機能を上げればデーモンも確実に新しくなる（開発用ビルドは明示したときだけ使う、古いエージェント登録の修復、版ずれや古いビルドの警告） |
| v0.12.2 | 内部構造の整理（動作は変わらない）、依存の更新で RustSec の警告2件を解消 |
| v0.12.3 | MAGATAMA の登録と更新を comP に任せられるように（comP が動作を確かめた版に保つ）、索引に形式の版を記録 |
| v1.0 | API の安定化 |

変更の詳細は [CHANGELOG](CHANGELOG.md) にあります。

---

## ライセンスと貢献

MIT ライセンスです（[LICENSE](LICENSE)）。不具合の報告や改善の案は、[Issue](https://github.com/tsucky230/comP/issues/new) か [Discussions](https://github.com/tsucky230/comP/discussions/new) へお寄せください。コードの貢献については、[CONTRIBUTING.md](CONTRIBUTING.md) を見てください。気に入ったら Star や [GitHub Sponsors](https://github.com/sponsors/tsucky230) で応援してもらえると、続ける励みになります。
