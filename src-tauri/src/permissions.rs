//! macOS 辅助功能（Accessibility）权限检查。
//! 没有这个权限，既读不到选区（模拟 ⌘C 发不出去），也收不到全局鼠标事件。

#[cfg(target_os = "macos")]
mod platform {
    use std::os::raw::c_void;

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrusted() -> bool;
        static kAXTrustedCheckOptionPrompt: *const c_void;
        fn AXIsProcessTrustedWithOptions(options: *const c_void) -> bool;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFDictionaryCreate(
            allocator: *const c_void,
            keys: *const *const c_void,
            values: *const *const c_void,
            num_values: isize,
            key_callbacks: *const c_void,
            value_callbacks: *const c_void,
        ) -> *const c_void;
        fn CFRelease(cf: *const c_void);
        static kCFBooleanTrue: *const c_void;
        static kCFTypeDictionaryKeyCallBacks: c_void;
        static kCFTypeDictionaryValueCallBacks: c_void;
    }

    pub fn is_trusted() -> bool {
        unsafe { AXIsProcessTrusted() }
    }

    /// 弹出系统自带的「是否允许控制这台电脑」对话框（只弹一次，不阻塞）。
    pub fn request() {
        unsafe {
            let keys = [kAXTrustedCheckOptionPrompt];
            let values = [kCFBooleanTrue];
            let dict = CFDictionaryCreate(
                std::ptr::null(),
                keys.as_ptr(),
                values.as_ptr(),
                1,
                &kCFTypeDictionaryKeyCallBacks as *const _ as *const c_void,
                &kCFTypeDictionaryValueCallBacks as *const _ as *const c_void,
            );
            AXIsProcessTrustedWithOptions(dict);
            if !dict.is_null() {
                CFRelease(dict);
            }
        }
    }
}

#[cfg(target_os = "macos")]
pub use platform::{is_trusted, request};

#[cfg(target_os = "macos")]
pub fn open_settings() {
    let _ = std::process::Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
        .spawn();
}

#[cfg(not(target_os = "macos"))]
pub fn is_trusted() -> bool {
    true
}

#[cfg(not(target_os = "macos"))]
pub fn request() {}

#[cfg(not(target_os = "macos"))]
pub fn open_settings() {}

/// 输入监控（监听全局鼠标/键盘事件）是否已授权。
/// 辅助功能（AXIsProcessTrusted）只覆盖「发送」事件，监听还需要这一项。
#[cfg(target_os = "macos")]
pub fn input_monitoring() -> Option<bool> {
    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOHIDCheckAccess(request: u32) -> u32;
    }
    // kIOHIDRequestTypeListenEvent = 1；0 = granted, 1 = denied, 2 = unknown
    let status = unsafe { IOHIDCheckAccess(1) };
    Some(status == 0)
}

#[cfg(not(target_os = "macos"))]
pub fn input_monitoring() -> Option<bool> {
    None
}
