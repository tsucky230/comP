# Markdown 品質・Lint 標準

**参照元**: CLAUDE.md → `@docs/STANDARDS_MARKDOWN.md`

---

## 1. Lint ツール

- 主ツール: `markdownlint-cli2` (Node.js)
- 代替: `mdl`

## 2. セットアップ

```bash
npm install --save-dev markdownlint-cli2
```

## 3. 設定ファイル `.markdownlint.json`

| ルール | 設定 | 理由 |
| --- | --- | --- |
| MD003（見出しスタイル） | 無効化 | プロジェクト混在許容 |
| MD013（行長） | 無効化 | 日本語折り返し問題 |
| MD024（重複見出し） | siblings_only | 兄弟見出しのみチェック |
| MD034（裸URL） | 無効化 | 内部リンク許容 |

`markdownlint-cli2` v0.12 は `.markdownlintignore` を読まない。除外は
`package.json` の `lint:md` グロブに `!` 付きで列挙する。

## 4. 実行コマンド

```bash
npm run lint:md          # チェックのみ
npm run lint:md:fix      # 自動修正
```

検査対象は 24 ファイル。git 管理外のディレクトリ（`temp/` `.venv/` `.comp/` `.claude/`）は
除外グロブで落とし、CI と同じ集合を検査する。git 管理外の `.md` を新設したら
除外グロブへの追加が必要になる。

### グロブは必ずダブルクォートで囲む

npm スクリプトは Windows では PowerShell ではなく `cmd.exe` 経由で実行される。
`cmd.exe` はシングルクォートを引用符と見なさないため、`'**/*.md'` はクォート込みの
リテラル文字列として渡り、**1ファイルもマッチしないまま `Summary: 0 error(s)` を返す**
（検査済みと誤認する事故になる）。ダブルクォートは `cmd.exe` も bash も剥がすため両対応。

結果を信用する前に出力の `Linting: N file(s)` の N を必ず確認すること。

## 5. 禁止される警告（必ず修正）

- 不正な見出しレベル（H1複数使用）
- テーブル不整形
- 空行不足
- リスト形式エラー

## 6. 許可される警告

- ドメイン固有の慣例（プロジェクト特有書式）

## 7. CI/CD連携

`.github/workflows/lint.yml`:

```yaml
name: Markdown Lint
on: [push, pull_request]
jobs:
  lint:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v3
      - run: npm install --save-dev markdownlint-cli2
      - run: npm run lint:md
```

## 8. VSCode 拡張

- 拡張機能: `vscode-markdownlint` (David Anson)
- 効果: 保存時自動チェック、問題パネルにエラー表示
