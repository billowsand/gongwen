//! JSONL 协议：一行一条 JSON，stdout 只走协议，stderr 只走诊断。
//
// 这里是 Rust 与 host 之间唯一的约定。协议版本不匹配、行太长、条数太多都直接失败，
// 不猜、不降级。模型密钥与资源 DEK 只出现在 init 输入里，绝不进入任何输出或日志。

import path from "node:path";

export const PROTOCOL_VERSION = 4;
export const HOST_VERSION = "0.4.1";

/** 单行输出的上限（含换行）。正文最多 8 MiB，JSON 转义后留出余量。 */
export const MAX_OUTPUT_LINE_BYTES = 16 * 1024 * 1024;
/** 整个进程最多发多少条消息，防止失控循环占满管道。 */
export const MAX_OUTPUT_MESSAGES = 50_000;
/** 输入 init 行的上限。Rust 侧正文与要求合计不超过 12 MiB。 */
export const MAX_INPUT_LINE_BYTES = 32 * 1024 * 1024;

/** 去掉控制字符并限长，用于进度、错误这类给人看的短文本。 */
export function sanitizeShort(text, max = 2_000) {
  const source = typeof text === "string" ? text : String(text ?? "");
  const cleaned = source
    .replace(/[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f]/g, " ")
    .replace(/\r\n?/g, "\n");
  return cleaned.length > max ? `${cleaned.slice(0, max)}…` : cleaned;
}

/** 把已知秘密从短文本里抹掉；正文内容不走这里。 */
export function scrubSecrets(text, secrets) {
  let out = typeof text === "string" ? text : String(text ?? "");
  for (const secret of secrets) {
    if (typeof secret === "string" && secret.length >= 4) {
      out = out.split(secret).join("[已隐藏]");
    }
  }
  return out;
}

export class ProtocolError extends Error {
  constructor(message) {
    super(message);
    this.name = "ProtocolError";
  }
}

/**
 * 建立一个只写 stdout 的发射器。`secrets` 里的字符串会从短文本字段里抹掉。
 * 返回的 `emit` 在超限时抛错，由 main 转成 error 事件。
 */
export function createEmitter({ write, secrets = [], maxLineBytes = MAX_OUTPUT_LINE_BYTES, maxMessages = MAX_OUTPUT_MESSAGES }) {
  let count = 0;
  return function emit(event) {
    if (count >= maxMessages) {
      throw new ProtocolError(`协议消息超过上限（${maxMessages} 条）`);
    }
    const safe = { ...event };
    for (const key of ["message", "error", "summary", "phase", "reason"]) {
      if (typeof safe[key] === "string") {
        safe[key] = scrubSecrets(sanitizeShort(safe[key]), secrets);
      }
    }
    const line = `${JSON.stringify(safe)}\n`;
    const bytes = Buffer.byteLength(line, "utf8");
    if (bytes > maxLineBytes) {
      throw new ProtocolError(`协议消息过大（${bytes} 字节，上限 ${maxLineBytes}）`);
    }
    count += 1;
    write(line);
  };
}

