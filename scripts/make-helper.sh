#!/usr/bin/env bash
# 编译 macOS 划词气泡助手（NSPanel 进程）。非 macOS 直接跳过。
set -euo pipefail
exec >>/tmp/make-helper.log 2>&1
echo "=== $(date) cwd=$PWD PATH=$PATH"
cd "$(dirname "$0")/.."

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "make-helper: skip (not macOS)"
  exit 0
fi

# tauri 的 beforeBundleCommand 用的是一个很干净的 PATH，这里自己补齐
export PATH="/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin:$PATH"
SWIFTC="$(command -v swiftc || true)"
if [[ -z "$SWIFTC" && -x /usr/bin/swiftc ]]; then SWIFTC=/usr/bin/swiftc; fi
if [[ -z "$SWIFTC" ]]; then
  echo "make-helper: swiftc not found, skipping helper (气泡会退回 webview 版本)" >&2
  exit 0
fi

SRC="src-tauri/helper/BubbleHelper.swift"
OUT_DIR="src-tauri/helper/bin"
OUT="$OUT_DIR/suiyi-helper"
mkdir -p "$OUT_DIR"

if [[ ! -f "$SRC" ]]; then
  echo "make-helper: missing $SRC" >&2
  exit 1
fi

# 已经是最新就不用重编
if [[ -x "$OUT" && "$OUT" -nt "$SRC" ]]; then
  echo "make-helper: up to date"
  exit 0
fi

"$SWIFTC" -O -o "$OUT" "$SRC" 2>&1 | grep -v "warning:" || true
chmod +x "$OUT"
echo "make-helper: built $OUT ($(du -h "$OUT" | cut -f1))"
