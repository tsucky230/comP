#!/bin/bash
PREV=""
echo "フォーカスの監視を開始します... (終了するには Ctrl+C)"
while true; do
  CUR=$(osascript -e 'tell application "System Events" to get name of first application process whose frontmost is true' 2>/dev/null)
  if [ "$CUR" != "$PREV" ] && [ -n "$CUR" ]; then
    echo "$(date '+%H:%M:%S') - 画面を奪ったアプリ: $CUR"
    PREV=$CUR
  fi
  sleep 0.2
done
