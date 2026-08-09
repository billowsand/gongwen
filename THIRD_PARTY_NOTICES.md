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

## LXGW Bright and LXGW Bright Code

The application embeds LXGW Bright Medium (proportional UI text) and LXGW
Bright Code (monospace Markdown editor text). Both fonts are distributed under
the SIL Open Font License 1.1.

- Project: https://github.com/lxgw/LxgwBright
- Project: https://github.com/lxgw/LxgwBright-Code
- License: SIL Open Font License 1.1

At build time, `build.rs` downloads the latest release assets from the two
projects and embeds them into the binary. The license texts are copied into
packaged builds under `licenses/fonts/` and are kept in the source tree at
`font/licenses/`.

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
