//! host 私有文件的统一明文视图与 Durable FileSystem。
//!
//! 所有内容读取都先解密，所有内容写入都先密封；文件大小与读取上限均按明文计算。
//! 加密模式的 append / truncate / rename 通过整份明文转换后原子重写，绝不向密文追加明文。

import { randomUUID } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { NodeExecutionEnv } from "@earendil-works/pi-durable/env/node";
import { ExecutionError, FileError, err, ok } from "@earendil-works/pi-durable/env";
import {
  EncryptionError,
  GWA1_MAGIC,
  GWA2_MAGIC,
  SEALED_FILE_OVERHEAD,
  openResource,
  resourceIdentity,
  sealGwa2,
} from "./encryption.mjs";

/** 加密文件追加后的明文总量上限：超过就停机报错，不允许无限整份重写。 */
export const MAX_ENCRYPTED_APPEND_BYTES = 64 * 1024 * 1024;
/** 未显式给上限时的单文件明文读取上限，防止一次读进任意大的文件把进程撑爆。 */
export const DEFAULT_READ_MAX_BYTES = 256 * 1024 * 1024;

/**
 * 原子写临时文件名：`.<pid>-<uuid>.tmp`。只认自己写出来的精确形态，
 * 避免误删模型或用户恰好同名的文件。
 */
const ATOMIC_TEMP_PATTERN = /^\.\d+-[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\.tmp$/i;

export function isAtomicTempName(name) {
  return typeof name === "string" && ATOMIC_TEMP_PATTERN.test(name);
}

/**
 * 清理目录下崩溃残留的原子写临时文件（递归、限深、限条目，尽力而为）。
 * 同一轮工作区同时只有一个 host，所以启动时还存在的临时文件一定是上一进程
 * 被打断留下的；这些半截密文会让 `listDir`/配额扫描解密失败。
 */
export function cleanStaleTemps(root, { maxEntries = 50_000, maxDepth = 12 } = {}) {
  if (typeof root !== "string" || !path.isAbsolute(root)) return 0;
  let removed = 0;
  let visited = 0;
  const pending = [{ dir: root, depth: 0 }];
  while (pending.length > 0) {
    const { dir, depth } = pending.pop();
    let entries;
    try {
      entries = fs.readdirSync(dir, { withFileTypes: true });
    } catch {
      continue;
    }
    for (const entry of entries) {
      if (++visited > maxEntries) return removed;
      const full = path.join(dir, entry.name);
      if (entry.isDirectory()) {
        if (depth < maxDepth) pending.push({ dir: full, depth: depth + 1 });
        continue;
      }
      if (!entry.isFile() || !isAtomicTempName(entry.name)) continue;
      try {
        fs.unlinkSync(full);
        removed += 1;
      } catch {
        // 清理是尽力而为：删不掉（权限/占用）就留给下一次。
      }
    }
  }
  return removed;
}

/** rename 之后尽力让父目录的目录项也落盘；Windows 不支持目录 fsync，忽略失败。 */
function fsyncDirectory(dir) {
  if (process.platform === "win32") return;
  let fd;
  try {
    fd = fs.openSync(dir, "r");
    fs.fsyncSync(fd);
  } catch {
    // 部分平台/文件系统不支持对目录 fsync；原子替换本身仍然成立。
  } finally {
    if (fd !== undefined) {
      try {
        fs.closeSync(fd);
      } catch {
        // 关闭失败不影响写入结果。
      }
    }
  }
}

/** 同目录临时文件 + fsync + rename；传入这里的必须已经是最终物理字节。 */
export function atomicWrite(target, content) {
  fs.mkdirSync(path.dirname(target), { recursive: true });
  const temp = path.join(path.dirname(target), `.${process.pid}-${randomUUID()}.tmp`);
  let fd;
  try {
    fd = fs.openSync(temp, "wx", 0o600);
    fs.writeFileSync(fd, content);
    fs.fsyncSync(fd);
    fs.closeSync(fd);
    fd = undefined;
    // Windows 的杀毒/索引器可能短暂持有目标文件，只有限重试，不删除旧件。
    for (let attempt = 0; ; attempt += 1) {
      try {
        fs.renameSync(temp, target);
        break;
      } catch (error) {
        if (process.platform !== "win32" || !["EPERM", "EACCES", "EBUSY"].includes(error?.code) || attempt >= 4) throw error;
        Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 20 * (attempt + 1));
      }
    }
    // rename 只保证内存视图原子；掉电后目录项可能还没落盘，紧随其后的提交会「回退」。
    fsyncDirectory(path.dirname(target));
  } catch (error) {
    if (fd !== undefined) {
      try {
        fs.closeSync(fd);
      } catch {
        // 关闭失败不覆盖原始错误。
      }
    }
    try {
      fs.unlinkSync(temp);
    } catch {
      // 临时文件可能还没创建。
    }
    throw error;
  }
}

