// tauri 的 beforeBuildCommand 要在 Windows / Linux / macOS 上都能跑。
// Windows 的开发机通常没有 bash（Git Bash 不一定在 PATH 里），所以这里先判断平台：
// 非 macOS 直接跳过（气泡助手本来就是 macOS 独有的 NSPanel 进程），
// macOS 上才真正去调 scripts/make-helper.sh 编出 slip-helper。
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

if (process.platform !== "darwin") {
  console.log("make-helper: skip (not macOS)");
  process.exit(0);
}

const here = dirname(fileURLToPath(import.meta.url));
const script = join(here, "make-helper.sh");
const r = spawnSync("bash", [script], { stdio: "inherit" });
if (r.error) {
  // 没有 bash 也别把构建卡死：气泡会退回 webview 版本，功能不丢
  console.warn(`make-helper: 无法执行 bash: ${r.error.message}`);
  process.exit(0);
}
process.exit(r.status ?? 0);
