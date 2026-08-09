# Portable TeX runtime

The application expects the following local-only release assets:

```text
runtime/
  tectonic/
    win-x64/tectonic.exe      # Windows x64 MSVC
    linux-arm64/tectonic      # Linux ARM64 musl
  texbundle/gongwen-texlive.ttb
  fonts/FangSong.ttf
  fonts/KaiTi.ttf
  fonts/SimHei.ttf
  fonts/SimSun.ttf
  fonts/XiaoBiaoSong.ttf
  SHA256SUMS.win-x64.txt
  SHA256SUMS.linux-arm64.txt
```

- `tectonic.exe` under `win-x64` is the official Tectonic 0.17.0 Windows
  x64 MSVC build.
- `tectonic` under `linux-arm64` is the official Tectonic 0.17.0
  `aarch64-unknown-linux-musl` build.
- `gongwen-texlive.ttb` is a project-specific TTB v1 bundle built from the
  TeX Live 2026 files actually needed by `gonghan-gwa.cls`. Its Tectonic
  content digest is
  `a7a9fdad147d59a8172ae625ea7bdeeee9493ebcaabd4a064928f3db7475de5a`.
- The font files are deployment assets supplied locally by the application
  distributor. They remain ignored by Git; redistribution authorization must
  be checked separately.

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
`billowsand/gongwen-runtime` release with the same tag; that archive must
contain the files directly under its root, including `tectonic/`,
`texbundle/`, `fonts/`, and the matching `SHA256SUMS.<suffix>.txt`.