function asBytes(content) {
  return Buffer.isBuffer(content)
    ? content
    : typeof content === "string"
      ? Buffer.from(content, "utf8")
      : Buffer.from(content.buffer, content.byteOffset, content.byteLength);
}

function tooLarge(maxBytes) {
  const error = new Error(`文件超过明文读取上限（${maxBytes} 字节）`);
  error.code = "EFBIG";
  return error;
}

/** 用打开后的 fd 有界读取，物理预算只比明文预算多固定封套开销。 */
function readPhysical(target, maxBytes) {
  if (!Number.isSafeInteger(maxBytes) || maxBytes < 0) throw new TypeError("maxBytes 必须是非负安全整数");
  const physicalLimit = maxBytes + SEALED_FILE_OVERHEAD;
  const fd = fs.openSync(target, "r");
  try {
    const stat = fs.fstatSync(fd);
    if (!stat.isFile()) {
      const error = new Error("不是普通文件");
      error.code = stat.isDirectory() ? "EISDIR" : "EINVAL";
      throw error;
    }
    if (stat.size > physicalLimit) throw tooLarge(maxBytes);
    const expected = Math.max(0, Math.min(stat.size, physicalLimit));
    const bytes = Buffer.alloc(expected + 1);
    let used = 0;
    while (used < bytes.length) {
      const count = fs.readSync(fd, bytes, used, bytes.length - used, null);
      if (count === 0) break;
      used += count;
    }
    if (used > physicalLimit) throw tooLarge(maxBytes);
    return bytes.subarray(0, used);
  } finally {
    fs.closeSync(fd);
  }
}

/** 只读前 4 字节魔数，不整份解密。 */
function fileHead(target) {
  const fd = fs.openSync(target, "r");
  try {
    const head = Buffer.alloc(4);
    const count = fs.readSync(fd, head, 0, 4, 0);
    return head.subarray(0, count);
  } finally {
    fs.closeSync(fd);
  }
}

/** 密封文件的明文长度 = 物理长度 - 固定封套；封套本身不完整按损坏处理。 */
function sealedPlaintextSize(stat, head) {
  if (!head.equals(GWA1_MAGIC) && !head.equals(GWA2_MAGIC)) return stat.size;
  if (stat.size < SEALED_FILE_OVERHEAD) {
    throw new EncryptionError("加密文件长度不符（封套不完整）", "encryption_corrupt");
  }
  return stat.size - SEALED_FILE_OVERHEAD;
}

function infoFromStat(target, stat, size = stat.size) {
  const kind = stat.isFile() ? "file" : stat.isDirectory() ? "directory" : stat.isSymbolicLink() ? "symlink" : undefined;
  if (!kind) throw Object.assign(new Error("不支持的文件类型"), { code: "EINVAL" });
  return { name: path.basename(target), path: target, kind, size, mtimeMs: stat.mtimeMs };
}

/** Windows 的 realpath 可能返回 `\\?\` 长路径前缀：去掉它，避免两边口径不同。 */
function stripLongPathPrefix(target) {
  if (typeof target !== "string") return target;
  if (target.startsWith("\\\\?\\UNC\\")) return `\\\\${target.slice(8)}`;
  if (target.startsWith("\\\\?\\")) return target.slice(4);
  return target;
}

