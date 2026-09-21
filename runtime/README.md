# Portable TeX runtime

The application expects the following local-only release assets:

```text
runtime/
  tectonic/
    win-x64/tectonic.exe      # Windows x64 MSVC
    linux-arm64/tectonic      # Linux ARM64 musl
    linux-amd64/tectonic      # Linux AMD64 musl
    darwin-arm64/tectonic     # macOS ARM64
  texbundle/gongwen-texlive.ttb
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
  SHA256SUMS.win-x64.txt
  SHA256SUMS.linux-arm64.txt
  SHA256SUMS.linux-amd64.txt
  SHA256SUMS.darwin-arm64.txt
```

- `tectonic.exe` under `win-x64` is the official Tectonic 0.17.0 Windows
  x64 MSVC build.
- `tectonic` under `linux-arm64` is the official Tectonic 0.17.0
  `aarch64-unknown-linux-musl` build.
- `tectonic` under `linux-amd64` is the official Tectonic 0.17.0
  `x86_64-unknown-linux-musl` build.
- `tectonic` under `darwin-arm64` is the official Tectonic 0.17.0
  `aarch64-apple-darwin` build.
- `gongwen-texlive.ttb` is a project-specific TTB v1 bundle resolved from
  Tectonic 0.17.0's pinned upstream bundle. It contains the dependencies
  actually exercised by all `gonghan-gwa.cls` document variants and by mdx's
  research warm-up document (including `ctexbook`, TikZ, listings, `gbt7714`
  and, since runtime v0.6.0, `amsmath`/`mathtools` for research-report math
  formulas). Both styles are recompiled with a fresh cache and
  `--only-cached --untrusted` before publishing. The authoritative checksum is
  the `texbundle/gongwen-texlive.ttb` line of each `SHA256SUMS.<suffix>.txt`;
  that is the value `scripts/package-portable.ps1` actually verifies.
- Rebuilding the bundle: bump the package lists in mdx's
  `resources/tectonic/warmup{,-official}.tex`, tag an mdx release so its
  workflow regenerates the directory bundle (`tectonic-bundle` artifact), then
  pack the directory into a TTB v1 with a writer matching
  `tectonic_bundles` 0.4.2's reader (66-byte header; per-file gzip content;
  embedded `FILELIST`/`SEARCH`/`SHA256SUM`; files under `resolved/`; index
  gzipped at the end; digest = SHA-256 of the plain index text). Verify by
  compiling both warm-up documents against the new `.ttb` with a fresh cache
  and `--only-cached --untrusted` before updating the checksums.
- Fonts come in two groups, both loaded **by file name**, never by family
  name, so no machine has to have them installed:
  - `FangSong` / `KaiTi` / `SimHei` / `SimSun` / `XiaoBiaoSong` are required by
    every official-document layout (`gonghan-gwa.cls`, via `\GwaFontPath`) and
    by the on-screen paper preview. Missing any of them disables PDF output
    entirely.
  - `FZ*` / `JetBrainsMono` / `texgyretermes-*` are required only by the
    research-report layout (mdx's `md2tex.cls`, via `\MdxFontPath`). Missing
    any of them disables research reports alone; official documents keep
    working.
- `md2tex.cls` is loaded with `fontset=none`, so the research layout never
  falls back to ctex's per-platform font detection. That is why no Fandol font
  is shipped here — nothing references it.
- The font files are deployment assets supplied locally by the application
  distributor. They remain ignored by Git; redistribution authorization must
  be checked separately. JetBrains Mono is distributed under the OFL and
  TeX Gyre Termes under the GUST Font License. Keep all license texts in
  `runtime/licenses`.

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
that archive must contain the files directly under its root, including `tectonic/`,
`texbundle/`, `fonts/`, and the matching `SHA256SUMS.<suffix>.txt`.
