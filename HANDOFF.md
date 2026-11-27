# 随译 (suiyi) 交接说明

> 目标：把这套「全局划词翻译」桌面应用从 macOS 接手到 Windows 上继续开发。
> 本文件同时是一份可交给 AI 助手的上下文说明（最后一节是现成的提示词）。

---

## 1. 一分钟速览

- 产品：**随译**，全局划词翻译。选中任意应用里的文字 → 松手弹「译」小图标 → 点击/悬停展开卡片 → 译文 + 词典 + AI 精解。
- 技术栈：**Tauri 2 + Rust**；前端是**手写的原生 HTML/JS**（没有框架、没有打包器）；macOS 的气泡是一个**独立的 Swift 辅助进程**（NSPanel），Windows 用 Tauri 的 webview 气泡。
- 仓库：`https://github.com/Souffle-78/suiyi`（main 分支，当前 **v0.7.1**）
- 发布：推 `v*` tag → GitHub Actions 三端构建 → 下载产物 → 手动改名 → `gh release create`
- 线上安装包：<https://github.com/Souffle-78/suiyi/releases>

---

## 2. Windows 上从零跑起来

前置（缺一不可）：

1. **Visual Studio Build Tools**（勾选 “使用 C++ 的桌面开发”，提供 MSVC 链接器）
2. **Rust**（`winget install Rustlang.Rustup`，默认 `stable-x86_64-pc-windows-msvc`）
3. **Node.js 18+**（`winget install OpenJS.NodeJS.LTS`）
4. **WebView2 Runtime**（Win11 自带；Win10 需装 Evergreen Runtime）
5. **Git**

```powershell
git clone https://github.com/Souffle-78/suiyi.git
cd suiyi
npm ci

# 开发（热重载 Rust 需要重跑）
npx tauri dev

# 打包（产物在 src-tauri\target\release\bundle\nsis\）
npx tauri build
```

> 没有 `scripts/build.sh` 的 Windows 版本——那个脚本只是「解锁 macOS 自签钥匙串 + 构建」，Windows 直接 `npx tauri build` 即可。

---

## 3. 目录结构 / 每个文件干什么

```
suiyi-desktop/
├─ src/                       前端（原生 HTML + JS，无构建步骤）
│  ├─ app.html                主界面：翻译 / 生词本 / 历史 / 设置 四个页签
│  ├─ bubble.html             Windows/Linux 的划词气泡（含单词卡片 renderCard()）
│  └─ permission.html         macOS 权限引导窗（Windows 用不到）
├─ src-tauri/
│  ├─ src/
│  │  ├─ main.rs              入口：托盘、窗口、命令注册、全局钩子编排、show_bubble
│  │  ├─ hook.rs              全局鼠标监听（macOS=CGEventTap，其它=rdev）
│  │  ├─ capture.rs           抓选区（模拟 Ctrl+C + 剪贴板 / macOS AX 兜底）
│  │  ├─ translate.rs         翻译：Google → MyMemory 兜底 → DeepSeek；英英词典
│  │  ├─ translate_flow.rs    划词主流程（先 loading 再结果，单词补词典）→ Card
│  │  ├─ dict_local.rs        内置 ECDICT SQLite 词库（离线兜底 + 考试标签）
│  │  ├─ ai.rs                DeepSeek「AI 精解」+ 本地缓存
│  │  ├─ bubble_helper.rs     macOS 助手进程的启动/停止 + JSON 文件 IPC
│  │  ├─ config.rs            config.json 读写
│  │  ├─ history.rs           history.jsonl（历史）
│  │  ├─ vocab.rs             vocab.jsonl（生词本）+ Markdown 导出
│  │  ├─ log.rs               suiyi.log 文件日志（滚动 512KB）
│  │  └─ permissions.rs       macOS 权限检测（辅助功能 / 输入监控）
│  ├─ helper/
│  │  ├─ BubbleHelper.swift   ★ macOS 专用：NSPanel 气泡 + 单词卡片（Windows 不需要）
│  │  └─ bin/suiyi-helper     编译产物（arm64 + x86_64 通用二进制）
│  ├─ resources/ecdict.sqlite 内置词库（7.1MB，两平台共用）
│  ├─ tauri.conf.json         主配置（窗口、权限、resources）
│  ├─ tauri.macos.conf.json   macOS 覆盖（把 helper 和词库打进 bundle）
│  └─ tauri.ci.conf.json      CI 覆盖（公共 runner 没有证书 → 关掉 macOS 签名）
└─ .github/workflows/release.yml  三端矩阵：mac arm64 / mac x64 / windows nsis
```

