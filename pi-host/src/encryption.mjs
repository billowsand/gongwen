//! GWA2 资源封套与路径身份。
//!
//! 布局与 Rust 完全一致：MAGIC ‖ 12 字节 nonce ‖ AES-256-GCM 密文 ‖ 16 字节 tag。
//! GWA2 的 AAD 绑定槽位、固定用途 pi 与去掉 alt- 后的配置根相对路径；GWA1 只读兼容。

import { createCipheriv, createDecipheriv, randomBytes } from "node:crypto";
import path from "node:path";

export const GWA1_MAGIC = Buffer.from("GWA1", "ascii");
export const GWA2_MAGIC = Buffer.from("GWA2", "ascii");
export const SEALED_FILE_OVERHEAD = 4 + 12 + 16;
const NONCE_BYTES = 12;
const TAG_BYTES = 16;

export class EncryptionError extends Error {
  constructor(message, code = "encryption_failed") {
    super(message);
    this.name = "EncryptionError";
    this.code = code;
  }
}

function keyBytes(key) {
  const bytes = Buffer.isBuffer(key) ? key : Buffer.from(key, "hex");
  if (bytes.length !== 32) throw new EncryptionError("资源密钥必须是 32 字节", "encryption_key");
  return bytes;
}

function slotNumber(slot) {
  if (slot === "real") return "0";
  if (slot === "decoy") return "1";
  throw new EncryptionError("资源槽位只支持 real / decoy", "encryption_slot");
}

/** 与 Rust resource_aad 的逐字节结果一致。 */
export function resourceAad(slot, logicalId, kind = "pi") {
  if (kind !== "pi") throw new EncryptionError("Pi host 的资源用途固定为 pi", "encryption_kind");
  if (typeof logicalId !== "string" || logicalId.length === 0 || Buffer.byteLength(logicalId, "utf8") > 4096 || logicalId.includes("\u0000")) {
    throw new EncryptionError("资源逻辑路径不合法", "encryption_path");
  }
  return Buffer.from(`gongwen-resource-v2\u0000${slotNumber(slot)}\u0000${kind}\u0000${logicalId}`, "utf8");
}

function outsideRoot(relative) {
  return relative === ".." || relative.startsWith(`..${path.sep}`) || path.isAbsolute(relative);
}

/** 相对某个根的逻辑组件；不在根内或含非法组件时返回 undefined。 */
function relativeParts(root, absolute) {
  const relative = path.relative(root, absolute);
  if (relative === "" || outsideRoot(relative)) return undefined;
  const parts = relative.split(path.sep);
  if (parts.some((part) => part === "" || part === "." || part === "..")) return undefined;
  return parts;
}

/**
 * 物理路径 → 槽位与稳定逻辑 ID。real 不得使用 alt- 首段，decoy 必须使用；
 * 首段的 alt- 只表示物理隔离，不进入逻辑 ID。
 *
 * 同一份配置目录可能以两种写法出现：配置根/目标的字面路径，以及
 * `realpath` 之后的路径（配置目录含符号链接、junction，或 macOS 把
 * `/var` 归一成 `/private/var`、大小写被系统归一）。Rust 侧用字面路径算
 * 逻辑 ID，Node 的工作区守卫会先把路径 realpath 一遍，因此这里按「字面根优先、
 * realpath 根兜底」两种口径各算一次，取第一个落在根内的结果；两种口径算出的
 * logicalId 必须逐字节相同（相对路径、alt- 首段剥离、正斜杠），否则解密会失败。
 * 越界与 `..` 仍然 fail-closed。
 */