function canonicalNearest(target) {
  let current = path.resolve(stripLongPathPrefix(target));
  const missing = [];
  for (;;) {
    try { return path.join(fs.realpathSync.native(current), ...missing.reverse()); }
    catch (error) {
      if (error?.code !== "ENOENT" && error?.code !== "ENOTDIR") throw error;
      const parent = path.dirname(current);
      if (parent === current) throw error;
      missing.push(path.basename(current)); current = parent;
    }
  }
}

export class ProtectedFiles {
  constructor(encryption = undefined, { maxAppendBytes = MAX_ENCRYPTED_APPEND_BYTES, maxReadBytes = DEFAULT_READ_MAX_BYTES } = {}) {
    for (const [name, value] of [["maxAppendBytes", maxAppendBytes], ["maxReadBytes", maxReadBytes]]) {
      if (!Number.isSafeInteger(value) || value < 0) throw new TypeError(`${name} 必须是非负安全整数`);
    }
    this.maxAppendBytes = maxAppendBytes;
    this.maxReadBytes = maxReadBytes;
    this.encryption = encryption;
    this.trees = [];
    if (!encryption) return;
    if (typeof encryption.keyHex !== "string" || !/^[0-9a-fA-F]{64}$/.test(encryption.keyHex)) {
      throw new EncryptionError("encryption.keyHex 必须是 64 位十六进制", "encryption_key");
    }
    if (!["real", "decoy"].includes(encryption.slot)) {
      throw new EncryptionError("encryption.slot 只支持 real / decoy", "encryption_slot");
    }
    if (typeof encryption.configRoot !== "string" || !path.isAbsolute(encryption.configRoot)) {
      throw new EncryptionError("encryption.configRoot 必须是绝对路径", "encryption_path");
    }
    this.key = Buffer.from(encryption.keyHex, "hex");
    this.slot = encryption.slot;
    this.configRoot = path.resolve(stripLongPathPrefix(encryption.configRoot));
    // 配置目录可能经符号链接/junction 访问：记下 realpath 口径，身份计算两种都认，
    // 否则 Node 的守卫把路径 realpath 过后 AAD 会和 Rust 写的对不上。
    let real = this.configRoot;
    try {
      real = stripLongPathPrefix(fs.realpathSync.native(this.configRoot));
    } catch {
      // 配置根还不存在（首次启动）时没有 realpath，用字面路径即可。
    }
    this.realConfigRoot = real;
  }

  // 名称和内容放进同一个 GWA2 文件；逻辑路径仅存在内存和密文里。
  mountTree(root, bundle, { readonly = false } = {}) {
    if (!this.encrypted) return;
    fs.mkdirSync(root, { recursive: true });
    root = fs.realpathSync.native(root);
    if (this.trees.some((t) => t.root === root)) return;
    let entries = new Map();
    try {
      const raw = this.readBytes(bundle, 512 * 1024 * 1024);
      const object = JSON.parse(raw.toString("utf8"));
      if (!object || Array.isArray(object) || typeof object !== "object") throw new Error("文件树格式错误");
      for (const [name, value] of Object.entries(object)) {
        if (!name || name.startsWith("/") || name.includes("\\") || name.split("/").some((p) => p === ".." || p === ".") || typeof value !== "string" || !/^(?:[0-9a-f]{2})*$/.test(value)) throw new Error("加密文件树含非法条目");
        entries.set(name, value);
      }
    } catch (error) { if (error?.code !== "ENOENT") throw error; }
    // 接管旧工作区：先完整加密写容器，再清理旧文件。失败保留容器，重入校验一致后继续。
    cleanStaleTemps(root);
    const pending = [root];
    const legacy = [];
    while (pending.length) {
      const dir = pending.pop();
      let children;
      try { children = fs.readdirSync(dir, { withFileTypes: true }); }
      catch (error) { if (error?.code === "ENOENT") continue; throw error; }
      for (const child of children) {
        const full = path.join(dir, child.name);
        if (child.isSymbolicLink()) throw new Error("旧文件树包含链接，拒绝迁移");
        if (child.isDirectory()) { pending.push(full); continue; }
        if (!child.isFile()) throw new Error("旧文件树包含特殊文件");
        const name = path.relative(root, full).split(path.sep).join("/");
        const value = this.readBytes(full).toString("hex");
        if (entries.has(name) && entries.get(name) !== value) throw new Error("旧文件与加密文件树冲突，保留两份");
        entries.set(name, value);
        legacy.push(full);
      }
    }
    if (legacy.length) {
      const bytes = Buffer.from(JSON.stringify(Object.fromEntries(entries)));
      if (bytes.length > 512 * 1024 * 1024) throw tooLarge(512 * 1024 * 1024);
      this.atomicWrite(bundle, bytes);
      for (const full of legacy) {
        const fd = fs.openSync(full, "r+");
        try {
          let left = fs.fstatSync(fd).size; const zeros = Buffer.alloc(65536); let offset = 0;
          while (left) { const n = Math.min(left, zeros.length); fs.writeSync(fd, zeros, 0, n, offset); left -= n; offset += n; }
          fs.fsyncSync(fd);
        } finally { fs.closeSync(fd); }
        fs.unlinkSync(full);
      }
      fs.rmSync(root, { recursive: true });
      fs.mkdirSync(root, { recursive: true });
    }
    this.trees.push({ root, bundle, entries, readonly, mtimeMs: Date.now() });
  }

