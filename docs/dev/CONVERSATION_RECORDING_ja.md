# 会話の自動記録は、既定OFFのβ版として出し、実機確認とエージェント追加を経て正式版にする

comP は、AI エージェントとの会話を1往復ずつ `.comp/history/` に残し、次のセッションで `session_recall` から読めるようにする。この自動記録を、当面は設定で明示的にONにした人だけが使えるβ版として提供する。このメモの目的は、β版の範囲と仕組みを共有し、正式版にする条件と、次に対応するエージェントの順番について判断をもらうことにある。

- 状態: β版（v0.11.7 で提供、2026-10-05）
- 対象読者: comP の開発者・保守者
- 利用者向けの手引き: [docs/user/BETA_FEATURES_ja.md](../user/BETA_FEATURES_ja.md)（英語版 [BETA_FEATURES.md](../user/BETA_FEATURES.md)）
- 上位の企画: [MULTI_AGENT_TRACE_ja.md](./MULTI_AGENT_TRACE_ja.md)（複数エージェントの追跡の T1）

## 背景: 配布版の comP では、comP リポジトリ以外の会話がまったく記録されていなかった

記録用の Stop フック `history-record.sh` は comP リポジトリの開発専用で、本体を `<作業フォルダ>/daemon/target/` から探していた。初期設定（comP: Setup Agents）は他のプロジェクトにフックを入れていなかったので、配布版では comP リポジトリ以外で会話が残らない。さらに、このフックは transcript の本文を最上位の `content` から読んでいた。実際の形式は `message.content` なので、comP リポジトリでも何も記録されていなかった（2026-10-05 に実物の transcript で確認）。

Claude Code 以外のエージェントは、もっと状況が悪い。外から往復を拾う仕組みがなく、LLM が自分から `session_log` を呼ばない限り何も残らない。それなのに、初期設定が書く案内文は `session_log` に触れていなかった。

## β版で提供するもの

### Claude Code の往復を、本体の `record-turn` が記録する

`comp-daemon record-turn [workspace_root]` は、Claude Code の Stop フックから呼ばれる。受け取った JSON から transcript を読み、最後の依頼と応答を取り出して、ロック付きで `.comp/history/log-YYYY-MM.jsonl` に追記する。本体だけで動くので bash も node も要らない。追記に失敗したときは spill ファイルに退避して終了コード 1 を返す。終了コード 2 は返さない。Stop フックで 2 を返すと、Claude Code は停止を妨げられたと解釈するからだ。実装は `daemon/src/mcp/record_turn.rs` にある。

初期設定で Claude Code を選ぶと、このフックが `.claude/settings.local.json` に入る。本体の絶対パスを含みマシンごとに違うので、リポジトリで共有される `settings.json` には書かない。拡張機能の更新やプロジェクトの移動で古くなったパスは、起動時の `repairStaleConfigs` が書き直す。

### 既定はOFFで、ONにした人だけが使える

スイッチは VS Code の設定画面（comP の項目）にある。

| 設定 | 既定 | 意味 |
|---|---|---|
| `comp.conversationRecording.enabled` | OFF | 会話の自動記録（β版）全体のスイッチ |
| `comp.conversationRecording.claudeCode` | ON | Claude Code の記録。全体がONのときだけ効く |

どちらも適用範囲は `resource` なので、ユーザー設定で全プロジェクトをONにすることも、ワークスペースごとに切り替えることもできる。拡張機能は、起動時と設定を変えたときに、この値を `.comp/config.json` の `conversationRecording` に書き写す。`record-turn` は transcript を読む前にこの値を見る。`enabled` が JSON の `true` でない限り（ファイルがない、キーがない、文字列の `"true"`、JSON が壊れている、なども含めて）何も記録しない。本人がONにしていないのに会話を記録し始める、という方向には倒さない設計にしている。

OFFに戻すと記録はすぐ止まる。入れたフックはそのまま残り、呼ばれても何もしない。フックを外したいときは、コマンド「comP: 会話記録フックを外す」（`comp.removeHistoryHooks`）を使う。外すのは `record-turn` のフックだけで、同じファイルにある他のフックや設定は残し、実行前に `.bak` のバックアップを取る。

利用者の操作は次のとおり。

1. 設定で `comp.conversationRecording.enabled` をONにする。
2. 「Setup Agents を実行」という通知が出るので、押して Claude Code を選ぶ（通知は、OFFからONに切り替えたときにだけ出る）。
3. Claude Code を再起動する。以後、各往復が `.comp/history/` に残る。

### Claude Code 以外のエージェントには、`session_log` を呼ぶよう案内する

初期設定が CLAUDE.md・AGENTS.md・GEMINI.md などに書く案内文に、「comP Session Logging」の節を足した。タスクを1つ終えたら `session_log` に `request` と `outcome` を渡して記録するよう求める内容だ。ただし `record-turn` のフックがある Claude Code では、二重に記録しないよう呼ばせない。この案内は LLM の自主性に頼るので、確実ではない。自動記録ができるエージェントから順に、次の節の仕組みへ置き換えていく。

## 他のエージェントにもターン終了のフックがあり、エージェントごとに対応すれば自動記録できる

2026-10-05 に各公式文書で確認した。表の「依頼」「応答」は、ターンの終わりに呼ばれるフックへ本文が直接渡されるかどうかを示す。

