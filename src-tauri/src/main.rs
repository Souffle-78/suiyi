#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::menu::MenuItemKind;
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition};

mod ai;
mod bubble_helper;
mod dict_local;
mod capture;
mod config;
mod history;
mod hook;
mod log;
mod permissions;
mod translate;
mod translate_flow;
mod vocab;

#[derive(Serialize, Clone)]
struct BubblePayload {
    source: String,
    text: String,
    detected: String,
    target: String,
    provider: String,
    loading: bool,
    error: Option<String>,
    /// 单词卡片的附加信息（Windows/Linux 的 webview 气泡用它渲染词典样式）
    #[serde(default)]
    card: Option<CardPayload>,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Default)]
pub struct CardPayload {
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub phonetic: String,
    #[serde(default)]
    pub phonetics: Vec<PhonLabel>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub senses: Vec<CardSense>,
    #[serde(default)]
    pub ai: Option<String>,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Default)]
pub struct PhonLabel {
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub text: String,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Default)]
pub struct CardSense {
    #[serde(default)]
    pub pos: String,
    #[serde(default)]
    pub defs: Vec<String>,
    #[serde(default)]
    pub examples: Vec<String>,
    #[serde(default)]
    pub zh: bool,
}

impl CardPayload {
    /// 纯文本版（老版本气泡 / 复制用）
    pub fn plain(&self) -> String {
        let mut out = String::new();
        for s in &self.senses {
            if s.defs.is_empty() {
                continue;
            }
            if !s.pos.is_empty() {
                out.push_str(&s.pos);
                out.push(' ');
            }
            out.push_str(&s.defs.join("；"));
            out.push('\n');
        }
        if let Some(ai) = &self.ai {
            if !ai.is_empty() {
                out.push_str("\nAI 精解\n");
                out.push_str(ai);
            }
        }
        out.trim_end().to_string()
    }
}

/// 非 macOS（或助手不可用）时，用 webview 气泡画卡片。
pub fn present_card(app: &AppHandle, card: crate::translate_flow::Card) {
    let body = if card.text.is_empty() && !card.loading {
        card.payload.plain()
    } else {
        card.text.clone()
    };
    show_bubble(
        app,
        card.lx as i32,
        card.ly as i32,
        BubblePayload {
            source: card.source,
            text: body,
            detected: String::new(),
            target: String::new(),
            provider: card.title,
            loading: card.loading,
            error: card.error,
            card: Some(card.payload),
        },
    );
}

static SHOW_GEN: AtomicU64 = AtomicU64::new(0);
/// 最近一次气泡内容：窗口刚创建时 emit 会丢事件，前端加载完主动来拿一次。
static LAST_PAYLOAD: Mutex<Option<BubblePayload>> = Mutex::new(None);
static HOOK_STARTED: AtomicBool = AtomicBool::new(false);

/// 菜单栏菜单句柄：主界面开关变化时要同步「暂停划词 / 开启划词」文案
struct MenuState(Menu<tauri::Wry>);

fn persist_selection(enabled: bool) {
    let mut cfg = config::load();
    cfg.selection = enabled;
    let _ = config::save(&cfg);
}

fn sync_tray_label(app: &AppHandle, enabled: bool) {
    if let Some(state) = app.try_state::<MenuState>() {
        if let Some(MenuItemKind::MenuItem(item)) = state.0.get("toggle") {
            let _ = item.set_text(if enabled { "暂停划词" } else { "开启划词" });
        }
    }
}

#[tauri::command]
async fn ai_explain(word: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || crate::ai::explain(&word))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
fn selection_enabled(shared: tauri::State<'_, Arc<Shared>>) -> bool {
    shared.enabled.load(Ordering::Relaxed)
}

#[tauri::command]
fn set_selection(app: AppHandle, shared: tauri::State<'_, Arc<Shared>>, enabled: bool) -> bool {
    shared.enabled.store(enabled, Ordering::Relaxed);
    persist_selection(enabled);
    sync_tray_label(&app, enabled);
    log::line(format!("selection translation {}", if enabled { "enabled" } else { "disabled" }));
    enabled
}

#[tauri::command]
fn get_config() -> config::Config {
    config::load()
}

#[tauri::command]
fn set_config(mut cfg: config::Config) -> Result<(), String> {
    // 设置窗口不带这个字段，别把「已弹过欢迎」的记录清掉
    if cfg.hello_version.is_empty() {
        cfg.hello_version = config::load().hello_version;
    }
    config::save(&cfg)
}

#[tauri::command]
fn bubble_state() -> Option<BubblePayload> {
    LAST_PAYLOAD.lock().ok().and_then(|guard| guard.clone())
}

#[tauri::command]
fn close_main_window(app: AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.hide();
    }
}

#[derive(serde::Serialize)]
struct TranslateOut {
    source: String,
    text: String,
    detected: String,
    target: String,
    provider: String,
}

/// 主窗口里的「翻译」按钮：同步走一遍翻译链路（在阻塞线程里跑，别卡 UI）。
#[tauri::command]
async fn translate_text(
    text: String,
    target: Option<String>,
    provider: Option<String>,
) -> Result<TranslateOut, String> {
    let mut cfg = config::load();
    if let Some(t) = target.as_ref().filter(|t| !t.is_empty()) {
        cfg.target = t.clone();
    }
    if let Some(p) = provider.as_ref().filter(|p| !p.is_empty()) {
        cfg.provider = p.clone();
    }
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err("empty".into());
    }
    tauri::async_runtime::spawn_blocking(move || translate::run(&text, &cfg))
        .await
        .map_err(|e| e.to_string())?
        .map(|t| {
            history::record(&t);
            TranslateOut {
                source: t.source,
                text: t.text,
                detected: t.detected,
                target: t.target,
                provider: t.provider,
            }
        })
}

