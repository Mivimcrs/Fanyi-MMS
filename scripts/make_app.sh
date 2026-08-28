#!/usr/bin/env bash
# 组装 macOS .app 包（在 macOS 上执行）
# 用法: bash scripts/make_app.sh <universal2 二进制路径>
set -euo pipefail

BIN="${1:?用法: make_app.sh <二进制路径>}"
APP="莱·梵壹会员系统.app"

mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/laifanyi"
chmod +x "$APP/Contents/MacOS/laifanyi"

cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>莱·梵壹会员系统</string>
    <key>CFBundleDisplayName</key>
    <string>莱·梵壹会员系统</string>
    <key>CFBundleIdentifier</key>
    <string>com.laifanyi.member-system</string>
    <key>CFBundleExecutable</key>
    <string>laifanyi</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>3.0.0</string>
    <key>CFBundleVersion</key>
    <string>3</string>
    <key>LSMinimumSystemVersion</key>
    <string>10.15</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <!-- false: Dock 显示图标，用户可从 Dock 退出；改 true 则完全后台（需托盘后启用） -->
    <key>LSUIElement</key>
    <false/>
</dict>
</plist>
PLIST

# 可选：应用图标（需要 iconutil 从 iconset 生成 icns）
# cp laifanyi.icns "$APP/Contents/Resources/AppIcon.icns"
# /usr/libexec/PlistBuddy -c "Add :CFBundleIconFile string AppIcon" "$APP/Contents/Info.plist"

echo "OK: $APP"
echo "提示："
echo "  - 双击运行；首次启动若被 Gatekeeper 拦截：右键 -> 打开 -> 打开"
echo "  - 正式分发：用 Developer ID 签名并公证，否则用户需手动放行"
echo "  - 数据表格默认查找 .app/Contents/MacOS 同目录，建议把 xlsx 与 .app 放同一文件夹"
