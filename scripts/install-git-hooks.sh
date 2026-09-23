#!/usr/bin/env bash
# 把 core.hooksPath 指向 scripts/git-hooks/，让仓库里的钩子生效。
# 只对本机本仓库生效，克隆到新机器后需要再跑一次。
#
# Usage: scripts/install-git-hooks.sh [--uninstall]
#
# 钩子说明见 scripts/git-hooks/post-commit：每次 commit 后跑
# cargo build --release --locked，确保 target/release/ 始终是最新二进制。
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
HOOKS_DIR="$SCRIPT_DIR/git-hooks"

if [[ "${1:-}" == "--uninstall" ]]; then
    git -C "$REPO_ROOT" config --unset core.hooksPath
    echo "已卸载 git hooks（恢复 Git 默认的 .git/hooks 行为）。"
    exit 0
fi

if [[ ! -d "$HOOKS_DIR" ]]; then
    echo "找不到 $HOOKS_DIR" >&2
    exit 1
fi

# 确保钩子脚本可执行。Windows 下 Git Bash 写出的文件没有 +x 位，
# 显式 chmod 一次不影响 Windows（Git for Windows 会忽略执行位）。
chmod +x "$HOOKS_DIR"/* 2>/dev/null || true

git -C "$REPO_ROOT" config core.hooksPath "$HOOKS_DIR"
echo "已设置 core.hooksPath = $HOOKS_DIR"
echo "当前钩子："
ls -1 "$HOOKS_DIR" 2>/dev/null || true