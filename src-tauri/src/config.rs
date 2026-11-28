use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct GlossaryEntry {
    pub from: String,
    pub to: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Config {
    pub target: String,
    pub provider: String,
    pub deepseek_key: String,
    pub deepseek_model: String,
    pub glossary: Vec<GlossaryEntry>,
    /// 上次展示欢迎气泡的版本号（同一版本只弹一次，别每次开机都弹）
    #[serde(default)]
    pub hello_version: String,
    /// 划词翻译总开关（主界面「设置 → 划词翻译」与菜单栏同步）
    #[serde(default = "default_true")]
    pub selection: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            target: "zh-CN".into(),
            provider: "google".into(),
            deepseek_key: String::new(),
            deepseek_model: "deepseek-chat".into(),
            glossary: Vec::new(),
            hello_version: String::new(),
            selection: true,
        }
    }
}

pub fn dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("slip")
}

pub fn path() -> PathBuf {
    dir().join("config.json")
}

/// 从旧名字（随译 / suiyi）的数据目录搬过来。
///
/// 改名之后数据目录从 `<config>/suiyi` 变成 `<config>/slip`，不搬的话老用户的历史、
/// 生词本、AI 缓存、API Key 全都会「消失」。只在**新目录还没有配置**时搬一次，
/// 之后用户在新版本里的改动不会被旧数据覆盖。
pub fn migrate_legacy() {
    let new_dir = dir();
    if new_dir.join("config.json").exists() {
        return;
    }
    let old_dir = match dirs::config_dir() {
        Some(base) => base.join("suiyi"),
        None => return,
    };
    if !old_dir.join("config.json").exists() {
        return;
    }
    if std::fs::create_dir_all(&new_dir).is_err() {
        return;
    }
    let mut copied = 0;
    if let Ok(entries) = std::fs::read_dir(&old_dir) {
        for entry in entries.flatten() {
            let from = entry.path();
            if !from.is_file() {
                continue;
            }
            let name = match from.file_name() {
                Some(n) => n,
                None => continue,
            };
            // 旧日志不搬：新版本写 slip.log，搬过去只是白占空间
            if name.to_string_lossy().ends_with(".log") {
                continue;
            }
            let to = new_dir.join(name);
            if to.exists() {
                continue;
            }
            if std::fs::copy(&from, &to).is_ok() {
                copied += 1;
            }
        }
    }
    if copied > 0 {
        crate::log::line(format!("migrated {copied} files from {}", old_dir.display()));
    }
}

pub fn load() -> Config {
    std::fs::read_to_string(path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(cfg: &Config) -> Result<(), String> {
    let d = dir();
    std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    let s = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    std::fs::write(path(), s).map_err(|e| e.to_string())
}

fn default_true() -> bool {
    true
}
