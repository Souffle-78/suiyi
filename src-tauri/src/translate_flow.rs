//! 划词翻译主流程（macOS 助手气泡 + 各平台共用）。
//!
//! 流程：拿到选区文本 → 先让助手把「翻译中」面板画出来 → 后台翻译 →
//! 单词再补一份词典释义 → 把结果推回助手。
//! 非 macOS（Windows/Linux）走原来的 webview 气泡，逻辑在同名函数里分支。

use crate::bubble_helper::{Phonetic, Sense};
use std::thread;
use tauri::{AppHandle, Manager};

/// 只由「一个英文单词」组成的文本才走词典（短语/句子只翻译）。
pub fn is_single_word(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() || t.len() > 32 {
        return false;
    }
    !t.contains(char::is_whitespace)
        && t.chars()
            .all(|c| c.is_ascii_alphabetic() || c == '-' || c == '\'')
}

fn panel_title(detected: &str, target: &str, provider: &str) -> String {
    if detected.is_empty() || target.is_empty() {
        "随译".to_string()
    } else {
        format!("{detected} → {target} · {provider}")
    }
}

/// 翻译并展示：先画 loading，再画结果（单词附词典）。
pub fn translate_and_show(app: AppHandle, text: String, lx: f64, ly: f64) {
    let source = text.trim().to_string();
    if source.is_empty() {
        return;
    }
    let word_mode = is_single_word(&source);

    let _ = crate::bubble_helper::show_panel(
        "翻译中".into(),
        source.clone(),
        String::new(),
        true,
        None,
        if word_mode { "word".into() } else { "sentence".into() },
        String::new(),
        Vec::new(),
        Vec::new(),
        lx,
        ly,
    );

    let cfg = crate::config::load();
    match crate::translate::run(&source, &cfg) {
        Ok(t) => {
            crate::history::record(&t);
            let (mode, phonetic, phonetics, senses) = if word_mode {
                // 先拿中文释义（有道），拿不到再退回英英词典
                if let Some(z) = crate::translate::zh_lookup(&source) {
                    let mut ph = Vec::new();
                    if !z.uk.is_empty() {
                        ph.push(Phonetic { label: "英".into(), text: format!("/{}/", z.uk) });
                    }
                    if !z.us.is_empty() && z.us != z.uk {
                        ph.push(Phonetic { label: "美".into(), text: format!("/{}/", z.us) });
                    }
                    let phonetic = ph.first().map(|p| p.text.clone()).unwrap_or_default();
                    let senses = z
                        .senses
                        .into_iter()
                        .map(|s| Sense { pos: s.pos, defs: s.defs, examples: s.examples, zh: s.zh })
                        .collect::<Vec<_>>();
                    ("word".to_string(), phonetic, ph, senses)
                } else {
                    match crate::translate::lookup(&source) {
                        Ok(d) => (
                            "word".to_string(),
                            d.phonetic,
                            d.phonetics
                                .into_iter()
                                .map(|p| Phonetic { label: p.label, text: p.text })
                                .collect(),
                            d.senses
                                .into_iter()
                                .map(|s| Sense {
                                    pos: s.pos,
                                    defs: s.defs,
                                    examples: s.examples,
                                    zh: s.zh,
                                })
                                .collect(),
                        ),
                        Err(_) => ("sentence".to_string(), String::new(), Vec::new(), Vec::new()),
                    }
                }
            } else {
                ("sentence".to_string(), String::new(), Vec::new(), Vec::new())
            };
            let _ = crate::bubble_helper::show_panel(
                panel_title(&t.detected, &t.target, &t.provider),
                source.clone(),
                t.text.clone(),
                false,
                None,
                mode,
                phonetic,
                phonetics,
                senses,
                lx,
                ly,
            );
        }
        Err(e) => {
            let _ = crate::bubble_helper::show_panel(
                "翻译失败".into(),
                source.clone(),
                String::new(),
                false,
                Some(e),
                "sentence".into(),
                String::new(),
                Vec::new(),
                Vec::new(),
                lx,
                ly,
            );
        }
    }
    let _ = app.get_webview_window("bubble"); // 保留 AppHandle 使用，避免未使用告警
}

/// 菜单栏「翻译剪贴板」：直接用当前光标位置弹结果。
pub fn translate_at_cursor(app: &AppHandle, text: &str) {
    let (lx, ly) = crate::last_cursor();
    let app = app.clone();
    let text = text.to_string();
    thread::spawn(move || translate_and_show(app, text, lx as f64, ly as f64));
}