/// 查词（英文单词）。失败时前端只显示翻译，不当成错误。
#[tauri::command]
async fn lookup_word(word: String) -> Result<translate::DictEntry, String> {
    tauri::async_runtime::spawn_blocking(move || translate::lookup(&word))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
fn js_log(message: String) {
    log::line(format!("[js] {message}"));
}

#[tauri::command]
fn list_history(limit: usize) -> Vec<history::Entry> {
    history::list(limit)
}

#[tauri::command]
fn clear_history() -> Result<(), String> {
    history::clear()
}

#[tauri::command]
fn add_vocab(
    source: String,
    text: String,
    detected: String,
    target: String,
) -> Result<bool, String> {
    vocab::add(&source, &text, &detected, &target)
}

#[tauri::command]
fn list_vocab(limit: usize) -> Vec<vocab::VocabEntry> {
    vocab::list(limit)
}

#[tauri::command]
fn remove_vocab(ts: u64) -> Result<(), String> {
    vocab::remove(ts)
}

#[tauri::command]
fn clear_vocab() -> Result<(), String> {
    vocab::clear()
}

#[tauri::command]
fn export_vocab_markdown() -> String {
    vocab::export_markdown()
}

#[tauri::command]
fn get_autostart(app: AppHandle) -> bool {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch().is_enabled().unwrap_or(false)
}

#[tauri::command]
fn set_autostart(app: AppHandle, enabled: bool) -> Result<bool, String> {
    use tauri_plugin_autostart::ManagerExt;
    let manager = app.autolaunch();
    let result = if enabled { manager.enable() } else { manager.disable() };
    result.map_err(|e| e.to_string())?;
    Ok(manager.is_enabled().unwrap_or(enabled))
}

fn open_window(app: &AppHandle, label: &str) {
    #[cfg(target_os = "macos")]
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
    if let Some(win) = app.get_webview_window(label) {
        let _ = win.show();
        let _ = win.set_focus();
    }
}

fn show_permission_window(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
    if let Some(win) = app.get_webview_window("permission") {
        let _ = win.show();
        let _ = win.set_focus();
    }
}

/// 打开主窗口（翻译 / 生词本 / 历史 / 设置 四个页签共用一个窗口）
fn open_app(app: &AppHandle, tab: &str) {
    open_window(app, "main");
    if let Some(win) = app.get_webview_window("main") {
        let tab = tab.to_string();
        // 事件通道在隐藏→显示那一瞬间不可靠，直接 eval 切页签
        let _ = win.eval(format!("window.__switchTab && window.__switchTab({})", serde_json::to_string(&tab).unwrap_or_else(|_| "\"translate\"".into())));
    }
}

fn open_settings_window(app: &AppHandle) {
    open_app(app, "translate");
}

/// 用户主动打开应用（双击图标 / 再次启动 / Reopen）时应看到什么：
/// 权限没给全就弹权限窗口，否则弹设置窗口。之前没有这个入口，应用只能靠菜单栏图标操作，
/// 用户会以为「打不开、卡死在后台」。
fn open_from_user(app: &AppHandle) {
    let trusted = permissions::is_trusted();
    let listening = permissions::input_monitoring();
    if !trusted || listening == Some(false) {
        show_permission_window(app);
    } else {
        open_settings_window(app);
    }
}

struct Shared {
    enabled: AtomicBool,
    last: Mutex<String>,
    last_at: Mutex<Instant>,
}

/// 逻辑点 → 物理像素（含该显示器原点偏移）。
fn logical_to_physical(lx: i32, ly: i32, origin: PhysicalPosition<i32>, scale: f64) -> (i32, i32) {
    let ox = origin.x as f64 / scale;
    let oy = origin.y as f64 / scale;
    (
        origin.x + ((lx as f64 - ox) * scale) as i32,
        origin.y + ((ly as f64 - oy) * scale) as i32,
    )
}

/// 光标（逻辑点）→ 物理像素，按它所在显示器换算。
fn cursor_physical(app: &AppHandle, lx: i32, ly: i32) -> (i32, i32) {
    match pick_monitor(app, lx, ly) {
        Some(m) => logical_to_physical(lx, ly, *m.position(), m.scale_factor()),
        None => (lx, ly),
    }
}

/// 松开鼠标的点是否落在我们自己的窗口里（那样不该把它当成「划词」）
/// 这个窗口是不是在「当前屏幕空间」上？主窗口可能停在别的桌面/全屏空间里，
/// 那种情况下它的矩形还会盖住选区坐标，不能拿它当「点到自己窗口」。
#[cfg(target_os = "macos")]
fn on_active_space(win: &tauri::WebviewWindow) -> bool {
    use std::sync::mpsc;
    let w = win.clone();
    let (tx, rx) = mpsc::channel();
    let handle = win.app_handle().clone();
    let _ = handle.run_on_main_thread(move || {
        let res = match w.ns_window() {
            Ok(ptr) => unsafe {
                use objc::runtime::Object;
                use objc::{msg_send, sel, sel_impl};
                let ns = ptr as *mut Object;
                let v: bool = msg_send![ns, isOnActiveSpace];
                v
            },
            Err(_) => true,
        };
        let _ = tx.send(res);
    });
    // 拿不到就当它在当前空间（宁可少触发，也别误判成别处）
    rx.recv_timeout(std::time::Duration::from_millis(150)).unwrap_or(true)
}

#[cfg(not(target_os = "macos"))]
fn on_active_space(_win: &tauri::WebviewWindow) -> bool {
    true
}

fn inside_own_window(app: &AppHandle, lx: i32, ly: i32) -> bool {
    let (px, py) = cursor_physical(app, lx, ly);
    for label in ["bubble", "main", "permission"] {
        let Some(win) = app.get_webview_window(label) else { continue };
        if !win.is_visible().unwrap_or(false) {
            continue;
        }
        if !on_active_space(&win) {
            continue;
        }
        if let (Ok(pos), Ok(size)) = (win.outer_position(), win.inner_size()) {
            let inside = px >= pos.x
                && px <= pos.x + size.width as i32
                && py >= pos.y
                && py <= pos.y + size.height as i32;
            if inside {
                log::line(format!(
                    "hit own window {label} at {:?} size {:?} (point {px},{py})",
                    pos, size
                ));
                return true;
            }
        }
    }
    false
}

/// 光标坐标（enigo）是逻辑点，Tauri 的窗口位置是物理像素；
/// 多显示器/Retina 下直接混用会让气泡跑到屏幕左上角，这里按所在显示器换算。
fn pick_monitor(app: &AppHandle, lx: i32, ly: i32) -> Option<tauri::Monitor> {
    let monitors = app.available_monitors().unwrap_or_default();
    for m in &monitors {
        let s = m.scale_factor();
        let pos = m.position();
        let size = m.size();
        let x0 = pos.x as f64 / s;
        let y0 = pos.y as f64 / s;
        let x1 = x0 + size.width as f64 / s;
        let y1 = y0 + size.height as f64 / s;
        if (lx as f64) >= x0 && (lx as f64) < x1 && (ly as f64) >= y0 && (ly as f64) < y1 {
            return Some(m.clone());
        }
    }
    monitors.into_iter().next()
}

/// 最近一次鼠标位置（逻辑点，左上角原点）。由全局钩子持续刷新。
static LAST_CURSOR: std::sync::Mutex<Option<(f64, f64)>> = std::sync::Mutex::new(None);

fn note_cursor(x: f64, y: f64) {
    if let Ok(mut c) = LAST_CURSOR.lock() {
        *c = Some((x, y));
    }
}

fn last_cursor() -> (i32, i32) {
    LAST_CURSOR
        .lock()
        .ok()
        .and_then(|c| *c)
        .map(|(x, y)| (x as i32, y as i32))
        .unwrap_or((600, 300))
}

/// 读剪贴板文本（菜单「翻译剪贴板」用）
fn clipboard_text() -> Option<String> {
    let mut cb = arboard::Clipboard::new().ok()?;
    let text = cb.get_text().ok()?;
    let trimmed = text.trim().to_string();
    if trimmed.is_empty() { None } else { Some(trimmed) }
}

/// 在鼠标附近翻译一段给定文本（不依赖划词）
fn translate_given(app: &AppHandle, text: String) {
    // macOS 助手可用时走新流程（小图标/可编辑面板都在助手那边）；
    // 其它平台继续用 webview 气泡。
    #[cfg(target_os = "macos")]
    if crate::bubble_helper::ensure_running() {
        crate::translate_flow::translate_at_cursor(app, &text);
        return;
    }
    let (lx, ly) = last_cursor();
    show_bubble(app, lx, ly, BubblePayload {
        source: text.chars().take(90).collect(),
        text: String::new(),
        detected: String::new(),
        target: String::new(),
        provider: String::new(),
        loading: true,
        error: None,
        card: None,
    });
    let app = app.clone();
    thread::spawn(move || {
        let cfg = config::load();
        match translate::run(&text, &cfg) {
            Ok(t) => {
                history::record(&t);
                show_bubble(&app, lx, ly, BubblePayload {
                    source: t.source.chars().take(90).collect(),
                    text: t.text.clone(),
                    detected: t.detected.clone(),
                    target: t.target.clone(),
                    provider: t.provider.clone(),
                    loading: false,
                    error: None,
                    card: None,
                });
            }
            Err(e) => {
                show_bubble(&app, lx, ly, BubblePayload {
                    source: text.chars().take(90).collect(),
                    text: String::new(),
                    detected: String::new(),
                    target: String::new(),
                    provider: String::new(),
                    loading: false,
                    error: Some(e),
                    card: None,
                });
            }
        }
    });
}


/// 气泡要跟在「当前所在的空间」上：桌面、别的桌面、以及 Safari 这类全屏应用的空间。
/// macOS 上用 NSWindowCollectionBehaviorCanJoinAllSpaces + FullScreenAuxiliary + Stationary，
/// 并把层级提到 NSStatusWindowLevel，才不会被全屏应用盖住。
///
/// 注意：所有 AppKit 调用都必须在主线程做。之前直接在 worker 线程 msg_send，
/// 触发 `NSWMWindowCoordinator performTransactionUsingBlock` 里的 SIGTRAP（EXC_BREAKPOINT）。
#[cfg(target_os = "macos")]
fn float_over_every_space(win: &tauri::WebviewWindow) {
    let w = win.clone();
    let handle = win.app_handle().clone();
    if let Err(e) = handle.run_on_main_thread(move || unsafe {
        use objc::runtime::Object;
        use objc::{msg_send, sel, sel_impl};

        // 实验：把窗口类换成 NSPanel（非激活面板），全屏空间里才肯浮出来。
        // 只在 SUIYI_PANEL_TRICK=1 时启用，验证通过再默认打开。
        if std::env::var("SUIYI_PANEL_TRICK").as_deref() == Ok("1") {
            extern "C" {
                fn objc_getClass(name: *const std::os::raw::c_char) -> *mut std::ffi::c_void;
                fn object_setClass(
                    obj: *mut std::ffi::c_void,
                    cls: *mut std::ffi::c_void,
                ) -> *mut std::ffi::c_void;
            }
            if let Ok(ptr) = w.ns_window() {
                let name = b"NSPanel\0";
                unsafe {
                    let cls = objc_getClass(name.as_ptr() as *const _);
                    if !cls.is_null() {
                        object_setClass(ptr, cls);
                        log::line("bubble switched to NSPanel");
                    } else {
                        log::line("NSPanel class not found");
                    }
                }
            }
        }
        // 注意：不要用 tao 的 set_visible_on_all_workspaces —— 它会把 collectionBehavior
        // 覆盖成只有 canJoinAllSpaces，丢掉 fullScreenAuxiliary，全屏空间里就看不见了。
        let _ = &w;
        match w.ns_window() {
            Ok(ptr) => {
                let ns = ptr as *mut Object;
                // 1 = CanJoinAllSpaces, 16 = Stationary, 256 = FullScreenAuxiliary
                let behavior: u64 = (1u64 << 0) | (1u64 << 4) | (1u64 << 8);
                let _: () = msg_send![ns, setCollectionBehavior: behavior];
                // 默认 NSPopUpMenuWindowLevel(101)：全屏应用之上要浮得住
                let level: i64 = std::env::var("SUIYI_BUBBLE_LEVEL")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(101);
                let _: () = msg_send![ns, setLevel: level];
                // 别人全屏时不要自己隐藏
                let _: () = msg_send![ns, setHidesOnDeactivate: false];
                // 主线程上调 order front（修掉之前 worker 线程 msg_send 的 SIGTRAP）
                let _: () = msg_send![ns, orderFrontRegardless];
                if std::env::var("SUIYI_ACTIVATE").as_deref() == Ok("1") {
                    let nsapp: *mut Object = msg_send![objc::class!(NSApplication), sharedApplication];
                    let _: () = msg_send![nsapp, activateIgnoringOtherApps: true];
                    let _: () = msg_send![ns, makeKeyAndOrderFront: std::ptr::null_mut::<Object>()];
                    log::line("bubble: NSApp activated + makeKeyAndOrderFront");
                }
                let got_behavior: u64 = msg_send![ns, collectionBehavior];
                let got_level: i64 = msg_send![ns, level];
                let on_space: bool = msg_send![ns, isOnActiveSpace];
                log::line(format!(
                    "bubble overlay: behavior=0x{got_behavior:x} level={got_level} onActiveSpace={on_space}"
                ));
                // 1 秒后再看一次：tao/Tauri 有时会在 show 之后异步把 collectionBehavior 改回去
                let w2 = w.clone();
                let h2 = w.app_handle().clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(1000));
                    let _ = h2.run_on_main_thread(move || unsafe {
                        use objc::runtime::Object;
                        use objc::{msg_send, sel, sel_impl};
                        if let Ok(ptr) = w2.ns_window() {
                            let ns = ptr as *mut Object;
                            let b: u64 = msg_send![ns, collectionBehavior];
                            let l: i64 = msg_send![ns, level];
                            let a: bool = msg_send![ns, isOnActiveSpace];
                            let v: bool = msg_send![ns, isVisible];
                            log::line(format!(
                                "bubble overlay +1s: behavior=0x{b:x} level={l} onActiveSpace={a} visible={v}"
                            ));
                            let want: u64 = (1u64 << 0) | (1u64 << 4) | (1u64 << 8);
                            if b != want {
                                let _: () = msg_send![ns, setCollectionBehavior: want];
                                let _: () = msg_send![ns, orderFrontRegardless];
                                log::line("bubble overlay re-applied");
                            }
                        }
                    });
                });
            }
            Err(e) => log::line(format!("bubble ns_window unavailable: {e}")),
        }
    }) {
        log::line(format!("bubble overlay dispatch failed: {e}"));
    }
}