/** 校验 init 消息。返回规范化后的配置；不认识就抛 ProtocolError。 */
export function validateInit(value) {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new ProtocolError("init 必须是 JSON 对象");
  }
  if (value.type !== "init") {
    throw new ProtocolError("第一条消息必须是 init");
  }
  if (value.protocol !== PROTOCOL_VERSION) {
    throw new ProtocolError(`协议版本不匹配：host 是 ${PROTOCOL_VERSION}，收到 ${value.protocol}`);
  }
  const requestId = requireText(value.requestId, "requestId", 200);
  const runId = requireText(value.runId, "runId", 200);
  const workspace = value.workspace;
  if (!workspace || typeof workspace !== "object") {
    throw new ProtocolError("缺少 workspace");
  }
  const root = requireText(workspace.root, "workspace.root", 4096);
  if (!isAbsolutePath(root)) {
    throw new ProtocolError("workspace.root 必须是绝对路径");
  }
  const state = requireText(workspace.state, "workspace.state", 4096);
  if (!isAbsolutePath(state)) {
    throw new ProtocolError("workspace.state 必须是绝对路径");
  }
  const delivery = requireText(workspace.delivery, "workspace.delivery", 4096);
  if (!isAbsolutePath(delivery)) {
    throw new ProtocolError("workspace.delivery 必须是绝对路径");
  }
  if (isWithinPath(root, delivery)) {
    throw new ProtocolError("workspace.delivery 必须在模型可写的工作区之外");
  }
  const model = value.model;
  if (!model || typeof model !== "object") {
    throw new ProtocolError("缺少 model");
  }
  const baseUrl = requireText(model.baseUrl, "model.baseUrl", 4096);
  let parsed;
  try {
    parsed = new URL(baseUrl);
  } catch {
    throw new ProtocolError("model.baseUrl 不是合法 URL");
  }
  if (!/^https?:$/.test(parsed.protocol)) {
    throw new ProtocolError("model.baseUrl 只支持 http/https");
  }
  const apiKey = typeof model.apiKey === "string" ? model.apiKey : "";
  const modelId = requireText(model.modelId, "model.modelId", 400);
  const contextWindow = positiveInt(model.contextWindow, "model.contextWindow");
  const maxTokens = positiveInt(model.maxTokens, "model.maxTokens");
  if (maxTokens > contextWindow) {
    throw new ProtocolError("model.maxTokens 不能大于 contextWindow");
  }
  const reasoning = model.reasoning === true;
  const thinkingLevel = reasoning ? optionalThinkingLevel(model.thinkingLevel) : undefined;
  const task = value.task;
  if (!task || typeof task !== "object") {
    throw new ProtocolError("缺少 task");
  }
  const request = requireText(task.request, "task.request", 200_000);
  const kind = typeof task.kind === "string" ? sanitizeShort(task.kind, 100) : "";
  const systemDate = typeof task.systemDate === "string" ? sanitizeShort(task.systemDate, 100) : "";
  const limits = value.limits && typeof value.limits === "object" ? value.limits : {};
  const recovery = validateRecovery(value.recovery);
  return {
    protocol: PROTOCOL_VERSION,
    requestId,
    runId,
    recovery,
    workspace: {
      root,
      state,
      delivery,
      current: typeof workspace.current === "string" ? workspace.current : "input/current.md",
      request: typeof workspace.request === "string" ? workspace.request : "input/request.md",
      skills: typeof workspace.skills === "string" ? workspace.skills : "skills/gongwen-markdown",
      piSkills: typeof workspace.piSkills === "string" ? workspace.piSkills : "skills/pi-skills",
      materials: typeof workspace.materials === "string" ? workspace.materials : "input/materials",
      ...(workspace.encryption === undefined ? {} : { encryption: validateEncryption(workspace.encryption) }),
    },
    model: { baseUrl, apiKey, modelId, contextWindow, maxTokens, reasoning, thinkingLevel },
    strategy: validateStrategy(value.strategy),
    task: {
      request,
      kind,
      systemDate,
      writingContext: boundedWritingContext(task.writingContext),
      skill: optionalText(task.skill, "task.skill", 100),
      selection: optionalSelection(task.selection),
      materials: optionalMaterials(task.materials),
    },
    limits: {
      maxFileBytes: optionalLimit(limits.maxFileBytes, 8 * 1024 * 1024),
      maxCandidateBytes: optionalLimit(limits.maxCandidateBytes, 2 * 1024 * 1024),
      timeoutMs: optionalLimit(limits.timeoutSeconds, 900) * 1000,
      maxQuestionBatches: optionalLimit(limits.maxQuestionBatches, 2),
      maxQuestions: optionalLimit(limits.maxQuestions, 3),
    },
  };
}

const MATERIAL_ROLES = new Set(["fact", "style_sample", "structure_sample", "incoming_letter"]);

