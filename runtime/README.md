# Portable runtime

PDFs are typeset in-process by the Typst engine compiled into the application
(see `docs/typst-engine.md`); no external typesetter ships with it. The
runtime directory only carries fonts and license texts:

```text
runtime/
  fonts/FZFangSong.ttf
  fonts/FZKai.ttf
  fonts/FZHei.ttf
  fonts/FZShuSong.ttf
  fonts/XiaoBiaoSong.ttf
  fonts/GWFangSongLatin.ttf
  fonts/GWSimSunLatin.ttf
  fonts/JetBrainsMono-Regular.ttf
  fonts/texgyretermes-regular.otf
  fonts/texgyretermes-bold.otf
  fonts/texgyretermes-italic.otf
  fonts/texgyretermes-bolditalic.otf
  licenses/...
  SHA256SUMS.win-x64.txt
  SHA256SUMS.linux-arm64.txt
  SHA256SUMS.linux-amd64.txt
  SHA256SUMS.darwin-arm64.txt
```

- Fonts come in two groups, both loaded **from the files here**, never by an
  installed family name, so no machine has to have them installed:
  - Chinese glyphs all come from the Founder GBK series (`FZ*`), which covers
    the full GBK repertoire; `XiaoBiaoSong` (方正小标宋_GBK) is the document
    title face. They are required by every official-document layout
    (`assets/typst/gongwen.typ`), by the research-report layout
    (`assets/typst/research.typ`) and by the on-screen paper preview. Missing
    any of them disables PDF output entirely.
  - `GWFangSongLatin.ttf` / `GWSimSunLatin.ttf` are tiny subsets (printable
    ASCII only) carved out of 仿宋_GB2312 and 宋体 by
    `scripts/make-latin-subsets.py`. They sit at the head of the fallback
    chain so that body Latin/digits keep the 仿宋_GB2312 look and page-number
    digits keep the 宋体 look, as the official-document standard expects.
  - `JetBrainsMono` / `texgyretermes-*` are required only by the
    research-report layout. Missing any of them disables research reports
    alone; official documents keep working.
- The font files are deployment assets supplied locally by the application
  distributor. They remain ignored by Git; redistribution authorization must
  be checked separately. JetBrains Mono is distributed under the OFL and
  TeX Gyre Termes under the GUST Font License. Keep all license texts in
  `runtime/licenses`.
- Runtime archives up to `gongwen-runtime` v0.7.0 still contain Tectonic
  (`tectonic/`), a TeX bundle (`texbundle/`), the retired pinyin IME data
  (`ime/`) and the Zhongyi font set from the old pipeline. They are no longer
  used: the packaging scripts skip them even when an archive's manifest lists
  them. v0.8.0 onwards ships only the fonts listed above.

All binary assets are ignored by Git intentionally. Run
`scripts/package-portable.ps1` after the assets have been placed here. The
script validates the platform SHA-256 manifest before building the portable
directory or archive:

```powershell
./scripts/package-portable.ps1 -Suffix win-x64 -ArchiveFormat zip
./scripts/package-portable.ps1 -Suffix linux-arm64 -ArchiveFormat tar.gz -SkipBuild
```

Use `-RuntimeManifest` to point at another manifest, `-OutputDir` and
`-ArchivePath` to control destinations, and `-Force` for a non-interactive
overwrite. The release workflow downloads `runtime-<suffix>.zip` from the
`billowsand/gongwen-runtime` release selected by its `RUNTIME_RELEASE_TAG`;
that archive must contain the files directly under its root, including
`fonts/` and the matching `SHA256SUMS.<suffix>.txt`. The release
smoke test typesets every document kind with the packaged runtime:

```text
GONGWEN_RUNTIME_DIR=<runtime> cargo test --locked --release --bin gongwen-assistant shipped_runtime_typesets_every_kind -- --ignored
```
