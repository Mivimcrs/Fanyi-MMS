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

use serde_json::{json, Value};
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

/// 读取 config.json 的对象值；不存在或非 JSON 时返回空对象
fn read_config_obj(app_dir: &Path) -> Value {
    std::fs::read(config_file(app_dir))
        .ok()
        .and_then(|d| serde_json::from_slice::<Value>(&d).ok())
        .unwrap_or_else(|| json!({}))
}

/// 写 config.json（保留已有字段如 store_name，不因换表而丢失）
fn write_config(app_dir: &Path, v: &Value) {
    let _ = std::fs::write(config_file(app_dir), serde_json::to_vec_pretty(v).unwrap_or_default());
}

/// 以相对路径保存（跨盘符时回退绝对路径），保证目录迁移后仍可定位
pub fn save_config(app_dir: &Path, excel_path: &Path) {
    let rel = match excel_path.strip_prefix(app_dir) {
        Ok(r) => r.display().to_string(),
        Err(_) => excel_path.display().to_string(),
    };
    let mut v = read_config_obj(app_dir);
    v["excel_path"] = json!(rel);
    write_config(app_dir, &v);
}

/// 门店名称（如 name.is_some() 表示此前设置或跳过后为 ""）
pub fn read_store_name(app_dir: &Path) -> Option<String> {
    let v = read_config_obj(app_dir);
    v.get("store_name").and_then(Value::as_str).map(|s| s.to_string())
}

/// 写门店名称；空串表示用户选择跳过
pub fn write_store_name(app_dir: &Path, name: &str) {
    let mut v = read_config_obj(app_dir);
    v["store_name"] = json!(name);
    write_config(app_dir, &v);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lfy_cfg_{}_{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn store_name_read_write_roundtrip() {
        let dir = temp_dir("roundtrip");
        // 首次运行：无 config → None（触发前端首窗）
        assert_eq!(read_store_name(&dir), None);
        write_store_name(&dir, "万达广场店");
        assert_eq!(read_store_name(&dir).as_deref(), Some("万达广场店"));
        // 跳过 → 空串；再次显示为已设置（非 None），前端不再自动弹
        write_store_name(&dir, "");
        assert_eq!(read_store_name(&dir).as_deref(), Some(""));
    }

    #[test]
    fn save_config_preserves_store_name() {
        let dir = temp_dir("preserve");
        write_store_name(&dir, "财富广场店");
        save_config(&dir, Path::new(dir.join("表格.xlsx").as_path()));
        assert_eq!(read_store_name(&dir).as_deref(), Some("财富广场店"));
        let v = read_config_obj(&dir);
        assert_eq!(v["excel_path"].as_str().unwrap(), "表格.xlsx");
    }
}
