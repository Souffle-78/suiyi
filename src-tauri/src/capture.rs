use arboard::Clipboard;
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

    /// macOS 的 AX 拿不到选区矩形（AXSelectedText 只有文字），返回空矩形列表：
    /// 调用方只在「有矩形」时才做位置校验，所以这里不影响行为。
    pub fn selection() -> Option<(String, Vec<(f64, f64, f64, f64)>)> {
        selected_text().map(|t| (t, Vec::new()))
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
    pub fn probe() -> String {        unsafe {
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

    use windows::Win32::Foundation::POINT as WinPoint;
    use windows::Win32::System::Com::SAFEARRAY;
    use windows::Win32::System::Ole::{
        SafeArrayAccessData, SafeArrayDestroy, SafeArrayGetLBound, SafeArrayGetUBound,
        SafeArrayUnaccessData,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Accessibility::{
        CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationTextPattern,
        IUIAutomationTextRange, UIA_TextPatternId,
    };

    /// 选区在屏幕上的矩形（物理像素，x/y/宽/高）。用来判断「这份选区是不是就在用户
    /// 刚划过的地方」——浏览器里残留的旧选区会被 UIA 一直报出来，光有文字不代表是刚选的。
    fn rects_of(range: &IUIAutomationTextRange) -> Vec<(f64, f64, f64, f64)> {
        let mut out = Vec::new();
        unsafe {
            let Ok(array) = range.GetBoundingRectangles() else {
                return out;
            };
            if array.is_null() {
                return out;
            }
            let mut data: *mut std::ffi::c_void = std::ptr::null_mut();
            if SafeArrayAccessData(array, &mut data).is_ok() && !data.is_null() {
                let lb = SafeArrayGetLBound(array, 1).unwrap_or(0);
                let ub = SafeArrayGetUBound(array, 1).unwrap_or(-1);
                if ub > lb {
                    let count = ((ub - lb + 1) / 4) as usize;
                    let slice = std::slice::from_raw_parts(data as *const f64, count * 4);
                    for i in 0..count {
                        out.push((
                            slice[i * 4],
                            slice[i * 4 + 1],
                            slice[i * 4 + 2],
                            slice[i * 4 + 3],
                        ));
                    }
                }
                let _ = SafeArrayUnaccessData(array);
            }
            let _ = SafeArrayDestroy(array);
        }
        out
    }

    fn read_focused_selection() -> Result<(String, Vec<(f64, f64, f64, f64)>), &'static str> {
        unsafe {
            // UIA 的 COM 对象要求当前线程初始化过 COM。失败（比如已经是别的套间）也继续试。
            let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let initialized = hr.is_ok();

            let result = (|| -> Result<(String, Vec<(f64, f64, f64, f64)>), &'static str> {
                let automation: IUIAutomation =
                    CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)
                        .map_err(|_| "CoCreateInstance failed")?;
                let focused = automation
                    .GetFocusedElement()
                    .map_err(|_| "GetFocusedElement failed")?;
                // 有些程序（Chrome/Electron：焦点常落在页面的小节点上）把 TextPattern 挂在
                // 上层文档节点，焦点元素本身没有。沿控件树往上找几层，找到会报 TextPattern 的
                // 那个元素为止。
                let mut pattern: Option<IUIAutomationTextPattern> = focused
                    .GetCurrentPatternAs(UIA_TextPatternId)
                    .ok();
                if pattern.is_none() {
                    if let Ok(walker) = automation.ControlViewWalker() {
                        let mut node = focused.clone();
                        for _ in 0..4 {
                            let Ok(parent) = walker.GetParentElement(&node) else {
                                break;
                            };
                            node = parent;
                            if let Ok(p) = node.GetCurrentPatternAs(UIA_TextPatternId) {
                                pattern = Some(p);
                                break;
                            }
                        }
                    }
                }
                let pattern = pattern.ok_or("no TextPattern on focused element")?;
                let ranges = pattern.GetSelection().map_err(|_| "GetSelection failed")?;
                let count = ranges.Length().map_err(|_| "Length failed")?;
                let mut out = String::new();
                let mut rects = Vec::new();
                for i in 0..count {
                    let Ok(range) = ranges.GetElement(i) else {
                        continue;
                    };
                    // -1 = 不限长度
                    if let Ok(b) = range.GetText(-1) {
                        let piece = b.to_string();
                        if !piece.trim().is_empty() {
                            rects.extend(rects_of(&range));
                            out.push_str(&piece);
                        }
                    }
                }
                let trimmed = out.trim();
                if trimmed.is_empty() {
                    Err("selection empty")
                } else {
                    Ok((trimmed.to_string(), rects))
                }
            })();

            if initialized {
                CoUninitialize();
            }
            result
        }
    }

    /// 读当前焦点控件的选区：文字 + 屏幕上各段的矩形。
    pub fn selection() -> Option<(String, Vec<(f64, f64, f64, f64)>)> {
        // UIA 调用偶尔会卡在目标程序的 UI 线程上；放到独立线程里跑，超时就当读不到，
        // 绝不拖住抓选区的主流程。
        let started = std::time::Instant::now();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(read_focused_selection());
        });
        let outcome = rx
            .recv_timeout(std::time::Duration::from_millis(1200))
            .ok();
        let ms = started.elapsed().as_millis();
        match outcome {
            Some(Ok((text, rects))) => {
                crate::log::line(&format!(
                    "ax: {ms}ms, got {} chars ({} rects)",
                    text.chars().count(),
                    rects.len()
                ));
                Some((text, rects))
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

    pub fn selected_text() -> Option<String> {
        selection().map(|(t, _)| t)
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
                Ok((t, rects)) => format!("ax selected: {t:?} (rects={}; {desc})", rects.len()),
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
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, VIRTUAL_KEY, VK_CONTROL,
    };

    /// 按一次虚拟键。`up` 为真时发抬起事件。
    unsafe fn key_event(vk: VIRTUAL_KEY, up: bool) -> bool {
        let input = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: windows::Win32::UI::Input::KeyboardAndMouse::INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    wScan: 0,
                    dwFlags: if up { KEYEVENTF_KEYUP } else { Default::default() },
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        SendInput(&[input], std::mem::size_of::<INPUT>() as i32) == 1
    }

    // 关键是发**真正的虚拟键码** VK_C。以前用 enigo 的 `Key::Unicode('c')`，它在 Windows
    // 上发的是 KEYEVENTF_UNICODE + 虚拟键 0，记事本、部分 Electron/Qt 程序不把它当快捷键，
    // 于是「Ctrl+C 没生效」——这是划词偶发抓不到选区的原因之一。
    unsafe {
        if !key_event(VK_CONTROL, false) {
            return None;
        }
        sleep(Duration::from_millis(18));
        let _ = key_event(VIRTUAL_KEY(0x43), false);
        sleep(Duration::from_millis(12));
        let _ = key_event(VIRTUAL_KEY(0x43), true);
        sleep(Duration::from_millis(8));
        let _ = key_event(VK_CONTROL, true);
    }
    Some(())
}

/// 松手位置下面是哪个窗口、它是不是当前前台窗口。
///
/// 为什么需要：模拟 Ctrl+C 只能送到**前台窗口**。用户在桌面上按住左键划一下时，
/// Windows 常常不会把前台交给桌面（尤其是程序注入的点击），于是 Ctrl+C 落到上一个
/// 还在前台的程序上，把它残留的选区又复制一遍——这就是「划空气/划桌面也弹翻译」的
/// 真正原因。松手处的窗口不是前台窗口时，这次复制读到的根本不是用户选的东西，别抓。
#[cfg(not(target_os = "macos"))]
pub struct Target {
    pub class: String,
    pub is_focused: bool,
}

#[cfg(not(target_os = "macos"))]
pub fn target_at(px: i32, py: i32) -> Target {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetAncestor, GetClassNameW, GetForegroundWindow, WindowFromPoint, GA_ROOT,
    };
    unsafe {
        let point = POINT { x: px, y: py };
        let hit = WindowFromPoint(point);
        let root = if hit.0.is_null() {
            hit
        } else {
            GetAncestor(hit, GA_ROOT)
        };
        let mut buf = [0u16; 256];
        let len = if root.0.is_null() {
            0
        } else {
            GetClassNameW(root, &mut buf).max(0) as usize
        };
        let class = String::from_utf16_lossy(&buf[..len.min(buf.len())]);
        let fore = GetForegroundWindow();
        let fore_root = if fore.0.is_null() { fore } else { GetAncestor(fore, GA_ROOT) };
        Target {
            class,
            is_focused: root == fore_root,
        }
    }
}

