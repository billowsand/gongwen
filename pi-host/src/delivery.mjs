//! 交付回执与不可变成品快照。
//!
//! host 在调用 `submit_document` / `submit_style` / `submit_task` / `finish_report`
//! 时先把成品复制到运行目录（模型不可写），写 `final.md` / `final.json` 与
//! `receipt.json`，再向 Rust 发事件。这样进程在「快照落盘、
//! 事件发送、界面装上提案」任意点崩溃，重开时都能用回执交回同一份产物，不会重新生成，
//! 也不会从模型可写的 `work/` 重读。
//!
//! 回执体积有硬上限：清单最多 100 条，整份序列化后不得超过 [`RECEIPT_MAX_BYTES`]，
//! 超出的条目从尾部舍弃并标注 `omitted`，Rust 侧的读取上限比它大一倍。

import path from "node:path";
import { createHash } from "node:crypto";
import { plaintextFiles } from "./encrypted-files.mjs";

export const RECEIPT_VERSION = 1;
/** 回执整份上限（写的这一侧）。Rust 读的上限是它的两倍。 */
export const RECEIPT_MAX_BYTES = 1024 * 1024;
/** 清单最多几条。 */
export const MAX_REPORT_ITEMS = 100;
const TITLE_MAX = 2_000;
const DETAIL_MAX = 4_000;
const SEVERITY_MAX = 50;
const SUMMARY_MAX = 2_000;

/** 文件型交付（正文、风格、任务 JSON）共用快照校验；清单单独一条路径。 */
const FILE_KINDS = ["document", "style", "task"];

/** 文件型交付的不可变快照文件名。 */
function snapshotName(kind) {
  return kind === "document" ? "final.md" : "final.json";
}

export class DeliveryError extends Error {
  constructor(message, code = "delivery_failed") {
    super(message);
    this.name = "DeliveryError";
    this.code = code;
  }
}

function clamp(text, max) {
  const value = typeof text === "string" ? text : String(text ?? "");
  return value.length > max ? value.slice(0, max) : value;
}

function nowIso() {
  return new Date().toISOString();
}

function writeJson(target, value, files) {
  const bytes = Buffer.from(JSON.stringify(value), "utf8");
  if (bytes.length > RECEIPT_MAX_BYTES) {
    throw new DeliveryError("交付回执超过大小上限，已拒绝交付", "receipt_too_large");
  }
  files.atomicWrite(target, bytes);
}

/**
 * 写正文交付：`final.md` 是调用时的固定内容（不是之后文件的样子），`receipt.json`
 * 绑定 runId、requestId、路径、字节数、哈希、摘要与调用标识。
 */
export function writeDocumentReceipt(deliveryDir, { runId, requestId, relative, content, sha256, bytes, summary, callId, kind = "document", files = plaintextFiles }) {
  if (typeof deliveryDir !== "string" || deliveryDir === "") {
    throw new DeliveryError("缺少交付目录，已拒绝交付", "receipt_no_dir");
  }
  if (!["document", "style"].includes(kind)) throw new DeliveryError("不支持的文件产物类型", "receipt_kind");
  files.atomicWrite(path.join(deliveryDir, kind === "style" ? "final.json" : "final.md"), content);
  const receipt = {
    version: RECEIPT_VERSION,
    runId,
    requestId,
    kind,
    path: relative,
    sha256,
    bytes,
    summary: clamp(summary, SUMMARY_MAX),
    callId: clamp(callId, 200),
    items: [],
    at: nowIso(),
  };
  writeJson(path.join(deliveryDir, "receipt.json"), receipt, files);
  return receipt;
}

/**
 * 写任务助手交付：`final.json` 是调用时的任务 JSON（不是之后文件的样子），回执
 * `kind = task`，与正文交付共用 `submitted` 事件和恢复路径。任务 JSON 本身由
 * `task.mjs` 校验，这里只负责落盘与体积上限。
 */
export function writeTaskReceipt(deliveryDir, { runId, requestId, relative, content, sha256, bytes, summary, callId, files = plaintextFiles }) {
  if (typeof deliveryDir !== "string" || deliveryDir === "") {
    throw new DeliveryError("缺少交付目录，已拒绝交付", "receipt_no_dir");
  }
  files.atomicWrite(path.join(deliveryDir, "final.json"), content);
  const receipt = {
    version: RECEIPT_VERSION,
    runId,
    requestId,
    kind: "task",
    path: relative,
    sha256,
    bytes,
    summary: clamp(summary, SUMMARY_MAX),
    callId: clamp(callId, 200),
    items: [],
    at: nowIso(),
  };
  writeJson(path.join(deliveryDir, "receipt.json"), receipt, files);
  return receipt;
}

/**
 * 写清单回执。条目按顺序装到写上限为止，剩下的计入 `omitted`；顺序与模型交的一致，
 * 不静默改变标题。
 */
