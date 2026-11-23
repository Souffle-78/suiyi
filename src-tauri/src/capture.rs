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
    pub fn selected_text() -> Option<String> {
        None
    }
    pub fn probe() -> String {
        "ax selected: unsupported on this platform".into()
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
    enigo.key(Key::Unicode('c'), Direction::Click).ok()?;
    sleep(Duration::from_millis(12));
    let _ = enigo.key(Key::Control, Direction::Release);
    Some(())
}

/// 通过模拟复制读选区：兜底路径，能应对 AX 拿不到选区的应用（浏览器、PDF 阅读器常见）。
fn clipboard_select() -> Option<String> {
    let mut cb = Clipboard::new().ok()?;
    let before = cb.get_text().ok().unwrap_or_default();

    send_copy()?;

    let deadline = Instant::now() + Duration::from_millis(420);
    let mut grabbed: Option<String> = None;
    while Instant::now() < deadline {
        sleep(Duration::from_millis(22));
        if let Ok(t) = cb.get_text() {
            let t = t.trim().to_string();
            if !t.is_empty() && t != before.trim() {
                grabbed = Some(t);
                break;
            }
        }
    }

    if grabbed.is_some() {
        // give the source app a beat before we overwrite the clipboard again
        sleep(Duration::from_millis(140));
        let _ = cb.set_text(before);
    }
    grabbed
}

/// Grab the current selection.
/// 先用模拟复制（快、跨应用最稳），拿不到再用辅助功能接口兜底（覆盖禁用复制的应用）。
pub fn select_text() -> Option<String> {
    if let Some(t) = clipboard_select() {
        let t = t.trim().to_string();
        if !t.is_empty() {
            return Some(t);
        }
    }
    ax::selected_text().map(|t| t.trim().to_string()).filter(|t| !t.is_empty())
}
