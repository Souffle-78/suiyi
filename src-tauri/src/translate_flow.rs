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
        None,
        Vec::new(),
        lx,
        ly,
    );

    let cfg = crate::config::load();
    match crate::translate::run(&source, &cfg) {
        Ok(t) => {
            crate::history::record(&t);
            let (mode, phonetic, phonetics, senses, tags) = if word_mode {
                let local = crate::dict_local::lookup(&source);
                if let Some(z) = crate::translate::zh_lookup(&source) {
                    // 在线释义更全更口语，优先；音标/考试标签用本地库补齐
                    let mut ph = Vec::new();
                    if !z.uk.is_empty() {
                        ph.push(Phonetic { label: "英".into(), text: format!("/{}/", z.uk) });
                    }
                    if !z.us.is_empty() && z.us != z.uk {
                        ph.push(Phonetic { label: "美".into(), text: format!("/{}/", z.us) });
                    }
                    if ph.is_empty() {
                        if let Some(l) = &local {
                            if !l.phonetic.is_empty() {
                                ph.push(Phonetic { label: String::new(), text: format!("/{}/", l.phonetic) });
                            }
                        }
                    }
                    let phonetic = ph.first().map(|p| p.text.clone()).unwrap_or_default();
                    let senses = z
                        .senses
                        .into_iter()
                        .map(|s| Sense { pos: s.pos, defs: s.defs, examples: s.examples, zh: s.zh })
                        .collect::<Vec<_>>();
                    let tags = local.as_ref().map(|l| l.tags.clone()).unwrap_or_default();
                    ("word".to_string(), phonetic, ph, senses, tags)
                } else if let Some(l) = local {
                    // 断网 / 有道没收录：直接用内置词库
                    let mut ph = Vec::new();
                    if !l.phonetic.is_empty() {
                        ph.push(Phonetic { label: String::new(), text: format!("/{}/", l.phonetic) });
                    }
                    let phonetic = ph.first().map(|p| p.text.clone()).unwrap_or_default();
                    let senses = crate::dict_local::senses_of(&l)
                        .into_iter()
                        .map(|(pos, items)| Sense { pos, defs: items, examples: Vec::new(), zh: true })
                        .collect::<Vec<_>>();
                    ("word".to_string(), phonetic, ph, senses, l.tags.clone())
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
                                .map(|s| Sense { pos: s.pos, defs: s.defs, examples: s.examples, zh: s.zh })
                                .collect(),
                            Vec::new(),
                        ),
                        Err(_) => ("sentence".to_string(), String::new(), Vec::new(), Vec::new(), Vec::new()),
                    }
                }
            } else {
                let none = Vec::new();
                ("sentence".to_string(), String::new(), Vec::new(), none, Vec::new())
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
                tags,
                None,
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
                None,
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