/// 桌面、任务栏、托盘溢出面板这些「外壳窗口」上没有可翻译的文字：
/// 在上面划一下什么都不该发生（和 macOS 一致）。
#[cfg(not(target_os = "macos"))]
fn is_shell_class(class: &str) -> bool {
    matches!(
        class,
        "Progman"
            | "WorkerW"
            | "SHELLDLL_DefView"
            | "SysListView32"
            | "Shell_TrayWnd"
            | "Shell_SecondaryTrayWnd"
            | "TrayNotifyWnd"
            | "NotifyIconOverflowWindow"
            | "TopLevelWindowForOverflowXamlIsland"
            | "XamlExplorerHostIslandWindow"
            | "MultitaskingViewFrame"
            | "ForegroundStaging"
            | "#32768"
    )
}

/// 复制到的是「文件名/路径」而不是正文时不要翻译（资源管理器里 Ctrl+C 选中文件
/// 会把路径写进文本剪贴板）。
fn looks_like_path(text: &str) -> bool {
    let t = text.trim();
    if t.contains('\n') || t.chars().count() > 260 {
        return false;
    }
    let lower = t.to_lowercase();
    let is_path = (t.len() > 3 && t.as_bytes()[1] == b':' && (t.as_bytes()[2] == b'\\' || t.as_bytes()[2] == b'/'))
        || lower.starts_with("\\\\")
        || lower.contains("\\users\\");
    if !is_path {
        return false;
    }
    // 路径里带空格也可能是文件名；只挡「明确的文件路径」形态
    [".lnk", ".exe", ".dll", ".msi", ".zip", ".7z", ".rar", ".png", ".jpg", ".mp4", ".txt", ".pdf", ".docx", ".xlsx"]
        .iter()
        .any(|ext| lower.ends_with(ext))
        || !lower.contains('.')
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

/// 剪贴板序号：Windows 每次有程序写剪贴板它就 +1，是 NSPasteboard changeCount 的等价物。
/// 用它判断「这次 Ctrl+C 到底有没有真的写剪贴板」，比只比对内容更可靠。
#[cfg(target_os = "macos")]
fn clipboard_seq() -> Option<u32> {
    None
}

#[cfg(not(target_os = "macos"))]
fn clipboard_seq() -> Option<u32> {
    use windows::Win32::System::DataExchange::GetClipboardSequenceNumber;
    unsafe { Some(GetClipboardSequenceNumber()) }
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
        // 哨兵写完之后记一次序号：之后序号没再变，就说明 Ctrl+C 根本没写剪贴板
        let seq_after_sentinel = clipboard_seq();
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
            // 序号变了 = 有程序真的写过剪贴板（这次 Ctrl+C 生效了）
            let wrote = match (clipboard_seq(), seq_after_sentinel) {
                (Some(now), Some(start)) => now != start,
                _ => true,
            };
            if let Ok(t) = cb.get_text() {
                let t = t.trim().to_string();
                if t.is_empty() {
                    continue;
                }
                let captured = match &sentinel {
                    // 还是哨兵 → 复制没生效；不是哨兵 → 这次复制拿到了东西
                    Some(s) => t != s.as_str() && wrote,
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
fn ax_selection() -> Option<(String, Vec<(f64, f64, f64, f64)>)> {
    ax::selection()
}

#[cfg(not(target_os = "macos"))]
fn ax_selection() -> Option<(String, Vec<(f64, f64, f64, f64)>)> {
    ax::selection()
}

/// 选区是否落在用户这次划过的范围里。
///
/// 浏览器/Word 这类程序会把「上一次的选区」一直保留着，UIA 也照样报出来；
/// 用户在某片空白上拖一下（什么都没选中）时，如果只看「有没有文字」，
/// 就会把旧选区当成本次结果翻译掉——这正是「划空气也弹翻译」。
/// 用 UIA 给的选区矩形和拖拽矩形求交，就能把这种残留挡掉。
/// 拿不到矩形（比如某些自绘控件）时不拦，避免误伤正常选区。
fn selection_matches_drag(rects: &[(f64, f64, f64, f64)], drag: Option<(i32, i32, i32, i32)>) -> bool {
    let Some((x0, y0, x1, y1)) = drag else {
        return true;
    };
    if rects.is_empty() {
        return true;
    }
    let (x0, x1) = (x0 as f64, x1 as f64);
    let (y0, y1) = (y0 as f64, y1 as f64);
    rects.iter().any(|(rx, ry, rw, rh)| {
        let (rx0, ry0, rx1, ry1) = (*rx, *ry, *rx + *rw, *ry + *rh);
        !(rx0 > x1 || rx1 < x0 || ry0 > y1 || ry1 < y0)
    })
}

/// Grab the current selection.
///
/// 顺序：先用辅助功能接口直接读（UI Automation / AX）——不碰剪贴板、不打扰用户、
/// 也不会把别处的残留选区当成新的；读不到再退回模拟复制。
///
/// `at` 是松手位置的物理像素坐标（Windows 用来确认「要复制的窗口确实是前台窗口」）。
/// `drag` 是这次按下→松手的矩形（物理像素），用来校验选区真的在划过的地方。
pub fn select_text_at(
    at: Option<(i32, i32)>,
    drag: Option<(i32, i32, i32, i32)>,
) -> Option<String> {
    #[cfg(not(target_os = "macos"))]
    if let Some((px, py)) = at {
        let target = target_at(px, py);
        if is_shell_class(&target.class) {
            crate::log::line(format!("skip: shell surface ({})", target.class));
            return None;
        }
        if !target.is_focused {
            crate::log::line(format!(
                "skip: window under cursor is not focused ({})",
                target.class
            ));
            return None;
        }
    }
    #[cfg(target_os = "macos")]
    let _ = at;

    // 1) 辅助功能接口：快且无副作用
    // 排障开关：某些老程序（或远程桌面会话）里 UIA 会拖住事件线程，设 SUIYI_NO_AX=1 就只用模拟复制。
    let ax_enabled = std::env::var("SUIYI_NO_AX").is_err();
    let mut ax_hit = if ax_enabled { ax_selection() } else { None };
    // 有些程序（记事本、部分 RichEdit 控件）在松手的那一瞬间会短暂报「没有选区」，
    // 而它其实还留着上一次选中的文字。等 60ms 再问一次：要么拿到那份残留选区
    // （随后会被下面的矩形校验挡掉），要么确认真的没选——总之别急着去模拟复制。
    if ax_hit.is_none() && ax_enabled {
        sleep(Duration::from_millis(60));
        ax_hit = ax_selection();
    }
    if let Some((text, rects)) = ax_hit {
        let t = text.trim().to_string();
        if !t.is_empty() {
            if !selection_matches_drag(&rects, drag) {
                crate::log::line(format!(
                    "skip: selection is elsewhere ({} rects, drag {:?})",
                    rects.len(),
                    drag
                ));
                return None;
            }
            return Some(t);
        }
    }

    // 排障开关：某些环境里模拟 Ctrl+C 会被拦（第三方输入法、热键冲突、远程桌面），
    // 设 SUIYI_NO_COPY=1 就只用 UI Automation。
    if std::env::var("SUIYI_NO_COPY").is_ok() {
        crate::log::line("capture failed — ax:none (copy disabled)");
        return None;
    }

    // 2) 模拟复制兜底：控制台、部分 Qt/Java 程序读不到 TextPattern
    match clipboard_select() {
        Some(t) => {
            let t = t.trim().to_string();
            if t.is_empty() {
                None
            } else if looks_like_path(&t) {
                crate::log::line(format!(
                    "skip: copied text looks like a file path ({} chars)",
                    t.chars().count()
                ));
                None
            } else if t.chars().count() > 4000 {
                crate::log::line(format!(
                    "skip: selection too long ({} chars)",
                    t.chars().count()
                ));
                None
            } else {
                Some(t)
            }
        }
        None => {
            crate::log::line("capture failed — copy:none ax:none");
            None
        }
    }
}

pub fn select_text() -> Option<String> {
    select_text_at(None, None)
}
