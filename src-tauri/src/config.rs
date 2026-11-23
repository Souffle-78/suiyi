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
        .join("suiyi")
}

pub fn path() -> PathBuf {
    dir().join("config.json")
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
