use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub fn versions_dir(app_dir: &Path) -> PathBuf {
    app_dir.join("versions")
}

fn slug_of(action: &str) -> &str {
    match action {
        "新增会员" => "add_member",
        "编辑会员" => "edit_member",
        "标记退卡" => "refund",
        "恢复会员" => "unrefund",
        "添加上课" => "add_class",
        "删除上课" => "del_class",
        "手动备份" => "manual",
        "恢复前自动备份" => "pre_restore",
        "更换表格前备份" => "pre_swap",
        "重试保存" => "retry",
        _ => "edit",
    }
}

/// 把磁盘上当前表格文件快照到 versions/，并写元数据（等价 Python _snapshot）
pub fn snapshot(excel_path: &Path, app_dir: &Path, action: &str, summary: &str, mid: Option<&str>) {
    let _ = std::fs::create_dir_all(versions_dir(app_dir));
    if !excel_path.is_file() {
        return;
    }
    let ts = crate::model::local_now().format("%Y%m%d_%H%M%S").to_string();
    let base_ts = format!("{}_{{}}_{}", ts, slug_of(action));
    let mut seq = 1u32;
    let base = loop {
        let cand = base_ts.replace("{}", &format!("{:02}", seq));
        if !versions_dir(app_dir).join(format!("{}.xlsx", cand)).exists() {
            break cand;
        }
        seq += 1;
    };
    let dst = versions_dir(app_dir).join(format!("{}.xlsx", base));
    if std::fs::copy(excel_path, &dst).is_err() {
        eprintln!("[WARN] snapshot failed");
        return;
    }
    let size = std::fs::metadata(&dst).map(|m| m.len()).unwrap_or(0);
    let meta = json!({
        "ts": ts,
        "action": action,
        "summary": summary,
        "mid": mid.map(|m| Value::String(m.to_string())).unwrap_or(Value::Null),
        "file": format!("{}.xlsx", base),
        "size": size,
    });
    let _ = std::fs::write(
        versions_dir(app_dir).join(format!("{}.json", base)),
        serde_json::to_vec_pretty(&meta).unwrap_or_default(),
    );
    prune(app_dir);
}

/// 超出上限时按修改时间裁剪最旧版本
pub fn prune(app_dir: &Path) {
    let dir = versions_dir(app_dir);
    let mut files: Vec<PathBuf> = match std::fs::read_dir(&dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().map(|x| x == "xlsx").unwrap_or(false))
            .collect(),
        Err(_) => return,
    };
    if files.len() <= crate::MAX_VERSIONS {
        return;
    }
    files.sort_by_key(|p| {
        std::fs::metadata(p)
            .and_then(|m| m.modified())
            .map(|t| t)
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
    });
    let excess = files.len() - crate::MAX_VERSIONS;
    for f in files.into_iter().take(excess) {
        let _ = std::fs::remove_file(&f);
        let j = f.with_extension("json");
        if j.is_file() {
            let _ = std::fs::remove_file(j);
        }
    }
}

/// 版本列表（新 -> 旧）
pub fn list(app_dir: &Path) -> Vec<Value> {
    let dir = versions_dir(app_dir);
    let mut out: Vec<Value> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.filter_map(|e| e.ok()) {
            let p = e.path();
            if p.extension().map(|x| x == "json").unwrap_or(false) {
                if let Ok(data) = std::fs::read(&p) {
                    if let Ok(v) = serde_json::from_slice::<Value>(&data) {
                        out.push(v);
                    }
                }
            }
        }
    }
    out.sort_by(|a, b| {
        let ka = a.get("ts").and_then(|x| x.as_str()).unwrap_or("");
        let kb = b.get("ts").and_then(|x| x.as_str()).unwrap_or("");
        kb.cmp(ka)
    });
    out
}

pub fn backup_note_snapshot(excel_path: &Path, app_dir: &Path, note: &str) {
    snapshot(excel_path, app_dir, "手动备份", note, None);
}
