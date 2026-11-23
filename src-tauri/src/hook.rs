//! 全局鼠标钩子。
//!
//! macOS 上原来用 `rdev::listen`，但它在事件回调里会调用 Text Services
//! (`TSMGetInputSourceProperty`) 把按键转成字符；这是主线程专属 API，从事件线程
//! 调用会触发 `dispatch_assert_queue` 断言并 SIGTRAP 崩溃（macOS 27 上必现）。
//!
//! 我们只需要鼠标，于是 macOS 改用 CGEventTap，只订阅三类鼠标事件；
//! 顺带解决了 rdev 拿不到拖拽过程 `leftMouseDragged` 位置的问题。

#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub enum MouseEvent {
    Down(f64, f64),
    Dragged(f64, f64),
    Up(f64, f64),
}

#[cfg(target_os = "macos")]
pub fn listen<F>(handler: F) -> Result<(), String>
where
    F: Fn(MouseEvent) + 'static,
{
    use core_foundation::runloop::CFRunLoop;
    use core_graphics::event::{
        CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventType,
        CallbackResult,
    };

    CGEventTap::with_enabled(
        CGEventTapLocation::Session,
        CGEventTapPlacement::HeadInsertEventTap,
        CGEventTapOptions::ListenOnly,
        vec![
            CGEventType::LeftMouseDown,
            CGEventType::LeftMouseDragged,
            CGEventType::LeftMouseUp,
        ],
        move |_proxy, etype, event| {
            let loc = event.location();
            let ev = match etype {
                CGEventType::LeftMouseDown => Some(MouseEvent::Down(loc.x, loc.y)),
                CGEventType::LeftMouseDragged => Some(MouseEvent::Dragged(loc.x, loc.y)),
                CGEventType::LeftMouseUp => Some(MouseEvent::Up(loc.x, loc.y)),
                _ => None,
            };
            if let Some(ev) = ev {
                handler(ev);
            }
            CallbackResult::Keep
        },
        || CFRunLoop::run_current(),
    )
    .map_err(|_| "CGEventTap 建立失败（通常是缺少「输入监控」权限）".to_string())
}

#[cfg(not(target_os = "macos"))]
pub fn listen<F>(handler: F) -> Result<(), String>
where
    F: Fn(MouseEvent) + 'static,
{
    use std::sync::Mutex;

    // 非 macOS 仍用 rdev：那边不碰 Text Services，没有问题。
    // 记下当前位置与按下位置，松开时把 Down/Up 一起交给上层算位移。
    struct State {
        cursor: (f64, f64),
        pressed: Option<(f64, f64)>,
    }
    let state = Mutex::new(State { cursor: (0.0, 0.0), pressed: None });
    let handler = Mutex::new(handler);

    rdev::listen(move |event| {
        let pair = match event.event_type {
            rdev::EventType::MouseMove { x, y } => {
                if let Ok(mut s) = state.lock() {
                    s.cursor = (x, y);
                }
                None
            }
            rdev::EventType::ButtonPress(rdev::Button::Left) => {
                if let Ok(mut s) = state.lock() {
                    s.pressed = Some(s.cursor);
                }
                None
            }
            rdev::EventType::ButtonRelease(rdev::Button::Left) => {
                match state.lock() {
                    Ok(mut s) => s.pressed.take().map(|down| (down, s.cursor)),
                    Err(_) => None,
                }
            }
            _ => None,
        };
        if let Some((down, up)) = pair {
            if let Ok(h) = handler.lock() {
                h(MouseEvent::Down(down.0, down.1));
                h(MouseEvent::Up(up.0, up.1));
            }
        }
    })
    .map_err(|e| format!("{e:?}"))
}