  treePath(target) {
    if (!this.trees.length) return undefined;
    const absolute = canonicalNearest(target);
    for (const tree of this.trees) {
      const relative = path.relative(tree.root, absolute).split(path.sep).join("/");
      if (relative !== ".." && !relative.startsWith("../") && !path.isAbsolute(relative)) return { tree, relative };
    }
    return undefined;
  }

  isVirtual(target) { return Boolean(this.treePath(target)); }

  saveTree(tree, entries) {
    if (tree.readonly) throw Object.assign(new Error("只读技能快照不能修改"), { code: "EACCES" });
    const bytes = Buffer.from(JSON.stringify(Object.fromEntries(entries)));
    if (bytes.length > 512 * 1024 * 1024) throw tooLarge(512 * 1024 * 1024);
    this.atomicWrite(tree.bundle, bytes);
    tree.entries = entries;
    tree.mtimeMs = Date.now();
  }

  createDir(target, options = {}) {
    const virtual = this.treePath(target);
    if (!virtual) return fs.mkdirSync(target, options);
    const { tree, relative } = virtual;
    if (!relative) return;
    const entries = new Map(tree.entries);
    entries.set(`${relative}/`, "");
    this.saveTree(tree, entries);
  }

  removePath(target, options = {}) {
    const virtual = this.treePath(target);
    if (!virtual) return fs.rmSync(target, options);
    const { tree, relative } = virtual;
    if (!relative) throw new Error("不能删除工作区根目录");
    const entries = new Map(tree.entries);
    const children = [...entries.keys()].filter((name) => name.startsWith(`${relative}/`));
    if (children.length && !options.recursive) throw Object.assign(new Error("目录非空"), { code: "ENOTEMPTY" });
    if (!entries.has(relative) && !children.length && !options.force) throw Object.assign(new Error("文件不存在"), { code: "ENOENT" });
    entries.delete(relative);
    for (const child of children) entries.delete(child);
    this.saveTree(tree, entries);
  }

  get encrypted() {
    return Boolean(this.encryption);
  }

  identity(target) {
    if (!this.encrypted) return undefined;
    return resourceIdentity(stripLongPathPrefix(target), {
      slot: this.slot,
      configRoot: this.configRoot,
      realConfigRoot: this.realConfigRoot,
    });
  }

  assertPath(target) {
    this.identity(target);
  }

