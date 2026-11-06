//! 工作区文件沙箱。
//!
//! `cwd` 不是沙箱：模型可以写 `../..`、绝对路径或一个指向工作区外的符号链接 / junction。
//! 这里在每次文件调用前把路径解析成真实路径（realpath），再检查它是否仍在工作区内；
//! 读允许 input / work / skills / resources，写只允许 work。先不接受 `bash`，因此没有
//! 子进程可以绕过这层检查。

import fs from "node:fs";
import path from "node:path";
import { ExecutionError, FileError, err, ok } from "@earendil-works/pi-durable/env";
import { ProtectedFileSystem, isAtomicTempName, plaintextFiles } from "./encrypted-files.mjs";

export { atomicWrite } from "./encrypted-files.mjs";

const MAX_PATH_LENGTH = 4096;

function fileError(code, message, target) {
  return err(new FileError(code, message, target));
}

function within(root, target) {
  const relative = path.relative(root, target);
  return relative === "" || (!relative.startsWith("..") && !path.isAbsolute(relative));
}

/** 解析最近一个存在的祖先，再把不存在的尾部接回去。 */
function realpathOfNearest(target) {
  return resolveLinkTarget(target, 0);
}

function resolveLinkTarget(target, depth) {
  if (depth > 32) throw new Error("符号链接层数过多");
  let current = path.parse(target).root;
  const rest = target.slice(current.length).split(path.sep).filter(Boolean);
  for (let index = 0; index < rest.length; index += 1) {
    const next = path.join(current, rest[index]);
    let stat;
    try {
      stat = fs.lstatSync(next);
    } catch (error) {
      if (error?.code === "ENOENT" || error?.code === "ENOTDIR") {
        return path.join(current, ...rest.slice(index));
      }
      throw error;
    }
    if (stat.isSymbolicLink()) {
      const link = fs.readlinkSync(next);
      current = resolveLinkTarget(path.resolve(path.dirname(next), link), depth + 1);
    } else {
      // 用系统 realpath 归一化每一段：Windows 的 8.3 短名（RUNNER~1）与
      // macOS 的 /var → /private/var、大小写差异都会在这里被拉成同一种写法，
      // 否则后面的 within() 会把同一个目录当成两个。
      try {
        current = fs.realpathSync.native(next);
      } catch {
        current = next;
      }
    }
  }
  return current;
}

/**
 * 把用户/模型给的路径变成工作区内的真实路径。
 * `mode` 为 `"write"` 时只允许 `work/` 下（不含 `work` 目录本身）。
 */
export function resolveInside(root, cwd, input, { mode = "read", maxFileBytes = 8 * 1024 * 1024, files = plaintextFiles } = {}) {
  if (typeof input !== "string" || input.trim() === "") {
    return fileError("invalid", "路径不能为空");
  }
  if (input.length > MAX_PATH_LENGTH) {
    return fileError("invalid", "路径过长");
  }
  if (input.includes("\u0000")) {
    return fileError("invalid", "路径含 NUL");
  }
  const rootReal = fs.realpathSync.native(root);
  // cwd 先按最近的真实祖先解析：macOS 的 /var → /private/var、Windows 的
  // 8.3 短名都会让字面路径与 realpath 不等，直接比较会把合法路径误拒。
  const lexical = path.resolve(realpathOfNearest(path.resolve(cwd)), input);
  if (!within(rootReal, lexical)) {
    return fileError("permission_denied", `路径越出工作区：${input}`, input);
  }
  let real;
  try {
    real = realpathOfNearest(lexical);
  } catch (error) {
    return fileError(error?.code === "EACCES" ? "permission_denied" : "invalid", `路径无法解析：${error.message}`, input);
  }
  if (!within(rootReal, real)) {
    return fileError("permission_denied", `路径经符号链接指向工作区外：${input}`, input);
  }
  if (mode === "write") {
    const work = path.join(rootReal, "work");
    if (!within(work, real) || path.normalize(real) === path.normalize(work)) {
      return fileError("permission_denied", `只能写 work/ 下的文件：${input}`, input);
    }
  }
  if (maxFileBytes > 0) {
    try {
      const stat = files.info(real);
      if (stat.kind === "file" && stat.size > maxFileBytes) {
        return fileError("invalid", `文件超过上限（${stat.size} / ${maxFileBytes} 字节）：${input}`, input);
      }
    } catch (error) {
      if (error?.code !== "ENOENT") {
        return fileError("invalid", `无法读取文件状态：${error.message}`, input);
      }
    }
  }
  return ok(real);
}

