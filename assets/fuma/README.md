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
4. **`ime-dev-tables` feature（默认关闭，仅本机自用）**：开启时把 `danzi.txt`
   （小鹤辅码）与 `quan.txt`（小鹤音形）编译进二进制，启动时自动装进
   `config_dir()/ime/`，首次运行默认就是「小鹤双拼 + 小鹤辅码 + 小鹤音形」。
   `scripts/package-dev.ps1` 带这个 feature 打本机测试包。它碰的是和上面
   完全同一个授权约束，只是把「设置页点按钮」换成「启动时自动装」——**产出的
   二进制一样不得对外分发**。

## 为什么不做成「开箱即用」

小鹤形码表复现的是已发表的输入方案，**权利归小鹤方案作者**，上游未取得再分发
授权（见 `vendor/qingjian/README.md`）。**任何形式的随包分发都构成侵权**，无论是
复制到安装包还是 `include_str!` 嵌入二进制。

因此应用必须永远让使用者**主动**完成导入：发布构建里设置页只有「导入码表…」按钮，
文件来自使用者本地、由使用者确认存在合法授权后才生效。`ime-builtin-xiaohe` 是个
开发者便利 feature，不改变上述约束。

`ime-dev-tables` 是唯一一条「自动装」的路径，且只针对**本机自用的开发构建**：
发布流程（`release.yml`）与 CI 都不带它。带它的二进制与安装包同样属于「不可
对外分发」，只在本机自己用。

## 文件清单

仓库里只跟踪 `xiaohe.txt` 一份；`danzi.txt`、`quan.txt` 被 `.gitignore` 挡住，
只有本机有。

- `xiaohe.txt`：小鹤形码单字两码表，每行 `字=两码`，约 8000 余字。
- `danzi.txt`（本机私存）：与小鹤形码同内容，`ime-dev-tables` 拿它当内置辅码。
- `quan.txt`（本机私存）：搜狗自定义短语格式的小鹤音形码表（UTF-16LE），
  `ime-dev-tables` 拿它当内置音形码表。
