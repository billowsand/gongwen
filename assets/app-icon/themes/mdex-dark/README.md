# MDEX 夜墨图标

纸墨的深色配套图标：暖墨色方形底、米白衬线 M 与鼠尾草绿光标。
与纸墨共用固定矢量轮廓，由 `scripts/generate-mdex-icon.py` 同时生成，保留 SVG、
16 至 1024 像素 PNG 及多尺寸 ICO，不依赖本机字体。

在「设置 → 界面主题 → 深色 → MDEX 夜墨」选择后，窗口与标题栏同步切换图标。
真实控件样张命令：`cargo test --locked mdex_theme_sample -- --ignored --test-threads=1`，
深色样张输出至 `tmp/mdex-dark-theme.png`。
