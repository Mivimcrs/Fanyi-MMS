//! Windows 资源嵌入：应用图标 + VERSIONINFO。
//! 补齐文件属性（描述/版本/公司）是降低杀软误报最有效的免费手段，
//! 同时让 exe 在资源管理器中显示品牌图标（详见 防误报指南.md）。
//!
//! 双路径：MSVC（CI）用 winresource 的 rc.exe 流程；GNU（本机 MinGW）下
//! winresource 内部调 windres 会因 popen 预处理在 cargo 环境失败
//! （"can't popen gcc -E ... Bad file descriptor"），此时回退到
//! `windres --use-temp-file` + `ar rsc` 手工流程（--use-temp-file 绕开 popen）。

use std::path::PathBuf;
use std::process::Command;

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    println!("cargo:rerun-if-changed=assets/app.ico");
    println!("cargo:rerun-if-changed=build.rs");

    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let ver = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    let numeric = format!("{}.0", ver); // x.y.z -> x.y.z.0（VERSIONINFO 要求四段数字）

    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/app.ico");
    res.set("FileDescription", "莱·梵壹会员实时跟踪管理系统");
    res.set("ProductName", "莱·梵壹会员系统");
    res.set("CompanyName", "莱·梵壹");
    res.set("LegalCopyright", "Copyright (C) 2026 莱·梵壹");
    res.set("OriginalFilename", "laifanyi.exe");
    res.set("FileVersion", &numeric);
    res.set("ProductVersion", &ver);

    match res.compile() {
        Ok(()) => {}
        Err(e) => {
            eprintln!("[build] winresource 失败（{}），回退到手工 windres 流程", e);
            manual_gnu_resource(&manifest, &out_dir, &numeric, &ver);
        }
    }
}

/// GNU 工具链手工资源编译：windres --use-temp-file + ar rsc
fn manual_gnu_resource(manifest: &std::path::Path, out_dir: &std::path::Path, numeric: &str, ver: &str) {
    let rc = out_dir.join("resource.rc");
    std::fs::write(
        &rc,
        format!(
            r#"#pragma code_page(65001)
1 VERSIONINFO
FILEVERSION {n1}, {n2}, {n3}, 0
PRODUCTVERSION {n1}, {n2}, {n3}, 0
FILEOS 0x40004
FILETYPE 0x1
FILESUBTYPE 0x0
FILEFLAGSMASK 0x3f
FILEFLAGS 0x0
{{
BLOCK "StringFileInfo"
{{
BLOCK "000004b0"
{{
VALUE "CompanyName", "莱·梵壹"
VALUE "FileDescription", "莱·梵壹会员实时跟踪管理系统"
VALUE "FileVersion", "{fv}"
VALUE "LegalCopyright", "Copyright (C) 2026 莱·梵壹"
VALUE "OriginalFilename", "laifanyi.exe"
VALUE "ProductName", "莱·梵壹会员系统"
VALUE "ProductVersion", "{ver}"
}}
}}
BLOCK "VarFileInfo" {{
VALUE "Translation", 0x0, 0x04b0
}}
}}
1 ICON "assets/app.ico"
"#,
            n1 = numeric.split('.').next().unwrap_or("0"),
            n2 = numeric.split('.').nth(1).unwrap_or("0"),
            n3 = numeric.split('.').nth(2).unwrap_or("0"),
            fv = numeric,
            ver = ver,
        ),
    )
    .expect("写 resource.rc 失败");
    let obj = out_dir.join("resource.o");
    run(
        Command::new("windres")
            .arg("--use-temp-file")
            .arg("--input-format")
            .arg("rc")
            .arg("--output-format")
            .arg("coff")
            .arg(format!("-I{}", manifest.display()))
            .arg(&rc)
            .arg(&obj),
        "windres",
    );
    let lib = out_dir.join("libresource.a");
    run(Command::new("ar").arg("rsc").arg(&lib).arg(&obj), "ar");
    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static:+whole-archive=resource");
}

fn run(cmd: &mut Command, name: &str) {
    let status = cmd
        .status()
        .unwrap_or_else(|e| panic!("{} 无法执行（检查 MinGW binutils 是否在 PATH）：{}", name, e));
    if !status.success() {
        panic!("{} 编译资源失败", name);
    }
}
