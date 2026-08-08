# Portable TeX runtime

The application expects the following local-only release assets beside this file:

```text
runtime/
  tectonic/tectonic.exe
  texbundle/gongwen-texlive.ttb
  fonts/FangSong.ttf
  fonts/KaiTi.ttf
  fonts/SimHei.ttf
  fonts/SimSun.ttf
  fonts/XiaoBiaoSong.ttf
```

- `tectonic.exe` is the official Tectonic 0.17.0 Windows x64 MSVC build.
- `gongwen-texlive.ttb` is a project-specific TTB v1 bundle built from the
  TeX Live 2026 files actually needed by `gonghan-gwa.cls`. Its Tectonic
  content digest is
  `a7a9fdad147d59a8172ae625ea7bdeeee9493ebcaabd4a064928f3db7475de5a`.
- The font files are deployment assets supplied locally by the application
  distributor. They remain ignored by Git; redistribution authorization must
  be checked separately.

All binary assets are ignored by Git intentionally. Run
`scripts/package-portable.ps1` on the controlled release machine after the
assets have been placed here. The script validates every SHA-256 hash before
building the portable directory. If the output directory already exists, the
script asks whether to overwrite it and treats Enter as Yes (`[Y/n]`). Use
`-Force` for an intentional non-interactive overwrite.
