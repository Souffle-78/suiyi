#!/usr/bin/env bash
# 用本地自签名证书重签 .app，让「辅助功能」授权在重新编译后依然有效。
# 首次准备（只做一次，见 README「签名」一节）：
#   security create-keychain -p slip ~/Library/Keychains/slip-signing.keychain-db
#   security import cert.p12 -k ~/Library/Keychains/slip-signing.keychain-db -P slip \
#     -T /usr/bin/codesign -T /usr/bin/security -A
#   security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k slip \
#     ~/Library/Keychains/slip-signing.keychain-db
set -euo pipefail
APP="${1:-src-tauri/target/release/bundle/macos/素笺.app}"
KC="${SLIP_KEYCHAIN:-$HOME/Library/Keychains/slip-signing.keychain-db}"
security unlock-keychain -p "${SLIP_KC_PASS:-slip-build}" "$KC"
codesign --force --deep --sign "Slip Dev" --keychain "$KC" "$APP"
codesign -d -r- "$APP" 2>&1 | grep designated
echo "signed: $APP"