| エージェント | ターン終了のフック | 依頼 | 応答 | 設定の場所（プロジェクト） |
|---|---|---|---|---|
| Gemini CLI | `AfterAgent` | `prompt` | `prompt_response` | `.gemini/settings.json` |
| Codex | `Stop` | なし（`UserPromptSubmit` の `prompt`） | `last_assistant_message` | `.codex/hooks.json` |
| Cursor | `stop`・`afterAgentResponse` | なし（`beforeSubmitPrompt` の `prompt`） | `afterAgentResponse` の `text` | `.cursor/hooks.json` |
| Windsurf | `post_cascade_response` | なし（`pre_user_prompt` の `tool_info.user_prompt`） | `tool_info.response` | `.devin/hooks.json` |
| GitHub Copilot（VS Code） | `Stop`（プレビュー） | transcript のみ（形式は未確認） | transcript のみ | `.github/hooks/*.json` |
| Cline | `TaskComplete` があるとの記事のみ | 未確認 | 未確認 | 未確認 |
| Continue・Aider・Antigravity | 見つからなかった | — | — | — |

Gemini CLI は、1回のフックで依頼と応答の両方が JSON で届くので、transcript を読む必要がない。もっとも素直に対応できる。Codex・Cursor・Windsurf は依頼と応答が別のフックで届くので、依頼のフックで一時保存し、終了のフックで組み合わせて記録する2段構えになる。Codex の transcript から依頼を取り出す手もあるが、公式文書が「transcript の形式は安定した仕様ではない」と書いているので頼らない。

自分は次の順に進めるのがよいと考えている。

1. Gemini CLI に対応する。`record-turn` に、フックの JSON から直接読む形式を足し、初期設定で `.gemini/settings.json` に入れる。
2. Codex・Cursor・Windsurf の2段構えを、1つの共通の仕組みとして作る。
3. Copilot と Cline は、入力の形式が公式文書で確認できてから着手する。

どのエージェントも、上の表と同じスイッチの下に `comp.conversationRecording.<エージェント>` を1つずつ足す。実装していないエージェントは、設定画面に出さない。

出典:

- [Gemini CLI Hooks reference](https://geminicli.com/docs/hooks/reference/)、[Gemini CLI hooks](https://geminicli.com/docs/hooks/)
- [Codex Lifecycle Hooks](https://learn.chatgpt.com/docs/hooks)、[Codex config reference](https://learn.chatgpt.com/docs/config-file/config-reference)
- [Cursor hooks](https://cursor.com/docs/agent/hooks)
- [Cascade Hooks（Windsurf / Devin）](https://docs.devin.ai/desktop/cascade/hooks)
- [VS Code agent hooks](https://code.visualstudio.com/docs/copilot/customization/hooks)、[GitHub Copilot CLI hooks reference](https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-hooks-reference)
- [Cline v3.36 hooks（ブログ）](https://cline.bot/blog/cline-v3-36-hooks)

## 記録は作業フォルダの中だけに残り、外部には送らない

記録先は、そのワークスペースの `.comp/history/` だけで、comP がネットワークに送ることはない。ただし中身は会話の本文（依頼600文字・応答400文字まで）なので、秘密の情報が入りうる。`.comp/` をリポジトリに含めないよう `.gitignore` に入れておくことを、README と設定の手引き（docs/user/CONFIGURATION.md）で勧める。

## 既知の制約

β版のうちに把握しておくべき制約が4つある。

1. **本体での動作確認は Windows だけで済んでいる。** 2026-10-05 に、Windows 11 と Claude Code 2.1.172 で、YASAKANI リポジトリに初期設定と同じ処理でフックを入れ、`claude -p` を2回動かした。記録ONでは往復の終わりにフックが起動して `.comp/history/` に1行記録され（`agent=claude-code`、依頼と応答の本文つき）、`.comp/config.json` だけOFFに変えると、フックが残っていても何も記録されなかった。macOS ではまだ確かめていない。
2. **VS Code の Copilot が `record-turn` を呼ぶ可能性がある。** 設定 `chat.useClaudeHooks` を有効にすると、Copilot は `.claude/settings.local.json` のフックも実行する（VS Code の公式文書による）。そうなると Copilot の会話が `claude-code` として記録されるか、形式が合わずに記録されないかのどちらかになる。どちらになるかは未確認だ。
3. **履歴は増える一方になる。** 今の `compact-history` は、まったく同じ行を消すだけだ。記録を常に続けると、`log-YYYY-MM.jsonl` は月ごとに大きくなり続ける。
4. **0.11.7 より前の本体では動かない。** 初期設定が書くフックは、拡張機能に同梱された本体を呼ぶ。0.11.6 以前の本体には `record-turn` がない。なお comP リポジトリ自身の `history-record.sh` も `record-turn` を呼ぶので、開発中に記録したいときは、このリポジトリでも設定をONにしておく必要がある。

## 正式版にする条件

次がそろったら、β版の表記を外して正式版にしてよいと考える。正式版でも既定はOFFのままにする（2026-10-05 決定）。会話の本文を作業フォルダに書き出す機能なので、本人が選んで使う形を保つ。

- Claude Code 本体で、フックの導入・記録・OFFでの停止・フックを外すコマンドを確認した（Windows は導入・記録・OFFでの停止まで確認済み。フックを外すコマンドの実機確認と macOS が残っている）
- 制約2（Copilot から呼ばれた場合）を確かめ、Copilot の呼び出しなら記録しないように見分けを入れた
- 履歴の容量を管理する手段（古い月の圧縮や削除）を用意した
- Gemini CLI の対応を1つ入れ、エージェントを増やしてもスイッチの設計が崩れないことを確かめた

## 判断してほしいこと

1. **次に対応するエージェントを、上の順番（Gemini CLI → Codex・Cursor・Windsurf → Copilot・Cline）で進めてよいか。** 利用者の多いエージェントを先にすべきという意見があれば、2段構えの共通部分を先に作る順番に入れ替えられる。
