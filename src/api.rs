use crate::store::{Store, StoreError};
use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub struct AppState {
    pub store: Arc<Mutex<Option<Store>>>,
    pub app_dir: PathBuf,
}

const INDEX_HTML: &str = include_str!("../static/index.html");

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/index.html", get(index))
        .route("/api/data", get(api_data))
        .route("/api/versions", get(api_versions))
        .route("/api/versions/download", get(api_versions_download))
        .route("/api/file", post(api_file))
        .route("/api/members", post(api_members))
        .route("/api/members/update", post(api_members_update))
        .route("/api/classes", post(api_classes))
        .route("/api/classes/delete", post(api_classes_delete))
        .route("/api/versions/backup", post(api_versions_backup))
        .route("/api/versions/restore", post(api_versions_restore))
        .route("/api/save", post(api_save))
        .route("/api/reload", post(api_reload))
        .route("/api/store-name", post(api_store_name))
        .route("/api/pending/recover", post(api_pending_recover))
        .route("/api/pending/discard", post(api_pending_discard))
        .with_state(state)
}

fn err(status: StatusCode, msg: &str) -> Response {
    (status, Json(json!({"ok": false, "error": msg}))).into_response()
}

fn biz(e: StoreError) -> Response {
    match e {
        StoreError::Biz(m) => err(StatusCode::BAD_REQUEST, &m),
        StoreError::Sys(m) => err(StatusCode::INTERNAL_SERVER_ERROR, &m),
    }
}

async fn index() -> Response {
    let mut headers = HeaderMap::new();
    headers.insert(header::CACHE_CONTROL, "no-cache".parse().unwrap());
    (headers, Html(INDEX_HTML)).into_response()
}

async fn api_data(State(st): State<Arc<AppState>>) -> Response {
    let mut guard = st.store.lock().unwrap();
    match guard.as_mut() {
        None => Json(json!({
            "ok": true,
            "no_file": true,
            "reason": format!("未找到同目录下名为「{}」的表格文件，请手动选择。", crate::DEFAULT_PREFIX),
            "store_name": crate::read_store_name(&st.app_dir),
        }))
        .into_response(),
        Some(s) => Json(json!({"ok": true, "data": s.data_json(), "store_name": crate::read_store_name(&st.app_dir)})).into_response(),
    }
}

async fn api_versions(State(st): State<Arc<AppState>>) -> Response {
    let guard = st.store.lock().unwrap();
    let versions = match &*guard {
        Some(s) => s.versions_json(),
        None => Vec::new(),
    };
    Json(json!({"ok": true, "versions": versions})).into_response()
}

async fn api_versions_download(
    State(st): State<Arc<AppState>>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let app_dir = st.app_dir.clone();
    let fn_arg = q.get("file").cloned().unwrap_or_default().replace('\\', "/");
    let safe = fn_arg.rsplit('/').next().unwrap_or("").to_string();
    if !safe.ends_with(".xlsx") {
        return (StatusCode::NOT_FOUND).into_response();
    }
    let p = crate::snapshot::versions_dir(&app_dir).join(&safe);
    if !p.is_file() {
        return (StatusCode::NOT_FOUND).into_response();
    }
    match std::fs::read(&p) {
        Ok(bytes) => (
            [(header::CONTENT_TYPE, "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet")],
            bytes,
        )
            .into_response(),
        Err(_) => (StatusCode::NOT_FOUND).into_response(),
    }
}

fn parse_body(bytes: &Bytes) -> Result<Value, Response> {
    if bytes.is_empty() {
        return Ok(Value::Object(serde_json::Map::new()));
    }
    serde_json::from_slice(bytes)
        .map_err(|_| err(StatusCode::BAD_REQUEST, "请求体不是合法 JSON"))
}

