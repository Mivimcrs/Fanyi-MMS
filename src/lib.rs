pub mod api;
pub mod compute;
pub mod model;
pub mod shell;
pub mod snapshot;
pub mod store;
pub mod xlsx;

pub const DEFAULT_PREFIX: &str = "莱·梵壹会员实时跟踪管理系统";
pub const SHEET_ARCHIVE: &str = "会员档案";
pub const SHEET_RECORDS: &str = "上课记录";
pub const ARCHIVE_MAX_ROW: u32 = 504;
pub const RECORDS_MAX_ROW: u32 = 1001;
pub const MAX_VERSIONS: usize = 100;

use serde_json::Value;
use std::path::{Path, PathBuf};

pub fn b64_decode(s: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.decode(s).ok()
}

fn config_file(app_dir: &Path) -> PathBuf {
    app_dir.join("config.json")
}

/// 读取上次手动选择的表格路径（存相对路径，迁移后仍可用）
pub fn read_config(app_dir: &Path) -> Option<PathBuf> {
    let data = std::fs::read(config_file(app_dir)).ok()?;
    let v: Value = serde_json::from_slice(&data).ok()?;
    let p = v.get("excel_path")?.as_str()?.to_string();
    let full = if Path::new(&p).is_absolute() {
        PathBuf::from(&p)
    } else {
        app_dir.join(&p)
    };
    if full.is_file() {
        Some(full)
    } else {
        None
    }
}

/// 以相对路径保存（跨盘符时回退绝对路径），保证目录迁移后仍可定位
pub fn save_config(app_dir: &Path, excel_path: &Path) {
    let rel = match excel_path.strip_prefix(app_dir) {
        Ok(r) => r.display().to_string(),
        Err(_) => excel_path.display().to_string(),
    };
    let v = serde_json::json!({"excel_path": rel});
    let _ = std::fs::write(config_file(app_dir), serde_json::to_vec_pretty(&v).unwrap_or_default());
}

/// 同目录自动查找：优先精确名，其次前缀匹配（字典序第一个）
pub fn find_default_excel(app_dir: &Path) -> Option<PathBuf> {
    let exact = app_dir.join(format!("{}.xlsx", DEFAULT_PREFIX));
    if exact.is_file() {
        return Some(exact);
    }
    let mut cands: Vec<PathBuf> = std::fs::read_dir(app_dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.extension()
                    .map(|x| x == "xlsx" || x == "xlsm")
                    .unwrap_or(false)
                && p.file_name()
                    .map(|n| n.to_string_lossy().starts_with(DEFAULT_PREFIX))
                    .unwrap_or(false)
        })
        .collect();
    cands.sort();
    cands.into_iter().next()
}
