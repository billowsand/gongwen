# Portable runtime

PDFs are typeset in-process by the Typst engine compiled into the application
(see `docs/typst-engine.md`); no external typesetter ships with it. The
runtime directory only carries fonts, input-method data and license texts:

```text
runtime/
  fonts/FangSong.ttf
  fonts/KaiTi.ttf
  fonts/SimHei.ttf
  fonts/SimSun.ttf
  fonts/XiaoBiaoSong.ttf
  fonts/FZShuSong.ttf
  fonts/FZHei.ttf
  fonts/FZKai.ttf
  fonts/FZXiaoBiaoSong.ttf
  fonts/JetBrainsMono-Regular.ttf
  fonts/texgyretermes-regular.otf
  fonts/texgyretermes-bold.otf
  fonts/texgyretermes-italic.otf
  fonts/texgyretermes-bolditalic.otf
  ime/dict.qj
  ime/lm.qj                     # optional sentence model (44 MB)
  licenses/...
  SHA256SUMS.win-x64.txt
  SHA256SUMS.linux-arm64.txt
  SHA256SUMS.linux-amd64.txt
  SHA256SUMS.darwin-arm64.txt
```

- Fonts come in two groups, both loaded **from the files here**, never by an
  installed family name, so no machine has to have them installed:
  - `FangSong` / `KaiTi` / `SimHei` / `SimSun` / `XiaoBiaoSong` are required by
    every official-document layout (`assets/typst/gongwen.typ`) and by the
    on-screen paper preview. Missing any of them disables PDF output entirely.
  - `FZ*` / `JetBrainsMono` / `texgyretermes-*` are required only by the
    research-report layout (`assets/typst/research.typ`). Missing any of them
    disables research reports alone; official documents keep working.
- The font files are deployment assets supplied locally by the application
  distributor. They remain ignored by Git; redistribution authorization must
  be checked separately. JetBrains Mono is distributed under the OFL and
  TeX Gyre Termes under the GUST Font License. Keep all license texts in
  `runtime/licenses`.
- Runtime archives up to `gongwen-runtime` v0.7.0 still contain Tectonic
  (`tectonic/`), a TeX bundle (`texbundle/`) and their licenses from the old
  TeX pipeline. They are no longer used: the packaging scripts skip them even
  when an archive's manifest lists them.

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
`fonts/`, `ime/` and the matching `SHA256SUMS.<suffix>.txt`. The release
smoke test typesets every document kind with the packaged runtime:

```text
GONGWEN_RUNTIME_DIR=<runtime> cargo test --locked --release --bin gongwen-assistant shipped_runtime_typesets_every_kind -- --ignored
```
