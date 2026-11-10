//! 生成随包 Pi 运行时的 manifest（构建期用，不随包运行）。
//!
//! 用法：`node scripts/pi-runtime-manifest.mjs <runtimeDir> <platform> <configPath>`
//!
//! manifest 声明平台、Node / Pi / host / 协议版本、lockfile 哈希，以及运行目录里
//! 每个文件的 SHA-256 与字节数。Rust 启动时按它校验关键入口，打包阶段校验全量。

import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";

const [runtimeDir, platform, configPath] = process.argv.slice(2);
if (!runtimeDir || !platform || !configPath) {
  process.stderr.write("用法：pi-runtime-manifest.mjs <runtimeDir> <platform> <configPath>\n");
  process.exit(2);
}
const config = JSON.parse(fs.readFileSync(configPath, "utf8"));
const platformConfig = config.platforms?.[platform];
if (!platformConfig) {
  process.stderr.write(`不认识的平台：${platform}\n`);
  process.exit(2);
}

function sha256(file) {
  return createHash("sha256").update(fs.readFileSync(file)).digest("hex");
}

function walk(dir, prefix = "") {
  const out = [];
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    if (entry.name.startsWith(".")) continue;
    const relative = prefix ? `${prefix}/${entry.name}` : entry.name;
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) out.push(...walk(full, relative));
    else if (entry.isFile()) out.push(relative);
  }
  return out;
}

const files = walk(runtimeDir)
  .filter((relative) => relative !== "manifest.json")
  .sort()
  .map((relative) => {
    const full = path.join(runtimeDir, relative.split("/").join(path.sep));
    const stat = fs.statSync(full);
    return { path: relative, bytes: stat.size, sha256: sha256(full) };
  });

const lockfile = path.resolve(configPath, "..", "..", config.host.lockfile);
if (!fs.existsSync(lockfile)) {
  process.stderr.write(`缺少 lockfile：${lockfile}\n`);
  process.exit(1);
}

// 版本必须取自**组装出来的 host 本身**，不能直接抄 config：升级时若改了
// packaging/pi-runtime.json 却漏改 pi-host 源码，直接抄配置会让 manifest「看起来正确」，
// 直到运行期握手（Rust 比对 ready.hostVersion）或离线验收才暴露。这里在构建期就失败。
const hostDir = path.join(runtimeDir, "host");
const hostPackageFile = path.join(hostDir, "package.json");
const hostProtocolFile = path.join(hostDir, "protocol.mjs");
for (const file of [hostPackageFile, hostProtocolFile]) {
  if (!fs.existsSync(file)) {
    process.stderr.write(`缺少 host 文件，无法核对版本：${file}\n`);
    process.exit(1);
  }
}
const builtHostPackage = JSON.parse(fs.readFileSync(hostPackageFile, "utf8"));
const builtHostProtocol = fs.readFileSync(hostProtocolFile, "utf8");
const builtHostVersion = builtHostProtocol.match(/export const HOST_VERSION = "([^"]+)";/)?.[1];
const builtProtocolVersion = Number(builtHostProtocol.match(/export const PROTOCOL_VERSION = (\d+);/)?.[1]);
if (builtHostVersion !== config.host.version || builtHostPackage.version !== config.host.version) {
  process.stderr.write(
    `host 源码版本与 packaging/pi-runtime.json 不一致：protocol.mjs=${builtHostVersion ?? "未知"}、` +
      `package.json=${builtHostPackage.version ?? "未知"}，配置要求 ${config.host.version}。请同步 host 源码后再组装。\n`,
  );
  process.exit(1);
}
if (!Number.isInteger(builtProtocolVersion) || builtProtocolVersion !== config.host.protocol) {
  process.stderr.write(
    `host 协议版本与 packaging/pi-runtime.json 不一致：protocol.mjs=${builtProtocolVersion}，配置要求 ${config.host.protocol}。\n`,
  );
  process.exit(1);
}

const manifest = {
  schema: 1,
  source: "npm-ci",
  platform,
  node: {
    version: config.node,
    file: platformConfig.file,
    sha256: platformConfig.sha256,
    binary: platformConfig.binary,
  },
  pi: config.pi,
  host: {
    version: builtHostVersion,
    protocol: builtProtocolVersion,
    main: config.host.main,
    lockfileSha256: sha256(lockfile),
  },
  files,
  generatedAt: new Date().toISOString(),
};
fs.writeFileSync(path.join(runtimeDir, "manifest.json"), `${JSON.stringify(manifest, null, 2)}\n`, "utf8");
process.stdout.write(
  `manifest: ${platform} · Node ${config.node} · host ${config.host.version} · ${files.length} files\n`,
);