/** DEK 只从 stdin init 接收；规范化配置保留它供内存加密，不进入任何协议输出。 */
function validateEncryption(value) {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new ProtocolError("workspace.encryption 必须是对象");
  }
  if (typeof value.keyHex !== "string" || !/^[0-9a-fA-F]{64}$/.test(value.keyHex)) {
    throw new ProtocolError("workspace.encryption.keyHex 必须是 64 位十六进制");
  }
  if (value.slot !== "real" && value.slot !== "decoy") {
    throw new ProtocolError("workspace.encryption.slot 只支持 real / decoy");
  }
  const configRoot = requireText(value.configRoot, "workspace.encryption.configRoot", 4096);
  if (!isAbsolutePath(configRoot)) {
    throw new ProtocolError("workspace.encryption.configRoot 必须是绝对路径");
  }
  return { keyHex: value.keyHex, slot: value.slot, configRoot };
}

/**
 * 分阶段倾向与有限能力元数据。旧 host 不认识这个字段会忽略，新 host 缺省按 `auto`。
 * 能力只收已验证的值；没有就不猜，按保守工作预算处理。
 */
function validateStrategy(value) {
  const source = value && typeof value === "object" ? value : {};
  const preference = source.preference === "phased" ? "phased" : "auto";
  const capability = source.capability && typeof source.capability === "object" ? source.capability : {};
  const verifiedWorkBudget =
    Number.isInteger(capability.verifiedWorkBudget) && capability.verifiedWorkBudget > 0
      ? capability.verifiedWorkBudget
      : undefined;
  return {
    preference,
    capability: {
      ...(verifiedWorkBudget === undefined ? {} : { verifiedWorkBudget }),
      needsShortSteps: capability.needsShortSteps === true,
      reason: typeof capability.reason === "string" ? capability.reason.slice(0, 500) : "",
    },
  };
}

/** 恢复模式：只认 `resume`，旧 host 遇到新字段会直接拒绝而不是猜。 */
function boundedWritingContext(value) {
  if (value === undefined) return undefined;
  if (!value || typeof value !== "object" || Array.isArray(value) || Buffer.byteLength(JSON.stringify(value), "utf8") > 48 * 1024) {
    throw new ProtocolError("写作背景必须是 48 KiB 内的对象");
  }
  return value;
}

function validateRecovery(value) {
  if (value === undefined || value === null) return undefined;
  if (!value || typeof value !== "object") {
    throw new ProtocolError("recovery 必须是对象");
  }
  if (value.mode !== "resume") {
    throw new ProtocolError(`不认识的恢复模式：${String(value.mode)}`);
  }
  const result = { resume: true };
  if (typeof value.requestId === "string" && value.requestId !== "") result.requestId = value.requestId;
  if (value.continuationId !== undefined) {
    const id = requireText(value.continuationId, "recovery.continuationId", 128);
    if (!/^[A-Za-z0-9_-]+$/.test(id)) throw new ProtocolError("recovery.continuationId 格式不正确");
    result.continuationId = id;
  }
  return result;
}

function isWithinPath(root, target) {
  const relative = path.relative(root, target);
  return relative === "" || (!relative.startsWith("..") && !path.isAbsolute(relative));
}

function optionalText(value, name, max) {
  if (value === undefined || value === null) return undefined;
  if (typeof value !== "string" || value.trim() === "") return undefined;
  return sanitizeShort(value, max);
}

function optionalSelection(value) {
  if (!value || typeof value !== "object") return undefined;
  const start = Number(value.start);
  const end = Number(value.end);
  if (!Number.isInteger(start) || !Number.isInteger(end) || start < 0 || end <= start) return undefined;
  return { start, end, text: sanitizeShort(value.text ?? "", 200_000) };
}

