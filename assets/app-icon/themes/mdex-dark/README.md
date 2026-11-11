# MDEX 夜墨图标

与纸墨共用折角和镂空「文」字轮廓，配深绿灰底和浅色剪影。运行
`python3 scripts/generate-brand-icons.py` 可重建 16 至 1024 像素 PNG、多尺寸 ICO
和应用内单色标记（需要 Pillow）。

在「设置 → 界面主题 → 深色 → MDEX 夜墨」选择后，应用按主题切换窗口图标。
真实控件样张由 `cargo test --locked mdex_theme_sample -- --ignored --test-threads=1`
生成，深色样张输出到 `tmp/mdex-dark-theme.png`。
