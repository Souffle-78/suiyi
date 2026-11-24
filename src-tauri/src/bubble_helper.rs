//! 划词气泡助手（macOS）：把气泡交给一个独立的 NSPanel 进程去画。
//!
//! 为什么要拆进程见 `src-tauri/helper/BubbleHelper.swift` 顶部说明。这里只负责：
//!   1. 保证助手在跑（按需拉起 bundle 里的 `suiyi-helper`）；
//!   2. 把「小图标 / 结果面板」写进 `bubble-request.json`（带递增 seq，助手轮询刷新）；
//!   3. 轮询 `bubble-action.json`，执行助手点出来的动作：
//!      展开翻译（translate）、改原文后重译（retranslate）、复制（copy）、存生词（vocab）。
//!
//! 非 macOS 上这些函数返回 false，调用方会退回原来的 webview 气泡。

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};

#[derive(serde::Serialize, Clone)]
pub struct Sense {
    pub pos: String,
    pub defs: Vec<String>,
}

#[derive(serde::Serialize, Clone)]
struct Request {
    seq: u64,
    /// "icon" = 选区旁的小图标；"panel" = 展开后的结果面板
    phase: String,
    title: String,
    source: String,
    text: String,
    loading: bool,
    error: Option<String>,
    /// "word" = 单词（带词典），"sentence" = 句子
    mode: String,
    phonetic: String,
    senses: Vec<Sense>,
    x: f64,
    y: f64,
}

static SEQ: AtomicU64 = AtomicU64::new(0);
static STARTED: AtomicBool = AtomicBool::new(false);
static LAST_ACTION_TS: AtomicI64 = AtomicI64::new(0);

#[cfg(target_os = "macos")]
fn dir() -> PathBuf {
    crate::config::dir()
}

#[cfg(target_os = "macos")]
fn helper_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    // .../随译.app/Contents/MacOS/suiyi -> .../随译.app/Contents/Resources/helper/bin/suiyi-helper
    let resources = exe.parent()?.parent()?.join("Resources");
    let p = resources.join("helper/bin/suiyi-helper");
    if p.exists() {
        return Some(p);
    }
    let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("helper/bin/suiyi-helper");
    if dev.exists() {
        return Some(dev);
    }
    None
}

#[cfg(target_os = "macos")]
fn process_alive(pid: i32) -> bool {
    unsafe { kill(pid, 0) == 0 }
}

#[cfg(target_os = "macos")]
extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
}

/// 确保助手进程在跑。返回 true 表示助手可用。
#[cfg(target_os = "macos")]
pub fn ensure_running() -> bool {
    let pid_file = dir().join("bubble-helper.pid");
    if let Ok(text) = std::fs::read_to_string(&pid_file) {
        if let Ok(pid) = text.trim().parse::<i32>() {
            if pid > 0 && process_alive(pid) {
                return true;
            }
        }
    }
    if STARTED.swap(true, Ordering::SeqCst) {
        std::thread::sleep(std::time::Duration::from_millis(400));
        if let Ok(text) = std::fs::read_to_string(&pid_file) {
            if let Ok(pid) = text.trim().parse::<i32>() {
                if pid > 0 && process_alive(pid) {
                    return true;
                }
            }
        }
    }
    let Some(path) = helper_path() else {
        crate::log::line("bubble helper binary not found");
        return false;
    };
    let _ = std::fs::remove_file(dir().join("bubble-shutdown"));
    match std::process::Command::new(&path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(child) => {
            crate::log::line(format!(
                "bubble helper started (pid {}) from {}",
                child.id(),
                path.display()
            ));
            true
        }
        Err(e) => {
            crate::log::line(format!("bubble helper spawn failed: {e}"));
            STARTED.store(false, Ordering::SeqCst);
            false
        }
    }
}

#[cfg(target_os = "macos")]
fn write_request(req: &Request) -> bool {
    let path = dir().join("bubble-request.json");
    let tmp = dir().join("bubble-request.json.tmp");
    let Ok(json) = serde_json::to_string(req) else {
        return false;
    };
    let ok = (|| -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(json.as_bytes())?;
        f.sync_all()?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    })();
    match ok {
        Ok(()) => true,
        Err(e) => {
            crate::log::line(format!("bubble request write failed: {e}"));
            false
        }
    }
}

