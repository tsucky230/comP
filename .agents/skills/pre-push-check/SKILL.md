---
name: pre-push-check
description: git push 前の統合チェック。単体テスト・カバレッジ・SAST・Markdown Lint を一括実行し、全GREENを確認してからpushを案内する。AGENTS.md PUSH要件に準拠。
---

# Pre-Push-Check スキル

## 起動条件

- `git push` 前のユーザー指示
- 「PR出す前にチェック」「リリース準備」
- CI失敗のローカル再現

## ⚠️ git push 自体は実行しない

STOP条件#6に従い、本スキルは**チェックのみ**実行。
push実行はユーザー承認後、別途実施。

## 実行手順

### 1. 環境確認

```bash
git status                # 未コミットファイル確認
git log origin/main..HEAD # push対象コミット確認
```

### 2. テスト実行

| 言語 | コマンド |
| --- | --- |
| Python | `pytest --cov=src --cov-report=term-missing` |
| Node.js | `npm test -- --coverage` |
| Rust | `cargo test --all` |

**判定**:
- 全テスト PASS
- カバレッジ 80% 以上

### 3. SAST（静的セキュリティ）

| 言語 | コマンド |
| --- | --- |
| Python | `bandit -r src/` + `semgrep --config p/security-audit src/` |
| Node.js | `npm audit --production` + `semgrep --config p/security-audit src/` |
| Rust | `cargo audit` |

**判定**:
- Critical/High = 0 件（必須）
- Medium = 警告（push可だが記録）

### 4. Markdown Lint

```bash
npm run lint:md
# または
npx markdownlint "**/*.md"
```

### 5. 依存関係チェック

```bash
# Python
pip list --outdated

# Node.js
npm outdated

# Rust
cargo outdated
```

### 6. レポート生成

`temp/log/pre_push_YYYYMMDD_HHmm.md` に結果保存：

```markdown
# Pre-Push チェック結果 YYYY-MM-DD HH:mm

## サマリ
- テスト: ✅ PASS (123/123)
- カバレッジ: ✅ 85%
- SAST: ✅ Critical/High 0
- Markdown Lint: ✅
- 依存関係: ⚠️ 2件のminor更新あり

## 詳細
[各セクションの出力]

## push可否
✅ 全GREEN → ユーザー承認後にpush可
```

### 7. ユーザー確認

レポート提示後：

「全チェック完了しました。`git push` を実施してもよいですか？
（STOP条件#6によりユーザー承認が必要です）」

## 禁止事項

- チェック未完了でのpush案内
- `--no-verify` でのフックスキップ
- 失敗テストをスキップ化してpush

## 参照

- [docs/STANDARDS_CICD.md](../../../docs/STANDARDS_CICD.md)
- [AGENTS.md](../../../AGENTS.md) STOP条件#6