  /** 本次读取的明文上限：显式值优先，未给时用实例默认上限（防 OOM）。 */
  #readLimit(maxBytes) {
    if (maxBytes === undefined) return this.maxReadBytes;
    if (!Number.isSafeInteger(maxBytes) || maxBytes < 0) throw new TypeError("maxBytes 必须是非负安全整数");
    return maxBytes;
  }

  readBytes(target, maxBytes = undefined) {
    const virtual = this.treePath(target);
    if (virtual) {
      const hex = virtual.tree.entries.get(virtual.relative);
      if (hex === undefined) throw Object.assign(new Error("文件不存在"), { code: "ENOENT" });
      if (hex.length / 2 > this.#readLimit(maxBytes)) throw tooLarge(this.#readLimit(maxBytes));
      return Buffer.from(hex, "hex");
    }
    const identity = this.identity(target);
    const limit = this.#readLimit(maxBytes);
    const raw = readPhysical(target, limit);
    if (!identity) {
      if (raw.length > limit) throw tooLarge(limit);
      return raw;
    }
    const plain = openResource(this.key, identity.slot, identity.logicalId, raw);
    if (plain.length > limit) throw tooLarge(limit);
    return plain;
  }

  readText(target, maxBytes = undefined) {
    return this.readBytes(target, maxBytes).toString("utf8");
  }

  atomicWrite(target, content) {
    const plain = asBytes(content);
    const virtual = this.treePath(target);
    if (virtual) {
      if (!virtual.relative) throw new Error("不能覆盖工作区根目录");
      const entries = new Map(virtual.tree.entries);
      entries.set(virtual.relative, plain.toString("hex"));
      this.saveTree(virtual.tree, entries);
      return;
    }
    const identity = this.identity(target);
    const physical = identity ? sealGwa2(this.key, identity.slot, identity.logicalId, plain) : plain;
    atomicWrite(target, physical);
  }

  appendFile(target, content) {
    const addition = asBytes(content);
    if (!this.encrypted) {
      fs.mkdirSync(path.dirname(target), { recursive: true });
      fs.appendFileSync(target, addition);
      return;
    }
    let previous;
    try {
      previous = this.readBytes(target);
    } catch (error) {
      if (error?.code !== "ENOENT") throw error;
      previous = Buffer.alloc(0);
    }
    const total = previous.length + addition.length;
    // 加密追加是「解密旧内容 → 内存拼接 → 整份原子重写」，不设上限会随着会话
    // 变长把单次提交拖到秒级、内存峰值不可控；超限必须明确报错而不是硬扛。
    if (total > this.maxAppendBytes) {
      const error = new Error(
        `加密文件追加后超过上限（${total} / ${this.maxAppendBytes} 字节）：${target}；会话或文件过大，请清理旧会话或新建文件`,
      );
      error.code = "EFBIG";
      throw error;
    }
    this.atomicWrite(target, Buffer.concat([previous, addition]));
  }

  truncateFile(target, size) {
    if (!Number.isSafeInteger(size) || size < 0) throw new TypeError("文件大小必须是非负安全整数");
    if (!this.encrypted) {
      fs.truncateSync(target, size);
      return;
    }
    const previous = this.readBytes(target);
    const next = Buffer.alloc(size);
    previous.copy(next, 0, 0, Math.min(size, previous.length));
    this.atomicWrite(target, next);
  }

  renameFile(source, destination) {
    if (!this.encrypted) {
      fs.mkdirSync(path.dirname(destination), { recursive: true });
      fs.renameSync(source, destination);
      return;
    }
    // GWA2 绑定目标路径，不能直接 rename 后留下旧 AAD 的密文。
    const plain = this.readBytes(source);
    this.atomicWrite(destination, plain);
    this.removePath(source);
  }

  flushFile(target) {
    if (this.encrypted) {
      this.assertPath(target);
      return;
    }
    const fd = fs.openSync(target, "r+");
    try {
      fs.fsyncSync(fd);
    } finally {
      fs.closeSync(fd);
    }
  }

  info(target) {
    this.assertPath(target);
    const virtual = this.treePath(target);
    if (virtual) {
      const { tree, relative } = virtual;
      const hex = tree.entries.get(relative);
      const directory = !relative || [...tree.entries.keys()].some((name) => name.startsWith(`${relative}/`));
      if (hex === undefined && !directory) throw Object.assign(new Error("文件不存在"), { code: "ENOENT" });
      return { name: path.basename(target), path: target, kind: hex === undefined ? "directory" : "file", size: hex === undefined ? 0 : hex.length / 2, mtimeMs: tree.mtimeMs };
    }
    const stat = fs.lstatSync(target);
    if (!stat.isFile()) return infoFromStat(target, stat, stat.size);
    // 崩溃残留的临时文件可能是半截密文：只为取大小，别去解密，也别让上层列举失败。
    if (isAtomicTempName(path.basename(target))) return infoFromStat(target, stat, stat.size);
    // 密封文件按「物理长度 - 固定封套」回推明文长度，不整份解密；
    // 明文模式直接看 stat，连读文件头都省掉。
    const size = this.encrypted ? sealedPlaintextSize(stat, fileHead(target)) : stat.size;
    return infoFromStat(target, stat, size);
  }

  listDir(target) {
    this.assertPath(target);
    const virtual = this.treePath(target);
    if (virtual) {
      if (this.info(target).kind !== "directory") throw Object.assign(new Error("不是目录"), { code: "ENOTDIR" });
      const prefix = virtual.relative ? `${virtual.relative}/` : "";
      const names = new Set([...virtual.tree.entries.keys()].filter((name) => name.startsWith(prefix)).map((name) => name.slice(prefix.length).split("/")[0]).filter(Boolean));
      return [...names].sort().map((name) => this.info(path.join(target, name)));
    }
    return fs
      .readdirSync(target, { withFileTypes: true })
      // 残留临时文件不是资源：跳过，否则 JsonlStorage.recover 会在 listDir 阶段
      // 因半截密文解密失败而整目录失败。
      .filter((entry) => !isAtomicTempName(entry.name))
      .map((entry) => this.info(path.join(target, entry.name)));
  }
}

