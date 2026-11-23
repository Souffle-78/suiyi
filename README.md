# 随译 · Suiyi

选中任意应用里的文字，立刻在鼠标旁弹出译文。Windows / macOS / Linux 三端。

## 现在能做什么

- 全局监听鼠标：拖选文字后松开，自动抓取选区
- 两条线路：**免费线路**（Google，免 key，双入口轮换重试）／**DeepSeek 精翻**（需 key，自动注入术语表）
- 设置窗口：目标语言（10 种）、线路切换、DeepSeek key 与模型、术语表（一行一条 `原文 = 译文`）
- 鼠标旁弹出结果气泡：复制 / 关闭；点别处或 25 秒自动收起
- 历史记录窗口：搜索、复制译文、清空（本地 JSONL，最多留 2000 条）
- **生词本**：气泡上「存生词」一键收进自己的词库，历史窗口第二个页签里搜索、复制、删除、导出 Markdown
- **开机自启**：设置里勾一下，登录后自动常驻菜单栏（LaunchAgent / 注册表 / autostart）
- 菜单栏图标：暂停划词、设置…、历史记录…、辅助功能权限…、退出
- 配置文件：`~/Library/Application Support/suiyi/config.json`（改完保存即生效，不用重启）

## 无界面自检

```bash
SUIYI_PROBE=1 ./src-tauri/target/debug/suiyi   # 翻译链路
SUIYI_PROBE=history ./src-tauri/target/debug/suiyi  # 历史读写
SUIYI_PROBE=vocab ./src-tauri/target/debug/suiyi    # 生词本增删查 + Markdown 导出
SUIYI_PROBE=autostart ./src-tauri/target/debug/suiyi # 开机自启开关（会先开后关，不留痕）
SUIYI_PROBE=fallback ./src-tauri/target/debug/suiyi  # 备用线路 MyMemory
SUIYI_PROBE=pipeline ./src-tauri/target/debug/suiyi  # 端到端（抓选区→翻译→写历史），不含鼠标钩子
SUIYI_PROBE=clip ./src-tauri/target/debug/suiyi      # 翻译剪贴板
cargo test                                  # 解析与坐标换算的单元测试
# OK google → 你好，世界…
```

## 打包

```bash
npx tauri icon src-tauri/icons/source.png   # 生成 icns/ico/各尺寸 png
npx tauri build                             # 产物在 src-tauri/target/release/bundle/
```

## 运行

```bash
npm install                 # 只装 tauri CLI
npm run dev                 # 开发模式（会编译 Rust，首次较慢）
npm run build               # 打 .app / .dmg（macOS）
```

首次运行需要给 **辅助功能（Accessibility）** 权限：
系统设置 → 隐私与安全性 → 辅助功能 → 勾选「随译」。
没有这个权限，读不到选区、也发不出 ⌘C。

## 目录

```
src/bubble.html            气泡窗口（结果卡片）
src-tauri/src/main.rs      托盘、气泡定位、鼠标监听编排
src-tauri/src/capture.rs   模拟 ⌘C 抓选区 + 剪贴板还原
src-tauri/src/translate.rs 翻译线路（Google 免费接口）
```

## 已知限制（待办）

- Windows、Linux 的后端（WH_MOUSE_LL / XRecord）还没接
- 只有中文界面
- Linux 仅 X11 可用；Wayland 不允许全局监听，只能退化为快捷键 + 剪贴板
- macOS 分发需要签名与公证，否则别人首次打开会被 Gatekeeper 拦
- 本机开发用自签证书固定签名（否则每次重编译「辅助功能 / 输入监控」授权都会失效）：
  ```bash
  security create-keychain -p suiyi-build ~/Library/Keychains/suiyi-signing.keychain-db
  security set-keychain-settings -lut 86400 ~/Library/Keychains/suiyi-signing.keychain-db
  security import cert.p12 -k ~/Library/Keychains/suiyi-signing.keychain-db -P suiyi \
    -T /usr/bin/codesign -T /usr/bin/security -A
  security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k suiyi-build \
    ~/Library/Keychains/suiyi-signing.keychain-db
  security list-keychains -d user -s ~/Library/Keychains/login.keychain-db \
    ~/Library/Keychains/suiyi-signing.keychain-db
  ```
  之后用 `npm run build`（会先解锁钥匙串再打包，避免 codesign 卡在系统密码弹窗）。

## 三端支持

| 平台 | 抓选区方式 | 状态 |
| --- | --- | --- |
| macOS 12+ | 模拟 ⌘C（兜底系统辅助功能接口） | 已跑通，需在「辅助功能」里授权 |
| Windows 10/11 | 模拟 Ctrl+C | 代码同一套，需在 Windows 机器上验证 |
| Linux X11 | 模拟 Ctrl+C（libxdo） | 代码同一套，需在 X11 桌面验证 |
| Linux Wayland | 全局钩子受限 | 需要桌面环境提供 portal，暂不支持 |

`rdev` 在 Linux 上依赖 `libx11`/`libxtst`，打包需要 `libwebkit2gtk-4.1-dev`、`libayatana-appindicator3-dev` 等，
`.github/workflows/release.yml` 里已经把三端的依赖和构建矩阵写好了（打 tag 触发）。

## 权限要求

- **macOS**：系统设置 → 隐私与安全性 → 「辅助功能」+「输入监控」都要给「随译」打开。
  没授权时菜单栏图标仍在，但划词不会触发；授权后新版会自动挂上钩子，不用重启。
- 重新编译覆盖 `/Applications/随译.app` 之后，macOS 可能把它当成新程序：如果授权过却仍无反应，
  去列表里把「随译」删掉（− 号）再重新勾一次。
- **Windows**：无需额外授权；部分以管理员权限运行的窗口（任务管理器等）需要同权限运行本程序。
- **Linux**：X11 下无需额外授权；Wayland 需要先切到 X11 会话。

## 常见问题

- **选中没反应**：先看 `~/Library/Application Support/suiyi/suiyi.log`（Windows: `%APPDATA%\suiyi\suiyi.log`，
  Linux: `~/.config/suiyi/suiyi.log`），里面有 `skip hook — accessibility not granted` 就是没授权。
- **译不出来**：设置里换线路试试；Google/Bing 免密钥但有频率限制，长文本建议用 DeepSeek 精翻。
- **气泡位置怪**：多屏 / 缩放比例不同时会在屏幕边界内夹取，仍可手动拖动（气泡可拖）。
