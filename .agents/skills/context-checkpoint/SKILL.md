---
name: context-checkpoint
description: 長時間タスク中にコンテキスト枯渇に備え、進捗・決定事項・次アクションを temp/log/checkpoint_*.md に保存する。会話30ターン超または重要決定後に手動起動。
---

# Context-Checkpoint スキル

## 起動条件

- 会話が長くなった（30ターン超の体感）
- 重要な設計決定が確定した直後
- 「チェックポイント保存」の明示指示
- セッション切れ前のユーザー指示

## 実行手順

### 1. 現状サマリ作成

`temp/log/checkpoint_YYYYMMDD_HHmm.md` に以下を記録：

```markdown
# チェックポイント YYYY-MM-DD HH:mm

## 現在のタスク
- [タスクの目的]
- [なぜこのタスクか]

## 完了済み
- [x] [完了項目1]
- [x] [完了項目2]

## 進行中
- [ ] [現在の作業内容]
- [ ] [次のステップ]

## 未着手
- [ ] [残タスク]

## 重要な決定事項
- [決定1]: [理由]
- [決定2]: [理由]

## 関連ファイル
- [ファイルパス1]: [何のためか]
- [ファイルパス2]: [何のためか]

## 次アクション
1. [次にすること]
2. [その次]

## ブロッカー・要確認
- [ユーザー確認待ち事項]
```

### 2. メモリ反映

AGENTS.md準拠で重要事項を `~/.Codex/projects/.../memory/` に追加：
- 新しい設計判断 → project memory
- ユーザー指摘・フィードバック → feedback memory

### 3. RTK統計記録

```bash
rtk gain >> temp/log/token_usage.txt
```

### 4. 復帰用ポインタ

`temp/log/last_checkpoint.txt` に最新ファイル名を記録：
```bash
echo "checkpoint_YYYYMMDD_HHmm.md" > temp/log/last_checkpoint.txt
```

## セッション復帰時

新セッション開始時、ユーザーが「前回の続き」と指示したら：

```bash
cat temp/log/last_checkpoint.txt
# → 最新チェックポイント名取得
cat temp/log/$(cat temp/log/last_checkpoint.txt)
# → 内容読込
```

## 禁止事項

- チェックポイントなしの長時間連続作業（30ターン超）
- 重要決定をメモリ・チェックポイントに残さず進行

## 参照

- [AGENTS.md](../../../AGENTS.md) コンテキスト永続化戦略
- [docs/CONSTITUTION_DETAIL.md](../../../docs/CONSTITUTION_DETAIL.md) 中間ファイル管理
