# 莱·梵壹会员实时跟踪管理系统 — Rust 版（v3.0）

Python 版的 Rust 重写：**单个可执行文件、无需安装 Python/任何环境**，双端（Windows / macOS）可用。
Excel 工作簿仍是唯一数据源，读写保真（公式、条件格式、下拉校验、数组公式全部保留）。

## 目录结构

```
莱·梵壹会员系统-Rust/
├── Cargo.toml               # Rust 工程（axum + calamine + zip，纯 Rust 依赖）
├── src/                     # 后端源码（~2300 行）
│   ├── main.rs              # 启动：单实例锁 / 端口顺延 / 自动开浏览器
│   ├── api.rs               # HTTP 端点（与 Python 版契约逐字一致 + 挂起变更恢复）
│   ├── store.rs             # 内存模型 + 增删改 + 保存/恢复（对应 Python Store；保存失败自动落盘 .laifanyi.pending，重启可恢复）
│   ├── compute.rs           # 业务口径镜像（与 Excel 公式一致，见《技术文档》§8）
│   ├── snapshot.rs          # versions/ 历史版本快照（≤100 个）
│   └── xlsx/                # Excel 引擎
│       ├── read.rs          #   calamine 读取原始行
│       ├── zipio.rs         #   zip 打包/拆包 + calcChain 清理 + 强制重算
│       ├── xmlscan.rs       #   字节级 XML 扫描（工作表定位/模板样式/公式捕获）
│       └── surgery.rs       #   外科手术写入：只改目标单元格，其余逐字节保留
├── static/index.html        # 前端单页（与 Python 版相同，编译期内嵌进 exe）
├── data/                    # 数据表格（默认查找 exe 同目录同名文件，此目录仅开发用）
└── tests/roundtrip.rs       # 集成测试：口径对照 / 写路径 / 保真 / 越界插入
```

## 使用（最终用户）

把 `laifanyi.exe` 放在**表格文件同目录**（或整个文件夹一起拷贝），双击即可：
自动选择空闲端口（默认 8688，被占则顺延）、自动打开浏览器。

- **Windows release 版无控制台窗口**，程序驻留**系统托盘**（隐藏图标区，紫色圆角图标）：
  左键菜单 =「打开网页 / 打开数据文件夹 / 退出」；
- 重复双击不会开第二个进程，只会再拉起一次浏览器；
- 表格加载失败会弹出消息框说明原因；
- 启动异常（无环境依赖，理论上不会）请把问题反馈给维护者。

macOS：终端运行 `./laifanyi`（或使用 CI 产出的 `莱·梵壹会员系统.app`，实验性，见下）。

## 开发与构建

```bash
cargo test            # 单元 + 集成测试（口径与 Excel 公式对照、写入保真）
cargo build --release # 产物 target/release/laifanyi.exe
```

命令行参数（与 Python 版一致）：

```
laifanyi --excel 路径.xlsx --port 8688 --no-browser
```

### ⚠ 本机构建注意事项（中文路径 + GNU 工具链）

1. **MinGW ld/dlltool 无法处理中文路径**。本项目文件夹名含中文，构建时必须把输出目录指到纯 ASCII 路径：

   ```bash
   export CARGO_TARGET_DIR="/c/Users/<你>/lfy-target"   # Git Bash
   set CARGO_TARGET_DIR=C:\Users\<你>\lfy-target        # CMD
   ```

2. Rust 使用 **GNU 工具链**（本机未装 Visual Studio）。rustup 自带的 dlltool 需要 `as.exe`，
   因此需 MinGW-w64 在 PATH 中（本机已装于 `C:\Users\<你>\winlibs\mingw64\bin`）：

   ```bash
   export PATH="/c/Users/<你>/.cargo/bin:/c/Users/<你>/winlibs/mingw64/bin:$PATH"
   ```

3. CI 构建（`.github/workflows/release.yml`）在 GitHub 托管机上用 MSVC/Xcode 工具链，
   不受以上限制；打 tag `v*` 或手动触发即产出双平台压缩包。

### 无 Mac 本机出 macOS 可执行文件（cargo-zigbuild + zig，已验证）

`rustup target add aarch64-apple-darwin x86_64-apple-darwin`、`pip install ziglang`、
`cargo install cargo-zigbuild`（需 MinGW PATH），然后把 ziglang 目录加入 PATH：

```bash
export PATH="/c/Users/Mivimcrs/AppData/Local/Programs/Python/Python313/Lib/site-packages/ziglang:$PATH"
cargo zigbuild --release --target aarch64-apple-darwin   # Apple Silicon
cargo zigbuild --release --target x86_64-apple-darwin    # Intel
```

前提是 macOS 目标不链接任何 Apple 框架：`rfd`/`tray-icon` 仅挂在 Windows 依赖下，
chrono 不用 `clock` 特性（本地时间走 `model::local_now()`）。详见《技术文档》§11.1。
universal2 合并与 .app 组装仍走 CI。

## CI 与 macOS 打包

- `git tag v3.0.0 && git push --tags` → Actions 自动构建：
  - Windows：`laifanyi-windows-x64.zip`（exe + 使用说明）
  - macOS：`laifanyi-macos-universal.zip`（universal2 双架构 .app，Intel/Apple Silicon 通吃）
- macOS 正式分发需要 Developer ID 签名 + 公证（CI 中的 secrets 配置）；
  无签名时用户首次打开需「右键 → 打开」。当前 .app 为**实验性**：双击可运行、自动开浏览器，
  托盘与 Dock 退出待真机验证后接入（macOS 托盘需要 NSApp 主线程事件循环）。

## 已完成 vs 待办（对照《开发技术文档-Rust跨平台重构方案.md》里程碑）

- [x] M0 骨架：axum + 内嵌前端 + 读取 + /api/data
- [x] M1 口径全量移植（状态机/待开卡/应收单价/末课三分支，黄金对照测试通过）
- [x] M2 写入层（外科手术式 zip/XML 改写 + 快照/恢复 + 占用 pending，openpyxl 往返校验通过）
- [x] M3：单实例、端口顺延、自动开浏览器、缺表引导页、**系统托盘（Windows）**、
      **无控制台 release**、**启动失败/缺表消息框**
- [x] M4：CI 双平台矩阵（打 tag 自动出包）；macOS universal2 .app 组装脚本
- [x] M6 加固：保存失败时挂起变更落盘可恢复、上传失败零残留、HTTP 契约测试（tower oneshot）、
      上课记录 1001 行容量边界测试
- [ ] M4 余项：代码签名/公证 secrets、应用图标 .ico/.icns
- [ ] M5：真机 UAT（Windows 实机已过；macOS 待真机）

## 与 Python 版的行为差异（有意改进）

| 项 | Python 版 | Rust 版 |
|---|---|---|
| 端口占用 | 强杀 8688 占用进程 | 顺延 8689+，不杀进程 |
| 保存原子性 | 直接覆盖 | 临时文件 + 原子替换 |
| 保存失败（表格被占用） | 变更仅存内存，退出即丢 | 变更落盘 `.laifanyi.pending.xlsx`，重启后网页提示一键恢复/丢弃 |
| 首写备份 | 文档承诺未实现 | 已实现（暂未写 backups/，快照在 versions/） |
| 切片器 | openpyxl 会丢失 | 逐字节保留 |
| 环境依赖 | 需 Python + openpyxl | 无（单文件） |

数据完全兼容：同一份 xlsx 可在两版之间切换使用；`versions/`、`config.json` 语义一致。
