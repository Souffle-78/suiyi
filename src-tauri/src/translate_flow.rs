//! 划词翻译主流程（macOS 用 NSPanel 助手，Windows/Linux 用 webview 气泡）。
//!
//! 流程：拿到选区文本 → 先画「翻译中」→ 后台翻译 → 单词再补词典释义 → 画结果。
//! 两个平台共用同一份 CardPayload，只是渲染端不同。

use crate::bubble_helper::{Phonetic, Sense};
use crate::{
    present_card, CardPayload, CardSense, PhonLabel,
};
use std::thread;
use tauri::AppHandle;

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
        "素笺".to_string()
    } else {
        format!("{detected} → {target} · {provider}")
    }
}

/// 一条「要怎么显示」的完整描述，macOS / Windows 共用。
pub struct Card {
    pub title: String,
    pub source: String,
    pub text: String,
    pub loading: bool,
    pub error: Option<String>,
    pub payload: CardPayload,
    pub lx: f64,
    pub ly: f64,
}

fn present(app: &AppHandle, card: Card) {
    #[cfg(target_os = "macos")]
    {
        let phonetics = card
            .payload
            .phonetics
            .iter()
            .map(|p| Phonetic { label: p.label.clone(), text: p.text.clone() })
            .collect::<Vec<_>>();
        let senses = card
            .payload
            .senses
            .iter()
            .map(|s| Sense {
                pos: s.pos.clone(),
                defs: s.defs.clone(),
                examples: s.examples.clone(),
                zh: s.zh,
            })
            .collect::<Vec<_>>();
        let shown = crate::bubble_helper::show_panel(
            card.title.clone(),
            card.source.clone(),
            card.text.clone(),
            card.loading,
            card.error.clone(),
            card.payload.mode.clone(),
            card.payload.phonetic.clone(),
            phonetics,
            card.payload.tags.clone(),
            card.payload.ai.clone(),
            senses,
            card.lx,
            card.ly,
        );
        if shown {
            return;
        }
    }
    // 非 macOS，或 macOS 助手起不来：退回 webview 气泡
    present_card(app, card);
}

fn blank(mode: &str) -> CardPayload {
    CardPayload { mode: mode.to_string(), ..Default::default() }
}

/// 翻译并展示：先画 loading，再画结果（单词附词典）。
pub fn translate_and_show(app: AppHandle, text: String, lx: f64, ly: f64) {
    let source = text.trim().to_string();
    if source.is_empty() {
        return;
    }
    let word_mode = is_single_word(&source);

    present(
        &app,
        Card {
            title: "翻译中".into(),
            source: source.clone(),
            text: String::new(),
            loading: true,
            error: None,
            payload: blank(if word_mode { "word" } else { "sentence" }),
            lx,
            ly,
        },
    );

    let cfg = crate::config::load();
    match crate::translate::run(&source, &cfg) {
        Ok(t) => {
            crate::history::record(&t);
            let mut card_payload = blank(if word_mode { "word" } else { "sentence" });
            if word_mode {
                let local = crate::dict_local::lookup(&source);
                let mut vocab = Vec::new();
                if let Some(z) = crate::translate::zh_lookup(&source) {
                    // 在线释义更全更口语，优先；音标/考试标签用本地库补齐
                    if !z.uk.is_empty() {
                        vocab.push(PhonLabel { label: "英".into(), text: format!("/{}/", z.uk) });
                    }
                    if !z.us.is_empty() && z.us != z.uk {
                        vocab.push(PhonLabel { label: "美".into(), text: format!("/{}/", z.us) });
                    }
                    if vocab.is_empty() {
                        if let Some(l) = &local {
                            if !l.phonetic.is_empty() {
                                vocab.push(PhonLabel { label: String::new(), text: format!("/{}/", l.phonetic) });
                            }
                        }
                    }
                    card_payload.senses = z
                        .senses
                        .into_iter()
                        .map(|s| CardSense { pos: s.pos, defs: s.defs, examples: s.examples, zh: s.zh })
                        .collect();
                    card_payload.tags = local.as_ref().map(|l| l.tags.clone()).unwrap_or_default();
                } else if let Some(l) = local {
                    // 断网 / 有道没收录：直接用内置词库
                    if !l.phonetic.is_empty() {
                        vocab.push(PhonLabel { label: String::new(), text: format!("/{}/", l.phonetic) });
                    }
                    card_payload.senses = crate::dict_local::senses_of(&l)
                        .into_iter()
                        .map(|(pos, items)| CardSense { pos, defs: items, examples: Vec::new(), zh: true })
                        .collect();
                    card_payload.tags = l.tags.clone();
                } else if let Ok(d) = crate::translate::lookup(&source) {
                    vocab = d
                        .phonetics
                        .into_iter()
                        .map(|p| PhonLabel { label: p.label, text: p.text })
                        .collect();
                    card_payload.senses = d
                        .senses
                        .into_iter()
                        .map(|s| CardSense { pos: s.pos, defs: s.defs, examples: s.examples, zh: s.zh })
                        .collect();
                }
                card_payload.phonetic = vocab.first().map(|p| p.text.clone()).unwrap_or_default();
                card_payload.phonetics = vocab;
            }
            present(
                &app,
                Card {
                    title: panel_title(&t.detected, &t.target, &t.provider),
                    source: source.clone(),
                    text: t.text.clone(),
                    loading: false,
                    error: None,
                    payload: card_payload,
                    lx,
                    ly,
                },
            );
        }
        Err(e) => {
            present(
                &app,
                Card {
                    title: "翻译失败".into(),
                    source: source.clone(),
                    text: String::new(),
                    loading: false,
                    error: Some(e),
                    payload: blank("sentence"),
                    lx,
                    ly,
                },
            );
        }
    }
}

/// 菜单栏「翻译剪贴板」：直接用当前光标位置弹结果。
pub fn translate_at_cursor(app: &AppHandle, text: &str) {
    let (lx, ly) = crate::last_cursor();
    let app = app.clone();
    let text = text.to_string();
    thread::spawn(move || translate_and_show(app, text, lx as f64, ly as f64));
}
