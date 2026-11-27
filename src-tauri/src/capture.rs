use arboard::Clipboard;
#[cfg(not(target_os = "macos"))]
use enigo::{Direction, Enigo, Key, Keyboard, Settings};
use std::thread::sleep;
use std::time::{Duration, Instant};

/// 优先用系统辅助功能接口直接读「当前焦点控件的选中文字」：
/// 不碰剪贴板，也不会因为目标应用禁用了复制而失败。
#[cfg(target_os = "macos")]
mod ax {
    use std::ffi::CString;
    use std::os::raw::{c_char, c_long, c_void};

    type CFTypeRef = *const c_void;

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXUIElementCreateSystemWide() -> CFTypeRef;
        fn AXUIElementCopyAttributeValue(
            element: CFTypeRef,
            attribute: CFTypeRef,
            value: *mut CFTypeRef,
        ) -> i32;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFStringCreateWithCString(
            allocator: CFTypeRef,
            c_str: *const c_char,
            encoding: u32,
        ) -> CFTypeRef;
        fn CFStringGetLength(s: CFTypeRef) -> c_long;
        fn CFStringGetMaximumSizeForEncoding(length: c_long, encoding: u32) -> c_long;
        fn CFStringGetCString(
            s: CFTypeRef,
            buffer: *mut c_char,
            buffer_size: c_long,
            encoding: u32,
        ) -> bool;
        fn CFGetTypeID(cf: CFTypeRef) -> usize;
        fn CFStringGetTypeID() -> usize;
        fn CFRelease(cf: CFTypeRef);
    }

    const UTF8: u32 = 0x0800_0100;

    /// 用字符串名字查属性，避开 AX 常量符号（在 dyld 共享缓存里不一定能直接链接）
    unsafe fn attr(name: &str) -> Option<CFTypeRef> {
        let c = CString::new(name).ok()?;
        let s = CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), UTF8);
        if s.is_null() {
            None
        } else {
            Some(s)
        }
    }

    unsafe fn to_rust(cf: CFTypeRef) -> Option<String> {
        if cf.is_null() {
            return None;
        }
        if CFGetTypeID(cf) != CFStringGetTypeID() {
            return None;
        }
        let len = CFStringGetLength(cf);
        let size = CFStringGetMaximumSizeForEncoding(len, UTF8) + 1;
        let mut buf = vec![0 as c_char; size as usize];
        let ok = CFStringGetCString(cf, buf.as_mut_ptr(), size, UTF8);
        if !ok {
            return None;
        }
        let cstr = std::ffi::CStr::from_ptr(buf.as_ptr());
        cstr.to_str().ok().map(|s| s.to_string())
    }

    pub fn selected_text() -> Option<String> {
        unsafe {
            let system = AXUIElementCreateSystemWide();
            if system.is_null() {
                return None;
            }
            let focused_attr = attr("AXFocusedUIElement")?;
            let mut focused: CFTypeRef = std::ptr::null();
            let err = AXUIElementCopyAttributeValue(system, focused_attr, &mut focused);
            CFRelease(focused_attr);
            CFRelease(system);
            if err != 0 || focused.is_null() {
                return None;
            }
            let selected_attr = attr("AXSelectedText")?;
            let mut value: CFTypeRef = std::ptr::null();
            let err = AXUIElementCopyAttributeValue(focused, selected_attr, &mut value);
            CFRelease(selected_attr);
            CFRelease(focused);
            if err != 0 {
                return None;
            }
            let text = to_rust(value);
            if !value.is_null() {
                CFRelease(value);
            }
            text
        }
    }

    /// 给自检用的说明性输出（带 AX 错误码，排障用）
    pub fn probe() -> String {
        unsafe {
            let system = AXUIElementCreateSystemWide();
            if system.is_null() {
                return "ax: systemwide=NULL".into();
            }
            let fa = match attr("AXFocusedUIElement") {
                Some(v) => v,
                None => return "ax: attr build failed".into(),
            };
            let mut focused: CFTypeRef = std::ptr::null();
            let e1 = AXUIElementCopyAttributeValue(system, fa, &mut focused);
            CFRelease(fa);
            CFRelease(system);
            if e1 != 0 || focused.is_null() {
                return format!("ax: focused err={e1} null={}", focused.is_null());
            }
            let ta = match attr("AXSelectedText") {
                Some(v) => v,
                None => {
                    CFRelease(focused);
                    return "ax: attr build failed".into();
                }
            };
            let mut value: CFTypeRef = std::ptr::null();
            let e2 = AXUIElementCopyAttributeValue(focused, ta, &mut value);
            CFRelease(ta);
            CFRelease(focused);
            if e2 != 0 {
                return format!("ax: selected err={e2}");
            }
            match to_rust(value) {
                Some(t) => {
                    if !value.is_null() {
                        CFRelease(value);
                    }
                    format!(
                        "ax: selected len={} {:?}",
                        t.chars().count(),
                        t.chars().take(30).collect::<String>()
                    )
                }
                None => {
                    if !value.is_null() {
                        CFRelease(value);
                    }
                    "ax: selected value not a string".into()
                }
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod ax {
    //! Windows 版的「辅助功能读选区」：UI Automation 的 TextPattern。
    //!
    //! macOS 用 `AXSelectedText` 兜底，Windows 的等价物是
    //! `IUIAutomation::GetFocusedElement` → `UIA_TextPatternId` → `GetSelection`。
    //! 只在模拟 Ctrl+C 拿不到东西之后才走这条路（有些程序禁用复制、或复制被拦截）。
    //!
    //! 覆盖范围：Chromium/Edge/Office/WPF/UWP/记事本等。
    //! 拿不到的情况：控制台（conhost/Windows Terminal 的缓冲区）、部分 Qt/Java 程序，
    //! 以及以管理员权限运行、而本进程不是管理员的窗口（UIPI 限制）。

    use windows::core::Interface;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Accessibility::{
        CUIAutomation, IUIAutomation, IUIAutomationTextPattern, UIA_TextPatternId,
    };

    fn read_focused_selection() -> Result<String, &'static str> {
        unsafe {
            // UIA 的 COM 对象要求当前线程初始化过 COM。失败（比如已经是别的套间）也继续试。
            let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let initialized = hr.is_ok();

            let result = (|| -> Result<String, &'static str> {
                let automation: IUIAutomation =
                    CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)
                        .map_err(|_| "CoCreateInstance failed")?;
                let element = automation
                    .GetFocusedElement()
                    .map_err(|_| "GetFocusedElement failed")?;
                let pattern: IUIAutomationTextPattern = element
                    .GetCurrentPatternAs(UIA_TextPatternId)
                    .map_err(|_| "no TextPattern on focused element")?;
                let ranges = pattern.GetSelection().map_err(|_| "GetSelection failed")?;
                let count = ranges.Length().map_err(|_| "Length failed")?;
                let mut out = String::new();
                for i in 0..count {
                    let Ok(range) = ranges.GetElement(i) else {
                        continue;
                    };
                    // -1 = 不限长度
                    if let Ok(b) = range.GetText(-1) {
                        out.push_str(&b.to_string());
                    }
                }
                let trimmed = out.trim();
                if trimmed.is_empty() {
                    Err("selection empty")
                } else {
                    Ok(trimmed.to_string())
                }
            })();

            if initialized {
                CoUninitialize();
            }
            result
        }
    }

    pub fn selected_text() -> Option<String> {
        // UIA 调用偶尔会卡在目标程序的 UI 线程上；放到独立线程里跑，超时就当读不到，
        // 绝不拖住抓选区的主流程。
        let started = std::time::Instant::now();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(read_focused_selection());
        });
        let outcome = rx
            .recv_timeout(std::time::Duration::from_millis(1500))
            .ok();
        let ms = started.elapsed().as_millis();
        match outcome {
            Some(Ok(text)) => {
                crate::log::line(&format!("ax: {ms}ms, got {} chars", text.chars().count()));
                Some(text)
            }
            Some(Err(why)) => {
                crate::log::line(&format!("ax: {ms}ms, {why}"));
                None
            }
            None => {
                crate::log::line(&format!("ax: {ms}ms, timeout"));
                None
            }
        }
    }

    pub fn probe() -> String {
        // 排障用：把焦点元素的身份打出来，方便判断「没有 TextPattern」是因为焦点根本不在
        // 文本控件上（比如焦点落在我们自己的气泡/主窗口），还是目标程序不支持 TextPattern。
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let automation: IUIAutomation =
                match CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) {
                    Ok(a) => a,
                    Err(_) => return "ax selected: none (CoCreateInstance failed)".into(),
                };
            let element = match automation.GetFocusedElement() {
                Ok(e) => e,
                Err(_) => return "ax selected: none (no focused element)".into(),
            };
            let name = element
                .CurrentName()
                .map(|b| b.to_string())
                .unwrap_or_default();
            let class = element
                .CurrentClassName()
                .map(|b| b.to_string())
                .unwrap_or_default();
            let ctype = element.CurrentControlType().map(|c| c.0).unwrap_or(0);
            let desc = format!("focus=name:{name:?} class:{class:?} type:{ctype}");
            match read_focused_selection() {
                Ok(t) => format!("ax selected: {t:?} ({desc})"),
                Err(why) => format!("ax selected: none ({why}; {desc})"),
            }
        }
    }
}