async fn api_file(State(st): State<Arc<AppState>>, body: Bytes) -> Response {
    let body = match parse_body(&body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let name_raw = body
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .replace('\\', "/");
    let name = name_raw.rsplit('/').next().unwrap_or("").trim().to_string();
    if name.is_empty() || !(name.to_lowercase().ends_with(".xlsx") || name.to_lowercase().ends_with(".xlsm")) {
        return err(StatusCode::BAD_REQUEST, "仅支持 .xlsx / .xlsm 格式的表格文件");
    }
    let data_b64 = body.get("data_b64").and_then(Value::as_str).unwrap_or("");
    let raw = match crate::b64_decode(data_b64) {
        Some(v) => v,
        None => return err(StatusCode::BAD_REQUEST, "文件内容解析失败，请重试"),
    };
    if raw.is_empty() {
        return err(StatusCode::BAD_REQUEST, "文件内容为空");
    }
    // 校验可解析 + 必须包含两个业务表
    {
        let cursor = std::io::Cursor::new(raw.clone());
        let wb = match calamine::Xlsx::new(cursor) {
            Ok(w) => w,
            Err(_) => return err(StatusCode::BAD_REQUEST, "无法解析该文件，请确认是有效的 Excel 表格"),
        };
        use calamine::Reader;
        let names = wb.sheet_names();
        let missing: Vec<&str> = [crate::SHEET_ARCHIVE, crate::SHEET_RECORDS]
            .iter()
            .filter(|s| !names.iter().any(|n| n == *s))
            .cloned()
            .collect();
        if !missing.is_empty() {
            return err(
                StatusCode::BAD_REQUEST,
                &format!("该表格缺少工作表：{}；不是本系统的会员表", missing.join("、")),
            );
        }
    }
    let dest = st.app_dir.join(&name);
    // 旧表先快照再切换；旧表即将退役，随切换释放其独占锁
    {
        let mut guard = st.store.lock().unwrap();
        if let Some(s) = guard.as_mut() {
            s.backup_current("更换表格前备份", &format!("更换为 {} 前的当前表格", name));
            s.release_file_lock();
        }
    }
    // 换表即放弃旧表的挂起变更（单槽位语义）
    crate::store::remove_pending_files(&st.app_dir);
    // 内存级完整加载校验：失败时不落盘、不影响当前表格
    let mut store = match Store::from_bytes(dest.clone(), st.app_dir.clone(), raw.clone()) {
        Ok(s) => s,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, &e),
    };
    // 校验全部通过后才原子写入目标位置（旧锁已释放，dest 与旧表同路径也安全）
    let tmp = dest.with_extension("xlsx.tmp");
    if let Err(e) = std::fs::write(&tmp, &raw).and_then(|_| std::fs::rename(&tmp, &dest)) {
        let _ = std::fs::remove_file(&tmp);
        return err(StatusCode::INTERNAL_SERVER_ERROR, &format!("OSError: {}", e));
    }
    store.acquire_file_lock();
    crate::save_config(&st.app_dir, &dest);
    let meta = store.meta();
    *st.store.lock().unwrap() = Some(store);
    Json(json!({"ok": true, "path": dest.display().to_string(), "meta": meta})).into_response()
}