export class WorkspaceGuard {
  constructor(root, files = plaintextFiles) {
    this.root = fs.realpathSync.native(root);
    this.work = path.join(this.root, "work");
    this.files = files;
  }

  read(input, cwd = this.root, maxFileBytes = 8 * 1024 * 1024) {
    return resolveInside(this.root, cwd, input, { mode: "read", maxFileBytes, files: this.files });
  }

  write(input, cwd = this.root, maxFileBytes = 8 * 1024 * 1024) {
    return resolveInside(this.root, cwd, input, { mode: "write", maxFileBytes, files: this.files });
  }

  /** 读取提交文件：必须是 work/ 下的普通文件，UTF-8，非空，限体积。 */
  readSubmission(input, { maxFileBytes = 8 * 1024 * 1024 } = {}) {
    const resolved = this.write(input, this.root, maxFileBytes);
    if (!resolved.ok) return resolved;
    let stat;
    try {
      stat = this.files.info(resolved.value);
    } catch (error) {
      return fileError(error?.code === "ENOENT" ? "not_found" : "invalid", `提交文件不存在：${error.message}`, input);
    }
    if (stat.kind !== "file") {
      return fileError("invalid", "提交路径不是普通文件", input);
    }
    if (stat.size === 0) {
      return fileError("invalid", "提交文件为空", input);
    }
    if (stat.size > maxFileBytes) {
      return fileError("invalid", `提交文件超过上限（${stat.size} / ${maxFileBytes} 字节）`, input);
    }
    let text;
    try {
      text = new TextDecoder("utf-8", { fatal: true }).decode(this.files.readBytes(resolved.value, maxFileBytes));
    } catch (error) {
      return fileError("invalid", `提交文件读不出来：${error.message}`, input);
    }
    if (text.trim() === "") {
      return fileError("invalid", "提交文件只有空白", input);
    }
    return ok({ absolute: resolved.value, relative: path.relative(this.root, resolved.value).replaceAll("\\", "/"), text });
  }

  /** work/ 里最新的 Markdown 候选：优先 work/final.md，其次最近修改的 .md。 */
  latestCandidate({ maxBytes = 2 * 1024 * 1024, maxFiles = 200 } = {}) {
    const candidates = [];
    const walk = (dir, depth) => {
      if (depth > 5 || candidates.length >= maxFiles) return;
      let entries;
      try {
        entries = fs.readdirSync(dir, { withFileTypes: true });
      } catch {
        return;
      }
      for (const entry of entries) {
        if (candidates.length >= maxFiles) return;
        if (entry.name.startsWith(".")) continue;
        const full = path.join(dir, entry.name);
        // 候选预览也要过路径守卫：work/ 里如果有人放了指向工作区外的链接，
        // 不能借“预览”把外部文件读进来。
        const checked = this.read(full, this.root, maxBytes);
        if (!checked.ok) continue;
        if (entry.isDirectory()) {
          walk(checked.value, depth + 1);
        } else if (entry.isFile() && /\.(md|markdown)$/i.test(entry.name)) {
          try {
            const stat = this.files.info(checked.value);
            if (stat.size > 0 && stat.size <= maxBytes) {
              candidates.push({ path: checked.value, mtimeMs: stat.mtimeMs, size: stat.size });
            }
          } catch {
            // 读不到就跳过；候选只是预览，不影响提交。
          }
        }
      }
    };
    walk(this.work, 0);
    if (candidates.length === 0) return undefined;
    const preferred = candidates.find((entry) => path.basename(entry.path).toLowerCase() === "final.md");
    const chosen = preferred ?? candidates.sort((a, b) => b.mtimeMs - a.mtimeMs)[0];
    try {
      const text = this.files.readText(chosen.path, maxBytes);
      if (text.trim() === "" || text.includes("\uFFFD")) return undefined;
      return { absolute: chosen.path, relative: path.relative(this.root, chosen.path).replaceAll("\\", "/"), text };
    } catch {
      return undefined;
    }
  }
}

