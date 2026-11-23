#!/usr/bin/env bash
# 用本地自签名证书重签 .app，让「辅助功能」授权在重新编译后依然有效。
# 首次准备（只做一次，见 README「签名」一节）：
#   security create-keychain -p suiyi ~/Library/Keychains/suiyi-signing.keychain-db
#   security import cert.p12 -k ~/Library/Keychains/suiyi-signing.keychain-db -P suiyi \
#     -T /usr/bin/codesign -T /usr/bin/security -A
#   security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k suiyi \
#     ~/Library/Keychains/suiyi-signing.keychain-db
set -euo pipefail
APP="${1:-src-tauri/target/release/bundle/macos/随译.app}"
KC="${SUIYI_KEYCHAIN:-$HOME/Library/Keychains/suiyi-signing.keychain-db}"
codesign --force --deep --sign "Suiyi Dev" --keychain "$KC" "$APP"
codesign -d -r- "$APP" 2>&1 | grep designated
echo "signed: $APP"
