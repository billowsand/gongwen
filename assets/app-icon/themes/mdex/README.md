# MDEX 纸墨图标

采用第二版设计稿的米白方形底、墨黑衬线 M 与绿色光标。图标轮廓为独立矢量几何，
不依赖本机字体；不是从概念图裁剪的截图。

- `app-icon.svg`：可编辑的矢量源。
- `app-icon-16.png` 至 `app-icon-1024.png`：透明背景的多尺寸资源。
- `app-icon.ico`：多尺寸 Windows 图标；应用运行时由主题系统选择 PNG。

在仓库根目录运行 `python scripts/generate-mdex-icon.py` 可重建上述资源（需要 Pillow）。
此主题可在「设置 → 界面主题 → 浅色 → MDEX 纸墨」选择，窗口图标会随主题同步切换。
安装包与快捷方式继续采用项目默认图标。

真实控件样张：`cargo test --locked mdex_theme_sample -- --ignored --test-threads=1`，
输出到 `tmp/mdex-theme.png`。