export function writeReportReceipt(deliveryDir, { runId, requestId, summary, items, callId, files = plaintextFiles }) {
  if (typeof deliveryDir !== "string" || deliveryDir === "") {
    throw new DeliveryError("缺少交付目录，已拒绝交付", "receipt_no_dir");
  }
  const source = Array.isArray(items) ? items.slice(0, MAX_REPORT_ITEMS) : [];
  const normalized = source.map((item) => ({
    title: clamp(item?.title, TITLE_MAX),
    detail: clamp(item?.detail, DETAIL_MAX),
    severity: clamp(item?.severity, SEVERITY_MAX),
  }));
  const receipt = {
    version: RECEIPT_VERSION,
    runId,
    requestId,
    kind: "report",
    path: "",
    sha256: "",
    bytes: 0,
    summary: clamp(summary, SUMMARY_MAX),
    callId: clamp(callId, 200),
    items: [],
    omitted: 0,
    at: nowIso(),
  };
  // 逐条累加，留出省略计数字段与结构开销的余量。
  const budget = RECEIPT_MAX_BYTES - 2_048;
  let used = Buffer.byteLength(JSON.stringify(receipt), "utf8");
  for (const item of normalized) {
    const size = Buffer.byteLength(JSON.stringify(item), "utf8") + 1;
    if (used + size > budget) {
      receipt.omitted = normalized.length - receipt.items.length;
      break;
    }
    receipt.items.push(item);
    used += size;
  }
  writeJson(path.join(deliveryDir, "receipt.json"), receipt, files);
  return receipt;
}

/** 校验读回的回执（恢复时先看它，有就直接交回，不再启动模型）。 */
export function readReceipt(deliveryDir, runId, files = plaintextFiles) {
  const file = path.join(deliveryDir, "receipt.json");
  let stat;
  try {
    stat = files.info(file);
  } catch (error) {
    if (error?.code === "ENOENT") return undefined;
    throw new DeliveryError(`读不到交付回执：${error.message}`, "receipt_unreadable");
  }
  if (stat.kind !== "file" || stat.size > RECEIPT_MAX_BYTES * 2) {
    throw new DeliveryError("交付回执超过大小上限，已拒绝", "receipt_too_large");
  }
  let receipt;
  try {
    receipt = JSON.parse(files.readText(file, RECEIPT_MAX_BYTES * 2));
  } catch (error) {
    throw new DeliveryError(`交付回执读不出来：${error.message}`, "receipt_corrupt");
  }
  if (receipt?.version !== RECEIPT_VERSION) {
    throw new DeliveryError("交付回执版本不认识，已拒绝", "receipt_version");
  }
  if (receipt.runId !== runId) {
    throw new DeliveryError("交付回执属于另一轮运行，已拒绝", "receipt_run_mismatch");
  }
  if (FILE_KINDS.includes(receipt.kind)) {
    const final = path.join(deliveryDir, snapshotName(receipt.kind));
    let text;
    try {
      const size = files.info(final).size;
      if (!Number.isSafeInteger(receipt.bytes) || receipt.bytes < 1 || size > 8 * 1024 * 1024) {
        throw new Error("成品快照超过读取上限或回执字节数不合法");
      }
      text = files.readText(final, 8 * 1024 * 1024);
    } catch (error) {
      throw new DeliveryError(`成品快照读不出来：${error.message}`, "receipt_snapshot");
    }
    if (Buffer.byteLength(text, "utf8") !== receipt.bytes) {
      throw new DeliveryError("成品快照的字节数与回执不一致，已拒绝", "receipt_snapshot");
    }
    if (createHash("sha256").update(text, "utf8").digest("hex") !== String(receipt.sha256).toLowerCase()) {
      throw new DeliveryError("成品快照的哈希与回执不一致，已拒绝", "receipt_snapshot");
    }
    return receipt;
  }
  if (receipt.kind !== "report") {
    throw new DeliveryError("交付回执的交付类型不认识，已拒绝", "receipt_kind");
  }
  return receipt;
}

/** 回执指向的不可变成品全文。 */
export function readReceiptDocument(deliveryDir, receipt, files = plaintextFiles) {
  if (!FILE_KINDS.includes(receipt?.kind)) {
    throw new DeliveryError("这份回执不是文件交付", "receipt_kind");
  }
  const file = path.join(deliveryDir, snapshotName(receipt.kind));
  if (files.info(file).size > 8 * 1024 * 1024) {
    throw new DeliveryError("成品快照超过读取上限", "receipt_snapshot");
  }
  return files.readText(file, 8 * 1024 * 1024);
}

/** 回执给 Rust 的事件（文件交付 / 清单两种形态）。 */
export function receiptEvents(deliveryDir, receipt, files = plaintextFiles) {
  if (FILE_KINDS.includes(receipt.kind)) {
    const content = readReceiptDocument(deliveryDir, receipt, files);
    return [
      {
        type: "submitted",
        path: receipt.path,
        summary: receipt.summary,
        content,
        sha256: receipt.sha256,
        bytes: receipt.bytes,
        recovered: true,
      },
      { type: "completed", submitted: true },
    ];
  }
  return [
    { type: "report", summary: receipt.summary, items: receipt.items, recovered: true },
    { type: "completed", submitted: false },
  ];
}