async fn api_members(State(st): State<Arc<AppState>>, body: Bytes) -> Response {
    let body = match parse_body(&body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    // 与 Python 版一致的必填校验
    let name = body.get("name").and_then(Value::as_str).unwrap_or("").trim();
    if name.is_empty() {
        return err(StatusCode::BAD_REQUEST, "会员姓名必填");
    }
    let card = body.get("card_type").and_then(Value::as_str).unwrap_or("").trim();
    if card.is_empty() {
        return err(StatusCode::BAD_REQUEST, "卡种必填");
    }
    if card == "次卡" && !crate::model::to_num(body.get("total_sessions").unwrap_or(&Value::Null)).map(|v| v > 0.0).unwrap_or(false) {
        return err(StatusCode::BAD_REQUEST, "次卡必须填写大于 0 的总次数");
    }
    if card == "期限卡" && body.get("expiry_date").and_then(Value::as_str).and_then(crate::model::parse_date_str).is_none() {
        return err(StatusCode::BAD_REQUEST, "期限卡必须填写有效期至");
    }

    let mut guard = st.store.lock().unwrap();
    let store = match guard.as_mut() {
        Some(s) => s,
        None => return err(StatusCode::BAD_REQUEST, "尚未加载表格文件，请先选择表格文件"),
    };
    match store.add_member(&body) {
        Ok(mid) => {
            let saved = store.save("新增会员", &format!("新增 {}（{}）", name, mid), Some(&mid));
            match saved {
                Ok(saved) => {
                    let meta = store.meta();
                    Json(json!({"ok": true, "member_id": mid, "saved": saved, "meta": meta})).into_response()
                }
                Err(e) => biz(e),
            }
        }
        Err(e) => biz(e),
    }
}

async fn api_members_update(State(st): State<Arc<AppState>>, body: Bytes) -> Response {
    let body = match parse_body(&body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let mid = body.get("id").and_then(Value::as_str).unwrap_or("").to_string();
    let empty = serde_json::Map::new();
    let fields = body.get("fields").and_then(Value::as_object).unwrap_or(&empty);

    let mut guard = st.store.lock().unwrap();
    let store = match guard.as_mut() {
        Some(s) => s,
        None => return err(StatusCode::BAD_REQUEST, "尚未加载表格文件，请先选择表格文件"),
    };
    match store.update_member(&mid, fields) {
        Ok(()) => {
            let keys: Vec<String> = fields.keys().cloned().collect();
            let summary = format!("{}：{}", mid, if keys.is_empty() { "无变更字段".to_string() } else { keys.join("、") });
            let action = if fields.get("refund").map(|v| v.as_bool().unwrap_or(false)).unwrap_or(false) {
                "标记退卡"
            } else if fields.contains_key("refund") {
                "恢复会员"
            } else {
                "编辑会员"
            };
            match store.save(action, &summary, Some(&mid)) {
                Ok(saved) => {
                    let meta = store.meta();
                    Json(json!({"ok": true, "saved": saved, "meta": meta})).into_response()
                }
                Err(e) => biz(e),
            }
        }
        Err(e) => biz(e),
    }
}

async fn api_classes(State(st): State<Arc<AppState>>, body: Bytes) -> Response {
    let body = match parse_body(&body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let mut guard = st.store.lock().unwrap();
    let store = match guard.as_mut() {
        Some(s) => s,
        None => return err(StatusCode::BAD_REQUEST, "尚未加载表格文件，请先选择表格文件"),
    };
    let mid = body.get("member_id").and_then(Value::as_str).unwrap_or("").to_string();
    let date = body.get("date").and_then(Value::as_str).unwrap_or("").to_string();
    match store.add_class(&body) {
        Ok(()) => match store.save("添加上课", &format!("{} 上课 {}", mid, date), Some(&mid)) {
            Ok(saved) => {
                let meta = store.meta();
                Json(json!({"ok": true, "saved": saved, "meta": meta})).into_response()
            }
            Err(e) => biz(e),
        },
        Err(e) => biz(e),
    }
}

async fn api_classes_delete(State(st): State<Arc<AppState>>, body: Bytes) -> Response {
    let body = match parse_body(&body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let row = body.get("row").and_then(Value::as_i64);
    let mut guard = st.store.lock().unwrap();
    let store = match guard.as_mut() {
        Some(s) => s,
        None => return err(StatusCode::BAD_REQUEST, "尚未加载表格文件，请先选择表格文件"),
    };
    let row = match row {
        Some(r) if r >= 0 => r as u32,
        _ => return err(StatusCode::BAD_REQUEST, "行号无效"),
    };
    match store.delete_class(row) {
        Ok(()) => match store.save("删除上课", &format!("删除上课记录行 {}", row), None) {
            Ok(saved) => {
                let meta = store.meta();
                Json(json!({"ok": true, "saved": saved, "meta": meta})).into_response()
            }
            Err(e) => biz(e),
        },
        Err(e) => biz(e),
    }
}

async fn api_versions_backup(State(st): State<Arc<AppState>>, body: Bytes) -> Response {
    let body = match parse_body(&body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let note = body
        .get("note")
        .and_then(Value::as_str)
        .unwrap_or("手动备份当前版本")
        .to_string();
    let mut guard = st.store.lock().unwrap();
    let store = match guard.as_mut() {
        Some(s) => s,
        None => return err(StatusCode::BAD_REQUEST, "尚未加载表格文件，请先选择表格文件"),
    };
    store.backup_current("手动备份", &note);
    let versions = store.versions_json();
    Json(json!({"ok": true, "versions": versions})).into_response()
}

async fn api_versions_restore(State(st): State<Arc<AppState>>, body: Bytes) -> Response {
    let body = match parse_body(&body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let file = body.get("file").and_then(Value::as_str).unwrap_or("").to_string();
    let mut guard = st.store.lock().unwrap();
    let store = match guard.as_mut() {
        Some(s) => s,
        None => return err(StatusCode::BAD_REQUEST, "尚未加载表格文件，请先选择表格文件"),
    };
    match store.restore_version(&file) {
        Ok(()) => {
            let meta = store.meta();
            let versions = store.versions_json();
            Json(json!({"ok": true, "meta": meta, "versions": versions})).into_response()
        }
        Err(e) => biz(e),
    }
}

async fn api_save(State(st): State<Arc<AppState>>) -> Response {
    let mut guard = st.store.lock().unwrap();
    let store = match guard.as_mut() {
        Some(s) => s,
        None => return err(StatusCode::BAD_REQUEST, "尚未加载表格文件，请先选择表格文件"),
    };
    match store.save("重试保存", "重试保存待写变更", None) {
        Ok(saved) => {
            let meta = store.meta();
            Json(json!({"ok": saved, "meta": meta})).into_response()
        }
        Err(e) => biz(e),
    }
}

async fn api_reload(State(st): State<Arc<AppState>>) -> Response {
    let mut guard = st.store.lock().unwrap();
    let store = match guard.as_mut() {
        Some(s) => s,
        None => return err(StatusCode::BAD_REQUEST, "尚未加载表格文件，请先选择表格文件"),
    };
    match store.reload_from_disk() {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(e) => biz(e),
    }
}

async fn api_pending_recover(State(st): State<Arc<AppState>>) -> Response {
    let mut guard = st.store.lock().unwrap();
    let store = match guard.as_mut() {
        Some(s) => s,
        None => return err(StatusCode::BAD_REQUEST, "尚未加载表格文件，请先选择表格文件"),
    };
    match store.recover_pending() {
        Ok(()) => {
            let meta = store.meta();
            Json(json!({"ok": true, "meta": meta})).into_response()
        }
        Err(e) => biz(e),
    }
}

async fn api_pending_discard(State(st): State<Arc<AppState>>) -> Response {
    let mut guard = st.store.lock().unwrap();
    match guard.as_mut() {
        Some(s) => s.discard_pending(),
        None => crate::store::remove_pending_files(&st.app_dir),
    }
    Json(json!({"ok": true})).into_response()
}

/// 设置/清除门店名称：{name} → trim，空串=跳过/清除（合法），≤20 字（超长 400）
async fn api_store_name(State(st): State<Arc<AppState>>, body: Bytes) -> Response {
    let body = match parse_body(&body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let name = body
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if name.chars().count() > 20 {
        return err(StatusCode::BAD_REQUEST, "门店名称不能超过 20 个字");
    }
    crate::write_store_name(&st.app_dir, &name);
    Json(json!({"ok": true, "store_name": name})).into_response()
}