export const plaintextFiles = new ProtectedFiles();

function fileError(error, target) {
  if (error instanceof FileError) return error;
  const codes = {
    ABORT_ERR: "aborted",
    ENOENT: "not_found",
    EACCES: "permission_denied",
    EPERM: "permission_denied",
    ENOTDIR: "not_directory",
    EISDIR: "is_directory",
    EINVAL: "invalid",
    EFBIG: "invalid",
  };
  return new FileError(codes[error?.code] ?? "unknown", error?.message ?? String(error), target, error instanceof Error ? error : undefined);
}

function aborted(context, target) {
  return context?.abortSignal?.aborted ? err(new FileError("aborted", "aborted", target)) : undefined;
}

class MemoryTextLineReader {
  constructor(bytes, target) {
    this.bytes = bytes;
    this.target = target;
    this.offset = 0;
    this.closed = false;
  }

  async readLine(context) {
    const stop = aborted(context, this.target);
    if (stop) return stop;
    if (this.closed) return err(new FileError("invalid", "Text line reader is closed", this.target));
    if (this.offset >= this.bytes.length) return ok(undefined);
    const newline = this.bytes.indexOf(0x0a, this.offset);
    const end = newline < 0 ? this.bytes.length : newline;
    const text = new TextDecoder().decode(this.bytes.subarray(this.offset, end));
    this.offset = newline < 0 ? this.bytes.length : newline + 1;
    return ok({ text, terminated: newline >= 0 });
  }

  async close() {
    this.closed = true;
    this.bytes = Buffer.alloc(0);
  }
}

function decodedSize(bytes) {
  return Buffer.byteLength(new TextDecoder().decode(bytes), "utf8");
}

function scanLines(bytes, startLine, endLine = Number.POSITIVE_INFINITY) {
  if (!Number.isSafeInteger(startLine) || startLine < 0 || !(endLine > startLine) ||
      (endLine !== Number.POSITIVE_INFINITY && !Number.isSafeInteger(endLine))) {
    throw new RangeError("Invalid line range");
  }
  const newlines = [];
  for (let index = bytes.indexOf(0x0a); index >= 0; index = bytes.indexOf(0x0a, index + 1)) newlines.push(index);
  const size = bytes.length;
  const totalLines = newlines.length + 1;
  if (startLine >= totalLines) {
    return { newlines: newlines.length, start: size, end: size, firstLineEnd: size, lastLineStart: size, selectedBytes: 0, firstLineBytes: 0 };
  }
  const start = startLine === 0 ? 0 : newlines[startLine - 1] + 1;
  const firstLineEnd = newlines[startLine] ?? size;
  const lastLine = Math.min(endLine, totalLines) - 1;
  const lastLineStart = lastLine === 0 ? 0 : newlines[lastLine - 1] + 1;
  const end = endLine <= newlines.length ? newlines[endLine - 1] : size;
  return {
    newlines: newlines.length,
    start,
    end,
    firstLineEnd,
    lastLineStart,
    selectedBytes: decodedSize(bytes.subarray(start, end)),
    firstLineBytes: decodedSize(bytes.subarray(start, firstLineEnd)),
  };
}

