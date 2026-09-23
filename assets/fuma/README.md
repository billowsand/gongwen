# `assets/fuma/` — 仓库内的辅码参考文件

本目录**不是随包资源**，**不**会被任何发布构建（Windows installer / Linux deb
/ Linux tar.xz / macOS DMG）打包进去：打包脚本（`scripts/` 下）只引用
`assets/app-icon/` 和 `assets/linux/`，从不动 `assets/fuma/`。

放在仓库里只是为了：

1. **开发期参考**：`src/ime/session.rs::import_fuma_table` 校验「`字=两码`」两列、
   每行一条时，用它当基准样本。
2. **自动化测试**：未来若要给 `FumaTable::from_path` 加行格式回归测试，可以直接
   拿这份文件做 fixture。
3. **`ime-builtin-xiaohe` feature（默认关闭）**：开启时，`Cargo.toml` 里的
   `include_str!("../../assets/fuma/xiaohe.txt")` 会把这份表**编译进二进制**。
   这是给开发者的便利，**不是发布渠道**——任何对外发布的构建都必须保持该
   feature 关闭。

## 为什么不做成「开箱即用」

小鹤形码表复现的是已发表的输入方案，**权利归小鹤方案作者**，上游未取得再分发
授权（见 `vendor/qingjian/README.md`）。**任何形式的随包分发都构成侵权**，无论是
复制到安装包还是 `include_str!` 嵌入二进制。

因此应用必须永远让使用者**主动**完成导入：发布构建里设置页只有「导入码表…」按钮，
文件来自使用者本地、由使用者确认存在合法授权后才生效。`ime-builtin-xiaohe` 是个
开发者便利 feature，不改变上述约束。

## 文件清单

- `xiaohe.txt`：小鹤形码单字两码表，每行 `字=两码`，约 8000 余字。