#!/usr/bin/env bash
# 在虚拟 X 显示（Xvfb + Mesa 软件渲染）里真正启动一次已安装的公文助手，
# 确认它不会一打开就闪退。
#
# Usage: smoke-launch-linux.sh [binary] [seconds] [artifact-dir]
#   binary        默认 gongwen-assistant（即 deb 装的 /usr/bin 入口）
#   seconds       窗口出现后至少要活多久，默认 20 秒
#   artifact-dir  启动日志与截图的输出目录，默认 ./launch-smoke
#
# 需要 xvfb、x11-utils（xwininfo）；装了 x11-apps + imagemagick 时顺带截图。
# 每个场景都用全新的 HOME，等于用户第一次打开软件。
#
# 场景：
#   default  普通 X11 会话
#   no-glx   X server 不提供 GLX（远程桌面、部分虚拟机/云桌面常见），
#            程序得退回 EGL，不能直接退出
#
# ldd 查不出 dlopen 的库（如 winit 运行时才加载的 libxkbcommon-x11），
# 缺了只会在启动那一刻 panic——这正是 verify-deb-install.sh 覆盖不到、
# 必须真起一次窗口的原因。
set -euo pipefail

BIN="${1:-gongwen-assistant}"
SECONDS_ALIVE="${2:-20}"
OUT_DIR="${3:-launch-smoke}"
# 按 WM_CLASS 找窗口：它就是 APP_ID，纯 ASCII，不受容器里 locale 的影响
# （标题是中文，C locale 下 xwininfo 打不出来）。
WINDOW_CLASS="cn.localtools.GongwenAssistant"
# 窗口最多等这么久出现；软件渲染下首帧装字体、词库要几秒。
WINDOW_TIMEOUT=60

mkdir -p "$OUT_DIR"
OUT_DIR="$(cd "$OUT_DIR" && pwd)"

if [ -r /etc/os-release ]; then
    . /etc/os-release
fi
echo "== Target: ${PRETTY_NAME:-unknown} ($(uname -m)), $(getconf GNU_LIBC_VERSION)"
command -v "$BIN" >/dev/null || { echo "error: $BIN not found" >&2; exit 1; }

xvfb_pid=""
app_pid=""
cleanup() {
    if [ -n "$app_pid" ]; then kill "$app_pid" 2>/dev/null || true; fi
    if [ -n "$xvfb_pid" ]; then kill "$xvfb_pid" 2>/dev/null || true; fi
    wait 2>/dev/null || true
    app_pid=""
    xvfb_pid=""
}
trap cleanup EXIT

run_scenario() {
    local name="$1"
    shift
    local display=":$((90 + RANDOM % 9))"
    local home
    home="$(mktemp -d)"
    local log="$OUT_DIR/$name.log"

    echo "== Scenario: $name (DISPLAY=$display, Xvfb $*)"
    Xvfb "$display" -screen 0 1600x1000x24 -nolisten tcp "$@" >"$OUT_DIR/$name.xvfb.log" 2>&1 &
    xvfb_pid=$!
    for _ in $(seq 1 50); do
        DISPLAY="$display" xwininfo -root >/dev/null 2>&1 && break
        sleep 0.2
    done
    DISPLAY="$display" xwininfo -root >/dev/null 2>&1 || {
        echo "error: Xvfb did not start" >&2
        cat "$OUT_DIR/$name.xvfb.log" >&2
        return 1
    }

    # 从 HOME 启动，与桌面图标双击时的工作目录一致。
    (cd "$home" && exec env -u WAYLAND_DISPLAY HOME="$home" DISPLAY="$display" \
        RUST_BACKTRACE=1 "$BIN") >"$log" 2>&1 &
    app_pid=$!

    local failed=0 waited=0
    # 1) 窗口必须出现。
    until DISPLAY="$display" xwininfo -root -tree 2>/dev/null | grep -qF "\"$WINDOW_CLASS\")"; do
        if ! kill -0 "$app_pid" 2>/dev/null; then
            echo "error: [$name] 窗口出现之前进程就退出了" >&2
            failed=1
            break
        fi
        if [ "$waited" -ge "$WINDOW_TIMEOUT" ]; then
            echo "error: [$name] ${WINDOW_TIMEOUT}s 内没有出现 $WINDOW_CLASS 窗口" >&2
            failed=1
            break
        fi
        sleep 1
        waited=$((waited + 1))
    done
    # 2) 窗口出来之后还要稳定活够 SECONDS_ALIVE 秒。
    if [ "$failed" -eq 0 ]; then
        echo "   窗口在 ${waited}s 内出现"
        for _ in $(seq 1 "$SECONDS_ALIVE"); do
            if ! kill -0 "$app_pid" 2>/dev/null; then
                echo "error: [$name] 窗口出现后进程退出了（闪退）" >&2
                failed=1
                break
            fi
            sleep 1
        done
    fi
    if [ "$failed" -eq 0 ] && command -v xwd >/dev/null && command -v convert >/dev/null; then
        DISPLAY="$display" xwd -root -silent | convert xwd:- "$OUT_DIR/$name.png" || true
    fi
    if [ "$failed" -ne 0 ] && ! kill -0 "$app_pid" 2>/dev/null; then
        local status=0
        wait "$app_pid" || status=$?
        echo "   进程退出码：$status" >&2
    fi
    # 3) 活着也不代表没出错：别的线程 panic 了日志里会留痕。
    if grep -qE "panicked at|Segmentation fault|core dumped" "$log"; then
        echo "error: [$name] 日志里有 panic / 崩溃记录" >&2
        failed=1
    fi

    # 4) 程序自己的崩溃日志（src/crash_log.rs）：有就说明出过错，内容一并打出来。
    local report
    for report in "$home"/.config/gongwenassistant/logs/crash-*.log; do
        [ -f "$report" ] || continue
        echo "error: [$name] 生成了崩溃日志 $(basename "$report")" >&2
        cp "$report" "$OUT_DIR/$name.$(basename "$report")"
        sed 's/^/   /' "$report" >&2
        failed=1
    done

    echo "-- $name.log:"
    sed 's/^/   /' "$log"
    cleanup
    rm -rf "$home"
    return "$failed"
}

failed=0
# `default` 是带 GLX 的 Xvfb：它要求 winit 0.30.13 在 XMapRaised 上拿到的 GLX
# context 合法。Debian buster 的 Mesa 18.3.6 太旧，会返回 GLXBadContextTag 让
# winit panic——这是上游 + 老 Mesa 的环境碰撞，跟我们的二进制无关。buster 的
# 容器里 GLX 路径本来就不可用，no-glx 场景已经覆盖了「打开不闪退」的契约。
mesa_major=""
if command -v dpkg >/dev/null 2>&1; then
    mesa_major="$(dpkg-query -W -f='${Version}' libgl1-mesa-dri 2>/dev/null | awk -F'\.' '{print $1}')"
fi
glx_unsafe=0
if [ -n "$mesa_major" ] && [ "$mesa_major" -lt 20 ] 2>/dev/null; then
    glx_unsafe=1
    echo "== Mesa $mesa_major.x is too old for winit's GLX request; skipping default (with-GLX) scenario on this image."
fi
if [ "$glx_unsafe" -eq 0 ]; then
    run_scenario default || failed=1
fi
run_scenario no-glx -extension GLX || failed=1

if [ "$failed" -ne 0 ]; then
    echo "error: launch smoke test failed on ${PRETTY_NAME:-this system}" >&2
fi
exit "$failed"