class MemoryBinaryReader {
  constructor(bytes, info) {
    this.bytes = bytes;
    this.details = info;
    this.closed = false;
  }

  usable(target) {
    return this.closed ? err(new FileError("invalid", "Binary reader is closed", target)) : undefined;
  }

  async info(context) {
    return aborted(context, this.details.path) ?? this.usable(this.details.path) ?? ok(this.details);
  }

  async read(offset, length, context) {
    const target = this.details.path;
    const unavailable = aborted(context, target) ?? this.usable(target);
    if (unavailable) return unavailable;
    if (!Number.isSafeInteger(offset) || offset < 0 || !Number.isSafeInteger(length) || length < 0) {
      return err(new FileError("invalid", "Offset and length must be non-negative safe integers", target));
    }
    return ok(this.bytes.subarray(offset, Math.min(this.bytes.length, offset + length)));
  }

  async scanLines(options, context) {
    const target = this.details.path;
    const unavailable = aborted(context, target) ?? this.usable(target);
    if (unavailable) return unavailable;
    try {
      return ok(scanLines(this.bytes, options.startLine, options.endLine));
    } catch {
      return err(new FileError("invalid", "Invalid line range", target));
    }
  }

  async close() {
    this.closed = true;
    this.bytes = Buffer.alloc(0);
  }
}

class MemoryDirReader {
  constructor(entries) {
    this.entries = entries;
    this.offset = 0;
    this.closed = false;
  }

  async next(maxEntries, context) {
    const target = this.entries[0]?.path;
    const stop = aborted(context, target);
    if (stop) return stop;
    if (this.closed) return err(new FileError("invalid", "Directory reader is closed", target));
    if (!Number.isSafeInteger(maxEntries) || maxEntries <= 0) {
      return err(new FileError("invalid", "maxEntries must be a positive safe integer", target));
    }
    const entries = this.entries.slice(this.offset, this.offset + maxEntries);
    this.offset += entries.length;
    return ok({ entries, done: this.offset >= this.entries.length });
  }

  async close() {
    this.closed = true;
    this.entries = [];
  }
}

/** 完整 FileSystem：任何 super.read* / fileInfo 都不会接触密文字节。 */
export class ProtectedFileSystem extends NodeExecutionEnv {
  constructor({ cwd, files = plaintextFiles }) {
    super({ cwd });
    this.files = files;
    this.id = `gongwen-protected:${path.resolve(cwd)}`;
  }

  async absolutePath(target, context) {
    const resolved = await super.absolutePath(target, context);
    if (!resolved.ok) return resolved;
    try {
      this.files.assertPath(resolved.value);
      return resolved;
    } catch (error) {
      return err(fileError(error, resolved.value));
    }
  }

  async joinPath(parts, context) {
    const joined = await super.joinPath(parts, context);
    if (!joined.ok) return joined;
    try {
      this.files.assertPath(joined.value);
      return joined;
    } catch (error) {
      return err(fileError(error, joined.value));
    }
  }

  async readTextFile(target, context) {
    const stop = aborted(context, target);
    if (stop) return stop;
    try {
      return ok(this.files.readText(target));
    } catch (error) {
      return err(fileError(error, target));
    }
  }

  async readBinaryFile(target, context) {
    const stop = aborted(context, target);
    if (stop) return stop;
    try {
      return ok(this.files.readBytes(target));
    } catch (error) {
      return err(fileError(error, target));
    }
  }

  async openTextLineReader(target, context) {
    const read = await this.readBinaryFile(target, context);
    return read.ok ? ok(new MemoryTextLineReader(read.value, target)) : read;
  }

  async readTextLines(target, options, context) {
    if (options?.maxLines !== undefined && options.maxLines <= 0) return ok([]);
    const opened = await this.openTextLineReader(target, context);
    if (!opened.ok) return opened;
    const lines = [];
    try {
      while (options?.maxLines === undefined || lines.length < options.maxLines) {
        const line = await opened.value.readLine(context);
        if (!line.ok) return line;
        if (line.value === undefined) break;
        lines.push(line.value.text);
      }
      return ok(lines);
    } finally {
      await opened.value.close(context);
    }
  }

