use crate::translate::Translated;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Entry {
    pub ts: u64,
    pub source: String,
    pub text: String,
    pub detected: String,
    pub target: String,
    pub provider: String,
}

const MAX_LINES: usize = 2000;

fn path() -> std::path::PathBuf {
    crate::config::dir().join("history.jsonl")
}

pub fn record(t: &Translated) {
    let entry = Entry {
        ts: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        source: t.source.clone(),
        text: t.text.clone(),
        detected: t.detected.clone(),
        target: t.target.clone(),
        provider: t.provider.clone(),
    };
    let Ok(line) = serde_json::to_string(&entry) else {
        return;
    };
    let p = path();
    if let Some(parent) = p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let too_many = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0) > 512 * 1024;
    if too_many {
        if let Ok(list) = read_all() {
            let keep: Vec<Entry> = list.into_iter().rev().take(MAX_LINES / 2).collect();
            let mut buf = String::new();
            for e in keep.into_iter().rev() {
                if let Ok(s) = serde_json::to_string(&e) {
                    buf.push_str(&s);
                    buf.push('\n');
                }
            }
            let _ = std::fs::write(&p, buf);
        }
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&p) {
        let _ = writeln!(f, "{line}");
    }
}

fn read_all() -> Result<Vec<Entry>, String> {
    let p = path();
    let f = match std::fs::File::open(&p) {
        Ok(f) => f,
        Err(_) => return Ok(Vec::new()),
    };
    let mut out = Vec::new();
    for line in BufReader::new(f).lines().flatten() {
        if let Ok(e) = serde_json::from_str::<Entry>(&line) {
            out.push(e);
        }
    }
    Ok(out)
}

pub fn list(limit: usize) -> Vec<Entry> {
    let mut all = read_all().unwrap_or_default();
    all.reverse();
    all.truncate(limit);
    all
}

pub fn clear() -> Result<(), String> {
    let p = path();
    if p.exists() {
        std::fs::remove_file(p).map_err(|e| e.to_string())?;
    }
    Ok(())
}
