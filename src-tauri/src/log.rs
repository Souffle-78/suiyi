use std::io::Write;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

fn path() -> std::path::PathBuf {
    crate::config::dir().join("slip.log")
}

static READY: OnceLock<()> = OnceLock::new();

pub fn line(msg: impl AsRef<str>) {
    let msg = msg.as_ref();
    eprintln!("[slip] {msg}");
    READY.get_or_init(|| {
        if let Some(parent) = path().parent() {
            let _ = std::fs::create_dir_all(parent);
        }
    });
    // 日志超过 512KB 就滚一次，避免无限增长
    if let Ok(meta) = std::fs::metadata(path()) {
        if meta.len() > 512 * 1024 {
            let _ = std::fs::remove_file(path());
        }
    }
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path())
    {
        let _ = writeln!(f, "{ts} {msg}");
    }
}
