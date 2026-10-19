# 词表资源

本机初始基础词表支持 `quan.txt` 与 `mabiao.txt`，记录格式为 `编码,位置=文字`。
Windows / Linux 优先 `quan.txt`，macOS 优先 `mabiao.txt`，缺少首选时使用另一份。
程序只查询编码，不区分任何拼音方案；基础表支持 1–4 码，公文词表使用四码。
文件作为本机资源不入库。打包脚本存在该文件时复制为 `runtime/ime/base.txt`。
首次载入复制到用户目录，之后动态维护 `ime/base.txt`，不会被重新安装覆盖。
不再使用 `danzi.txt`、`xiaohe.txt` 等辅码资源，也不再把词表编进二进制。