#[cfg(target_os = "macos")]
static OVERLAY_PANEL: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// 把气泡的 contentView 搬进一个真正的 NSPanel。
/// 有道/欧路这类划词工具能盖在全屏应用上，靠的就是「非激活面板 + 跨空间」这两个属性，
/// 而这些属性必须在窗口**创建时**由 styleMask 决定，普通 NSWindow 事后改 collectionBehavior 不够。
#[cfg(target_os = "macos")]
unsafe fn overlay_panel(win: &tauri::WebviewWindow, x: i32, y: i32, w: i32, h: i32) -> bool {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};
    #[repr(C)]
    struct Pt {
        x: f64,
        y: f64,
    }
    #[repr(C)]
    struct Sz {
        w: f64,
        h: f64,
    }
    #[repr(C)]
    struct Rc {
        origin: Pt,
        size: Sz,
    }
    let Ok(ptr) = win.ns_window() else {
        return false;
    };
    let ns = ptr as *mut Object;
    let view: *mut Object = msg_send![ns, contentView];
    if view.is_null() {
        return false;
    }
    let mut panel = OVERLAY_PANEL.load(std::sync::atomic::Ordering::Relaxed) as *mut Object;
    if panel.is_null() {
        let rect = Rc {
            origin: Pt { x: x as f64, y: y as f64 },
            size: Sz {
                w: w as f64,
                h: h as f64,
            },
        };
        // 1<<7 NonactivatingPanel, 1<<0 Borderless
        let mask: u64 = (1u64 << 7) | 1u64;
        let alloc: *mut Object = msg_send![class!(NSPanel), alloc];
        panel = msg_send![alloc, initWithContentRect: rect styleMask: mask backing: 2u64 defer: false];
        if panel.is_null() {
            return false;
        }
        let _: () = msg_send![panel, setFloatingPanel: true];
        let _: () = msg_send![panel, setBecomesKeyOnlyIfNeeded: true];
        let _: () = msg_send![panel, setLevel: 101i64];
        let _: () = msg_send![panel, setCollectionBehavior: 0x111u64];
        let _: () = msg_send![panel, setHidesOnDeactivate: false];
        let _: () = msg_send![panel, setOpaque: false];
        let clear: *mut Object = msg_send![class!(NSColor), clearColor];
        let _: () = msg_send![panel, setBackgroundColor: clear];
        let _: () = msg_send![panel, setContentView: view];
        OVERLAY_PANEL.store(panel as usize, std::sync::atomic::Ordering::Relaxed);
        let _ = win.hide();
        log::line("bubble transplanted into NSPanel");
        let _: () = msg_send![panel, orderFrontRegardless];
        return true;
    }
    let rect = Rc {
        origin: Pt { x: x as f64, y: y as f64 },
        size: Sz {
            w: w as f64,
            h: h as f64,
        },
    };
    let _: () = msg_send![panel, setFrame: rect display: true];
    let _: () = msg_send![panel, orderFrontRegardless];
    let on_space: bool = msg_send![panel, isOnActiveSpace];
    let vis: bool = msg_send![panel, isVisible];
    log::line(format!("overlay panel frame=({x},{y},{w},{h}) onActiveSpace={on_space} visible={vis}"));
    true
}

