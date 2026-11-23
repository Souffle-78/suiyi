use crate::config::Config;
use serde::Serialize;
use std::time::Duration;

#[derive(Serialize, Clone, Debug)]
pub struct Translated {
    pub source: String,
    pub text: String,
    pub detected: String,
    pub target: String,
    pub provider: String,
}

const MAX_CHARS: usize = 4000;

fn language_name(code: &str) -> &'static str {
    match code {
        "zh-CN" => "简体中文",
        "zh-TW" => "繁体中文",
        "en" => "英语",
        "ja" => "日语",
        "ko" => "韩语",
        "fr" => "法语",
        "de" => "德语",
        "es" => "西班牙语",
        "ru" => "俄语",
        "pt" => "葡萄牙语",
        _ => "中文",
    }
}

pub fn run(text: &str, cfg: &Config) -> Result<Translated, String> {
    match cfg.provider.as_str() {
        "deepseek" => deepseek(text, cfg),
        _ => google(text, &cfg.target),
    }
}

fn client(secs: u64) -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(secs))
        .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36")
        .build()
        .map_err(|e| e.to_string())
}

/// 免费线路：Google 翻译的公开接口，不需要 key。
/// 两个入口轮换使用（clients5 更快、gtx 更稳），任一 429 时换另一个重试。
fn google(text: &str, target: &str) -> Result<Translated, String> {
    let clipped: String = text.chars().take(MAX_CHARS).collect();
    if clipped.trim().is_empty() {
        return Err("empty".into());
    }
    let mut last = String::new();
    for attempt in 0..3u64 {
        let result = if attempt % 2 == 0 {
            google_clients5(&clipped, target)
        } else {
            google_gtx(&clipped, target)
        };
        match result {
            Ok(t) => return Ok(t),
            Err(e) => {
                last = e;
                std::thread::sleep(Duration::from_millis(180 * (attempt + 1)));
            }
        }
    }
    // Google 全挂了（被墙 / 429）→ 用免 key 的 MyMemory 兜底
    match my_memory(&clipped, target) {
        Ok(t) => Ok(t),
        Err(e) => Err(format!("{last}；备用线路也失败：{e}")),
    }
}

fn google_clients5(text: &str, target: &str) -> Result<Translated, String> {
    let url = format!(
        "https://clients5.google.com/translate_a/t?client=dict-chrome-ex&sl=auto&tl={}&q={}",
        urlencoding::encode(target),
        urlencoding::encode(text)
    );
    let resp = client(8)?.get(&url).send().map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let data: serde_json::Value = resp.json().map_err(|e| e.to_string())?;
    let (out, detected) = parse_clients5(&data).ok_or_else(|| "no translation".to_string())?;
    Ok(Translated {
        source: text.to_string(),
        text: out,
        detected,
        target: target.to_string(),
        provider: "google".into(),
    })
}

/// clients5 / dict-chrome-ex 的返回：[[译文片段, 语言], ...]
fn parse_clients5(data: &serde_json::Value) -> Option<(String, String)> {
    let mut out = String::new();
    let mut detected = "auto".to_string();
    for (i, item) in data.as_array()?.iter().enumerate() {
        if let Some(seg) = item.get(0).and_then(|v| v.as_str()) {
            out.push_str(seg);
            if i == 0 {
                if let Some(lang) = item.get(1).and_then(|v| v.as_str()) {
                    detected = lang.to_string();
                }
            }
        }
    }
    if out.trim().is_empty() {
        return None;
    }
    Some((out, detected))
}

fn google_gtx(text: &str, target: &str) -> Result<Translated, String> {
    let url = format!(
        "https://translate.googleapis.com/translate_a/single?client=gtx&sl=auto&tl={}&dt=t&q={}",
        urlencoding::encode(target),
        urlencoding::encode(text)
    );
    let resp = client(9)?.get(&url).send().map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let data: serde_json::Value = resp.json().map_err(|e| e.to_string())?;
    let (out, detected) = parse_gtx(&data).ok_or_else(|| "no translation".to_string())?;
    Ok(Translated {
        source: text.to_string(),
        text: out,
        detected,
        target: target.to_string(),
        provider: "google".into(),
    })
}

/// gtx 的返回：[[[译文, 原文, ...], ...], null, "en", ...]
fn parse_gtx(data: &serde_json::Value) -> Option<(String, String)> {
    let mut out = String::new();
    for seg in data.get(0)?.as_array()? {
        if let Some(s) = seg.get(0).and_then(|v| v.as_str()) {
            out.push_str(s);
        }
    }
    if out.trim().is_empty() {
        return None;
    }
    let detected = data
        .get(2)
        .and_then(|v| v.as_str())
        .unwrap_or("auto")
        .to_string();
    Some((out, detected))
}

