//! 「AI 精解」：用 DeepSeek 生成更贴近语境的中文释义、例句与搭配。
//! 只在用户点按钮时调用，结果缓存在 ai-cache.json，避免同一词反复花钱。

use serde_json::json;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

static CACHE: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

fn cache_path() -> PathBuf {
    let mut p = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
    p.push("suiyi");
    p.push("ai-cache.json");
    p
}

fn load_cache() -> HashMap<String, String> {
    let mut guard = CACHE.lock().unwrap();
    if guard.is_none() {
        let map = std::fs::read_to_string(cache_path())
            .ok()
            .and_then(|s| serde_json::from_str::<HashMap<String, String>>(&s).ok())
            .unwrap_or_default();
        *guard = Some(map);
    }
    guard.clone().unwrap_or_default()
}

fn save_cache(map: &HashMap<String, String>) {
    let mut guard = CACHE.lock().unwrap();
    *guard = Some(map.clone());
    if let Some(dir) = cache_path().parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(s) = serde_json::to_string_pretty(map) {
        let _ = std::fs::write(cache_path(), s);
    }
}

const PROMPT: &str = "你是给中文母语者看的中英词典编辑。针对用户给的英文单词或短语，输出简洁的中文精解，严格按下面格式，不要寒暄、不要markdown标题：\n\
第1行：按词性给中文释义，格式如 `n. 行动；行为  v. 处理`（最多6条义项，短语式，不要长句）\n\
第2行起：`例：<英文例句> — <中文翻译>`\n\
然后一行：`搭配：<搭配1>；<搭配2>`\n\
如果词有易混点，最后加一行：`提示：<一句话>`\n\
总长度不超过220字。";

pub fn explain(word: &str) -> Result<String, String> {
    let key = word.trim();
    if key.is_empty() {
        return Err("没有要解释的词".into());
    }
    let cache = load_cache();
    if let Some(hit) = cache.get(&key.to_lowercase()) {
        return Ok(hit.clone());
    }
    let cfg = crate::config::load();
    if cfg.deepseek_key.trim().is_empty() {
        return Err("还没填 DeepSeek API Key（设置 → 精翻）".into());
    }
    let body = json!({
        "model": if cfg.deepseek_model.trim().is_empty() { "deepseek-chat" } else { cfg.deepseek_model.trim() },
        "messages": [
            {"role": "system", "content": PROMPT},
            {"role": "user", "content": key}
        ],
        "temperature": 0.4,
        "max_tokens": 400
    });
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(25))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .post("https://api.deepseek.com/chat/completions")
        .bearer_auth(cfg.deepseek_key.trim())
        .json(&body)
        .send()
        .map_err(|e| format!("请求失败：{e}"))?;
    let status = resp.status();
    let text = resp.text().unwrap_or_default();
    if !status.is_success() {
        return Err(format!("DeepSeek 返回 {status}：{}", text.chars().take(160).collect::<String>()));
    }
    let data: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let content = data["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("")
        .trim()
        .to_string();
    if content.is_empty() {
        return Err("AI 没有返回内容".into());
    }
    let mut map = load_cache();
    map.insert(key.to_lowercase(), content.clone());
    save_cache(&map);
    Ok(content)
}