pub fn ax_probe() -> String {
    ax::probe()
}

/// macOS：直接用 CGEvent 发 ⌘C。
///
/// 不再用 enigo —— 它在 macOS 上会通过 Text Services 把字符映射成键，
/// 而 `TSMCurrentKeyboardInputSourceRefCreate` 是主线程专属 API，
/// 从事件线程调用会被 libdispatch 的 `dispatch_assert_queue` 断言打死（SIGTRAP）。
#[cfg(target_os = "macos")]
fn send_copy() -> Option<()> {
    use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation};
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

    const KEY_C: u16 = 8;
    let src = CGEventSource::new(CGEventSourceStateID::HIDSystemState).ok()?;
    let down = CGEvent::new_keyboard_event(src.clone(), KEY_C, true).ok()?;
    down.set_flags(CGEventFlags::CGEventFlagCommand);
    down.post(CGEventTapLocation::HID);
    sleep(Duration::from_millis(18));
    let up = CGEvent::new_keyboard_event(src, KEY_C, false).ok()?;
    up.set_flags(CGEventFlags::CGEventFlagCommand);
    up.post(CGEventTapLocation::HID);
    sleep(Duration::from_millis(12));
    Some(())
}

#[cfg(not(target_os = "macos"))]
fn send_copy() -> Option<()> {
    let mut enigo = Enigo::new(&Settings::default()).ok()?;
    enigo.key(Key::Control, Direction::Press).ok()?;
    sleep(Duration::from_millis(18));
    // 必须用真正的虚拟键码 VK_C（0x43）。以前用 Key::Unicode('c')，它在 Windows 上
    // 发的是 KEYEVENTF_UNICODE + 虚拟键 0，记事本、部分 Electron/Qt 程序不把它当
    // 快捷键处理，于是「Ctrl+C 没生效」——这就是划词偶发抓不到选区的原因。
    enigo.key(Key::Other(0x43), Direction::Click).ok()?;
    sleep(Duration::from_millis(12));
    let _ = enigo.key(Key::Control, Direction::Release);
    Some(())
}

