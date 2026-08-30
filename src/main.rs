// Windows release 版隐藏控制台（退出走托盘菜单；调试版仍显示控制台）
#![cfg_attr(all(not(debug_assertions), target_os = "windows"), windows_subsystem = "windows")]

use laifanyi_core::api::{router, AppState};
use laifanyi_core::store::Store;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

fn main() {
    // ---- 参数 ----
    let mut excel_arg: Option<PathBuf> = None;
    let mut port: u16 = 8688;
    let mut no_browser = false;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--excel" => excel_arg = it.next().map(PathBuf::from),
            "--port" => port = it.next().and_then(|v| v.parse().ok()).unwrap_or(8688),
            "--no-browser" => no_browser = true,
            other => {
                eprintln!("[WARN] unknown arg: {}", other);
            }
        }
    }

    // ---- 程序目录（exe 同目录，便携式） ----
    let app_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    // ---- 单实例锁（重复双击 = 直接拉起浏览器） ----
    let mut _keep_lock: Option<std::fs::File> = None;
    match std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(app_dir.join(".laifanyi.lock"))
    {
        Ok(file) => {
            use fs2::FileExt;
            if file.try_lock_exclusive().is_err() {
                let running_port: u16 = std::fs::read_to_string(app_dir.join(".laifanyi.port"))
                    .ok()
                    .and_then(|s| s.trim().parse().ok())
                    .unwrap_or(8688);
                let url = format!("http://127.0.0.1:{}", running_port);
                println!("[i] already running, opening {}", url);
                let _ = open::that(url);
                return;
            }
            _keep_lock = Some(file);
        }
        Err(_) => { /* 无法创建锁文件时照常运行 */ }
    }

    // ---- 端口（被占则顺延，不杀进程） ----
    let listener = (port..port.saturating_add(50))
        .find_map(|p| std::net::TcpListener::bind(("127.0.0.1", p)).ok())
        .expect("no free port");
    let bound = listener.local_addr().map(|a| a.port()).unwrap_or(port);
    let _ = std::fs::write(app_dir.join(".laifanyi.port"), bound.to_string());

    // ---- 表格加载：--excel > 同目录精确/前缀匹配 > config.json ----
    let excel_path = match excel_arg {
        Some(p) if p.is_file() => {
            println!("[OK] excel from --excel: {}", p.display());
            Some(p)
        }
        _ => match laifanyi_core::find_default_excel(&app_dir) {
            Some(p) => {
                println!("[OK] excel auto-detected: {}", p.display());
                Some(p)
            }
            None => match laifanyi_core::read_config(&app_dir) {
                Some(p) => {
                    println!("[OK] excel from config.json: {}", p.display());
                    Some(p)
                }
                None => None,
            },
        },
    };

    let store = match excel_path {
        Some(p) => match Store::load(p, app_dir.clone()) {
            Ok(mut s) => {
                let data = s.data_json();
                let dash = &data["dashboard"];
                let n_members = dash["total"].as_u64().unwrap_or(0);
                let n_classes = data["classes"].as_array().map(|a| a.len()).unwrap_or(0);
                let liability = dash["liability"].as_f64().unwrap_or(0.0);
                println!("[OK] members={} classes={} liability={:.2}", n_members, n_classes, liability);
                Some(s)
            }
            Err(e) => {
                laifanyi_core::shell::fatal_error(
                    "莱·梵壹会员系统启动失败",
                    &format!(
                        "表格文件加载失败：{}\n\n请确认表格文件存在、未被 Excel/WPS 独占损坏，\n或将其放到本程序同一文件夹后重试。",
                        e
                    ),
                );
                std::process::exit(1);
            }
        },
        None => {
            println!("[WARN] no excel found; web UI will guide manual selection");
            laifanyi_core::shell::info_box(
                "未找到表格文件",
                "没有在程序文件夹找到「莱·梵壹会员实时跟踪管理系统.xlsx」。\n浏览器打开后，请按页面提示选择表格文件（选择后会自动复制到程序目录）。",
            );
            None
        }
    };

    let state = Arc::new(AppState {
        store: Arc::new(Mutex::new(store)),
        app_dir: app_dir.clone(),
    });

    // ---- 自动打开浏览器 ----
    if !no_browser {
        let url = format!("http://127.0.0.1:{}", bound);
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(1500));
            let _ = open::that(url);
        });
    }

    // ---- 托盘（打开网页 / 打开数据文件夹 / 退出） ----
    let tray_url = format!("http://127.0.0.1:{}", bound);
    laifanyi_core::shell::spawn_tray(tray_url, app_dir.clone());

    println!("[OK] serving at http://127.0.0.1:{} (close this window to stop)", bound);
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(async move {
        listener.set_nonblocking(true).expect("set nonblocking");
        let listener = tokio::net::TcpListener::from_std(listener).expect("tokio listener");
        axum::serve(listener, router(state)).await.expect("server error");
    });
}
