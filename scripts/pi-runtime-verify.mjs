//! 校验随包 Pi 运行时（构建期用，Linux staging 与 deb / Arch 打包共用）。
//!
//! 用法：`node scripts/pi-runtime-verify.mjs <runtimeDir> <platform> [--list]`
//!
//! 失败以非 0 退出并打印中文原因。`--list` 成功时按行输出 manifest 里的相对路径，
//! 供打包脚本复制。

import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const projectRoot = path.resolve(scriptDir, "..");
const [runtimeArg, platform, ...flags] = process.argv.slice(2);
if (!runtimeArg || !platform) {
  process.stderr.write("用法：pi-runtime-verify.mjs <runtimeDir> <platform> [--list]\n");
  process.exit(2);
}
const runtimeDir = path.resolve(runtimeArg);
const list = flags.includes("--list");
const config = JSON.parse(fs.readFileSync(path.join(projectRoot, "packaging", "pi-runtime.json"), "utf8"));
const platformConfig = config.platforms?.[platform];
if (!platformConfig) {
  process.stderr.write(`不支持的平台：${platform}\n`);
  process.exit(1);
}
const manifestPath = path.join(runtimeDir, "manifest.json");
if (!fs.existsSync(manifestPath)) {
  process.stderr.write(`缺少 Pi 运行时 manifest：${manifestPath}。先运行 scripts/stage-pi-runtime.sh。\n`);
  process.exit(1);
}
const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
if (manifest.schema !== 1) {
  process.stderr.write(`manifest schema 是 ${manifest.schema}，期望 1\n`);
  process.exit(1);
}
if (manifest.source !== "npm-ci") {
  process.stderr.write(`运行时不是 npm ci 产物（source: ${manifest.source}），拒绝打包\n`);
  process.exit(1);
}
if (manifest.platform !== platform) {
  process.stderr.write(`运行时是给 ${manifest.platform} 的，本包是 ${platform}\n`);
  process.exit(1);
}
if (manifest.node.version !== config.node || manifest.node.sha256 !== platformConfig.sha256) {
  process.stderr.write("运行时 Node 版本或压缩包哈希与 packaging/pi-runtime.json 不一致\n");
  process.exit(1);
}
if (manifest.host.version !== config.host.version || manifest.host.protocol !== config.host.protocol) {
  process.stderr.write("运行时 host 版本或协议与配置不一致\n");
  process.exit(1);
}
const lockfileHash = createHash("sha256")
  .update(fs.readFileSync(path.join(projectRoot, config.host.lockfile)))
  .digest("hex");
if (manifest.host.lockfileSha256 !== lockfileHash) {
  process.stderr.write("运行时依赖锁文件已过期，请重新组装 Pi 运行时\n");
  process.exit(1);
}
for (const [name, version] of Object.entries(config.pi)) {
  if (manifest.pi?.[name] !== version) {
    process.stderr.write(`依赖 ${name} 是 ${manifest.pi?.[name]}，需要 ${version}\n`);
    process.exit(1);
  }
}
const required = new Set([
  platformConfig.binary,
  "host/main.mjs",
  "host/package.json",
  "host/node_modules/@earendil-works/pi-durable/package.json",
  "host/node_modules/@earendil-works/pi-ai/package.json",
  "host/node_modules/@earendil-works/chord/package.json",
  "licenses/LICENSE.node",
  "licenses/DEPENDENCIES.json",
]);
const listed = new Set();
for (const file of manifest.files ?? []) {
  const relative = String(file.path ?? "");
  if (!relative || relative.includes("..") || relative.startsWith("/") || relative.includes("\\") || relative.includes(":") || relative.includes("\0")) {
    process.stderr.write(`manifest 含不安全路径：${relative}\n`);
    process.exit(1);
  }
  const full = path.join(runtimeDir, relative);
  if (!fs.existsSync(full) || !fs.statSync(full).isFile()) {
    process.stderr.write(`manifest 列出的文件不存在：${full}\n`);
    process.exit(1);
  }
  if (file.bytes !== undefined && fs.statSync(full).size !== file.bytes) {
    process.stderr.write(`文件体积不符：${full}\n`);
    process.exit(1);
  }
  const actual = createHash("sha256").update(fs.readFileSync(full)).digest("hex");
  if (actual !== String(file.sha256).toLowerCase()) {
    process.stderr.write(`文件哈希不符：${full}\n`);
    process.exit(1);
  }
  listed.add(relative);
}
for (const relative of required) {
  if (!listed.has(relative)) {
    process.stderr.write(`manifest 没有列出必需文件：${relative}\n`);
    process.exit(1);
  }
}
const node = path.join(runtimeDir, platformConfig.binary);
// POSIX 平台必须带可执行位：zip 中转、ditto、cp 丢权限时，症状会是运行期
// EACCES（「Pi 不可用」），而不是打包期失败。这里在打包阶段就拦下来。
if (process.platform !== "win32") {
  try {
    fs.accessSync(node, fs.constants.X_OK);
  } catch {
    process.stderr.write(
      `随包 Node 没有可执行位：${node}。请 chmod 755 后重新打包（staging 阶段请用 scripts/stage-pi-runtime.sh）。\n`,
    );
    process.exit(1);
  }
}
const version = String(
  (await import("node:child_process")).execFileSync(node, ["--version"], { encoding: "utf8" }),
).trim();
if (version !== `v${config.node}`) {
  process.stderr.write(`随包 Node 报告 ${version}，期望 v${config.node}\n`);
  process.exit(1);
}
if (list) {
  for (const relative of [...listed].sort()) process.stdout.write(`${relative}\n`);
} else {
  process.stdout.write(
    `Pi runtime verified: ${platform} · Node ${config.node} · host ${config.host.version} · ${listed.size} files\n`,
  );
}