/// 通过模拟复制读选区：兜底路径，能应对 AX 拿不到选区的应用（浏览器、PDF 阅读器常见）。
/// macOS：用 NSPasteboard 的 changeCount 判断「这次复制到底成功没有」。
/// 之前用「内容和旧剪贴板不同」来判断，选中的词恰好等于剪贴板内容时就永远失败。
#[cfg(target_os = "macos")]
fn paste_change_count() -> Option<i64> {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};
    unsafe {
        let pb: *mut Object = msg_send![class!(NSPasteboard), generalPasteboard];
        if pb.is_null() {
            return None;
        }
        let count: i64 = msg_send![pb, changeCount];
        Some(count)
    }
}

#[cfg(not(target_os = "macos"))]
fn paste_change_count() -> Option<i64> {
    None
}

/// Windows / Linux 没有 NSPasteboard 的 changeCount，用一个「哨兵」代替：
/// 复制前先往剪贴板写一个不可能出现在选区里的唯一串，之后只要剪贴板不是它，
/// 就证明这次 Ctrl+C 真的生效了。顺带修掉「选中的词恰好等于剪贴板旧内容」时
/// 永远判定失败的老问题（macOS 上是用 changeCount 解决的同一个 bug）。
#[cfg(target_os = "macos")]
fn clipboard_sentinel() -> Option<String> {
    None
}