/** 只收程序写的编号文件路径与已知角色；用户文字只放在标题 / 来源里。 */
function optionalMaterials(value) {
  if (!Array.isArray(value)) return [];
  return value.slice(0, 20).flatMap((item) => {
    if (!item || typeof item !== "object") return [];
    const file = typeof item.file === "string" ? item.file : "";
    if (!/^input\/materials\/\d{2}\.md$/.test(file)) return [];
    const role = MATERIAL_ROLES.has(item.role) ? item.role : "fact";
    return [
      {
        file,
        title: sanitizeShort(item.title ?? "", 200),
        source: sanitizeShort(item.source ?? "", 200),
        role,
        sha256: typeof item.sha256 === "string" ? item.sha256.slice(0, 64) : "",
        bytes: Number.isInteger(item.bytes) ? item.bytes : 0,
      },
    ];
  });
}

function optionalThinkingLevel(value) {
  const levels = new Set(["off", "minimal", "low", "medium", "high", "xhigh", "max"]);
  return levels.has(value) ? value : "high";
}

function requireText(value, name, max) {
  if (typeof value !== "string" || value.trim() === "") {
    throw new ProtocolError(`${name} 必须是非空字符串`);
  }
  if (value.length > max) {
    throw new ProtocolError(`${name} 过长`);
  }
  if (value.includes("\u0000")) {
    throw new ProtocolError(`${name} 含 NUL`);
  }
  return value;
}

function positiveInt(value, name) {
  if (!Number.isInteger(value) || value <= 0) {
    throw new ProtocolError(`${name} 必须是正整数`);
  }
  return value;
}

function optionalLimit(value, fallback) {
  return Number.isInteger(value) && value > 0 ? value : fallback;
}

function isAbsolutePath(path) {
  return path.startsWith("/") || /^[A-Za-z]:[\\/]/.test(path) || /^\\\\/.test(path);
}

/**
 * 从流里逐行读 JSON。超过 `maxLineBytes` 立即失败，不做截断解析。
 * 回调返回 false 时停止读。
 */
export function readJsonLines(stream, { maxLineBytes = MAX_INPUT_LINE_BYTES, onValue }) {
  return new Promise((resolve, reject) => {
    let buffer = "";
    let done = false;
    const finish = (fn, value) => {
      if (done) return;
      done = true;
      stream.off("data", onData);
      stream.off("end", onEnd);
      stream.off("error", onError);
      fn(value);
    };
    const handleLine = (line) => {
      if (line.trim() === "") return true;
      const bytes = Buffer.byteLength(line, "utf8");
      if (bytes > maxLineBytes) {
        finish(reject, new ProtocolError(`输入行过大（${bytes} 字节，上限 ${maxLineBytes}）`));
        return false;
      }
      let value;
      try {
        value = JSON.parse(line);
      } catch {
        // JSON.parse 的错误可能包含原文片段，而 init 原文含 API key。
        finish(reject, new ProtocolError("输入不是合法 JSON"));
        return false;
      }
      try {
        if (onValue(value) === false) {
          finish(resolve);
          return false;
        }
      } catch (error) {
        finish(reject, error);
        return false;
      }
      return true;
    };
    const onData = (chunk) => {
      buffer += chunk.toString("utf8");
      if (Buffer.byteLength(buffer, "utf8") > maxLineBytes && !buffer.includes("\n")) {
        finish(reject, new ProtocolError(`输入行超过上限 ${maxLineBytes} 字节`));
        return;
      }
      let index = buffer.indexOf("\n");
      while (index >= 0) {
        const line = buffer.slice(0, index);
        buffer = buffer.slice(index + 1);
        if (!handleLine(line)) return;
        index = buffer.indexOf("\n");
      }
    };
    const onEnd = () => {
      if (buffer.trim() !== "" && !handleLine(buffer)) return;
      finish(resolve);
    };
    const onError = (error) => finish(reject, error);
    stream.setEncoding("utf8");
    stream.on("data", onData);
    stream.on("end", onEnd);
    stream.on("error", onError);
  });
}