#[cfg(not(target_os = "macos"))]
fn float_over_every_space(_win: &tauri::WebviewWindow) {}

fn show_bubble(app: &AppHandle, x: i32, y: i32, payload: BubblePayload) -> u64 {
    // macOS：气泡完全交给 NSPanel 助手（它才能盖在别的 App 的全屏空间上）。
    // webview 气泡在 macOS 上已经退役——只在助手不可用时兜底，避免出现两个气泡。
    #[cfg(target_os = "macos")]
    if std::env::var("SUIYI_FORCE_WEBVIEW").as_deref() != Ok("1") && bubble_helper::ensure_running() {
        let _ = (x, y, &payload);
        return 0;
    }
    let _ = app;
    // 实验：按需新建窗口。旧窗口固化在它创建时所在的空间，切到全屏空间后
    // canJoinAllSpaces 也不一定把它挪过来；新建的窗口会落在当前活动空间。
    if std::env::var("SUIYI_FRESH_BUBBLE").as_deref() == Ok("1") {
        if let Some(old) = app.get_webview_window("bubble") {
            let _ = old.destroy();
        }
        if let Ok(w) = tauri::WebviewWindowBuilder::new(app, "bubble", tauri::WebviewUrl::App("bubble.html".into()))
            .inner_size(380.0, 200.0)
            .decorations(false)
            .transparent(true)
            .always_on_top(true)
            .skip_taskbar(true)
            .focused(false)
            .visible(false)
            .build()
        {
            log::line("fresh bubble window built");
            return show_bubble_in(app, &w, x, y, payload);
        }
        log::line("fresh bubble build failed");
    }
    let Some(win) = app.get_webview_window("bubble") else {
        return 0;
    };
    show_bubble_in(app, &win, x, y, payload)
}

fn show_bubble_in(app: &AppHandle, win: &tauri::WebviewWindow, x: i32, y: i32, payload: BubblePayload) -> u64 {
    let win = win.clone();
    float_over_every_space(&win);
    let width = 380.0;
    let word = payload.card.as_ref().map(|c| c.mode == "word").unwrap_or(false);
    let has_ai = payload.card.as_ref().map(|c| c.ai.is_some()).unwrap_or(false);
    let height = if payload.loading {
        120.0
    } else if word {
        if has_ai { 460.0 } else { 340.0 }
    } else {
        200.0
    };

    let monitor = pick_monitor(app, x, y);
    let scale = monitor.as_ref().map(|m| m.scale_factor()).unwrap_or(1.0);
    let origin = monitor.as_ref().map(|m| *m.position()).unwrap_or(PhysicalPosition::new(0, 0));

    let (mut px, mut py) = logical_to_physical(x, y + 18, origin, scale);

    if let Some(m) = monitor.as_ref() {
        let size = m.size();
        let pos = m.position();
        let max_x = pos.x + size.width as i32 - (width * scale) as i32 - 8;
        let max_y = pos.y + size.height as i32 - (height * scale) as i32 - 8;
        px = px.min(max_x).max(pos.x + 8);
        py = py.min(max_y).max(pos.y + 8);
    }

    log::line(format!(
        "bubble at ({px},{py}) requested ({x},{y}) size {width}x{height} loading={}",
        payload.loading
    ));
    if let Ok(mut guard) = LAST_PAYLOAD.lock() {
        *guard = Some(payload.clone());
    }
    let _ = win.set_position(PhysicalPosition::new(px, py));
    // 事件通道偶发丢包（隐藏窗口刚 show 时 webview 还没挂好监听），
    // 这里直接 eval 调用页面里的全局渲染函数，保证一定能画出来。
    match serde_json::to_string(&payload) {
        Ok(json) => {
            let js = format!("window.__suiyiRender && window.__suiyiRender({json})");
            match win.eval(&js) {
                Ok(()) => {}
                Err(e) => log::line(format!("bubble eval failed: {e}")),
            }
        }
        Err(e) => log::line(format!("bubble payload encode failed: {e}")),
    }
    let _ = win.emit("suiyi://payload", payload);
    #[cfg(target_os = "macos")]
    if std::env::var("SUIYI_PANEL_TRANSPLANT").as_deref() == Ok("1") {
        let app2 = app.clone();
        let w2 = win.clone();
        let _ = app.run_on_main_thread(move || unsafe {
            use objc::runtime::Object;
            use objc::{msg_send, sel, sel_impl};
            if let Ok(ptr) = w2.ns_window() {
                let ns = ptr as *mut Object;
                // 先让 webview 完成首帧，再把 contentView 搬进面板
                let _: () = msg_send![ns, orderFrontRegardless];
            }
            overlay_panel(&w2, px, py, width as i32, height as i32);
        });
        let _ = app2;
        return SHOW_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    }
    let _ = win.show();
    // show() 之后 tao 可能又把层级/行为改回去，这里再确认一次（主线程、只置前不抢焦点）
    float_over_every_space(&win);
    let gen = SHOW_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    gen
}

