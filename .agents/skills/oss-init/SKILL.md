---
name: oss-init
description: OSSプロジェクト初期化時、必須メタデータ（README英/日、LICENSE、CHANGELOG、SBOM、.gitignore、CONTRIBUTING）を一括生成する。AGENTS.mdのOSS作法に準拠。
---

# OSS-Init スキル

## 起動条件

- 新規プロジェクト作成
- 「OSS公開準備」の指示
- 「README/LICENSE等を整える」の指示

## 実行手順

### 1. ユーザーヒアリング

- プロジェクト名（英/日）
- 概要（3行以内）
- 主要機能（3〜5個）
- 対象ユーザー
- ライセンス選択（MIT / Apache-2.0 / その他）
- 使用言語（Python/Node.js/Rust等）

### 2. 必須ファイル生成（STOP条件#1該当 → 個別確認）

複数の新規ファイル生成のため、**ユーザー承認を得てから順次作成**：

| ファイル | テンプレート |
| --- | --- |
| `README.md` | 英語、9セクション構成 |
| `README_ja.md` | 日本語版、同構成 |
| `LICENSE` | SPDX identifier準拠 |
| `CHANGELOG.md` | Keep a Changelog + Semver |
| `.gitignore` | 言語別GitHub template |
| `CONTRIBUTING.md` | 貢献ガイド |

### 3. SBOM 生成

```bash
# Python
pip install cyclonedx-bom
cyclonedx-py -o SBOM.json requirements.txt

# Node.js
npm install -g @cyclonedx/npm
cyclonedx-npm -o SBOM.json

# Rust
cargo install cargo-cyclonedx
cargo cyclonedx --format json
```

### 4. CI/CD ワークフロー

`.github/workflows/ci.yml` を [docs/STANDARDS_CICD.md](../../../docs/STANDARDS_CICD.md) テンプレートから生成。
STOP条件#5該当のため、**ユーザー承認必須**。

### 5. README.md 必須セクション

1. What It Does（3行）
2. Prerequisites
3. Installation
4. Quick Start（5分以内）
5. Usage（ユースケース3〜5）
6. Output（効果・数値）
7. Troubleshooting
8. License（SPDX）
9. Contributing

### 6. 検証

```bash
npm run lint:md           # Markdown Lint
markdownlint README*.md
```

### 7. 完了報告

生成ファイル一覧、SBOM内容サマリ、残タスク（実装後に更新すべき箇所）を提示。

## 禁止事項

- 一括バルク生成（個別確認なし）
- SBOM省略
- ライセンス不明のまま生成

## 参照

- [docs/STANDARDS_OSS_INIT.md](../../../docs/STANDARDS_OSS_INIT.md)
