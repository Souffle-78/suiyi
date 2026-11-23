#!/usr/bin/env bash
# 打包前先解锁签名钥匙串，否则 codesign 会卡在系统密码弹窗（errSecInternalComponent）。
# 钥匙串与证书的准备见 README「签名」一节。
set -euo pipefail
KC="${SUIYI_KEYCHAIN:-$HOME/Library/Keychains/suiyi-signing.keychain-db}"
PASS="${SUIYI_KC_PASS:-suiyi-build}"
if [ -f "$KC" ]; then
  security unlock-keychain -p "$PASS" "$KC" >/dev/null 2>&1 || echo "警告：解锁钥匙串失败，签名可能失败" >&2
fi
exec npx tauri build "$@"