/**
 * 文件工具实际使用的环境。只实现文件工具会调用的方法；其余能力明确报不支持，
 * 不留给调用方一个“看着能用”的空实现。
 */
export class WorkspaceEnv extends ProtectedFileSystem {
  constructor({ root, cwd, maxFileBytes = 8 * 1024 * 1024, maxTotalBytes = 64 * 1024 * 1024, maxWorkEntries = 1024, files = plaintextFiles }) {
    // macOS 的临时目录（/var → /private/var）和 Windows 的 8.3 短名会让
    // realpath(root) 与传入的 cwd 字面路径不一致，路径守卫会把正常工作区
    // 误判成“越出工作区”。root 与 cwd 都先真实化，再交给守卫比较。
    const realRoot = fs.realpathSync.native(root);
    const realCwd = cwd ? realpathOfNearest(path.resolve(cwd)) : realRoot;
    super({ cwd: realCwd, files });
    this.root = realRoot;
    this.maxFileBytes = maxFileBytes;
    this.maxTotalBytes = maxTotalBytes;
    this.maxWorkEntries = maxWorkEntries;
    this.guard = new WorkspaceGuard(this.root, files);
    this.id = `gongwen-pi:${this.root}`;
  }

  #read(p) {
    return this.guard.read(p, this.cwd, this.maxFileBytes);
  }

  #write(p) {
    return this.guard.write(p, this.cwd, this.maxFileBytes);
  }

  // 文件工具可以不断建立新文件；单文件上限不足以限制一轮写作的磁盘用量。
  #checkQuota(target, bytes) {
    const work = path.join(this.root, "work");
    const pending = [work];
    let total = 0;
    let entries = 0;
    while (pending.length) {
      for (const entry of fs.readdirSync(pending.pop(), { withFileTypes: true })) {
        // 崩溃残留的原子写临时文件可能解不开；它不是模型成果，跳过不参与配额。
        if (isAtomicTempName(entry.name)) continue;
        if (++entries > this.maxWorkEntries) throw new Error("工作区文件和目录数量超过上限");
        const full = path.join(entry.parentPath ?? entry.path, entry.name);
        if (entry.isDirectory()) pending.push(full);
        else if (entry.isFile()) total += this.files.info(full).size;
      }
    }
    let oldBytes = 0;
    if (fs.existsSync(target)) {
      const stat = this.files.info(target);
      if (stat.kind === "file") oldBytes = stat.size;
    }
    for (let current = target; current !== work; current = path.dirname(current)) {
      if (!within(work, current)) throw new Error("写入目标不在 work/ 下");
      if (!fs.existsSync(current)) entries += 1;
    }
    if (entries > this.maxWorkEntries) throw new Error("工作区文件和目录数量超过上限");
    if (total - oldBytes + bytes > this.maxTotalBytes) throw new Error("工作区总文件体积超过上限");
  }

  async absolutePath(p, _context) {
    return this.#read(p);
  }

  async joinPath(parts, _context) {
    return this.#read(path.join(...parts));
  }

  async readTextFile(p, context) {
    const resolved = this.#read(p);
    return resolved.ok ? super.readTextFile(resolved.value, context) : resolved;
  }

  async readTextLines(p, options, context) {
    const resolved = this.#read(p);
    return resolved.ok ? super.readTextLines(resolved.value, options, context) : resolved;
  }

  async openTextLineReader(p, context) {
    const resolved = this.#read(p);
    return resolved.ok ? super.openTextLineReader(resolved.value, context) : resolved;
  }

  async readBinaryFile(p, context) {
    const resolved = this.#read(p);
    return resolved.ok ? super.readBinaryFile(resolved.value, context) : resolved;
  }

  async openBinaryReader(p, options, context) {
    const resolved = this.#read(p);
    return resolved.ok ? super.openBinaryReader(resolved.value, options, context) : resolved;
  }

  async fileInfo(p, context) {
    const resolved = this.#read(p);
    return resolved.ok ? super.fileInfo(resolved.value, context) : resolved;
  }

  async listDir(p, context) {
    const resolved = this.#read(p);
    return resolved.ok ? super.listDir(resolved.value, context) : resolved;
  }

  async canonicalPath(p, _context) {
    return this.#read(p);
  }

  async exists(p, context) {
    const resolved = this.#read(p);
    return resolved.ok ? super.exists(resolved.value, context) : resolved;
  }

  async openDirReader(p, context) {
    const resolved = this.#read(p);
    return resolved.ok ? super.openDirReader(resolved.value, context) : resolved;
  }

  async writeFile(p, content, _context) {
    const resolved = this.#write(p);
    if (!resolved.ok) return resolved;
    const bytes = typeof content === "string" ? Buffer.byteLength(content, "utf8") : content.byteLength;
    if (bytes > this.maxFileBytes) {
      return fileError("invalid", `写入超过单文件上限（${bytes} / ${this.maxFileBytes} 字节）`, p);
    }
    try {
      this.#checkQuota(resolved.value, bytes);
      this.files.atomicWrite(resolved.value, content);
      return ok(undefined);
    } catch (error) {
      return fileError("unknown", `写入失败：${error.message}`, p);
    }
  }

  async appendFile(p, _content, _context) {
    return fileError("not_supported", "不支持追加写入；请用 write / edit 整体替换", p);
  }

  async truncateFile(p, _size, _context) {
    return fileError("not_supported", "不支持截断文件", p);
  }

  async flushFile(p, _context) {
    return fileError("not_supported", "原子写入已经落盘，不需要单独 flush", p);
  }

  async renameFile(source, destination, _context) {
    const from = this.#write(source);
    if (!from.ok) return from;
    const to = this.#write(destination);
    if (!to.ok) return to;
    try {
      this.files.renameFile(from.value, to.value);
      return ok(undefined);
    } catch (error) {
      return fileError("unknown", `移动文件失败：${error.message}`, source);
    }
  }

  async createDir(p, options, _context) {
    const resolved = this.#write(p);
    if (!resolved.ok) return resolved;
    try {
      fs.mkdirSync(resolved.value, { recursive: options?.recursive === true });
      return ok(undefined);
    } catch (error) {
      return fileError("unknown", `创建目录失败：${error.message}`, p);
    }
  }

  async remove(p, options, _context) {
    const resolved = this.#write(p);
    if (!resolved.ok) return resolved;
    try {
      fs.rmSync(resolved.value, { recursive: options?.recursive === true, force: options?.force === true });
      return ok(undefined);
    } catch (error) {
      return fileError("unknown", `删除失败：${error.message}`, p);
    }
  }

  async createTempDir(_prefix, _context) {
    return fileError("not_supported", "工作区不提供临时目录；请在 work/ 下建文件", this.cwd);
  }

  async createTempFile(_options, _context) {
    return fileError("not_supported", "工作区不提供临时文件；请在 work/ 下建文件", this.cwd);
  }

  async watch(_targets, _onChange, _context) {
    return fileError("not_supported", "不向模型提供文件监听", this.cwd);
  }

  async exec(_command, _options, _context) {
    return err(new ExecutionError("shell_unavailable", "本阶段不向模型提供命令执行"));
  }

  async cleanup(_context) {
    // 没有需要释放的长期资源。
  }
}
