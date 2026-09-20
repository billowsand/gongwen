# Third-party notices

## Tectonic

The portable Windows package includes Tectonic 0.17.0, distributed under the
MIT License. The complete notice is copied to
`runtime/licenses/TECTONIC-LICENSE.txt`.

- Project: <https://github.com/tectonic-typesetting/tectonic>
- Release: `tectonic@0.17.0`, Windows x64 MSVC

## TeX Live support files

`runtime/texbundle/gongwen-texlive.ttb` contains a minimal dependency closure
selected from TeX Live 2026 for this application's generated documents. TeX
Live and CTAN components use multiple free-software licenses. The TeX Live
distribution notices are copied to `runtime/licenses/LICENSE.TL` and
`runtime/licenses/LICENSE.CTAN`.

## Document fonts

The document fonts in `runtime/fonts` are locally supplied deployment
assets. Their redistribution authorization must be verified by the package
distributor. See `runtime/licenses/FONT-LICENSE-REQUIRED.txt`.

The research-report layout additionally uses JetBrains Mono (SIL Open Font
License) and TeX Gyre Termes (GUST Font License); their notices are copied to
`runtime/licenses/JetBrainsMono-OFL.txt` and
`runtime/licenses/TeX-Gyre-GUST-FONT-LICENSE.txt`.

## mdx

Research reports are converted to TeX by mdx, distributed under the MIT
License. A source copy is vendored in `vendor/mdx` and linked into the
executable; the upstream notice is preserved at `vendor/mdx/LICENSE`.

- Project: <https://github.com/billowsand/mdx>
- Version: `mdx@2.15.4`

## Hayro and Vello CPU

The application uses Hayro 0.7.1 and Vello CPU to parse and rasterize PDF
pages entirely in Rust. These crates are available under the MIT License or
the Apache License 2.0; this distribution uses the MIT terms.

- Hayro: <https://github.com/LaurenzV/hayro>
- Vello: <https://github.com/linebender/vello>

```text
Copyright (c) The Hayro Authors
Copyright 2020 the Vello Authors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

Hayro's optional fallback for the 14 standard PDF fonts embeds compact Foxit
font data originally distributed with PDFium:

```text
Copyright 2014 PDFium Authors. All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:

* Redistributions of source code must retain the above copyright notice,
  this list of conditions and the following disclaimer.
* Redistributions in binary form must reproduce the above copyright notice,
  this list of conditions and the following disclaimer in the documentation
  and/or other materials provided with the distribution.
* Neither the name of Google Inc. nor the names of its contributors may be
  used to endorse or promote products derived from this software without
  specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT OWNER OR CONTRIBUTORS BE
LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR
CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF
SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS
INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN
CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE)
ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE
POSSIBILITY OF SUCH DAMAGE.
```

Hayro also embeds a compact CMYK color profile from Compact ICC Profiles,
made available under CC0-1.0.

## flype-word (小鹤双拼编码器)

`src/lexicon/flypy.rs` is adapted from the flype-word project by the same
author, distributed under the MIT License. Only the encoder (the Xiaohe
double-pinyin key table and the four-code word rules) is copied; the
segmentation, revision and CLI/GUI layers of that project are not used.

- Project: <https://github.com/billowsand/flype-word>
- License: MIT

## Lucide Icons

The SVG interface icons under `assets/icons/` are from Lucide 1.28.0.

- Source: <https://lucide.dev/>
- License: ISC
- Copyright: Lucide Contributors

```text
ISC License

Copyright (c) for portions of Lucide are held by Cole Bemis 2013-2022 as part of
Feather (MIT). All other copyright (c) for Lucide are held by Lucide
Contributors 2022.

Permission to use, copy, modify, and/or distribute this software for any
purpose with or without fee is hereby granted, provided that the above
copyright notice and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH
REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY
AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT,
INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING FROM
LOSS OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR
OTHER TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR
PERFORMANCE OF THIS SOFTWARE.
```

## Inno Setup Simplified Chinese translation

The Windows installer embeds `scripts/ChineseSimplified.isl` from the Inno
Setup source repository, maintained by Zhenghan Yang (Kira) and distributed
with the Inno Setup project.

- Source: <https://github.com/jrsoftware/issrc>

## pinyin (Rust crate)

The application uses the `pinyin` crate to look up the default reading of a
Chinese character when encoding lexicon entries for the input method. It is
distributed under the MIT License.

- Project: <https://github.com/mozillazg/rust-pinyin>
- License: MIT

## 字在输入法（青简）内核

应用内拼音输入法的引擎、词库与整句模型取自**字在**（青简 Qingjian 的 Windows
分支），以 GPL-3.0-or-later 分发。取的是平台无关的六个 crate（format / dictionary /
core / lm / translate / learning），随源码存放在 `vendor/qingjian/`，许可证原文保留在
`vendor/qingjian/LICENSE`；上游的 Windows 平台壳（TSF DLL、Server 进程、自绘渲染器、
语音 Worker）一个都没有搬，本项目用 egui 自己当壳。

- 上游（字在）：<https://github.com/billowsand/zizai>
- 上游（青简 Qingjian）：<https://github.com/qingjian-team/qingjian>
- 取出提交：`9b643e1c8315c30e040e7798a9dadba0519a1807`
- License: GPL-3.0-or-later

随包数据放在 `runtime/ime/`：

| 文件 | 内容 | 许可 |
| --- | --- | --- |
| `dict.qj` | 拼音词库（约 9.3 万条） | MIT AND Unicode-3.0（词表来自《通用规范汉字表》《现代汉语常用词表》与 THUOCL，读音取自 Unihan） |
| `lm.qj` | bigram 语言模型（约 486 万组，可选） | CC-BY-SA-4.0 AND MIT（语料：中文维基百科与 LCCC） |

`lm.qj` 不在时输入法退到词级候选 + 个人 n-gram，仍可正常打字；取舍与重打办法见
`vendor/qingjian/README.md`。

**双拼辅码（形码）表不随包分发。** 小鹤辅码表复现的是已发表的输入方案，
权利归方案作者，上游未取得再分发授权（见上游 `assets/fuma/README.md`）。
因此本发行版不包含任何辅码表，只提供导入入口：使用者在设置页自行导入一份
`字=两码` 的文本，文件保存在本机用户目录（`config_dir()/ime/fuma/`）。
