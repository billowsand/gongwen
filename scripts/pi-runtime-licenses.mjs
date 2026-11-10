//! 收集随包 Pi 运行时的传递依赖许可证（构建期用）。
//!
//! 用法：`node scripts/pi-runtime-licenses.mjs <runtimeDir>`
//!
//! 输出 `licenses/DEPENDENCIES.json`（名称、版本、许可、主页）与
//! `licenses/deps/<name>@<version>/` 下的许可原文。只读 `host/node_modules`，
//! 不联网、不执行任何安装脚本。

import fs from "node:fs";
import path from "node:path";

const runtimeDir = process.argv[2];
if (!runtimeDir) {
  process.stderr.write("用法：pi-runtime-licenses.mjs <runtimeDir>\n");
  process.exit(2);
}
const modules = path.join(runtimeDir, "host", "node_modules");
if (!fs.existsSync(modules)) {
  process.stderr.write(`缺少 host/node_modules：${modules}\n`);
  process.exit(1);
}

const licenseFile = /^(LICEN[CS]E|COPYING|NOTICE|UNLICENSE)/i;
const packages = new Map();

function safeName(name) {
  return name.replace(/^@/, "").replaceAll("/", "__").replace(/[^\w.@-]/g, "_");
}

function readPackage(dir) {
  const file = path.join(dir, "package.json");
  if (!fs.existsSync(file)) return;
  let data;
  try {
    data = JSON.parse(fs.readFileSync(file, "utf8"));
  } catch {
    return;
  }
  if (!data?.name || !data?.version) return;
  const key = `${data.name}@${data.version}`;
  if (packages.has(key)) return;
  const license = typeof data.license === "string" ? data.license : data.license?.type ?? "";
  packages.set(key, {
    name: data.name,
    version: data.version,
    license,
    homepage: typeof data.homepage === "string" ? data.homepage : "",
  });
  const target = path.join(runtimeDir, "licenses", "deps", safeName(key));
  let copied = 0;
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    if (!entry.isFile() || !licenseFile.test(entry.name) || copied >= 3) continue;
    fs.mkdirSync(target, { recursive: true });
    fs.copyFileSync(path.join(dir, entry.name), path.join(target, entry.name));
    copied += 1;
  }
  // 有的包把许可原文放在 LICENSES/ 子目录，且文件名带前缀（不保证以 LICENSE 开头），
  // 例如 @earendil-works/pi-schema 内置的 LICENSES/typebox-LICENSE。
  let nestedDirs = [];
  try {
    nestedDirs = fs
      .readdirSync(dir, { withFileTypes: true })
      .filter((entry) => entry.isDirectory() && /^licen[cs]es$/i.test(entry.name))
      .map((entry) => path.join(dir, entry.name));
  } catch {
    nestedDirs = [];
  }
  for (const nested of nestedDirs) {
    let entries = [];
    try {
      entries = fs.readdirSync(nested, { withFileTypes: true });
    } catch {
      continue;
    }
    for (const entry of entries) {
      if (copied >= 3) break;
      if (!entry.isFile() || !/LICEN[CS]E|COPYING|NOTICE|UNLICENSE/i.test(entry.name)) continue;
      fs.mkdirSync(target, { recursive: true });
      fs.copyFileSync(path.join(nested, entry.name), path.join(target, entry.name));
      copied += 1;
    }
  }
}

function walk(dir, depth) {
  if (depth > 8) return;
  let entries;
  try {
    entries = fs.readdirSync(dir, { withFileTypes: true });
  } catch {
    return;
  }
  for (const entry of entries) {
    if (!entry.isDirectory() || entry.name.startsWith(".")) continue;
    const full = path.join(dir, entry.name);
    // 作用域目录（@earendil-works）本身不是包，往下再走一层。
    if (entry.name.startsWith("@")) {
      walk(full, depth + 1);
      continue;
    }
    readPackage(full);
    const nested = path.join(full, "node_modules");
    if (fs.existsSync(nested)) walk(nested, depth + 1);
  }
}

walk(modules, 0);
const list = [...packages.values()].sort((a, b) => a.name.localeCompare(b.name));
fs.mkdirSync(path.join(runtimeDir, "licenses"), { recursive: true });
fs.writeFileSync(
  path.join(runtimeDir, "licenses", "DEPENDENCIES.json"),
  `${JSON.stringify({ generatedAt: new Date().toISOString(), packages: list }, null, 2)}\n`,
  "utf8",
);
process.stdout.write(`licenses: ${list.length} packages\n`);
