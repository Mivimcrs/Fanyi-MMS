//! HTTP 层契约测试：用 router().oneshot() 锁死与 Python 版兼容的接口行为。

use axum::body::Body;
use axum::http::{Request, StatusCode};
use laifanyi_core::api::{router, AppState};
use laifanyi_core::store::Store;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lfy_api_{}_{}", std::process::id(), name));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn fixture_bytes() -> Vec<u8> {
    std::fs::read(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixture.xlsx")).unwrap()
}

/// 每个测试独立 app：dir 内放 fixture 副本作为已加载表格
fn app_with_store(dir: PathBuf) -> Arc<AppState> {
    let p = dir.join("测试表格.xlsx");
    std::fs::write(&p, fixture_bytes()).unwrap();
    let store = Store::load(p, dir.clone()).unwrap();
    Arc::new(AppState {
        store: Arc::new(Mutex::new(Some(store))),
        app_dir: dir,
    })
}

fn app_without_store(dir: PathBuf) -> Arc<AppState> {
    Arc::new(AppState {
        store: Arc::new(Mutex::new(None)),
        app_dir: dir,
    })
}

async fn call(
    app: &Arc<AppState>,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let builder = Request::builder().method(method).uri(uri);
    let req = match body {
        Some(v) => builder
            .header("content-type", "application/json")
            .body(Body::from(v.to_string()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };
    let resp = router(app.clone()).oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let j = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, j)
}

fn ok(j: &Value) -> bool {
    j.get("ok").and_then(Value::as_bool).unwrap_or(false)
}

#[tokio::test]
async fn data_without_file_guides_setup() {
    let app = app_without_store(temp_dir("nofile"));
    let (st, j) = call(&app, "GET", "/api/data", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(j["no_file"], json!(true));
    assert!(j["reason"].as_str().unwrap().contains("莱·梵壹"));
}

#[tokio::test]
async fn data_with_file_returns_members_and_meta() {
    let app = app_with_store(temp_dir("withfile"));
    let (st, j) = call(&app, "GET", "/api/data", None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(ok(&j));
    assert_eq!(j["data"]["dashboard"]["total"], json!(5));
    assert_eq!(j["data"]["meta"]["member_capacity"], json!(503));
    assert_eq!(j["data"]["meta"]["class_capacity"], json!(1000));
    assert_eq!(j["data"]["meta"]["saved"], json!(true));
    assert_eq!(j["data"]["meta"]["pending_recovery"], Value::Null);
}

#[tokio::test]
async fn member_validation_errors() {
    let app = app_with_store(temp_dir("val"));
    // 姓名必填
    let (st, j) = call(&app, "POST", "/api/members",
        Some(json!({"card_type":"次卡","total_sessions":"10"}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert!(j["error"].as_str().unwrap().contains("姓名"));
    // 卡种必填
    let (st, _) = call(&app, "POST", "/api/members", Some(json!({"name":"甲"}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    // 次卡必须总次数 > 0
    let (st, j) = call(&app, "POST", "/api/members",
        Some(json!({"name":"甲","card_type":"次卡","total_sessions":"0"}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert!(j["error"].as_str().unwrap().contains("总次数"));
    // 期限卡必须有效期
    let (st, j) = call(&app, "POST", "/api/members",
        Some(json!({"name":"甲","card_type":"期限卡"}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert!(j["error"].as_str().unwrap().contains("有效期"));
}

#[tokio::test]
async fn member_add_update_persists_to_disk() {
    let dir = temp_dir("member");
    let app = app_with_store(dir.clone());
    let (st, j) = call(&app, "POST", "/api/members", Some(json!({
        "name":"接口员","phone":"13900000010","teacher":"测试师","member_type":"私教",
        "card_type":"次卡","purchase_date":"2026-08-29","total_sessions":"12",
        "receivable":"2400","amount":"600"
    }))).await;
    assert_eq!(st, StatusCode::OK);
    assert!(ok(&j));
    let mid = j["member_id"].as_str().unwrap().to_string();
    assert!(mid.starts_with('M'));
    assert_eq!(j["saved"], json!(true), "临时目录无占用，应直接落盘");

    // 落盘校验（全新加载）
    let st2 = Store::load(dir.join("测试表格.xlsx"), dir.clone()).unwrap();
    let m = st2.members.iter().find(|m| m.id == mid).unwrap();
    assert_eq!(m.name, "接口员");
    assert_eq!(m.amount, Some(600.0));
    assert_eq!(m.receivable, Some(2400.0));

    // 编辑
    let (st, j) = call(&app, "POST", "/api/members/update",
        Some(json!({"id": mid, "fields": {"name":"接口员改","remark":"接口备注"}}))).await;
    assert_eq!(st, StatusCode::OK);
    assert!(ok(&j));
    let st3 = Store::load(dir.join("测试表格.xlsx"), dir).unwrap();
    let m3 = st3.members.iter().find(|m| m.id == mid).unwrap();
    assert_eq!(m3.name, "接口员改");
    assert_eq!(m3.remark, "接口备注");
    // 编辑不存在的会员 -> 400
    let app2 = app_with_store(temp_dir("member2"));
    let (st, _) = call(&app2, "POST", "/api/members/update",
        Some(json!({"id":"M999","fields":{"name":"x"}}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn class_add_delete_flow() {
    let dir = temp_dir("class");
    let app = app_with_store(dir);
    let (st, _) = call(&app, "POST", "/api/classes", Some(json!({
        "member_id":"M001","date":"2026-08-29","time":"10:00","course":"接口课","effect":true
    }))).await;
    assert_eq!(st, StatusCode::OK);

    // 不存在的会员 -> 400
    let (st, j) = call(&app, "POST", "/api/classes",
        Some(json!({"member_id":"NOPE","date":"2026-08-29"}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert!(j["error"].as_str().unwrap().contains("不存在"));
    // 无效日期 -> 400
    let (st, _) = call(&app, "POST", "/api/classes",
        Some(json!({"member_id":"M001","date":"bad-date"}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    // 找到刚加的记录并删除
    let (_, j) = call(&app, "GET", "/api/data", None).await;
    let rec = j["data"]["classes"].as_array().unwrap()
        .iter()
        .find(|c| c["member_id"] == "M001" && c["course"] == "接口课")
        .unwrap()
        .clone();
    let row = rec["row"].as_u64().unwrap();
    let (st, j) = call(&app, "POST", "/api/classes/delete", Some(json!({"row": row}))).await;
    assert_eq!(st, StatusCode::OK);
    assert!(ok(&j));
    // 重复删除 -> 400
    let (st, _) = call(&app, "POST", "/api/classes/delete", Some(json!({"row": row}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    // 非法行号 -> 400
    let (st, _) = call(&app, "POST", "/api/classes/delete", Some(json!({"row": 1}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn file_upload_and_errors() {
    let dir = temp_dir("upload");
    let app = app_without_store(dir.clone());
    // 非法扩展名
    let (st, _) = call(&app, "POST", "/api/file",
        Some(json!({"name":"a.csv","data_b64":""}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    // 非法 base64
    let (st, _) = call(&app, "POST", "/api/file",
        Some(json!({"name":"a.xlsx","data_b64":"!!not-b64!!"}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    // 非法 zip 内容
    let (st, _) = call(&app, "POST", "/api/file",
        Some(json!({"name":"a.xlsx","data_b64":"aGVsbG8="}))).await; // "hello"
    assert_eq!(st, StatusCode::BAD_REQUEST);

    // 合法 fixture 上传全流程
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(fixture_bytes());
    let (st, j) = call(&app, "POST", "/api/file",
        Some(json!({"name":"上传表格.xlsx","data_b64": b64}))).await;
    assert_eq!(st, StatusCode::OK, "上传加载失败: {}", j);
    assert!(ok(&j));
    let (_, j) = call(&app, "GET", "/api/data", None).await;
    assert_eq!(j["data"]["dashboard"]["total"], json!(5));
    // config.json 已指向新表
    let cfg = std::fs::read_to_string(dir.join("config.json")).unwrap();
    assert!(cfg.contains("上传表格.xlsx"));
    // 无 .tmp 残留
    let leftovers: Vec<_> = std::fs::read_dir(&dir).unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map(|x| x == "tmp").unwrap_or(false))
        .collect();
    assert!(leftovers.is_empty(), "不应残留 .tmp 文件");
}

#[tokio::test]
async fn versions_backup_download_restore() {
    let dir = temp_dir("ver");
    let app = app_with_store(dir);
    // 手动备份
    let (st, j) = call(&app, "POST", "/api/versions/backup",
        Some(json!({"note":"接口测试"}))).await;
    assert_eq!(st, StatusCode::OK);
    let versions = j["versions"].as_array().unwrap();
    assert!(!versions.is_empty());
    let file = versions[0]["file"].as_str().unwrap().to_string();
    // 列表
    let (_, j) = call(&app, "GET", "/api/versions", None).await;
    assert!(j["versions"].as_array().unwrap().len() >= 1);
    // 下载
    let (st, _) = call(&app, "GET", &format!("/api/versions/download?file={}", file), None).await;
    assert_eq!(st, StatusCode::OK);
    // 路径穿越 -> 404
    let (st, _) = call(&app, "GET", "/api/versions/download?file=../evil.xlsx", None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = call(&app, "GET", "/api/versions/download?file=不存在的.xlsx", None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    // 恢复
    let (st, j) = call(&app, "POST", "/api/versions/restore", Some(json!({"file": file}))).await;
    assert_eq!(st, StatusCode::OK);
    assert!(ok(&j));
    // 恢复产生的"恢复前自动备份"也应在列表里
    let (_, j) = call(&app, "GET", "/api/versions", None).await;
    let actions: Vec<&str> = j["versions"].as_array().unwrap()
        .iter().filter_map(|v| v["action"].as_str()).collect();
    assert!(actions.contains(&"恢复前自动备份"));
}

#[tokio::test]
async fn save_and_reload() {
    let app = app_with_store(temp_dir("save"));
    let (st, j) = call(&app, "POST", "/api/save", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(j["ok"], json!(true));
    let (st, j) = call(&app, "POST", "/api/reload", None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(ok(&j));
    // 未加载表格 -> 400
    let app2 = app_without_store(temp_dir("save2"));
    let (st, _) = call(&app2, "POST", "/api/save", None).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = call(&app2, "POST", "/api/reload", None).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn pending_endpoints_contract() {
    let app = app_with_store(temp_dir("pending_api"));
    // 无挂起变更时恢复 -> 400
    let (st, j) = call(&app, "POST", "/api/pending/recover", None).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert!(j["error"].as_str().unwrap().contains("没有可恢复"));
    // 丢弃总是成功（幂等）
    let (st, j) = call(&app, "POST", "/api/pending/discard", None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(ok(&j));
    // 未加载表格时丢弃也成功
    let app2 = app_without_store(temp_dir("pending_api2"));
    let (st, j) = call(&app2, "POST", "/api/pending/discard", None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(ok(&j));
}

#[tokio::test]
async fn pending_recover_via_http() {
    let dir = temp_dir("pending_http");
    let p = dir.join("测试表格.xlsx");
    std::fs::write(&p, fixture_bytes()).unwrap();

    // 第一步：在独立 Store 里做内存变更并写挂起工件（模拟保存失败后进程退出）
    {
        let mut st = Store::load(p.clone(), dir.clone()).unwrap();
        let mid = st.add_member(&json!({
            "name":"恢复员","phone":"13900000099","card_type":"次卡",
            "total_sessions":"8","receivable":"800"
        })).unwrap();
        st.write_pending_artifacts("新增会员", "HTTP 恢复测试");
        let _ = mid;
    }
    // 第二步：启动 app，应检测到挂起变更
    let app = app_with_store(dir.clone());
    let (_, j) = call(&app, "GET", "/api/data", None).await;
    assert!(j["data"]["meta"]["pending_recovery"].is_object(), "应暴露恢复标记");
    assert_eq!(j["data"]["meta"]["pending_recovery"]["summary"], "HTTP 恢复测试");
    // 磁盘还是旧状态
    assert_eq!(j["data"]["dashboard"]["total"], json!(5));
    // 第三步：HTTP 恢复
    let (st, j) = call(&app, "POST", "/api/pending/recover", None).await;
    assert_eq!(st, StatusCode::OK, "恢复失败: {}", j);
    assert!(ok(&j));
    let (_, j) = call(&app, "GET", "/api/data", None).await;
    assert_eq!(j["data"]["dashboard"]["total"], json!(6), "恢复后应为 6 名会员");
    assert_eq!(j["data"]["meta"]["pending_recovery"], Value::Null);
    // 工件已清理
    assert!(!laifanyi_core::store::pending_bytes_path(&dir).is_file());
    assert!(!laifanyi_core::store::pending_meta_path(&dir).is_file());
}
