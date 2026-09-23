# shellcheck shell=bash
# deb 的 Depends / Recommends，由 scripts/package-deb.sh 与
# scripts/repack-deb-depends.sh 共同 source，只在这里维护一份。

# Runtime dependencies for eframe/glow + rfd (GTK3) on X11/Wayland. Kept as a
# deterministic list instead of dpkg-shlibdeps so the package does not depend
# on which libraries happen to be installed on the packaging host.
#
# 基线是 glibc 2.28（Debian buster / Ubuntu 20.04 / 麒麟 V10）。这些老发行版
# 上有两个库还用旧包名，只写新名字 dpkg 会直接报"未安装软件包"：
#   - gdk-pixbuf：Debian bullseye / Ubuntu 20.10 起才改名 libgdk-pixbuf-2.0-0，
#     之前一直叫 libgdk-pixbuf2.0-0（麒麟 V10 上就是这个名字）。
#   - libgcc：Debian bullseye / Ubuntu 20.04 起才叫 libgcc-s1，buster 上是 libgcc1。
# 用 "新名 | 旧名" 两者择一，新旧系统都能满足。
DEPENDS="libc6 (>= 2.28), libgcc-s1 | libgcc1, libglib2.0-0, libgtk-3-0, \
libgdk-pixbuf-2.0-0 | libgdk-pixbuf2.0-0, libpango-1.0-0, libcairo2, libatk1.0-0, libx11-6, \
libx11-xcb1, libxcb1, libxcb-render0, libxcb-shape0, libxcb-xfixes0, \
libxkbcommon0, libxkbcommon-x11-0, libxrender1, libxrandr2, libxcursor1, \
libxinerama1, libxext6, libxi6, libxfixes3, libxdamage1, libxcomposite1, \
libwayland-client0, libwayland-cursor0, libwayland-egl1, libegl1, libgl1, \
libglx0, libfontconfig1, libfreetype6, libdbus-1-3, zlib1g"

# 打印走 CUPS 的 lp。列为 Recommends 而不是 Depends：apt 默认会装上，但缺了
# 也只是打印按钮置灰，不该拦住整个应用的安装。
RECOMMENDS="cups-client"