/// 免费备用线路：MyMemory（免 key，日额度有限，够兜底用）。
/// Google 被墙或 429 时自动切到这里，避免「划词了但没反应」。
pub fn my_memory(text: &str, target: &str) -> Result<Translated, String> {
    let source = guess_source(text);
    let url = format!(
        "https://api.mymemory.translated.net/get?q={}&langpair={}|{}",
        urlencoding::encode(text),
        urlencoding::encode(&source),
        urlencoding::encode(target)
    );
    let resp = client(9)?.get(&url).send().map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let data: serde_json::Value = resp.json().map_err(|e| e.to_string())?;
    let out = data
        .get("responseData")
        .and_then(|v| v.get("translatedText"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_default();
    if out.trim().is_empty() || out.to_lowercase().contains("mymemory warning") {
        return Err("no translation".into());
    }
    Ok(Translated {
        source: text.to_string(),
        text: out,
        detected: source,
        target: target.to_string(),
        provider: "mymemory".into(),
    })
}

/// MyMemory 不支持自动识别语种，这里做个粗判：有中日韩字符就当目标语是源语，否则按英文。
fn guess_source(text: &str) -> String {
    let cjk = text.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c));
    if cjk { "zh-CN".to_string() } else { "en".to_string() }
}

/// 精翻线路：DeepSeek 对话接口，可带术语表。
fn deepseek(text: &str, cfg: &Config) -> Result<Translated, String> {
    let clipped: String = text.chars().take(MAX_CHARS).collect();
    if clipped.trim().is_empty() {
        return Err("empty".into());
    }
    if cfg.deepseek_key.trim().is_empty() {
        return Err("还没填 DeepSeek API Key（设置 → 精翻）".into());
    }

    let mut glossary = String::new();
    for g in &cfg.glossary {
        if !g.from.trim().is_empty() {
            glossary.push_str(&format!("- {} → {}\n", g.from.trim(), g.to.trim()));
        }
    }
    let mut system = format!(
        "你是专业翻译。把用户给的内容翻译成{}，只输出译文本身，不要解释、不要加引号。保持原文的换行与代码/变量名不变。",
        language_name(&cfg.target)
    );
    if !glossary.is_empty() {
        system.push_str("\n术语表（必须严格按此译法）：\n");
        system.push_str(&glossary);
    }
    if clipped.len() > 600 {
        system.push_str("\n这段内容较长，请保持术语与句式前后一致。");
    }

    let body = serde_json::json!({
        "model": if cfg.deepseek_model.trim().is_empty() { "deepseek-chat" } else { cfg.deepseek_model.trim() },
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": clipped}
        ],
        "temperature": 1.0,
        "stream": false
    });

    let resp = client(30)?
        .post("https://api.deepseek.com/chat/completions")
        .bearer_auth(cfg.deepseek_key.trim())
        .json(&body)
        .send()
        .map_err(|e| e.to_string())?;

    let status = resp.status();
    let data: serde_json::Value = resp.json().map_err(|e| e.to_string())?;
    if !status.is_success() {
        let msg = data
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
            .unwrap_or("DeepSeek 请求失败");
        return Err(format!("{msg}（HTTP {status}）"));
    }
    let out = data
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if out.is_empty() {
        return Err("DeepSeek 返回为空".into());
    }
    Ok(Translated {
        source: clipped,
        text: out,
        detected: "auto".into(),
        target: cfg.target.clone(),
        provider: "deepseek".into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_clients5_segments() {
        let v: serde_json::Value =
            serde_json::from_str(r#"[["你好","en"],["，世界","en"]]"#).unwrap();
        let (text, lang) = parse_clients5(&v).expect("should parse");
        assert_eq!(text, "你好，世界");
        assert_eq!(lang, "en");
    }

    #[test]
    fn parses_gtx_segments() {
        let v: serde_json::Value = serde_json::from_str(
            r#"[[["你好","Hello",null,null,10],["，世界"," world",null,null,10]],null,"en"]"#,
        )
        .unwrap();
        let (text, lang) = parse_gtx(&v).expect("should parse");
        assert_eq!(text, "你好，世界");
        assert_eq!(lang, "en");
    }

    #[test]
    fn rejects_empty_payloads() {
        let empty: serde_json::Value = serde_json::from_str("[]").unwrap();
        assert!(parse_clients5(&empty).is_none());
        assert!(parse_gtx(&empty).is_none());
        let junk: serde_json::Value = serde_json::from_str(r#"{"error":"x"}"#).unwrap();
        assert!(parse_clients5(&junk).is_none());
    }

    #[test]
    fn guesses_source_language() {
        assert_eq!(guess_source("hello world"), "en");
        assert_eq!(guess_source("你好，世界"), "zh-CN");
        assert_eq!(guess_source("混合 mixed"), "zh-CN");
    }
}