export function resourceIdentity(target, { slot, configRoot, realConfigRoot } = {}) {
  if (typeof target !== "string" || !path.isAbsolute(target) || target.includes("\u0000")) {
    throw new EncryptionError("资源路径必须是绝对路径且不含 NUL", "encryption_path");
  }
  if (target.split(/[\\/]/).includes("..")) {
    throw new EncryptionError("资源路径不得包含 ..", "encryption_path");
  }
  if (typeof configRoot !== "string" || !path.isAbsolute(configRoot)) {
    throw new EncryptionError("encryption.configRoot 必须是绝对路径", "encryption_path");
  }
  if (realConfigRoot !== undefined && (typeof realConfigRoot !== "string" || !path.isAbsolute(realConfigRoot))) {
    throw new EncryptionError("encryption.realConfigRoot 必须是绝对路径", "encryption_path");
  }
  if (slot !== "real" && slot !== "decoy") {
    throw new EncryptionError("资源槽位只支持 real / decoy", "encryption_slot");
  }
  const absolute = path.resolve(target);
  const roots = [path.resolve(configRoot)];
  if (realConfigRoot !== undefined) {
    const resolved = path.resolve(realConfigRoot);
    if (!roots.includes(resolved)) roots.push(resolved);
  }
  for (const root of roots) {
    const parts = relativeParts(root, absolute);
    if (parts === undefined) continue;
    // 第一个命中的口径即最终口径：字面口径命中时不再看 realpath 口径，
    // 保证「Rust 用哪个路径写，Node 就用哪个路径算」。
    const decoy = parts[0].startsWith("alt-");
    if ((slot === "real" && decoy) || (slot === "decoy" && !decoy)) {
      throw new EncryptionError("资源路径的 alt- 前缀与当前槽位不一致", "encryption_slot_path");
    }
    if (decoy) {
      parts[0] = parts[0].slice(4);
      if (parts[0] === "") throw new EncryptionError("alt- 后必须有逻辑目录名", "encryption_path");
    }
    return { slot, logicalId: parts.join("/") };
  }
  throw new EncryptionError("资源路径必须位于 configRoot 内且不能是配置根本身", "encryption_path");
}

/** 生成 GWA2；nonce 可显式传入仅用于确定性测试，生产始终随机。 */
export function sealGwa2(key, slot, logicalId, plaintext, nonce = randomBytes(NONCE_BYTES)) {
  const plain = Buffer.isBuffer(plaintext) ? plaintext : Buffer.from(plaintext);
  const iv = Buffer.from(nonce);
  if (iv.length !== NONCE_BYTES) throw new EncryptionError("GWA2 nonce 必须是 12 字节", "encryption_nonce");
  const cipher = createCipheriv("aes-256-gcm", keyBytes(key), iv);
  cipher.setAAD(resourceAad(slot, logicalId));
  const encrypted = Buffer.concat([cipher.update(plain), cipher.final()]);
  return Buffer.concat([GWA2_MAGIC, iv, encrypted, cipher.getAuthTag()]);
}

/** GWA1 只读：没有 AAD；调用方仍须先验证资源路径与槽位。 */
export function sealGwa1ForTest(key, plaintext, nonce = randomBytes(NONCE_BYTES)) {
  const plain = Buffer.isBuffer(plaintext) ? plaintext : Buffer.from(plaintext);
  const iv = Buffer.from(nonce);
  if (iv.length !== NONCE_BYTES) throw new EncryptionError("GWA1 nonce 必须是 12 字节", "encryption_nonce");
  const cipher = createCipheriv("aes-256-gcm", keyBytes(key), iv);
  const encrypted = Buffer.concat([cipher.update(plain), cipher.final()]);
  return Buffer.concat([GWA1_MAGIC, iv, encrypted, cipher.getAuthTag()]);
}

function decrypt(key, sealed, aad) {
  const iv = sealed.subarray(4, 4 + NONCE_BYTES);
  const tag = sealed.subarray(sealed.length - TAG_BYTES);
  const encrypted = sealed.subarray(4 + NONCE_BYTES, sealed.length - TAG_BYTES);
  try {
    const decipher = createDecipheriv("aes-256-gcm", keyBytes(key), iv);
    if (aad.length > 0) decipher.setAAD(aad);
    decipher.setAuthTag(tag);
    return Buffer.concat([decipher.update(encrypted), decipher.final()]);
  } catch {
    throw new EncryptionError("文件解密失败（密钥、槽位、用途或路径不符，或内容损坏）", "encryption_auth");
  }
}

/** 明文原样返回；GWA1/GWA2 绝不会按 UTF-8 或 JSON 交给上层。 */
export function openResource(key, slot, logicalId, bytes) {
  const sealed = Buffer.isBuffer(bytes) ? bytes : Buffer.from(bytes);
  if (sealed.subarray(0, 4).equals(GWA1_MAGIC)) {
    if (sealed.length < SEALED_FILE_OVERHEAD) throw new EncryptionError("GWA1 文件长度不符", "encryption_corrupt");
    return decrypt(key, sealed, Buffer.alloc(0));
  }
  if (sealed.subarray(0, 4).equals(GWA2_MAGIC)) {
    if (sealed.length < SEALED_FILE_OVERHEAD) throw new EncryptionError("GWA2 文件长度不符", "encryption_corrupt");
    return decrypt(key, sealed, resourceAad(slot, logicalId));
  }
  return sealed;
}