---

## 4. 平台差异（重点）

| 能力 | macOS | Windows | 说明 |
|---|---|---|---|
| 划词触发 | CGEventTap（全屏空间可用） | **rdev**（`hook.rs`） | Windows 侧未在真机验证 |
| 抓选区 | 模拟 ⌘C + AX 兜底 | 模拟 **Ctrl+C** + 剪贴板 | `capture.rs` 里 `#[cfg]` 分支 |
| 气泡 UI | Swift NSPanel 助手（盖全屏空间） | `bubble.html` webview 窗口 | 0.7.1 起两边数据一致（`CardPayload`） |
| 悬浮窗层级 | `collectionBehavior` 等 objc 调用 | 普通置顶窗口 | macOS 那套 `.fullScreenAuxiliary` 逻辑 Windows 无效 |
| 权限 | 辅助功能 + 输入监控（TCC） | 无 | `permissions.rs` 全是 macOS |
| 托盘 | `TrayIconBuilder`（都要 `app.manage` 保活） | 同 | |
| 开机自启 | LaunchAgent | 注册表 Run 键 | `tauri-plugin-autostart` 两平台通用 |
| 单实例 | 已启用 | 已启用 | `tauri-plugin-single-instance` |

**结论**：`hook.rs` / `capture.rs` / `main.rs` 里所有 `#[cfg(target_os = "macos")]` 之外的代码，就是 Windows 的实际运行路径。

---

## 5. 数据与日志位置

| | macOS | Windows |
|---|---|---|
| 配置 | `~/Library/Application Support/suiyi/config.json` | `%APPDATA%\suiyi\config.json` |
| 历史 | 同目录 `history.jsonl` | `%APPDATA%\suiyi\history.jsonl` |
| 生词本 | 同目录 `vocab.jsonl` | 同上 |
| 日志 | 同目录 `suiyi.log` | `%APPDATA%\suiyi\suiyi.log` |
| AI 缓存 | 同目录 `ai-cache.json` | 同上 |
| 词库 | App 内 `resources/ecdict.sqlite` | 安装目录 `resources\ecdict.sqlite` |

