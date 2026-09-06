# Changelog

All notable changes to comP are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/) and this project adheres to [Semantic Versioning](https://semver.org/).

---

## [0.11.1] - 2026-09-06

### Added

- **セッションメモリを複数LLM共有可能に拡張**: `.comp/session-memory.json`（単一ファイル）をエージェント別ファイル `.comp/session-memory/<agent>.json` に分割。複数の `comp-daemon` プロセス（Claude Code / Codex などが各自起動）が同じファイルへロックなしで read-modify-write すると起きていたロストアップデート／破損を、エージェントIDでファイルを分けることで構造的に解消（`daemon/src/mcp/mod.rs`、`docs/ARCHITECTURE_ja.md`）
  - `SessionCall` に `agent` フィールドを追加し、どの LLM が書いた記録かを追跡可能に
  - `.comp/history/*.jsonl` への追記を `std::fs::File::lock()` による排他ロック付きの単一 `write_all` に統一（Windows の `LockFileEx` は共有ロック保持ハンドルからの書き込みを拒否するため、追記・コンパクションとも排他ロックに統一）
  - 手動トリガー専用の `compact_history_file`（要約・重複排除用）を追加。排他ロック下で in-place 書き換えし、`.bak` へ事前スナップショットを残す
  - `comp-daemon` に `append-history` CLI サブコマンドを追加（Stop hook が将来これ経由で書き込むことでロックを迂回する書き手を無くす想定。フック側の切り替えは別途）

### Fixed

- **CI の clippy エラーを修正**: `compact_history_file` が本番コード経路から未使用（手動トリガー機構が未実装）のため `#[allow(dead_code)]` を付与。doc comment 内の `+ atomic rename` の `+` が Markdown 箇条書きと解釈され後続行が未インデント継続行エラーになっていたのを文章の書き直しで解消（`daemon/src/mcp/mod.rs`）
- **`daemon/Cargo.toml` のバージョン陳腐化を修正**: v0.9.2 のまま v0.9.3〜v0.11.0 の間更新されておらず、`get_stats` が自己申告する `daemon_version`（`CARGO_PKG_VERSION` 由来）が実際のリリースバージョンとズレていた
- **ARCHITECTURE_ja.md の Codex 記述を修正**: 「将来的な Codex 連携」という記述が、v0.11.0 で Codex 対応が実装・リリース済みになった後も残っていたのを是正

---

## [0.11.0] - 2026-09-05

### Added

- **Codex（OpenAI）を `comP: Setup Agents` の対象に追加**（`src/mcp/AgentSetup.ts`）。Codex の設定は TOML なので、既存の JSON マージ経路では書き込めず、これまで対象外だった
  - 書き込み先は `~/.codex/config.toml`（`$CODEX_HOME` があればそちら）と `<workspace>/.codex/config.toml` の 2 箇所。CLI と VS Code 拡張は同じファイルを読むため、1 回の設定で両方に効く
  - グローバルを先に書く。Codex はグローバル設定を常に読むが、プロジェクト側の `.codex/config.toml` は**信頼済みプロジェクトでしか読まない**ため、確実に効く方を先頭に置く。この制約は再起動手順にも明記する
  - `env` はサブテーブル（`[mcp_servers.comp.env]`）ではなく**インラインテーブル**で書く。エントリが 1 つの連続ブロックになり、再実行時に「comP のブロックだけ差し替える」処理の終端が「次の行頭 `[`」で一意に決まるため
  - パスは TOML の**リテラル文字列**（`'…'`）で書く。基本文字列だと Windows の `C:\Users\…` がエスケープとして解釈されて壊れるため（`yamlQuote` と同じ理由）
  - 他の MCP サーバーの行はバイト単位で保持する。`codex mcp add` が書いた `[mcp_servers.comp.env]` 形式の古いエントリも、サブテーブルごと差し替える
  - 安全に書き換えられない 3 つの形（インラインテーブル `mcp_servers = { … }` / `[mcp_servers]` 配下のドット形 `comp = { … }` / 既存の `[mcp_servers.comp]` が複数行にまたがる値を持つ場合）は書き換えず失敗として報告し、貼り付け用のテーブルを提示する（Aider の `mcp-servers:` と同じ方針。新規依存は追加していない）
    - 判定は `[mcp_servers]` テーブル内に限定する。無関係な `[tui]` の `comp = true` や、名前が `comp` で始まるだけの別サーバー（`[mcp_servers.company]`）で誤検知しないため
    - 複数行の値の検出は括弧の対応で行う。セクションの終端を「次の行頭 `[`」で決めているため、`args` の途中に `["--flag"],` のような行があると途中で切れ、**comP だけでなく Codex の設定ファイル全体が壊れる**。切れた場合は必ず括弧が開いたままになるので、それを検出して書き換えを中止する
  - `AGENTS.md` に comP の利用ルールを追記する（Codex が読む指示ファイル）
- **comP 自身が出すメッセージ（Setup Agents の QuickPick・確認ダイアログ・通知・出力パネルのレポート・手動設定用 Markdown、CLAUDE.md 等に追記する comP 利用ルール、拡張更新後の daemon パス修復通知、chat participant の説明文）を英語圏のユーザー向けに整備**。多言語化の仕組み（vscode-nls / l10n）が無く、これらは全て日本語決め打ちだったため、VS Code の表示言語が日本語以外なら常に日本語が表示されていた
  - VS Code の表示言語（`vscode.env.language`）で英語 / 日本語を切り替える最小限の仕組みを追加（`src/i18n.ts` の `t(en, ja)`、既定は英語）。`AgentSetup.ts` は `vscode` モジュールに依存させたくない（ユニットテストが VS Code 拡張ホスト無しで直接動く設計のため）ので、呼び出し側が解決したロケールを `AgentSetupOptions.locale` として注入する形にした
  - `package.json` の chat participant の説明文だけは実行時コードが届かないため、VS Code 標準の `package.nls.json` / `package.nls.ja.json` で対応
  - `docs/user/MCP_TOOLS.md`（`session_log` / `session_recall` の節が丸ごと日本語だった）と `SECURITY.md`（サポート版数表の一部ラベルのみ日本語だった）も英語に統一
- **ブランチ切り替え時にインデックスを自動でフル再インデックスする**（`src/git/branchWatcher.ts` 新設、`src/extension.ts`）。従来 comP は git ブランチを一切認識しておらず、`FileSystemWatcher` も共有デバウンスタイマー1個を全ファイルで使い回すため、ブランチ切り替え相当の大量ファイル変更はほぼ握りつぶされ、インデックスが新旧どちらのブランチともズレた状態になっていた
  - `.git/HEAD` を3秒間隔でポーリングして変更を検知する（`git` プロセスは起動せず、ファイル読み取りのみ）。detached HEAD ではコミットSHAをそのまま識別子として使う
  - 検知したら既存の `forceReindex`（`clear_index()` + フル `index_workspace(None, ...)`）をそのまま自動発火する。増分更新用の新規RPCは検討したが見送った——全ファイルを「変更あり」として扱うフル再構築でないと、依存関係edgeの再解決が変更ファイルのアウトバウンド分しか行われず、無関係なファイルからの依存edgeが壊れる（詳細は次項）
  - 手動の `comp.forceReindex` コマンドと違い、確認ダイアログは出さない（自動処理なのでユーザー操作を妨げない）。失敗時もトースト通知は出さずログのみ
  - 「ブランチ名の列をインデックスに追加してハッシュ比較で無料スキップする」という当初案は、Gemini のレビューで技術的な誤りと判明したため見送った。`files.path` は `UNIQUE` で1ファイルにつき直近1回分のハッシュしか持てず、ブランチを往復しても毎回差分ファイル分の再パースコストが発生する（キャッシュにはならない）ため、複雑さに見合わないと判断

### Fixed

- **編集保存のたびに `nodes`（シンボル）行が孤児化して溜まり続けるバグを修正**（`daemon/src/graph/mod.rs`、`daemon/src/indexer/mod.rs`）。ブランチ切り替え機能の実装中に発見。`upsert_file` が `INSERT OR REPLACE INTO files (path, ...)` を使っており、`path` の `UNIQUE` 制約に衝突すると SQLite内部では「既存行削除→新規挿入」として処理される。`files.id` は `AUTOINCREMENT` のため、ファイルを保存するたびに `file_id` が新しく採番され、旧IDを参照していた `nodes` 行が永久に孤児として残っていた（FK制約は無効、カスケードなし）。日常の編集保存で常に発生しており、ブランチ機能とは無関係の既存バグ
  - `upsert_file` を `ON CONFLICT(path) DO UPDATE` による真のUPSERTに変更し、`file_id` を安定させた
  - 新規メソッド `clear_file_symbols(file_id)` を追加（`delete_file` の edges→nodes削除カスケードを、`files` 行自体は消さない形で流用）し、`parse_and_extract` で新規nodesを挿入する前に旧nodes/edgesを明示的に削除するようにした
  - **受け入れたトレードオフ**: 変更されたファイルに依存する未変更のファイルからのedgeは、依存元ファイル自体が別の理由で再パースされるまで欠落したままになる。Geminiのレビューでこの副作用を指摘され、「無限に肥大化し続ける現状よりはマシ」と判断した上で修正した
  - 実際に動作するdaemonバイナリに対し、同一ファイルを4回連続で保存してもnode数が増え続けないこと、およびブランチを往復してもnode/file数が両方向で正しく元に戻ることを確認済み

- **`repairStaleConfigs` が TOML 設定を扱えるようにした**（`src/mcp/AgentSetup.ts`）。この関数は VS Code 起動のたびに走り、拡張更新で `<publisher>.<name>-<version>` ディレクトリが消えたあとの陳腐化したデーモンパスを直している。JSON パーサに Codex の TOML を通すと、正常な設定を毎回 `invalid JSON` として報告しながら、本来直すべきパスは永久に直らず、**拡張を更新するたびに Codex から comP が無言で落ちる**状態になる。同じ判定順序（変数参照・相対パス・実在チェック）を TOML 上で再現した専用の修復経路を追加
- テストで `AgentSetupManager` を組み立てる際に `codexHome` を注入するようにした。Codex は `CODEX_HOME` を読むため、注入しないと `CODEX_HOME` を設定している開発者の実設定をテストが書き換えうる（`homeDir` を注入しているのと同じ理由）
- **Codex の TOML マージにあった正規表現の穴を3件修正**（`src/mcp/AgentSetup.ts`）。まっさらな目線でのレビューを Gemini に依頼して見つかった
  - `unsupportedServerShape` がドット表記の前後の空白（`comp . command = "x"` — TOML 仕様上合法）を見落としており、すり抜けると後続の `[mcp_servers.comp]` と重複キーになって Codex がファイル全体をパース拒否する
  - `refreshWorkspaceRootToml` の `COMP_WORKSPACE_ROOT` 正規表現に行頭アンカーが無く、セクション内のコメント本文（例: `# old: COMP_WORKSPACE_ROOT = '/old/path'`）に左端優先でマッチし、本物の値ではなくコメントの方を書き換えてしまっていた。コメント本文だけを同じ文字数・改行を保ったまま空白化する `maskComments` を追加して対処
  - `decodeTomlString` が `tomlQuote` の `\uXXXX` エスケープを復号できておらず、制御文字を含むパスで書き込みと読み取りが非対称になり、`repairStaleConfigs` が起動のたびに「壊れている」と誤判定し続けていた

---

## [0.10.0] - 2026-09-01

### Changed

- **`comP: Setup Agents` からコピペ作業を廃止**。従来は Cursor / Cline / Windsurf / Continue の設定を `.comp/config/` 配下に生成しており、そこは**どのツールも読まない場所**だったため、生成された文字列を手で正しい場所へ貼り付けないと設定が一切効かなかった。各エージェントが実際に読む設定ファイルへ直接マージ書き込みするよう変更（`src/mcp/AgentSetup.ts`、`src/ui/commands.ts`）
  - Cursor → `.cursor/mcp.json` と `~/.cursor/mcp.json`
  - Cline → VS Code globalStorage 配下の `saoudrizwan.claude-dev/settings/cline_mcp_settings.json`（拡張自身の globalStorage の兄弟ディレクトリとして解決するため、Insiders / VSCodium と OS 差を吸収する）
  - Windsurf → `~/.codeium/windsurf/mcp_config.json`
  - Continue → `.continue/mcpServers/comp.yaml` と `~/.continue/mcpServers/comp.yaml`（Python 辞書ではなく、現行仕様の YAML ブロックファイル形式）
  - グローバル設定には `COMP_WORKSPACE_ROOT` を書き込まない。1プロジェクトを指す値を全プロジェクト共有の設定に焼き込むと、他プロジェクトを開いたときにこのプロジェクトをインデックスしてしまうため。デーモンは未設定時に作業ディレクトリへフォールバックする
  - 既存ファイルは書き換え前に `<file>.bak` へ退避する。バックアップに失敗した場合は本体を書き換えない
  - 書き込みは同一ディレクトリの一時ファイル経由の rename（アトミック）。中断しても設定が半端な状態にならない
  - 解析できない設定ファイルは上書きせず失敗として報告する。ユーザーが設定した他の MCP サーバーを黙って捨てないため
- **`run_pipeline` 優先ルールの `CLAUDE.md` 追記を自動化**。従来は「このプロンプトを貼り付けてください」という指示を出すだけだった。Session Continuity 節と同じ仕組みで自動追記し、マーカーで重複を防ぐ
- **エージェント選択を複数選択に変更**。インストール済みのエージェントを検出して初期選択にする。検出は初期選択のみに使い、未検出でも一覧から外さない
- **設定結果を出力パネル「comP Setup」に表示**。書き込んだファイル・スコープ・バックアップ先・失敗理由と、エージェントごとの再起動手順を出す。ホームディレクトリや VS Code の globalStorage へも書き込むため、何がどこに変わったかを確認する手段が他にないため
- コピペ用の文字列は**書き込みに失敗した項目についてのみ**提示する（権限エラーや JSON 破損で自動化が止まったときにユーザーが詰まないようにするため）

### Fixed

- **`claude mcp add` の生成コマンドが 2 箇所間違っていた**問題を修正（`src/mcp/AgentSetup.ts`）
  - `--scope user` が無く、説明文は「ユーザーレベルで登録」と書いてあるのに実際は `local` スコープに登録されていた
  - `--` セパレータが無く、デーモンのパスが CLI の位置引数として解釈されていた
- Claude Code のユーザースコープ登録を拡張から実行できるようにした。stdin を閉じ、15秒のタイムアウトと `windowsHide` を付けて起動する（CLI が初回同意や再ログインのプロンプトで待ち状態に入っても拡張ホストがハングしないようにするため）
- **Windows で `claude` CLI をまったく起動できなかった問題を修正**。npm 版の Claude Code は `claude.cmd` であり、`child_process.execFile` は Windows で `.cmd` を解決できず ENOENT になる（Node の CVE-2024-27980 対応が入ったバージョンでは EINVAL）。ネイティブの `.exe` を併せて入れていない環境では CLI 連携が常に失敗していた。`spawn` + `shell`（Windows のみ）へ変更
  - `shell: true` は引数をエスケープせず連結する（Node DEP0190）ため、コマンドラインは自前でクォートして組み立てる。`C:\Users\John Smith\` のように空白を含むパスが途中で切れないようにするため
  - daemon のパスに cmd.exe のメタ文字（`" & | < > ^ %`）が含まれる場合は自動登録を見送り、コマンド文字列の提示に切り替える
- `repairStaleConfigs()` の対象に新しい設定パスを追加。旧 `.comp/config/*.json` も後方互換のため残す

### Docs

- `docs/user/MCP_SETUP.md` を全面改稿。エージェントごとの再起動・ウィンドウリロード手順と確認方法を追加

---

## [0.9.4] - 2026-07-30

### Fixed

- **拡張のアップグレード後に MCP 設定が壊れる問題を修正**。VS Code は拡張を `<publisher>.<name>-<version>` に展開し、更新時に旧ディレクトリを削除する。`comP: Setup Agents` が生成した設定には `comp-daemon` の絶対パスが焼き込まれるため、バージョンが上がるたびに存在しない実行ファイルを指したまま残り、MCP クライアントが起動に失敗していた（拡張本体は自分のバイナリを実行時に解決するため動き続け、「サイドバーは動くが MCP ツールだけ出ない」症状になる）
  - `AgentSetupManager.repairStaleConfigs()` を追加し、`activate()` のたびに `.vscode/mcp.json` / `.mcp.json` / `.comp/config/{cursor,cline,windsurf}_config.json` / Antigravity のグローバル設定を検査。`command` が実在しない絶対パスなら現在の daemon パスへ書き戻す（`src/mcp/AgentSetup.ts`、`src/extension.ts`）
  - 変数参照（`${...}`）を含む値・相対パス・実在するパスは書き換えない。置換先バイナリが見つからない場合も書き換えず、壊れたパスを別の壊れたパスに差し替えない
  - グローバル設定（`~/.gemini/antigravity-ide/mcp_config.json`）にはバンドル版バイナリのみを書き込む。全プロジェクト共有の設定に、たまたま開いていたワークスペースの cargo ビルドを書き込まないため
  - バージョン変化ではなく毎回検査する。`globalState` は VS Code 全体で共有されるため、バージョンで判定すると最初に開いたワークスペースだけ修復され、残りが取り残される
- **`COMP_WORKSPACE_ROOT` の陳腐化も同時に修復**。ワークスペース配下の設定に限り、現在開いているワークスペースルートと不一致なら書き戻す。プロジェクトを移動した場合やマシンをまたいだ場合に、デーモンが存在しないディレクトリをインデックスしようとする問題を解消。グローバル設定の値は他プロジェクトを指している可能性があるため触らない
  - なお `${workspaceFolder}` への置き換えは採用していない。VS Code 公式ドキュメントは `mcp.json` での変数展開に対応と記載しているが、[microsoft/vscode#297794](https://github.com/microsoft/vscode/issues/297794)（未解決）で `Variable workspaceFolder can not be resolved` が報告されており、`env` フィールドへの適用も明記されていないため

---

## [0.9.3] - 2026-07-16

### Fixed

- **対話履歴の BM25 インデックス化（0.8.6）が実際には機能していなかった問題を修正**（MAGATAMA 側からの指摘）。3 つの独立した欠陥が重なっていた：
  1. `.comp/history/` のカーブアウトが `should_skip_relative_path`（単一ファイル更新ガード）にしか無く、バッチ走査 `walk()` は隠しディレクトリ除外で `.comp/` に降りないため、初回・全体インデックスで履歴が files テーブルに登録されない → `walk()` に `.comp/history/*.jsonl` の補完走査を追加（`daemon/src/indexer/walker.rs`）
  2. `session_log` が `index_file` で登録した履歴行が、次回デーモン起動時の全体インデックスで walk から見えず「削除ファイル」と誤判定され purge されていた（自己消滅） → 1. の補完走査で found 扱いになり解消
  3. BM25 の対象言語フィルタ（`markdown | docx | pptx | xlsx | pdf`）に `jsonl` が無く、仮に登録されても全文検索対象外だった（`parse_jsonl` は先頭行のキー名しかシンボル化しないため LIKE でも本文にヒットしない） → フィルタに `jsonl` を追加（`daemon/src/mcp/mod.rs`）
- 回帰テスト追加: walker のバッチ走査取り込み・削除誤判定（`walker.rs`）、session_log → run_pipeline で履歴 JSONL が pivot_files に載る E2E（`mcp/mod.rs`）

### Notes

- comP 自身に「インデックス → 履歴変更 → 再検知」の自己ループは無い: MCP 呼び出しの自動記録は `session-memory.json` 行き（履歴 JSONL には書かない）、VS Code 拡張のウォッチャは `.` 始まりセグメントを除外済みかつ監視パターンに `jsonl` が含まれない。MAGATAMA の patrol には `.comp/` 除外の予防措置を推奨（先方指摘の通り）

---

## [Unreleased] - 0.9.2

### Added

- **`related_files` を実装**: `run_pipeline` レスポンスの `related_files`（長らく空配列の TODO だった）を実装。ピボットファイルから依存エッジ 1 ホップで繋がる他ファイルを、接続エッジ数順に最大 10 件返す（`{"path", "edge_count"}` 形式）。`GraphDB::get_related_files` を追加し、双方向エッジを集計（`daemon/src/graph/mod.rs`）
- **デーモンのバージョン自己申告**: `getStats` レスポンスに `daemon_version` を追加。MCP `initialize` の `serverInfo.version` もハードコード "0.1.0" から `CARGO_PKG_VERSION` に修正。アップグレード後に旧バイナリが稼働し続けていることをクライアント側で検知可能に
- **`comp.generateContext` コマンドを本実装**: タスク入力 → デーモン `run_pipeline` 呼び出し → 結果 JSON を新規エディタに表示（従来は入力を受けて何もしない殻実装だった）
- **`comp.showImpactGraph` コマンドを本実装**: カーソル下のシンボルを `get_symbol` で照会し、依存・被依存情報の Markdown を新規エディタに表示（従来は位置情報のメッセージ表示のみ）

### Changed

- **`session_recall` の出力を要約化**: Symbols・Files の列挙を各エントリ先頭 5 件までにキャップし、超過分は `… (+N more)` 表記に変更。run_pipeline 自動記録エントリが数十件のシンボル・ファイルを含み、recall 出力自体がトークンを浪費していた問題を解消
- **トークン推定を実ファイルサイズベースに刷新**: 従来の「シンボル数 × 50」ヒューリスティックから、インデックス済み `char_count / 4`（業界標準近似）ベースに変更。シンボル数が同じでも実サイズが 10 倍違うファイルを正しく区別できるように。未インデックス時のみシンボル数フォールバック。圧縮レベル係数（level 1: ×0.70、level 2: ×0.25）は据え置き

### Fixed

- **`session_recall` が Stop hook 記録を全件取りこぼすバグ**: `history-record.sh` が書く JSONL は `{"timestamp","request","outcome"}` 形式だが、`SessionCall` のデシリアライズが `query` フィールド必須・`symbols`/`files`/`tokens`/`stale` にデフォルトなしのため、フック書き込み行が全てパース失敗で黙って捨てられていた。`#[serde(alias = "request")]` と `#[serde(default)]` を追加して両形式を受理（`daemon/src/mcp/mod.rs`）。フック形式行の回帰テストを追加
- **README_ja の Office 対応記述の矛盾を修正**: 「サポート予定（v1.0～）」とあった Office ドキュメント対応は v0.2〜v0.3 でリリース済みのため記述を実態に合わせた

### Notes

- Windows では実行中の `comp-daemon.exe` がロックされ `cargo build --release` が os error 5 で失敗する。ビルド前にデーモン停止が必要（今回 v0.8.6 の修正が反映されず旧バイナリが稼働し続けていた根本原因）
- 残 TODO: `search/mod.rs` の tiktoken-rs による厳密トークン計測（依存追加が必要なため見送り）、`parser.rs` の AST 完全走査による import 抽出

---

## [0.9.1] - 2026-06-27

### Added

- **Markdown 圧縮対応**: Skeleton モード（level 2）で見出し・リスト・コードブロックを保持したまま Markdown を圧縮（`daemon/src/mcp/compress.rs`）
- **AST 圧縮の言語拡大**: 既存の Rust/TypeScript/JavaScript/Python/Go/HTML に加え C、C++、Java に対応
- **FAQ セクション追加**: README_ja に「LLM が直接ファイルを読み続ける」問題への 3 つの対処法を記載

### Fixed

- セットアップガイドのバッククォート整形崩れを修正（`src/mcp/AgentSetup.ts`、`copilot-instructions.md`）

---

## [0.9.0] - 2026-06-27

### Added

- **README マーケティング刷新**: トークン削減率を 60-80% から **94%** に更新。入力トークンの「劇的削減」を強調。
- **セッション記憶の価値主張**: LLM 標準機能では実現不可能な「全セッション横断の永続チャット履歴」を README で強調。v0.9 のロードマップ説明を拡充。
- **セキュリティ・プライバシーセクション追加**: 「完全ローカル実行」「クラウドに何も上げない」「企業対応」などを明記し、データ保護の安心感をアピール。
- **MAGATAMA 連携の明記**: comP インデックスを MAGATAMA が自動的に活用することで、1/500 トークンでの深い分析が可能になることを記載。

### Documentation

- **README.md 全面改善**: オープニングを日本語と英語で再構成。ヒーロー画像から「🧠 知的記憶」「94% 削減」「セッション横断検索」を前面に。
- **README_ja.md 同期**: 日本語版も同じ構成で改善。マーケティング・セキュリティ・v0.9 説明を統一。
- **GitHub Pages 向け（準備）**: ランディングページの訴求力強化へ向けた基盤整備。

---

## [0.8.6] - 2026-06-27

### Added

- **`session_log` MCPツール**: ユーザーの依頼と対応結果を `.comp/history/log-YYYY-MM.jsonl` に永続記録するツールを追加（`daemon/src/mcp/mod.rs`）。書き込み直後に `index_file` を呼び BM25 インデックスへ即時反映するため、以降の `run_pipeline` が過去のやりとりを自然にランク付け候補として扱う。
- **`session_recall` 全セッション横断化**: 従来は現行セッション内のみを返していた `session_recall` を、デーモン再起動をまたいだ全セッション横断検索に拡張。`.comp/session-memory.json` の全セッションエントリと `.comp/history/*.jsonl` のログを統合し、日時付き・新しい順で返却。`query` フィルタは `request`・`outcome` 両フィールドに対して部分一致検索。`limit` パラメータ追加（デフォルト 20）。
- **対話履歴の BM25 インデックス化**: `walker.rs` に `.comp/history/` ディレクトリのカーブアウトを追加し、隠しディレクトリスキップルールから除外。`run_pipeline` の全文検索が `.comp/history/*.jsonl` の内容を自動的に候補として扱う。
- **Stop hook 自動記録**: `.claude/hooks/history-record.sh` を追加。Stop hook 発火時にトランスクリプトを解析し、直近のユーザー依頼・アシスタント応答を `.comp/history/` へ自動追記。LLM の自発的記録に依存せず確実に履歴を蓄積する。
- **UserPromptSubmit hook 自動注入**: `context-inject.sh` を拡張。プロンプト送信ごとに `.comp/history/` から直近 5 件を読み込み `<system-reminder>` として自動注入し、セッション切れ後も前回の作業文脈が自動復元される。

### Changed

- `SessionCall` 構造体に `outcome: Option<String>` を追加（`#[serde(default)]` で後方互換）。`session_recall` レスポンスにアウトカム行を追加。

---

## [0.8.5] - 2026-06-26

### Security

- **lopdf 0.32 → 0.42**: スタックオーバーフロー脆弱性 (RUSTSEC-2026-0187, High 7.5) を修正。深くネストされた PDF オブジェクトによる DoS が可能だった問題。

### Tests

- TypeScript ユニットテストカバレッジを 44.56% → 57.17% に改善（`src/ui` は 80% 達成）
- `DaemonManager`, `CodeLens`, `SidebarPanel`, `commands` に 29 テスト追加

---

## [0.8.4] - 2026-06-26

### Fixed

- **`.jsonl` ファイルがインデックスされないバグ**: `walker.rs` の `detect_language()` に `"jsonl"` エントリが欠落しており、`.jsonl` ファイルが `"unknown"` に分類されてシンボル0件でインデックスされていた問題を修正。`doc_parser.rs::parse_jsonl()` および `mod.rs` 側は実装済みだった。

---

## [0.8.3] - 2026-06-21

### Fixed

- **MCP 通知への不正応答で接続が切れるバグ**: JSON-RPC 通知（`id` を持たないメッセージ）を通常リクエストとして処理し、Claude Code がハンドシェイク直後に送る `notifications/initialized` に対して `id: null` のエラー応答を返していた。厳格な MCP クライアント（Claude Code）がこれを受けてトランスポートを破棄し `-32000: Connection closed` となる問題を修正。通知には応答を返さないようガードを追加（`daemon/src/mcp/mod.rs`）
- **依存エッジがほぼ生成されないバグ**: 依存解析が実質スタブ状態で、48,762 ノードに対しエッジが 31 本しか作られず、影響グラフ（CodeLens の dependents / `get_impact_graph`）が機能していなかった問題を修正。
  - Python / Go の依存抽出が常に空を返していたのを実装（`from ... import`、関数・メソッド呼び出し）
  - import の `from` が常に `"module"` 固定でノードに一致しなかった問題を、呼び出し元＝行直前の最近接シンボルで解決する方式に変更
  - クロスファイル解決を有効化（同一ファイル → グローバル索引の順で解決。同名 export が複数ある曖昧ケースはスキップし誤エッジを防止）
  - Rust/TypeScript に一般呼び出し・`new` 型参照の抽出を追加
  - 同一プロジェクトでの実測: エッジ 31 → 636

### Changed

- インデックス処理を 2 パス化（全ノード登録後にエッジを解決）。`GraphDB::get_global_symbol_index` / `clear_file_edges` を追加し、再インデックス時の stale エッジを排除
- Rust テスト 6 件追加（Python/Go 抽出・クロスファイル解決・曖昧/自己参照スキップ・2 ファイル間エッジ生成の統合テスト）

---

## [0.8.1] - 2026-06-13

### Fixed

- **`.venv` 等の大量混入バグ**: ディレクトリ走査を `walkdir` から `ignore` クレートへ置き換え。`filter_entry` で除外ディレクトリのサブツリーごと枝刈りするようにし、`.venv/Lib/site-packages/...` が再帰的にインデックスされ forceReindex がタイムアウトする問題を解消（別リポジトリで約 4,400 → 123 ファイルに減少）
- **部分一致による誤除外**: `path.contains("build")` 方式をパスセグメント完全一致に変更。`src/builder.rs` や `targets.rs` が誤って除外される問題を修正
- **FileSystemWatcher / `index_file` の除外漏れ**: daemon 側に単一ファイル用スキップガード、拡張側 watcher に早期 return を追加し、`.venv` 配下変更による不要な再インデックスを抑止

### Added

- **`.comp/ignore`**: gitignore 構文の補助除外ファイルに対応（`ignore` クレートの `add_ignore`）
- **`comp.exclude` 設定**: VS Code 設定で除外ディレクトリ名を指定可能に。`.comp/config.json` の `exclude` 配列へ同期され、daemon の除外リストへ反映
- **自動制限**: 5 MiB 超のファイルをスキップ、2,000 ファイル超で上位ディレクトリ内訳付きの警告ログを出力

### Changed

- **`workspace_root` を daemon state へ一元化**: 各ハンドラが `COMP_WORKSPACE_ROOT` 環境変数を都度読む方式から、起動時に `AppState` が保持する値を使う方式に変更し、起動時 root との乖離を排除
- **ドキュメント同期**: CONFIGURATION / GETTING_STARTED に Python プロジェクト向け除外手順・`comp.exclude`・自動制限を追記。`.gitignore` を尊重する旨の誤記を実態（`.comp/ignore`）に修正

---

## [0.8.0] - 2026-06-10

### Added

- **git diff スコアブースト**: `run_pipeline` が `git diff HEAD` の変更ファイルを候補の先頭に昇格。`pivot_file` エントリに `git_diff: true` マーカーと `coverage.git_diff_boosted` カウントを追加。git 未使用環境ではサイレントにフォールバック
- **サイドバー Re-index ボタン**: SidebarPanel に ↺ ボタンを追加。既存の `comp.forceReindex` コマンドに接続。デーモン停止中は無効化

### Changed

- Rust テスト 6 件（git-diff エッジケース全網羅）、TS テスト 2 件（Re-index ボタン表示・メッセージディスパッチ）を追加

---

## [0.7.1] - 2026-06-08

### Changed

- **ドキュメント全同期**: Aider 対応・Export Debug Log・compression_rules・run_pipeline レスポンスフィールド・MCP_SETUP Aider セクションを v0.7.0 に合わせ更新
- **run_pipeline 優先ルール強化**: ツール説明に「常に最初に呼ぶ」「Read/Bash/grep 代替禁止」を明記
- **Setup Agents 出力にエージェント別憲法ガイドを追加**: CLAUDE.md / .cursor/rules / .clinerules 等へ自動追記するプロンプトを生成
- `.mcp.json` を `.gitignore` に追加（マシン固有の絶対パスを含むため）

---

## [0.7.0] - 2026-06-07

### Added

- **拡張子別圧縮ルール(#7)**: `.comp/config.json`に`compression_rules`フィールドを追加。`{ "*.md": 0, "*.rs": 2 }`のようなパターンでファイルごとの圧縮レベルを指定可能。`run_pipeline`レスポンスに`compression_rules_applied`フラグを追加
- **Aiderエージェント対応(#8)**: `comp setupAgents`でAiderを選択可能に。`.aider.conf.yml`に`mcp-servers`ブロックを生成。既存の設定ファイルがある場合はマージ警告を追加
- **デバッグログエクスポート(#10)**: `comP: Export Debug Log`コマンドを追加。`session-memory.json`をエディターで開くか、任意のパスにエクスポート可能

### Fixed

- **トークン可視化の状態不整合 (#5)**:
  - `startDaemonStack()`完了時にStatusBarが`efficiency`なしで"Ready"を表示していた問題を修正。起動直後に`getStats`を呼んでトークン統計を即時反映
  - `forceReindex`後の`updateStats`で`efficiency`が渡されず表示がリセットされていたバグを修正

### Why

v0.7.xクイックウィン群。圧縮ルールによりMarkdownドキュメントを常に非圧縮（level 0）に保ちつつコードを強圧縮するなど、プロジェクト固有の最適化が可能になる。

---

## [0.5.4] - 2026-06-05

### Added

- **Multi-agent MCP setup**: `comp setupAgents` command now generates configuration templates for Antigravity, GitHub Copilot, Cursor, Cline, and Continue.dev in addition to Claude Code
- **Antigravity IDE support**: comP now officially supports Antigravity with automatic MCP server registration via `mcp-servers-manifest.json`
- **MCP Setup Guide**: New [docs/user/MCP_SETUP.md](docs/user/MCP_SETUP.md) with per-agent configuration instructions including troubleshooting
- **Multi-agent configuration docs**: Updated [docs/user/CONFIGURATION.md](docs/user/CONFIGURATION.md) with multi-workspace setup and simultaneous agent usage guidelines
- **MCP server development guide**: Added [CONTRIBUTING.md](CONTRIBUTING.md) section for MCP tool development with testing checklist and examples

### Changed

- **GitHub Copilot instructions**: Added [src/templates/copilot-instructions.md](src/templates/copilot-instructions.md) with best practices for using comP with Copilot Chat
- **README.md**: Added reference to MCP_SETUP.md for detailed multi-agent configuration

### Why

Feedback from Antigravity users showed that comP's MCP server wasn't discoverable without explicit setup documentation. This release makes comP a "first-class citizen" MCP server that agents can detect and register automatically, with clear setup instructions for all major AI platforms.

---

## [0.5.3] - 2026-06-04

### Added

- **Markdown heading signature**: `parse_markdown()` now captures the first body line after each heading as `signature`, improving BM25 search precision and `get_file_summary` previews
- **`run_pipeline` coverage field**: Response now includes `coverage.indexed_doc_files`, `coverage.bm25_hits`, and `coverage.pivot_file_types` — gives agents a verifiable signal that Markdown and document files were searched, preventing false "not indexed" assumptions

### Changed

- **`run_pipeline` tool description**: Updated to explicitly include documentation tasks (writing/editing Markdown, updating docs), preventing agents from skipping the tool for non-code tasks

---

## [0.4.0] - 2026-06-03

### Added

- **`run_pipeline` content mode**: New `include_content` (bool) and `compression_level` (0/1/2) params — returns compressed file content directly in pivot_file entries, eliminating a second round-trip
- **`get_git_diff_context` tool**: New MCP tool — runs `git diff --name-only <base_ref>` and maps changed files to indexed symbols; useful for PR review and change impact analysis
- **Enhanced `get_project_overview`**: Now includes language distribution (files per language) and top-10 files by symbol count before the full file table

---

## [0.3.0] - 2026-06-02

### Added

- **PDF support**: lopdf-based text extraction; PDFs indexed as page-level symbols with BM25 full-text search
- **Advanced impact analysis**: `max_depth` parameter for `get_impact_graph` — limits BFS hop count (0 = unlimited)
- **TF-IDF search wired to `run_pipeline`**: After indexing, `SearchEngine.build_index()` is called; `run_pipeline` now merges LIKE + TF-IDF results for better recall
- **Multi-path support**: `additional_paths` array in `.comp/config.json` — index monorepo sub-directories or sibling projects into the same graph DB
- **AST-based compression** (`get_symbol` `compression_level` param):
  - Level 0: full source (no-op)
  - Level 1 (compact): comments and blank lines removed via tree-sitter
  - Level 2 (skeleton): function/class bodies replaced with `{ ... }`
- **Slim Markdown output for `get_symbol`**: more concise format with one-liner dependency summaries

---

## [0.2.1] - 2026-05-31

### Fixed

- CI release workflow: fixed vsce publish option and suppressed Node 20 deprecation warnings
- Resolved invalid secrets reference in release.yml

---

## [0.2.0] - 2026-05-28

### Added

- Word (.docx), PowerPoint (.pptx), and Excel (.xlsx) automatic indexing
- BM25 full-text search for Markdown and Office documents
- New MCP tools: `get_symbol`, `get_dependencies`, `get_file_summary`, `get_project_overview`, `session_recall`
- Token compression roadmap preparation

---

## [0.1.0] - 2026-05-21

### Added

#### Core Daemon Features (Phases 3-7)

- **GraphDB Module**: SQLite-based code graph database
  - Persistent storage of symbols and dependencies
  - SHA256-based file change detection
  - Incremental indexing support
  - Full schema with performance indexes

- **Code Parser Integration**: Language-aware symbol extraction
  - tree-sitter support for 30+ languages
  - JSON/XML/Markdown document parsing
  - Dependency analysis with regex patterns
  - Symbol-to-node-ID mapping

- **Search Engine**: TF-IDF semantic search
  - Tokenization for camelCase, snake_case, SCREAMING_CASE
  - Cosine similarity ranking (0.0-1.0)
  - BFS-based impact graph traversal
  - Fuzzy symbol matching

- **MCP Tools** (JSON-RPC 2.0):
  1. `run_pipeline`: Full context generation with token counting
  2. `get_context`: Query-based semantic search
  3. `get_impact_graph`: Change impact analysis
  4. `list_indexed_files`: Index statistics
  5. `get_token_usage`: Token consumption metrics

- **AppState Integration**: Unified state management
  - GraphDB + SearchEngine initialization
  - Mutex-protected concurrent access
  - Automatic workspace detection

- **Testing**: 66 unit tests + 4 integration tests (97% success rate)

#### Build & Release

- **SBOM.json**: CycloneDX 1.4 format dependency tracking
- **GitHub Release Workflow**: Automated VSIX + SBOM upload on tag push
- **Release Notes**: Feature summary with installation instructions

### Changed

- N/A (initial release)

### Fixed

- Fixed regex patterns with proper quote escaping
- Fixed SQLite DEFAULT clause for timestamp columns
- Resolved async/await issues in test suite
- Compilation warnings addressed

### Deprecated

- N/A

### Removed

- N/A

### Security

- All 12 Rust dependencies use MIT or Apache 2.0 licenses
- No external network connectivity required
- SBOM.json provides full license and vulnerability tracking
- Data stays local in .comp/index.db within workspace

---

## Versioning Policy

### Major.Minor.Patch (MAJOR.MINOR.PATCH)

- **MAJOR**: Breaking changes to API or MCP tools
- **MINOR**: New features that are backward compatible
- **PATCH**: Bug fixes and maintenance

Example: `0.1.0` → `0.2.0` (feature) → `0.2.1` (bugfix)

---

## Future Versions (Planned)

### v0.2.0

- Word (.docx) document support
- Advanced impact analysis with transitive dependencies
- Custom context generation templates

### v0.3.0

- Embedding-based semantic search
- Cross-repository indexing
- Real-time symbol navigation

### v1.0.0

- Stable API guarantee
- Extended agent support
- Community integrations

---

## How to Report Changes

When submitting a PR:

1. Update `CHANGELOG.md` under **[Unreleased]** section
2. Choose the appropriate section: Added, Changed, Fixed, Deprecated, Removed, Security
3. Use clear, concise language describing the change
4. Reference issue number (e.g., "Resolves #123")

Example:

```markdown
### Added

- New `get_symbols` MCP tool for listing all exported symbols (Resolves #45)
- Support for Kotlin language via tree-sitter-kotlin

### Fixed

- MCP connection timeout on large repositories (Fixes #38)
```

---

## Release Checklist

Before releasing a new version:

1. [ ] All tests pass locally
2. [ ] Update version in `package.json`
3. [ ] Update CHANGELOG.md with release date and version
4. [ ] Verify all features/fixes are documented
5. [ ] Run Markdown linting: `npm run lint:md:fix`
6. [ ] Commit and create annotated tag: `git tag -a vX.Y.Z -m "Release vX.Y.Z"`
7. [ ] Push commits and tag: `git push origin main && git push origin vX.Y.Z`
8. [ ] Verify GitHub Actions completed successfully
9. [ ] Review GitHub Release with VSIX and SBOM artifacts
