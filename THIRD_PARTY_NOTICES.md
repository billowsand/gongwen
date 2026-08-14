# Third-party notices

## Tectonic

The portable Windows package includes Tectonic 0.17.0, distributed under the
MIT License. The complete notice is copied to
`runtime/licenses/TECTONIC-LICENSE.txt`.

- Project: https://github.com/tectonic-typesetting/tectonic
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

## Hayro and Vello CPU

The application uses Hayro 0.7.1 and Vello CPU to parse and rasterize PDF
pages entirely in Rust. These crates are available under the MIT License or
the Apache License 2.0; this distribution uses the MIT terms.

- Hayro: https://github.com/LaurenzV/hayro
- Vello: https://github.com/linebender/vello

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

## Lucide Icons

The SVG interface icons under `assets/icons/` are from Lucide 1.28.0.

- Source: https://lucide.dev/
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

- Source: https://github.com/jrsoftware/issrc