/// 只有拿到权限才真正挂全局钩子；没有权限时挂上也收不到事件，白白吓人。
fn ensure_hook(app: AppHandle, shared: Arc<Shared>) {
    if !permissions::is_trusted() {
        log::line("skip hook — accessibility not granted yet");
        return;
    }
    if HOOK_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    log::line("starting global mouse hook");
    start_hook(app, shared);
}

fn start_hook(app: AppHandle, shared: Arc<Shared>) {
    thread::spawn(move || {
        let trace = std::env::var("SUIYI_TRACE").is_ok();
        let handler_app = app.clone();
        let handler_shared = shared.clone();
        let pressed: std::cell::Cell<Option<(f64, f64, Instant)>> = std::cell::Cell::new(None);
        // 连续点击计数：双击选词、三击选段落也能触发（很多人根本不拖拽）
        let last_up: std::cell::Cell<Option<(Instant, f64, f64, u8)>> = std::cell::Cell::new(None);

        let result = hook::listen(move |ev| {
            if trace {
                log::line(format!("ev {:?}", ev));
            }
            match ev {
                hook::MouseEvent::Down(x, y) => {
                    note_cursor(x, y);
                    pressed.set(Some((x, y, Instant::now())));
                    hide_bubble_if_outside(&handler_app, x, y);
                }
                hook::MouseEvent::Dragged(x, y) => note_cursor(x, y),
                hook::MouseEvent::Up(x, y) => {
                    let Some((sx, sy, at)) = pressed.take() else { return };
                    let clicks = match last_up.take() {
                        Some((t, px, py, n))
                            if t.elapsed() < Duration::from_millis(420)
                                && (x - px).abs() < 8.0
                                && (y - py).abs() < 8.0 =>
                        {
                            (n + 1).min(3)
                        }
                        _ => 1,
                    };
                    last_up.set(Some((Instant::now(), x, y, clicks)));
                    on_release(&handler_app, &handler_shared, sx, sy, at, x, y, clicks);
                }
            }
        });

        if let Err(e) = result {
            // 监听鼠标需要「输入监控」权限；没有时 CGEventTap 建不起来。
            // 把 HOOK_STARTED 复位，watchdog 会在用户勾选权限后自动重启钩子（无需重开 App）。
            HOOK_STARTED.store(false, Ordering::SeqCst);
            let hint = match permissions::input_monitoring() {
                Some(false) => "（鼠标监听需要「输入监控」权限，请在 系统设置 → 隐私与安全性 → 输入监控 里勾选随译）",
                _ => "（稍后自动重试）",
            };
            log::line(format!("input hook failed: {e:?} {hint}"));
            thread::sleep(Duration::from_secs(2));
        }
    });
}

/// 松开鼠标：判断是否为「拖动选择」，然后抓选区、翻译、弹气泡。
fn on_release(
    app: &AppHandle,
    shared: &Arc<Shared>,
    sx: f64,
    sy: f64,
    at: Instant,
    x: f64,
    y: f64,
    clicks: u8,
) {
    if !shared.enabled.load(Ordering::Relaxed) {
        return;
    }
    let dx = x - sx;
    let dy = y - sy;
    let dragged = (dx * dx + dy * dy).sqrt() >= 7.0;
    log::line(format!("release dragged={dragged} clicks={clicks} at ({x:.0},{y:.0})"));
    // 拖动 = 拖选；双击/三击 = 选词/选段落（这两类人很常见，之前完全没接）
    let picked = dragged || clicks >= 2;
    if !picked || at.elapsed() > Duration::from_secs(6) {
        return;
    }
    if inside_own_window(app, x as i32, y as i32) {
        log::line("release inside own window — skip");
        return;
    }
    let (bx, by) = (x as i32, y as i32);
    let app = app.clone();
    let shared = shared.clone();
    thread::spawn(move || {
        // 给目标应用一点时间把「点击 → 选中」和窗口激活处理完，再发 ⌘C
        thread::sleep(Duration::from_millis(120));
        log::line("capturing selection…");
        let Some(text) = capture::select_text() else {
            log::line("no selection captured");
            if !permissions::is_trusted() {
                show_bubble(
                    &app,
                    bx,
                    by,
                    BubblePayload {
                        source: String::new(),
                        text: String::new(),
                        detected: String::new(),
                        target: String::new(),
                        provider: String::new(),
                        loading: false,
                        error: Some("需要「辅助功能」权限才能读取选区：系统设置 → 隐私与安全性 → 辅助功能 → 勾选随译".into()),
                        card: None,
                    },
                );
            }
            return;
        };
        if text.trim().is_empty() {
            return;
        }
        {
            let mut last = shared.last.lock().unwrap();
            let mut last_at = shared.last_at.lock().unwrap();
            if *last == text && last_at.elapsed() < Duration::from_secs(2) {
                return;
            }
            *last = text.clone();
            *last_at = Instant::now();
        }
        // macOS：先只在选区旁冒一个小图标；点它 / 悬停才真正翻译（有道的手感）。
        #[cfg(target_os = "macos")]
        if crate::bubble_helper::show_icon(&text, x, y) {
            log::line(format!("bubble icon shown ({} chars)", text.chars().count()));
            return;
        }
        let cfg = config::load();
        show_bubble(
            &app,
            bx,
            by,
            BubblePayload {
                source: text.clone(),
                text: String::new(),
                detected: String::new(),
                target: cfg.target.clone(),
                provider: cfg.provider.clone(),
                loading: true,
                error: None,
                card: None,
            },
        );
        let payload = match translate::run(&text, &cfg) {
            Ok(t) => {
                history::record(&t);
                BubblePayload {
                    source: t.source,
                    text: t.text,
                    detected: t.detected,
                    target: t.target,
                    provider: t.provider,
                    loading: false,
                    error: None,
                    card: None,
                }
            }
            Err(e) => BubblePayload {
                source: text,
                text: String::new(),
                detected: String::new(),
                target: cfg.target.clone(),
                provider: cfg.provider.clone(),
                loading: false,
                error: Some(e),
                card: None,
            },
        };
        let gen = show_bubble(&app, bx, by, payload);
        let app_hide = app.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_secs(25));
            if SHOW_GEN.load(Ordering::SeqCst) == gen {
                if let Some(win) = app_hide.get_webview_window("bubble") {
                    let _ = win.hide();
                }
            }
        });
    });
}

