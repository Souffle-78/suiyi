use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader};
use std::time::{SystemTime, UNIX_EPOCH};

/// 生词本条目：原文 + 译文 + 语境（可选），用于以后复习/导出 Anki。
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct VocabEntry {
    pub ts: u64,
    pub source: String,
    pub text: String,
    #[serde(default)]
    pub detected: String,
    #[serde(default)]
    pub target: String,
}

const MAX_LINES: usize = 5000;

fn path() -> std::path::PathBuf {
    crate::config::dir().join("vocab.jsonl")
}

fn read_all() -> Vec<VocabEntry> {
    let Ok(f) = std::fs::File::open(path()) else {
        return Vec::new();
    };
    BufReader::new(f)
        .lines()
        .flatten()
        .filter_map(|line| serde_json::from_str::<VocabEntry>(&line).ok())
        .collect()
}

fn write_all(list: &[VocabEntry]) -> Result<(), String> {
    let p = path();
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut buf = String::new();
    for e in list {
        let line = serde_json::to_string(e).map_err(|e| e.to_string())?;
        buf.push_str(&line);
        buf.push('\n');
    }
    std::fs::write(&p, buf).map_err(|e| e.to_string())
}

/// 存一条生词；同一段原文只留一条（重复保存返回 Ok(false)）。
pub fn add(source: &str, text: &str, detected: &str, target: &str) -> Result<bool, String> {
    let source = source.trim();
    if source.is_empty() {
        return Ok(false);
    }
    let mut list = read_all();
    if list.iter().any(|e| e.source == source) {
        return Ok(false);
    }
    list.push(VocabEntry {
        ts: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        source: source.to_string(),
        text: text.trim().to_string(),
        detected: detected.to_string(),
        target: target.to_string(),
    });
    if list.len() > MAX_LINES {
        let drop = list.len() - MAX_LINES;
        list.drain(0..drop);
    }
    write_all(&list)?;
    Ok(true)
}

pub fn list(limit: usize) -> Vec<VocabEntry> {
    let mut all = read_all();
    all.reverse();
    all.truncate(limit);
    all
}

pub fn remove(ts: u64) -> Result<(), String> {
    let mut all = read_all();
    all.retain(|e| e.ts != ts);
    write_all(&all)
}

pub fn clear() -> Result<(), String> {
    let p = path();
    if p.exists() {
        std::fs::remove_file(p).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 导出成 Markdown 表格，方便粘到笔记里或转 Anki
pub fn export_markdown() -> String {
    let mut out = String::from("| 原文 | 译文 | 语言 | 时间 |\n| --- | --- | --- | --- |\n");
    for e in list(usize::MAX) {
        let esc = |s: &str| s.replace('|', "\\|").replace('\n', " ");
        out.push_str(&format!(
            "| {} | {} | {} → {} | {} |\n",
            esc(&e.source),
            esc(&e.text),
            e.detected,
            e.target,
            e.ts
        ));
    }
    out
}