  async openBinaryReader(target, options, context) {
    if (options?.noFollow === true && !this.files.isVirtual(target)) {
      try {
        if (fs.lstatSync(target).isSymbolicLink()) {
          return err(new FileError("invalid", "Refusing to follow a symbolic link", target));
        }
      } catch (error) {
        return err(fileError(error, target));
      }
    }
    const read = await this.readBinaryFile(target, context);
    if (!read.ok) return read;
    const info = await this.fileInfo(target, context);
    return info.ok ? ok(new MemoryBinaryReader(read.value, info.value)) : info;
  }

  async writeFile(target, content, context) {
    const stop = aborted(context, target);
    if (stop) return stop;
    try {
      this.files.atomicWrite(target, content);
      return ok(undefined);
    } catch (error) {
      return err(fileError(error, target));
    }
  }

  async appendFile(target, content, context) {
    const stop = aborted(context, target);
    if (stop) return stop;
    try {
      this.files.appendFile(target, content);
      return ok(undefined);
    } catch (error) {
      return err(fileError(error, target));
    }
  }

  async truncateFile(target, size, context) {
    const stop = aborted(context, target);
    if (stop) return stop;
    try {
      this.files.truncateFile(target, size);
      return ok(undefined);
    } catch (error) {
      return err(fileError(error, target));
    }
  }

  async flushFile(target, context) {
    const stop = aborted(context, target);
    if (stop) return stop;
    try {
      this.files.flushFile(target);
      return ok(undefined);
    } catch (error) {
      return err(fileError(error, target));
    }
  }

  async renameFile(source, destination, context) {
    const stop = aborted(context, destination);
    if (stop) return stop;
    try {
      this.files.renameFile(source, destination);
      return ok(undefined);
    } catch (error) {
      return err(fileError(error, source));
    }
  }

  async fileInfo(target, context) {
    const stop = aborted(context, target);
    if (stop) return stop;
    try {
      return ok(this.files.info(target));
    } catch (error) {
      return err(fileError(error, target));
    }
  }

  async listDir(target, context) {
    const stop = aborted(context, target);
    if (stop) return stop;
    try {
      return ok(this.files.listDir(target));
    } catch (error) {
      return err(fileError(error, target));
    }
  }

  async openDirReader(target, context) {
    const listed = await this.listDir(target, context);
    return listed.ok ? ok(new MemoryDirReader(listed.value)) : listed;
  }

  async canonicalPath(target, context) {
    if (this.files.isVirtual(target)) { try { this.files.info(target); return ok(path.resolve(target)); } catch (error) { return err(fileError(error, target)); } }
    const resolved = await super.canonicalPath(target, context);
    if (!resolved.ok) return resolved;
    try {
      this.files.assertPath(resolved.value);
      return resolved;
    } catch (error) {
      return err(fileError(error, resolved.value));
    }
  }

  async exists(target, context) {
    const info = await this.fileInfo(target, context);
    if (info.ok) return ok(true);
    return info.error.code === "not_found" ? ok(false) : info;
  }

  async createDir(target, options, context) {
    try {
      this.files.assertPath(target);
    } catch (error) {
      return err(fileError(error, target));
    }
    try { this.files.createDir(target, { recursive: options?.recursive === true }); return ok(undefined); } catch (error) { return err(fileError(error, target)); }
  }

  async remove(target, options, context) {
    try {
      this.files.assertPath(target);
    } catch (error) {
      return err(fileError(error, target));
    }
    try { this.files.removePath(target, options); return ok(undefined); } catch (error) { return err(fileError(error, target)); }
  }

  async createTempDir(_prefix, _context) {
    return err(new FileError("not_supported", "受保护存储不使用系统临时目录"));
  }

  async createTempFile(_options, _context) {
    return err(new FileError("not_supported", "受保护存储不使用系统临时文件"));
  }

  async watch(_targets, _onChange, _context) {
    return err(new FileError("not_supported", "受保护存储不提供文件监听"));
  }

  async exec(_command, _options, _context) {
    return err(new ExecutionError("shell_unavailable", "受保护存储不提供命令执行"));
  }
}