/// 点击落在气泡外面时把气泡藏掉。
fn hide_bubble_if_outside(app: &AppHandle, x: f64, y: f64) {
    if let Some(win) = app.get_webview_window("bubble") {
        if win.is_visible().unwrap_or(false) {
            if let (Ok(pos), Ok(size)) = (win.outer_position(), win.inner_size()) {
                let (px, py) = cursor_physical(app, x as i32, y as i32);
                let inside = px >= pos.x
                    && px <= pos.x + size.width as i32
                    && py >= pos.y
                    && py <= pos.y + size.height as i32;
                if !inside {
                    let _ = win.hide();
                }
            }
        }
    }
}

/// 从划词卡片点「查词」：打开主窗口并把这个词填进输入框
pub fn open_main_with_word(app: &AppHandle, word: &str) {
    open_app(app, "translate");
    let payload = serde_json::to_string(word).unwrap_or_else(|_| "\"\"".into());
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.eval(&format!("window.__setSource && window.__setSource({payload})"));
    }
}

#[tauri::command]
fn open_permission_settings() {
    permissions::open_settings();
}

#[tauri::command]
fn check_permission() -> bool {
    permissions::is_trusted()
}

#[tauri::command]
fn open_input_settings() {
    permissions::open_input_monitoring();
}

#[tauri::command]
fn permission_state() -> serde_json::Value {
    serde_json::json!({
        "accessibility": permissions::is_trusted(),
        "inputMonitoring": permissions::input_monitoring(),
    })
}

#[tauri::command]
fn close_permission_window(app: AppHandle) {
    if let Some(win) = app.get_webview_window("permission") {
        let _ = win.hide();
    }
    // 不要退回 Accessory：那会让 Dock 图标消失，用户就找不到入口了。
    // 只有 macOS 需要「授权完成后回到主窗口」；Windows 没有权限流程
    // （is_trusted 恒 true、input_monitoring 恒 None），permission.html 的
    // 轮询会在启动 1.5s 后立刻走进这里——若无条件 show()，主窗口会自己弹出来，
    // 和 setup 里「launched — main window kept closed」的预期相反。
    #[cfg(target_os = "macos")]
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.show();
        let _ = win.set_focus();
    }
}