/// 选区旁先冒一个小图标（不翻译，等用户点/悬停）。
#[cfg(target_os = "macos")]
pub fn show_icon(source: &str, x: f64, y: f64) -> bool {
    if !ensure_running() {
        return false;
    }
    write_request(&Request {
        seq: SEQ.fetch_add(1, Ordering::SeqCst) + 1,
        phase: "icon".into(),
        title: String::new(),
        source: source.chars().take(2000).collect(),
        text: String::new(),
        loading: false,
        error: None,
        mode: "sentence".into(),
        phonetic: String::new(),
        senses: Vec::new(),
        x,
        y,
    })
}

/// 展开后的结果面板。
#[cfg(target_os = "macos")]
#[allow(clippy::too_many_arguments)]
pub fn show_panel(
    title: String,
    source: String,
    text: String,
    loading: bool,
    error: Option<String>,
    mode: String,
    phonetic: String,
    senses: Vec<Sense>,
    x: f64,
    y: f64,
) -> bool {
    if !ensure_running() {
        return false;
    }
    write_request(&Request {
        seq: SEQ.fetch_add(1, Ordering::SeqCst) + 1,
        phase: "panel".into(),
        title,
        source: source.chars().take(2000).collect(),
        text,
        loading,
        error,
        mode,
        phonetic,
        senses,
        x,
        y,
    })
}

/// 后台线程：轮询助手写回来的动作。
#[cfg(target_os = "macos")]
pub fn start_action_watcher(app: tauri::AppHandle) {
    std::thread::spawn(move || {
        let path = dir().join("bubble-action.json");
        // 启动时把已有的动作当成「历史」，别把上一次会话最后一条动作再执行一遍
        if let Ok(raw) = std::fs::read_to_string(&path) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
                let ts = v.get("ts").and_then(|t| t.as_f64()).unwrap_or(0.0) as i64;
                LAST_ACTION_TS.store(ts, Ordering::SeqCst);
            }
        }
        loop {
            std::thread::sleep(std::time::Duration::from_millis(300));
            let Ok(raw) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else {
                continue;
            };
            let ts = v.get("ts").and_then(|t| t.as_f64()).unwrap_or(0.0) as i64;
            if ts <= LAST_ACTION_TS.load(Ordering::SeqCst) {
                continue;
            }
            LAST_ACTION_TS.store(ts, Ordering::SeqCst);
            let action = v.get("action").and_then(|a| a.as_str()).unwrap_or("");
            let source = v
                .get("source")
                .and_then(|a| a.as_str())
                .unwrap_or("")
                .to_string();
            let text = v.get("text").and_then(|a| a.as_str()).unwrap_or("");
            match action {
                "translate" | "retranslate" => {
                    let src = source.trim().to_string();
                    if src.is_empty() {
                        continue;
                    }
                    let (x, y) = v
                        .get("x")
                        .and_then(|n| n.as_f64())
                        .zip(v.get("y").and_then(|n| n.as_f64()))
                        .unwrap_or((700.0, 400.0));
                    crate::log::line(format!("bubble action: {action} ({} chars)", src.len()));
                    let app2 = app.clone();
                    std::thread::spawn(move || {
                        crate::translate_flow::translate_and_show(app2, src, x, y);
                    });
                }
                "vocab" => match crate::vocab::add(&source, text, "", "zh-CN") {
                    Ok(true) => crate::log::line("bubble: saved to vocabulary"),
                    Ok(false) => crate::log::line("bubble: already in vocabulary"),
                    Err(e) => crate::log::line(format!("bubble: vocab save failed: {e}")),
                },
                "copy" => crate::log::line("bubble: copied translation"),
                other => crate::log::line(format!("bubble: unknown action {other}")),
            }
        }
    });
}

/// 主程序退出时把助手一起关掉。
#[cfg(target_os = "macos")]
pub fn shutdown() {
    let _ = std::fs::write(dir().join("bubble-shutdown"), b"1");
}

#[cfg(not(target_os = "macos"))]
pub fn ensure_running() -> bool {
    false
}

#[cfg(not(target_os = "macos"))]
pub fn show_icon(_source: &str, _x: f64, _y: f64) -> bool {
    false
}

#[cfg(not(target_os = "macos"))]
#[allow(clippy::too_many_arguments)]
pub fn show_panel(
    _title: String,
    _source: String,
    _text: String,
    _loading: bool,
    _error: Option<String>,
    _mode: String,
    _phonetic: String,
    _senses: Vec<Sense>,
    _x: f64,
    _y: f64,
) -> bool {
    false
}

#[cfg(not(target_os = "macos"))]
pub fn start_action_watcher(_app: tauri::AppHandle) {}

#[cfg(not(target_os = "macos"))]
pub fn shutdown() {}
