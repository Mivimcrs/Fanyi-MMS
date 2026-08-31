//! 桌面集成：托盘菜单、消息泵、启动失败弹窗。
//! Windows：托盘 + Win32 消息泵；release 版隐藏控制台后用户通过托盘退出。
//! macOS：暂不启托盘（需 NSApp 主线程事件循环，留待真机阶段），终端运行 + 关闭即退出。

use std::path::PathBuf;

/// 启动失败时给出可见提示。无控制台的 Windows release 版必须用消息框。
pub fn fatal_error(title: &str, msg: &str) {
    #[cfg(all(not(debug_assertions), target_os = "windows"))]
    {
        rfd::MessageDialog::new()
            .set_title(title)
            .set_description(msg)
            .show();
    }
    eprintln!("[ERROR] {}: {}", title, msg);
}

/// 非致命提示（如缺表格文件时的引导说明）
pub fn info_box(title: &str, msg: &str) {
    #[cfg(all(not(debug_assertions), target_os = "windows"))]
    {
        rfd::MessageDialog::new()
            .set_title(title)
            .set_description(msg)
            .show();
    }
    eprintln!("[INFO] {}: {}", title, msg);
}

/// Windows 托盘：打开网页 / 打开数据文件夹 / 退出
#[cfg(windows)]
pub fn spawn_tray(open_url: String, data_dir: PathBuf) {
    std::thread::spawn(move || {
        if let Err(e) = tray_main(open_url, data_dir) {
            eprintln!("[WARN] tray unavailable: {}", e);
        }
    });
}

#[cfg(not(windows))]
pub fn spawn_tray(_open_url: String, _data_dir: PathBuf) {}

#[cfg(windows)]
fn tray_main(open_url: String, data_dir: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    use tray_icon::menu::{Menu, MenuEvent, MenuItem};
    use tray_icon::TrayIconBuilder;

    let open_item = MenuItem::with_id("open", "打开网页", true, None);
    let folder_item = MenuItem::with_id("folder", "打开数据文件夹", true, None);
    let quit_item = MenuItem::with_id("quit", "退出", true, None);
    let menu = Menu::with_items(&[&open_item, &folder_item, &quit_item])?;

    let icon = load_tray_icon()?;

    let _tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_icon(icon)
        .with_tooltip("莱·梵壹会员系统")
        .build()?;

    // Win32 消息泵（托盘窗口的消息必须由创建线程处理），同时轮询菜单事件
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE,
    };
    let receiver = MenuEvent::receiver();
    loop {
        while let Ok(ev) = receiver.try_recv() {
            match ev.id.as_ref() {
                "open" => {
                    let _ = open::that(&open_url);
                }
                "folder" => {
                    let _ = open::that(&data_dir);
                }
                "quit" => std::process::exit(0),
                _ => {}
            }
        }
        unsafe {
            let mut msg: MSG = std::mem::zeroed();
            if PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(60));
    }
}

/// 托盘图标：优先从 exe 内嵌资源读取 build.rs 嵌入的 app.ico（资源 ID 1），
/// 从而与任务栏/资源管理器里的程序图标同源一致；加载失败时回退到程序化渐变圆。
#[cfg(windows)]
fn load_tray_icon() -> Result<tray_icon::Icon, tray_icon::BadIcon> {
    // build.rs 用 windres 写成 `1 ICON "assets/app.ico"`，此处按资源 ID 1 读取
    if let Ok(icon) = tray_icon::Icon::from_resource(1, Some((32, 32))) {
        return Ok(icon);
    }
    let (rgba, w, h) = icon_rgba();
    tray_icon::Icon::from_rgba(rgba, w, h)
}

/// 回退用：程序化生成 32x32 品牌色（紫罗兰渐变圆角）托盘图标，无需外部资源文件
#[cfg(windows)]
fn icon_rgba() -> (Vec<u8>, u32, u32) {
    const S: u32 = 32;
    let mut v = Vec::with_capacity((S * S * 4) as usize);
    let (r1, g1, b1) = (124u32, 92u32, 240u32); // #7c5cf0
    let (r2, g2, b2) = (167u32, 139u32, 250u32); // #a78bfa
    let rad = 7i32;
    for y in 0..S {
        for x in 0..S {
            let t = (x + y) as f32 / ((2 * S - 2) as f32);
            let r = (r1 as f32 + (r2 - r1) as f32 * t) as u8;
            let g = (g1 as f32 + (g2 - g1) as f32 * t) as u8;
            let b = (b1 as f32 + (b2 - b1) as f32 * t) as u8;
            // 圆角遮罩
            let (xi, yi) = (x as i32, y as i32);
            let cx = if xi < rad { rad } else if xi >= S as i32 - rad { S as i32 - 1 - rad } else { xi };
            let cy = if yi < rad { rad } else if yi >= S as i32 - rad { S as i32 - 1 - rad } else { yi };
            let inside = (xi - cx) * (xi - cx) + (yi - cy) * (yi - cy) <= rad * rad || (xi == cx || yi == cy);
            let alpha: u8 = if inside { 255 } else { 0 };
            v.extend_from_slice(&[r, g, b, alpha]);
        }
    }
    (v, S, S)
}

/// Windows：验证 build.rs 嵌入的资源 ID 1（app.ico）能被 tray-icon 读到，
/// 即托盘图标与任务栏/资源管理器程序图标同源。
#[cfg(all(test, windows))]
mod tests {
    #[test]
    fn exe_resource_icon_loadable() {
        let icon = tray_icon::Icon::from_resource(1, Some((32, 32)));
        assert!(icon.is_ok(), "app.ico 资源 ID 1 无法从 exe 读取：{:?}", icon.err());
    }

    #[test]
    fn fallback_gradient_icon_loadable() {
        let (rgba, w, h) = super::icon_rgba();
        assert_eq!(rgba.len() as u32, w * h * 4);
        let icon = tray_icon::Icon::from_rgba(rgba, w, h);
        assert!(icon.is_ok(), "渐变回退图标构造失败：{:?}", icon.err());
    }
}