> 在 Windows 上 `dirs::config_dir()` = `C:\Users\<你>\AppData\Roaming`，所以是 `...\Roaming\suiyi\`。

---

## 6. 自检探针（不用真的划词就能测）

```powershell
# 命令行自检（建议在项目根目录跑 target\release\suiyi.exe）
$env:SUIYI_PROBE="1";                .\src-tauri\target\release\suiyi.exe   # 翻译线路
$env:SUIYI_PROBE="history";          ...                                    # 历史读写
$env:SUIYI_PROBE="vocab";            ...                                    # 生词本
$env:SUIYI_PROBE="autostart";        ...                                    # 开机自启开关
$env:SUIYI_PROBE="dict"; $env:SUIYI_PROBE_WORD="action"; ...                # 词典解析（先打 zh 再打 en）
$env:SUIYI_PROBE="pipeline";         ...                                    # 抓选区→翻译→落盘（要先去别的窗口选中文字）
$env:SUIYI_TRACE="1";                ...                                    # 打印每一个原始鼠标事件
```

---

## 7. Windows 待验证 / 待改清单

按优先级：

1. **全局钩子是否真的能收到鼠标事件**（rdev 在 Windows 上一般没问题，但要确认它不会被 UAC 提权窗口/UWP 应用挡住）。
2. **Ctrl+C 抓取是否可靠**：某些应用（Electron、终端、WPS、部分 Qt 程序）复制行为不同；`capture.rs` 的 120ms + 3 次重试需要在真机调。
3. **webview 气泡的位置/尺寸**：`show_bubble_in()` 里用 `logical_to_physical()` + 多显示器夹取，Windows 的 DPI 缩放（125%/150%）与 macOS 2x 不同，重点看边缘夹取。
4. **气泡高度**：`height` 目前按 `word/sentence` 写死（340/200），Windows 字体渲染更高，可能需要测量后自适应。
5. **卡片里 ✨ 精解按钮**：走 `invoke('ai_explain')`，Windows 上首次调用可能出现 webview 权限/异步时序问题。
6. **托盘菜单**：「翻译剪贴板 / 设置 / 历史 / 权限 / 退出」在 Windows 上是否都能点。
7. **开机自启**：注册表键是否写对、是否影响单实例。
8. **NSIS 安装包**：未签名 → SmartScreen 会拦，需要 EV 证书或让用户点「仍要运行」。
9. **Windows 上没有 Swift 助手**：`bubble_helper::*` 全是 macOS 空实现，别误以为气泡走那边。

---

## 8. 发布流程

```powershell
# 1) 改版本号（四处必须同步）
#    package.json / src-tauri/tauri.conf.json / src-tauri/Cargo.toml / src/app.html 里的 '0.7.1'
# 2) 更新 CHANGELOG.md 和 README.md
git add -A; git commit -m "feat(x.y.z): ..."; git tag vX.Y.Z; git push origin main --tags

# 3) 等 CI 绿
gh run list --limit 1

# 4) 下载产物
gh run download <run-id> -D rel

# 5) 改 ASCII 名（GitHub 会吞掉中文前缀）
#    随译_X.Y.Z_x64-setup.exe → Suiyi_X.Y.Z_x64-setup.exe
#    macOS 两个 dmg 需要在 mac 上用 Suiyi Dev 证书重签（脚本 scripts/sign.sh 的思路）

# 6) 发布
gh release create vX.Y.Z --title "随译 X.Y.Z — ..." --notes-file notes.md Suiyi_X.Y.Z_*
```

---

## 9. 交给 AI 助手的提示词（直接复制）

> 我在维护一个叫「随译」的全局划词翻译桌面应用，仓库 `https://github.com/Souffle-78/suiyi`（main 分支，v0.7.1）。
> 现在要把 macOS 上的开发接到 Windows 上继续。请先读仓库里的 `HANDOFF.md`、`README.md`、`CHANGELOG.md`，
> 再读 `src-tauri/src/{main,hook,capture,translate,translate_flow,bubble_helper}.rs` 和 `src/bubble.html`，
> 搞清楚 macOS 与 Windows 的分支（所有 `#[cfg(target_os = "macos")]` 之外的就是 Windows 路径）。
>
> 技术栈：Tauri 2 + Rust，前端是手写原生 HTML/JS（无框架），macOS 的气泡是独立 Swift NSPanel 助手（Windows 没有）。
>
> 第一步不要改代码：先在本机跑 `npm ci && npx tauri dev`，确认能起窗口；再 `npx tauri build` 出 NSIS 安装包。
> 然后用 `$env:SUIYI_PROBE` 系列探针（见 HANDOFF.md 第 6 节）确认翻译线路、词典、生词本、历史可用。
>
> 之后按 HANDOFF.md 第 7 节的清单逐条在 Windows 真机验证，重点是：
> 1) 全局鼠标钩子能否收到事件；2) 模拟 Ctrl+C 抓选区是否稳定（Electron/终端/WPS 等应用）；
> 3) 高 DPI 下气泡位置与尺寸；4) 单词卡片与 ✨ AI 精解按钮是否正常。
>
> 工作方式：小步提交，每个修复先写复现步骤，再改代码，再给我验证方法；不要顺手重写架构，
> 不要引入前端框架或打包器；改 `.rs` 或 `.html` 时保持 `#[cfg]` 平台分支的既有结构。