#[cfg(not(target_os = "macos"))]
fn clipboard_sentinel() -> Option<String> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    Some(format!("\u{2}suiyi-{}-{}", std::process::id(), nanos))
}

fn clipboard_select() -> Option<String> {
    let mut cb = Clipboard::new().ok()?;
    let before = cb.get_text().ok().unwrap_or_default();
    let count_before = paste_change_count();
    let sentinel = clipboard_sentinel();

    // 第一次 ⌘C 可能落在「窗口刚被激活」的瞬间被吞掉，隔 180ms 再补一次
    let mut grabbed: Option<String> = None;
    for attempt in 0..3 {
        if attempt > 0 {
            sleep(Duration::from_millis(200));
        }
        if let Some(s) = &sentinel {
            // 先把剪贴板泡在哨兵里；写失败也没关系，后面的判断会退回「内容变了」那条
            let _ = cb.set_text(s.as_str());
            sleep(Duration::from_millis(15));
        }
        if send_copy().is_none() {
            continue;
        }
        let deadline = Instant::now() + Duration::from_millis(600);
        while Instant::now() < deadline {
            sleep(Duration::from_millis(20));
            let fresh = match (paste_change_count(), count_before) {
                (Some(after), Some(before_n)) => after != before_n,
                _ => false,
            };
            if let Ok(t) = cb.get_text() {
                let t = t.trim().to_string();
                if t.is_empty() {
                    continue;
                }
                let captured = match &sentinel {
                    // 还是哨兵 → 复制没生效；不是哨兵 → 这次复制拿到了东西
                    Some(s) => t != s.as_str(),
                    None => fresh || t != before.trim(),
                };
                // changeCount 变了 / 不是哨兵 → 是这次复制的内容（哪怕和旧内容一样）
                if captured {
                    grabbed = Some(t);
                    break;
                }
            }
        }
        if grabbed.is_some() {
            break;
        }
    }

    if grabbed.is_none() {
        crate::log::line("capture: copy attempts exhausted");
    }
    // give the source app a beat before we overwrite the clipboard again
    sleep(Duration::from_millis(140));
    // 无论成功与否都要把剪贴板还原：失败时里面可能正躺着哨兵
    let _ = cb.set_text(before);
    grabbed
}

#[cfg(target_os = "macos")]
fn ax_text() -> Option<String> {
    ax::selected_text()
}

#[cfg(not(target_os = "macos"))]
fn ax_text() -> Option<String> {
    ax::selected_text()
}

/// Grab the current selection.
/// 先用模拟复制（快、跨应用最稳），拿不到再用辅助功能接口兜底（覆盖禁用复制的应用）。
pub fn select_text() -> Option<String> {
    // 排障开关：某些环境里模拟 Ctrl+C 会被拦（第三方输入法、热键冲突、远程桌面），
    // 设 SUIYI_NO_COPY=1 就跳过复制，直接走 UI Automation。
    let skip_copy = std::env::var("SUIYI_NO_COPY").is_ok();
    let via_copy = if skip_copy {
        None
    } else {
        clipboard_select()
    };
    if let Some(t) = via_copy {
        let t = t.trim().to_string();
        if !t.is_empty() {
            return Some(t);
        }
    }
    match ax_text() {
        Some(t) if !t.trim().is_empty() => Some(t.trim().to_string()),
        _ => {
            crate::log::line("capture failed — copy:none ax:none");
            None
        }
    }
}
