//! 内置离线词库（ECDICT 精简版，MIT）。
//!
//! 只在查不到在线释义、或者断网时兜底；数据随安装包一起走 `resources/ecdict.sqlite`。

use rusqlite::{Connection, OpenFlags};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::OnceLock;

static DB: OnceLock<Option<Mutex<Connection>>> = OnceLock::new();

#[derive(Serialize, Clone, Debug)]
pub struct LocalWord {
    pub word: String,
    pub phonetic: String,
    /// ECDICT 的 translation：多行，每行一个词性的中文释义
    pub translation: String,
    /// 考试标签，已翻成中文：中考/高考/四级/六级/考研/托福/雅思/GRE
    pub tags: Vec<String>,
    pub collins: u8,
    pub exchange: String,
}

pub fn init(res_dir: PathBuf) {
    DB.get_or_init(|| {
        // Tauri 会把 resources 里的相对路径原样带上（resources/ecdict.sqlite），
        // 老版本/开发态可能在根目录，两种都试一下。
        let mut path = res_dir.join("resources").join("ecdict.sqlite");
        if !path.exists() {
            path = res_dir.join("ecdict.sqlite");
        }
        if !path.exists() {
            crate::log::line(&format!("local dict missing: {}", path.display()));
            return None;
        }
        match Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX) {
            Ok(conn) => {
                crate::log::line(&format!("local dict ready: {}", path.display()));
                Some(Mutex::new(conn))
            }
            Err(e) => {
                crate::log::line(&format!("local dict open failed: {e}"));
                None
            }
        }
    });
}

fn tag_labels(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    for t in raw.split_whitespace() {
        let label = match t {
            "zk" => "中考",
            "gk" => "高考",
            "cet4" => "四级",
            "cet6" => "六级",
            "ky" => "考研",
            "toefl" => "托福",
            "ielts" => "雅思",
            "gre" => "GRE",
            _ => continue,
        };
        if !out.iter().any(|x: &String| x == label) {
            out.push(label.to_string());
        }
    }
    out
}

/// 查一个英文词；查不到返回 None（调用方再走在线接口）。
pub fn lookup(word: &str) -> Option<LocalWord> {
    let db = DB.get()?.as_ref()?;
    let key = word.trim().to_lowercase();
    if key.is_empty() || key.len() > 48 {
        return None;
    }
    let conn = db.lock().ok()?;
    let mut stmt = conn
        .prepare_cached("SELECT word, phonetic, translation, tag, collins, exchange FROM words WHERE word = ?1")
        .ok()?;
    let row = stmt
        .query_row([&key], |r| {
            Ok(LocalWord {
                word: r.get::<_, String>(0)?,
                phonetic: r.get::<_, String>(1)?,
                translation: r.get::<_, String>(2)?,
                tags: tag_labels(&r.get::<_, String>(3).unwrap_or_default()),
                collins: r.get::<_, i64>(4).unwrap_or(0) as u8,
                exchange: r.get::<_, String>(5).unwrap_or_default(),
            })
        })
        .ok()?;
    Some(row)
}

/// 把 ECDICT 的多行释义切成「词性 + 义项列表」，交给卡片直接渲染。
pub fn senses_of(w: &LocalWord) -> Vec<(String, Vec<String>)> {
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for line in w.translation.split('\n') {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // 行首一般是 "n. " / "vt. " / "[网络] " / "adj. "
        let (pos, rest) = match line.find(' ') {
            Some(i) if i <= 8 => (line[..i].to_string(), line[i..].trim().to_string()),
            _ => (String::new(), line.to_string()),
        };
        let items: Vec<String> = rest
            .split('；')
            .flat_map(|s| s.split(';'))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if items.is_empty() {
            continue;
        }
        if let Some(slot) = out.iter_mut().find(|(p, _)| *p == pos) {
            slot.1.extend(items);
        } else {
            out.push((pos, items));
        }
    }
    out
}