fn main() {
    // 无界面自检：SUIYI_PROBE=1 ./suiyi
    if let Ok(mode) = std::env::var("SUIYI_PROBE") {
        if mode == "autostart" {
            // 交给 setup 里的自检分支处理（需要 AppHandle）
        } else if mode == "icon" {
            // 交给 setup 里的自检分支处理（直接让助手弹出划词图标）
        } else if mode == "bubble" {
            // 交给 setup 里的自检分支处理（需要拿到窗口）
        } else if mode == "vocab" {
            let added = vocab::add("hello world", "你好，世界", "en", "zh-CN");
            println!("add -> {added:?}");
            let again = vocab::add("hello world", "你好，世界", "en", "zh-CN");
            println!("add duplicate -> {again:?}");
            for e in vocab::list(5) {
                println!("- {} => {} ({})", e.source, e.text, e.target);
            }
            println!("markdown head:");
            for line in vocab::export_markdown().lines().take(3) {
                println!("{line}");
            }
            return;
        }
        if mode == "dict" {
            let word = std::env::var("SUIYI_PROBE_WORD").unwrap_or_else(|_| "action".into());
            match translate::zh_lookup(&word) {
                Some(z) => println!("zh {}", serde_json::to_string(&z).unwrap_or_default()),
                None => match translate::lookup(&word) {
                    Ok(d) => println!("dict {}", serde_json::to_string(&d).unwrap_or_default()),
                    Err(e) => println!("ERR {e}"),
                },
            }
            return;
        }
        if mode == "clip" {
            match clipboard_text() {
                Some(text) => {
                    println!("clipboard {} chars", text.chars().count());
                    let cfg = config::load();
                    match translate::run(&text, &cfg) {
                        Ok(t) => {
                            history::record(&t);
                            println!("provider={} → {}", t.provider, t.text.chars().take(80).collect::<String>());
                        }
                        Err(e) => println!("ERR {e}"),
                    }
                }
                None => println!("clipboard empty"),
            }
            return;
        }
        if mode == "pipeline" {
            // 端到端（不含鼠标钩子）：抓选区 → 翻译 → 写历史
            println!("trusted={}", permissions::is_trusted());
            let started = Instant::now();
            match capture::select_text() {
                Some(text) => {
                    println!("captured {} chars: {}", text.chars().count(), text.chars().take(60).collect::<String>());
                    let cfg = config::load();
                    match translate::run(&text, &cfg) {
                        Ok(t) => {
                            history::record(&t);
                            println!("provider={} detected={} target={}", t.provider, t.detected, t.target);
                            println!("text: {}", t.text.chars().take(80).collect::<String>());
                        }
                        Err(e) => println!("translate ERR {e}"),
                    }
                }
                None => println!("capture FAILED (no selection?)"),
            }
            println!("elapsed {:?}", started.elapsed());
            return;
        }
        if mode == "fallback" {
            match translate::my_memory("Hello world, this is the offline fallback line.", "zh-CN") {
                Ok(t) => println!("OK {} → {}", t.provider, t.text),
                Err(e) => println!("ERR {e}"),
            }
            return;
        }
        if mode == "capture" {
            println!("trusted={}", permissions::is_trusted());
            println!("{}", capture::ax_probe());
            match capture::select_text() {
                Some(t) => println!(
                    "CAPTURED {} chars: {}",
                    t.chars().count(),
                    t.chars().take(120).collect::<String>()
                ),
                None => println!("CAPTURE FAILED (no selection or no permission)"),
            }
            return;
        }
        if mode == "history" {
            for e in history::list(20) {
                println!("{} [{}] {} => {}", e.ts, e.provider, e.source.chars().take(40).collect::<String>(), e.text.chars().take(40).collect::<String>());
            }
            return;
        }
        if mode != "autostart" && mode != "bubble" && mode != "bubbleshow" && mode != "icon" {
            let cfg = config::load();
            println!("provider={} target={}", cfg.provider, cfg.target);
            match translate::run("Hello world, this is a quick probe.", &cfg) {
                Ok(t) => println!("OK {} → {}", t.provider, t.text),
                Err(e) => println!("ERR {e}"),
            }
            return;
        }
    }

    let shared = Arc::new(Shared {
        enabled: AtomicBool::new(config::load().selection),
        last: Mutex::new(String::new()),
        last_at: Mutex::new(Instant::now() - Duration::from_secs(60)),
    });

    let shared_setup = shared.clone();

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            open_from_user(app);
        }))
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--autostart"]),
        ))
        .setup(move |app| {
            // 自检：SUIYI_PROBE=autostart ./suiyi
            if std::env::var("SUIYI_PROBE").as_deref() == Ok("autostart") {
                use tauri_plugin_autostart::ManagerExt;
                let manager = app.autolaunch();
                println!("before -> {:?}", manager.is_enabled());
                println!("enable -> {:?}", manager.enable());
                println!("after  -> {:?}", manager.is_enabled());
                println!("disable -> {:?}", manager.disable());
                println!("final  -> {:?}", manager.is_enabled());
                return Ok(());
            }
            // 之前这里设成 Accessory（无 Dock 图标），菜单栏图标一隐藏，用户就完全找不到入口，
            // 表现成「打不开、卡在后台」。改成普通应用：有 Dock 图标，关窗口只是隐藏。
            // 内置离线词库（resources/ecdict.sqlite），查不到在线释义时兜底
            if let Ok(dir) = app.path().resource_dir() {
                dict_local::init(dir);
            }

            // 命令里用 State<Arc<Shared>> 读开关状态，必须先托管
            app.manage(shared_setup.clone());
            bubble_helper::start_action_watcher(app.handle().clone());
            bubble_helper::ensure_running();

            // 自检：SUIYI_PROBE=icon ./suiyi —— 直接让助手弹一个单词卡片的小图标，
            // 方便在没人手动划词时验证卡片交互（不依赖真实选区）。
            #[cfg(target_os = "macos")]
            if std::env::var("SUIYI_PROBE").as_deref() == Ok("icon") {
                let word = std::env::var("SUIYI_PROBE_WORD").unwrap_or_else(|_| "resilient".into());
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(800));
                    if bubble_helper::show_icon(&word, 640.0, 360.0) {
                        log::line(format!("probe icon shown for {word}"));
                    } else {
                        log::line("probe icon failed");
                    }
                });
            }

            // macOS 上 webview 气泡已退役（改用助手进程），这里不再给它设置浮层行为，
            // 否则那套「+1s 重新应用 + orderFrontRegardless」会把隐藏的气泡窗口顶上来。
            #[cfg(not(target_os = "macos"))]
            if let Some(bubble) = app.get_webview_window("bubble") {
                float_over_every_space(&bubble);
                if std::env::var("SUIYI_PROBE").as_deref() == Ok("bubbleshow") {
                    let h = app.handle().clone();
                    std::thread::spawn(move || {
                        let mut n = 0;
                        loop {
                            std::thread::sleep(std::time::Duration::from_secs(5));
                            n += 1;
                            let _ = show_bubble(&h, 700, 400, BubblePayload {
                                source: "自检".into(),
                                text: format!("probe #{n}"),
                                detected: "en".into(),
                                target: "zh-CN".into(),
                                provider: "probe".into(),
                                loading: false,
                                error: None,
                                card: None,
                            });
                            log::line(format!("probe bubble #{n} shown"));
                        }
                    });
                } else if std::env::var("SUIYI_PROBE").as_deref() == Ok("bubble") {
                    #[cfg(target_os = "macos")]
                    unsafe {
                        use objc::runtime::Object;
                        use objc::{msg_send, sel, sel_impl};
                        let ns = bubble.ns_window().unwrap() as *mut Object;
                        let behavior: u64 = msg_send![ns, collectionBehavior];
                        let level: i64 = msg_send![ns, level];
                        println!(
                            "collectionBehavior=0x{:x} (canJoinAllSpaces={} fullScreenAuxiliary={}) level={}",
                            behavior,
                            behavior & 1 != 0,
                            behavior & 256 != 0,
                            level
                        );
                    }
                    std::process::exit(0);
                }
            }

            #[cfg(target_os = "macos")]
            {
                let policy = std::env::var("SUIYI_POLICY").unwrap_or_default();
                if policy == "accessory" {
                    app.set_activation_policy(tauri::ActivationPolicy::Accessory);
                    log::line("activation policy forced to accessory (probe)");
                } else {
                    app.set_activation_policy(tauri::ActivationPolicy::Regular);
                }
            }

            // 自动登录启动时不弹窗，手动打开时直接把设置窗口摆出来
            let launched_by_autostart = std::env::args().any(|a| a == "--autostart" || a == "--hidden");

            let handle = app.handle().clone();
            let tray_handle = handle.clone();
            let toggle_shared = shared_setup.clone();

            let selection_on = config::load().selection;
            let toggle = MenuItem::with_id(
                app,
                "toggle",
                if selection_on { "暂停划词" } else { "开启划词" },
                true,
                None::<&str>,
            )?;
            let settings_item = MenuItem::with_id(app, "settings", "设置…", true, None::<&str>)?;
            let history_item = MenuItem::with_id(app, "history", "历史记录…", true, None::<&str>)?;
            let clip_item = MenuItem::with_id(app, "clip", "翻译剪贴板", true, None::<&str>)?;
            let perm = MenuItem::with_id(app, "perm", "辅助功能权限…", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "退出随译", true, None::<&str>)?;
            let menu = Menu::with_items(
                app,
                &[&toggle, &clip_item, &settings_item, &history_item, &perm, &quit],
            )?;
            // 主界面的开关要能同步菜单栏文案，所以把菜单托管起来
            app.manage(MenuState(menu.clone()));

            // 菜单栏图标必须看得见：bundle 图标偶尔取不到（default_window_icon 为 None 时
            // NSStatusItem 会变成一条看不见的空白），所以额外挂文字标题兜底。
            let mut tray = TrayIconBuilder::new().tooltip("随译 · 选中即翻译");
            match app.default_window_icon() {
                Some(icon) => {
                    log::line(format!("tray icon from bundle: {}x{}", icon.width(), icon.height()));
                    tray = tray.icon(icon.clone());
                }
                None => log::line("tray icon missing — relying on the title label"),
            }
            let tray_icon = tray
                .menu(&menu)
                .show_menu_on_left_click(true)
                .on_menu_event(move |_app, event| match event.id.as_ref() {
                    "quit" => std::process::exit(0),
                    "perm" => permissions::open_settings(),
                    "settings" => open_app(_app, "settings"),
                    "history" => open_app(_app, "history"),
                    "clip" => match clipboard_text() {
                        Some(text) => translate_given(_app, text),
                        None => { let _ = show_bubble(_app, 600, 300, BubblePayload {
                            source: "剪贴板".into(),
                            text: String::new(),
                            detected: String::new(),
                            target: String::new(),
                            provider: String::new(),
                            loading: false,
                            error: Some("剪贴板里没有可翻译的文字".into()),
                            card: None,
                        }); }
                    },
                    "toggle" => {
                        let now = !toggle_shared.enabled.load(Ordering::Relaxed);
                        toggle_shared.enabled.store(now, Ordering::Relaxed);
                        let _ = toggle.set_text(if now { "暂停划词" } else { "开启划词" });
                        persist_selection(now);
                    }
                    _ => {}
                })
                .on_tray_icon_event(move |_tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        if let Some(win) = tray_handle.get_webview_window("bubble") {
                            if win.is_visible().unwrap_or(false) {
                                let _ = win.hide();
                            }
                        }
                    }
                })
                .build(app)?;
            // TrayIcon 是引用计数的：不托管给 app 的话，函数一返回就 Drop，菜单栏图标随即消失
            // （这正是「看不见图标、以为应用卡在后台」的原因）。
            {
                // 菜单栏项在 macOS 26 上可能要 ~1s 才布局完成，也可能被系统收进「≪」折叠区，
                // 记一次位置方便排查「图标看不见」。
                let probe = tray_icon.clone();
                thread::spawn(move || {
                    thread::sleep(Duration::from_millis(900));
                    match probe.rect() {
                        Ok(Some(r)) => log::line(format!("tray rect: {:?} {:?}", r.position, r.size)),
                        Ok(None) => log::line("tray rect: none (系统折叠了菜单栏项)"),
                        Err(e) => log::line(format!("tray rect err: {e}")),
                    }
                });
            }
            app.manage(tray_icon);

            let trusted_now = permissions::is_trusted();
            let listening_now = permissions::input_monitoring();
            if !trusted_now {
                log::line("accessibility permission missing — asking");
                permissions::request();
            } else if listening_now == Some(false) {
                log::line("input monitoring missing — 鼠标监听需要它，请在 系统设置 → 隐私与安全性 → 输入监控 里勾选随译");
                show_permission_window(&handle);
            }

            log::line(format!(
                "started · tray icon: {} · trusted: {} · input-monitoring: {:?} · config: {}",
                app.default_window_icon().is_some(),
                permissions::is_trusted(),
                crate::permissions::input_monitoring(),
                config::path().display()
            ));

            ensure_hook(handle.clone(), shared_setup.clone());

            // 权限通常是在运行中才点开的，这里轮询一次，拿到权限就自动挂上钩子，不用重启应用
            let watch_app = handle.clone();
            let watch_shared = shared_setup.clone();
            thread::spawn(move || loop {
                thread::sleep(Duration::from_millis(1500));
                let can_listen = permissions::is_trusted()
                    && permissions::input_monitoring() != Some(false);
                if can_listen && !HOOK_STARTED.load(Ordering::SeqCst) {
                    log::line("permission granted at runtime — starting hook");
                    ensure_hook(watch_app.clone(), watch_shared.clone());
                    if let Some(win) = watch_app.get_webview_window("permission") {
                        let _ = win.hide();
                    }
                    show_bubble(
                        &watch_app,
                        700,
                        260,
                        BubblePayload {
                            source: "权限已开启".into(),
                            text: "现在可以划词了：在任意应用里拖选一段文字，松开鼠标就会翻译。".into(),
                            detected: "suiyi".into(),
                            target: "ready".into(),
                            provider: "local".into(),
                            loading: false,
                            error: None,
                            card: None,
                        },
                    );
                }
            });

            // 第一次运行没有配置文件时，直接把设置窗口推出来
            if !config::path().exists() {
                open_settings_window(&handle);
            }

            if !trusted_now {
                let app_perm = handle.clone();
                thread::spawn(move || {
                    thread::sleep(Duration::from_millis(400));
                    show_permission_window(&app_perm);
                });
            } else if !launched_by_autostart {
                // 手动启动时不再自动弹主窗口：1020×720 的窗口正好压在屏幕中间，
                // 用户想在别的应用里划词时，选中的区域往往被它挡住，表现成「划词没反应」。
                // Dock 图标常驻，点一下（Reopen）就能打开主窗口；菜单栏里也有入口。
                log::line("launched — main window kept closed (Dock icon / menu opens it)");
            }

            let app_hello = handle.clone();
            thread::spawn(move || {
                if !trusted_now {
                    return; // 没权限时只显示权限窗口，别两个窗口一起弹
                }
                let version = env!("CARGO_PKG_VERSION");
                let mut cfg = config::load();
                if cfg.hello_version == version {
                    return;
                }
                thread::sleep(Duration::from_millis(500));
                // 放在主屏顶部中间，别硬编码到某个坐标
                let (lx, ly) = match app_hello.primary_monitor() {
                    Ok(Some(m)) => {
                        let scale = m.scale_factor();
                        let pos = m.position();
                        let size = m.size();
                        let cx = (pos.x as f64 + size.width as f64 / 2.0) / scale - 190.0;
                        ((cx as i32), ((pos.y as f64 / scale) + 64.0) as i32)
                    }
                    _ => (700, 120),
                };
                let hello_text = "选中任意应用里的文字、松开鼠标，就会在选区旁边先冒出一个小图标；\n点它（或悬停一下）才展开译文。单词会额外显示音标和词性释义。\n想调语言、线路或术语表，去菜单栏的「随译」菜单。".to_string();
                #[cfg(target_os = "macos")]
                {
                    let _ = bubble_helper::show_panel(
                        format!("suiyi → {version} · local"),
                        "随译已启动".into(),
                        hello_text.clone(),
                        false,
                        None,
                        "sentence".into(),
                        String::new(),
                        Vec::new(),
                        Vec::new(),
                        None,
                        Vec::new(),
                        lx as f64,
                        ly as f64,
                    );
                }
                #[cfg(not(target_os = "macos"))]
                {
                    show_bubble(
                        &app_hello,
                        lx,
                        ly,
                        BubblePayload {
                            source: "随译已启动".into(),
                            text: hello_text,
                            detected: "suiyi".into(),
                            target: version.into(),
                            provider: "local".into(),
                            loading: false,
                            error: None,
                            card: None,
                        },
                    );
                }
                let _ = &app_hello;
                cfg.hello_version = version.to_string();
                let _ = config::save(&cfg);
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            open_permission_settings,
            open_input_settings,
            permission_state,
            check_permission,
            close_permission_window,
            get_config,
            set_config,
            bubble_state,
            js_log,
            translate_text,
            lookup_word,
            close_main_window,
            ai_explain,
            list_history,
            clear_history,
            add_vocab,
            list_vocab,
            remove_vocab,
            clear_vocab,
            export_vocab_markdown,
            get_autostart,
            set_autostart,
            selection_enabled,
            set_selection
        ])
        .on_window_event(|window, event| {
            if window.label() == "main" {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.hide();
                    // 保持 Dock 图标：用户随时点回来都能再打开设置
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            // RunEvent::Reopen 只在 macOS 存在，别的平台必须编译掉
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = event {
                open_from_user(app_handle);
            }
            match event {
                tauri::RunEvent::ExitRequested { .. } => bubble_helper::shutdown(),
                tauri::RunEvent::Exit => bubble_helper::shutdown(),
                _ => {}
            }
            #[cfg(not(target_os = "macos"))]
            let _ = app_handle;
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_logical_to_physical_on_retina() {
        // 主屏逻辑 (700, 260) → 2x 屏物理 (1400, 520)
        let origin = PhysicalPosition::new(0, 0);
        assert_eq!(logical_to_physical(700, 260, origin, 2.0), (1400, 520));
    }

    #[test]
    fn converts_with_offset_monitor() {
        // 第二块屏在右边，物理原点 (1920,0)，scale 1 → 逻辑原点也是 1920
        let origin = PhysicalPosition::new(1920, 0);
        assert_eq!(logical_to_physical(2100, 100, origin, 1.0), (2100, 100));
        // 第二块屏是 2x：物理 (3840,0)，逻辑原点 1920
        let origin2 = PhysicalPosition::new(3840, 0);
        assert_eq!(logical_to_physical(2000, 300, origin2, 2.0), (4000, 600));
    }
}
