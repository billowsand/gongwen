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
  SEALED_FILE_OVERHEAD,
  openResource,
  resourceIdentity,
  sealGwa2,
} from "./encryption.mjs";

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
  if (maxBytes === undefined) return fs.readFileSync(target);
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

function infoFromStat(target, stat, size = stat.size) {
  const kind = stat.isFile() ? "file" : stat.isDirectory() ? "directory" : stat.isSymbolicLink() ? "symlink" : undefined;
  if (!kind) throw Object.assign(new Error("不支持的文件类型"), { code: "EINVAL" });
  return { name: path.basename(target), path: target, kind, size, mtimeMs: stat.mtimeMs };
}

export class ProtectedFiles {
  constructor(encryption = undefined) {
    this.encryption = encryption;
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
    this.configRoot = path.resolve(encryption.configRoot);
  }

  get encrypted() {
    return Boolean(this.encryption);
  }

  identity(target) {
    if (!this.encrypted) return undefined;
    return resourceIdentity(target, { slot: this.slot, configRoot: this.configRoot });
  }

  assertPath(target) {
    this.identity(target);
  }

  readBytes(target, maxBytes = undefined) {
    const identity = this.identity(target);
    const raw = readPhysical(target, maxBytes);
    if (!identity) {
      if (maxBytes !== undefined && raw.length > maxBytes) throw tooLarge(maxBytes);
      return raw;
    }
    const plain = openResource(this.key, identity.slot, identity.logicalId, raw);
    if (maxBytes !== undefined && plain.length > maxBytes) throw tooLarge(maxBytes);
    return plain;
  }

  readText(target, maxBytes = undefined) {
    return this.readBytes(target, maxBytes).toString("utf8");
  }

  atomicWrite(target, content) {
    const plain = asBytes(content);
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
    fs.unlinkSync(source);
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
    const stat = fs.lstatSync(target);
    return infoFromStat(target, stat, stat.isFile() ? this.readBytes(target).length : stat.size);
  }

  listDir(target) {
    this.assertPath(target);
    return fs.readdirSync(target, { withFileTypes: true }).map((entry) => this.info(path.join(target, entry.name)));
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
    if (options?.noFollow === true) {
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
    return super.createDir(target, options, context);
  }

  async remove(target, options, context) {
    try {
      this.files.assertPath(target);
    } catch (error) {
      return err(fileError(error, target));
    }
    return super.remove(target, options, context);
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
