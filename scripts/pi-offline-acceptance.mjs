//! 无公网验收：对真实组装好的随包运行时跑一轮「读技能 → 读材料 → 编辑文件 → 提交」，
//! 再验回执重放与 Durable JSONL 会话目录重开。用本地假 OpenAI 服务，不访问公网。
//!
//! 用法（用随包 Node 运行，不 import 源码）：
//!   <runtime>/pi/node.exe scripts/pi-offline-acceptance.mjs --runtime <runtime>/pi [--work <dir>]
//!
//! 脚本会：
//! - 只监听 127.0.0.1，并检查 host 子进程没有非回环的 ESTABLISHED 连接；
//! - 清空 HOME / USERPROFILE / Pi 配置与模型密钥环境变量，PATH 只留 Node 与系统目录；
//! - 从安装目录启动 `host/main.mjs`，不依赖开发 node_modules 或全局 Pi；
//! - 第一次运行校验 submitted / completed / 回执 / 快照 / Durable JSONL；
//! - 第二次用 `recovery: resume` 启动，断言不再调用模型、交回同一份成品。
//!
//! 退出码 0 表示全部通过；任何一步失败打印原因并以非 0 退出。干净目标机上的
//! 真实安装 / 升级仍需人工按 docs/pi-runtime.md 做，不能拿本脚本冒充。

import { spawn, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { pathToFileURL } from "node:url";

function parseArgs(argv) {
  const args = {};
  for (let index = 0; index < argv.length; index += 1) {
    const key = argv[index];
    if (key === "--runtime") args.runtime = argv[++index];
    else if (key === "--work") args.work = argv[++index];
    else if (key === "--keep") args.keep = true;
  }
  return args;
}

const args = parseArgs(process.argv.slice(2));
const scriptDir = path.dirname(path.resolve(process.argv[1]));
const runtimeDir = args.runtime ? path.resolve(args.runtime) : path.resolve("runtime", "pi");
const nodeName = process.platform === "win32" ? "node.exe" : path.join("bin", "node");
const nodePath = path.join(runtimeDir, nodeName);
const hostPath = path.join(runtimeDir, "host", "main.mjs");
const manifestPath = path.join(runtimeDir, "manifest.json");

function fail(message) {
  process.stderr.write(`FAIL: ${message}\n`);
  process.exit(1);
}

if (!fs.existsSync(nodePath)) fail(`缺少随包 Node：${nodePath}`);
if (!fs.existsSync(hostPath)) fail(`缺少随包 host：${hostPath}`);
const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
if (manifest.source !== "npm-ci") fail("运行时不是 npm ci 产物，拒绝验收");

// 当前系统的运行时平台名，与 packaging/pi-runtime.json 的 platforms 键一致。
// darwin 的 node 同样在 bin/node，运行路径无需特判。
function currentPlatform() {
  if (process.platform === "win32") return "win-x64";
  if (process.platform === "darwin") return process.arch === "arm64" ? "darwin-arm64" : "darwin-x64";
  return process.arch === "arm64" ? "linux-arm64" : "linux-x64";
}

if (manifest.platform !== currentPlatform()) {
  fail(`运行时平台是 ${manifest.platform}，与当前系统不符（需要 ${currentPlatform()}）`);
}

const work = args.work ? path.resolve(args.work) : fs.mkdtempSync(path.join(os.tmpdir(), "gongwen-pi-offline-"));
const workspaceRoot = path.join(work, "workspace");
const runDir = path.join(work, "run");
const home = path.join(work, "home");
for (const sub of ["input/materials", "work", "skills/gongwen-markdown", "skills/pi-skills/gongwen-draft"]) {
  fs.mkdirSync(path.join(workspaceRoot, sub), { recursive: true });
}
fs.mkdirSync(runDir, { recursive: true });
fs.mkdirSync(home, { recursive: true });
fs.writeFileSync(path.join(workspaceRoot, "input", "current.md"), "# 当前正文\n\n原有内容。\n");
fs.writeFileSync(
  path.join(workspaceRoot, "input", "request.md"),
  "按材料写一份简短通知。\n\n## 技能\n- gongwen-draft：按材料起草一份完整公文\n\n## 材料\n- input/materials/01.md：防火检查通知（fact）\n",
);
fs.writeFileSync(path.join(workspaceRoot, "input", "materials", "01.md"), "各单位应于十月开展一次防火检查。\n");
fs.writeFileSync(path.join(workspaceRoot, "skills", "gongwen-markdown", "SKILL.md"), "# 公文语法\n用一级标题写题名。\n");
fs.writeFileSync(
  path.join(workspaceRoot, "skills", "pi-skills", "gongwen-draft", "SKILL.md"),
  "# 起草\n先读材料，再写完整正文，最后 submit_document。\n",
);

const expected = "# 关于开展防火检查的通知\n\n各单位应于十月开展一次防火检查。\n";

// 分阶段离线验收用：一段带哈希的材料，让 host 走进分阶段调度。
const phasedExpected = "# 关于开展防火检查的通知\n\n各单位应于十月开展一次防火检查。\n";
const phasedMaterial = "各单位应于十月开展一次防火检查。\n";
const phasedMaterialSha = createHash("sha256").update(phasedMaterial, "utf8").digest("hex");
let phased = false;
let phasedCallIndex = 0;
const phasedPlan = [
  ["write", { path: "work/notes/01.md", content: "检查要求：十月开展一次防火检查。" }],
  ["record_progress", {
    phase: "distill",
    unit: "distill:input/materials/01.md#1",
    path: "work/notes/01.md",
    summary: "十月的防火检查要求",
    sources: [{ path: "input/materials/01.md", quote: "各单位应于十月开展一次防火检查" }],
  }],
  null,
  ["write", { path: "work/outline.md", content: "# 检查安排\n\n写清十月开展一次防火检查。\n" }],
  ["record_progress", {
    phase: "outline",
    unit: "outline",
    path: "work/outline.md",
    summary: "一节提纲",
    uses: ["distill:input/materials/01.md#1"],
    sections: ["检查安排"],
  }],
  null,
  ["write", { path: "work/sections/01.md", content: "各单位应于十月开展一次防火检查。\n" }],
  ["record_progress", {
    phase: "sections",
    unit: "section:1",
    path: "work/sections/01.md",
    summary: "第一节",
    uses: ["outline", "distill:input/materials/01.md#1"],
  }],
  null,
  ["write", { path: "work/final.md", content: phasedExpected }],
  ["record_progress", {
    phase: "review",
    unit: "final",
    path: "work/final.md",
    summary: "合稿完成",
    uses: ["outline", "section:1"],
  }],
  null,
  ["submit_document", { path: "work/final.md", summary: "按材料写完通知" }],
].filter(Boolean);

// 本地假 OpenAI：按步骤返回 read / read / write / submit_document。
let modelCalls = 0;
const requests = [];
const server = http.createServer((req, res) => {
  if (!req.url.endsWith("/chat/completions")) {
    res.writeHead(404).end();
    return;
  }
  let body = "";
  req.on("data", (part) => {
    body += part;
  });
  req.on("end", () => {
    const parsed = JSON.parse(body || "{}");
    requests.push(parsed);
    modelCalls += 1;
    const messages = parsed.messages ?? [];
    const toolResults = messages.filter((message) => message.role === "tool");
    const toolCalls = messages.flatMap((message) =>
      (message.tool_calls ?? []).map((call) => call.function?.name),
    );
    const step = phased ? phasedCallIndex++ : toolCalls.length;
    let name;
    let callArgs;
    if (phased) {
      const action = phasedPlan[step];
      if (action === undefined) {
        // 交付后模型只需结束，不应再发起工具调用。
        res.writeHead(200, { "content-type": "text/event-stream" });
        const text = { id: "chatcmpl-stub", object: "chat.completion.chunk", created: 0, model: "stub", choices: [{ index: 0, delta: { role: "assistant", content: "已提交。" }, finish_reason: "stop" }] };
        res.write(`data: ${JSON.stringify(text)}\n\ndata: [DONE]\n\n`);
        res.end();
        return;
      }
      if (action === null) {
        res.writeHead(200, { "content-type": "text/event-stream" });
        const text = { id: "chatcmpl-stub", object: "chat.completion.chunk", created: 0, model: "stub", choices: [{ index: 0, delta: { role: "assistant", content: "这一步完成。" }, finish_reason: "stop" }] };
        res.write(`data: ${JSON.stringify(text)}\n\ndata: [DONE]\n\n`);
        res.end();
        return;
      }
      [name, callArgs] = action;
    } else if (step === 0) {
      name = "read";
      callArgs = { path: "skills/pi-skills/gongwen-draft/SKILL.md" };
    } else if (step === 1) {
      name = "read";
      callArgs = { path: "input/materials/01.md" };
    } else if (step === 2) {
      name = "write";
      callArgs = { path: "work/final.md", content: expected };
    } else if (step === 3) {
      name = "submit_document";
      callArgs = { path: "work/final.md", summary: "按材料起草防火检查通知" };
    } else {
      res.writeHead(200, { "content-type": "text/event-stream" });
      const text = { id: "chatcmpl-stub", object: "chat.completion.chunk", created: 0, model: "stub", choices: [{ index: 0, delta: { role: "assistant", content: "已提交。" }, finish_reason: "stop" }] };
      res.write(`data: ${JSON.stringify(text)}\n\ndata: [DONE]\n\n`);
      res.end();
      return;
    }
    res.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-cache" });
    const delta = {
      index: 0,
      delta: {
        role: "assistant",
        tool_calls: [{ index: 0, id: `call-${step}`, type: "function", function: { name, arguments: JSON.stringify(callArgs) } }],
      },
      finish_reason: null,
    };
    res.write(`data: ${JSON.stringify({ id: "chatcmpl-stub", object: "chat.completion.chunk", created: 0, model: "stub", choices: [delta] })}\n\n`);
    const end = { index: 0, delta: {}, finish_reason: "tool_calls" };
    res.write(`data: ${JSON.stringify({ id: "chatcmpl-stub", object: "chat.completion.chunk", created: 0, model: "stub", choices: [end], usage: { prompt_tokens: 10, completion_tokens: 5, total_tokens: 15 } })}\n\n`);
    res.write("data: [DONE]\n\n");
    res.end();
  });
});

await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
const { port } = server.address();
const baseUrl = `http://127.0.0.1:${port}/v1`;

function initMessage(recovery, options = {}) {
  const rootDir = options.workspaceRoot ?? workspaceRoot;
  const run = options.runDir ?? runDir;
  return {
    type: "init",
    protocol: manifest.host.protocol,
    requestId: options.requestId ?? "offline-1",
    runId: options.runId ?? "offline-run-1",
    workspace: {
      root: rootDir,
      state: path.join(run, "session"),
      delivery: run,
      current: "input/current.md",
      request: "input/request.md",
      skills: "skills/gongwen-markdown",
      piSkills: "skills/pi-skills",
      materials: "input/materials",
    },
    model: {
      baseUrl,
      apiKey: "sk-offline-not-real",
      modelId: "offline-stub",
      contextWindow: options.contextWindow ?? 32768,
      maxTokens: 1024,
      reasoning: false,
    },
    strategy: options.strategy ?? { preference: "auto", capability: {} },
    task: {
      request: options.request ?? "按材料写一份简短通知。",
      kind: "通知",
      systemDate: "2026-11-01",
      skill: "gongwen-draft",
      ...(options.materials ? { materials: options.materials } : {}),
    },
    limits: { maxFileBytes: 8388608, maxCandidateBytes: 2097152, timeoutSeconds: 120, maxQuestionBatches: 2, maxQuestions: 3 },
    ...(recovery ? { recovery: { mode: "resume", requestId: options.requestId ?? "offline-1" } } : {}),
  };
}

const guardPath = pathToFileURL(path.join(scriptDir, "pi-offline-guard.mjs")).href;
const tempDir = path.join(work, "tmp");
fs.mkdirSync(tempDir, { recursive: true });
// 最小白名单环境：不继承用户的 NODE_PATH / NODE_OPTIONS / LD_LIBRARY_PATH / 模型凭据，
// 也不依赖用户缓存与全局 Pi。
const cleanEnv = {
  PATH: [path.dirname(nodePath), process.platform === "win32" ? "C:\\Windows\\System32" : "/usr/bin:/bin"].join(
    path.delimiter,
  ),
  HOME: home,
  USERPROFILE: home,
  XDG_CONFIG_HOME: path.join(home, ".config"),
  APPDATA: path.join(home, "AppData"),
  LOCALAPPDATA: path.join(home, "AppData", "Local"),
  TEMP: tempDir,
  TMP: tempDir,
  TMPDIR: tempDir,
  LANG: "C.UTF-8",
  LC_ALL: "C.UTF-8",
};
for (const key of ["SystemRoot", "windir", "COMSPEC", "PATHEXT"]) {
  if (process.env[key]) cleanEnv[key] = process.env[key];
}

function runHost(init) {
  return new Promise((resolve, reject) => {
    const auditPath = path.join(work, `offline-audit-${Date.now()}-${Math.random().toString(16).slice(2)}.jsonl`);
    const child = spawn(nodePath, ["--import", guardPath, "--disable-warning=ExperimentalWarning", hostPath], {
      cwd: runtimeDir,
      env: { ...cleanEnv, GONGWEN_PI_OFFLINE_AUDIT: auditPath },
      stdio: ["pipe", "pipe", "pipe"],
    });
    let out = "";
    let err = "";
    let publicConnection;
    let auditError;
    const audit = setInterval(() => {
      try {
        const found = nonLoopbackConnections(child.pid);
        if (found.length > 0) publicConnection = found;
      } catch (error) {
        auditError = error;
      }
    }, 250);
    child.stdout.on("data", (data) => {
      out += data;
    });
    child.stderr.on("data", (data) => {
      err += data;
    });
    const timer = setTimeout(() => {
      child.kill("SIGKILL");
      reject(new Error(`host 超时\nstdout:\n${out}\nstderr:\n${err}`));
    }, 120_000);
    child.on("exit", (code) => {
      clearInterval(audit);
      clearTimeout(timer);
      if (auditError) {
        reject(auditError);
        return;
      }
      let auditLines = [];
      try {
        auditLines = fs
          .readFileSync(auditPath, "utf8")
          .split("\n")
          .filter((line) => line.trim() !== "")
          .map((line) => JSON.parse(line));
      } catch {
        auditLines = [];
      }
      if (!auditLines.some((line) => line.kind === "guard" && line.host === "started")) {
        reject(new Error("无公网验收的 preload 守卫没有生效（缺 guard 启动记录）"));
        return;
      }
      const denied = auditLines.filter((line) => line.denied);
      if (denied.length > 0) {
        reject(
          new Error(
            `host 尝试访问非回环目标：${denied.map((line) => `${line.kind}:${line.host}`).join(", ")}`,
          ),
        );
        return;
      }
      if (publicConnection) {
        reject(new Error(`host 出现非回环连接：${publicConnection.join(", ")}`));
        return;
      }
      const lines = out
        .split("\n")
        .filter((line) => line.trim() !== "")
        .map((line) => {
          try {
            return JSON.parse(line);
          } catch {
            return { type: "unparsed", line };
          }
        });
      resolve({ code, lines, err });
    });
    child.stdin.write(`${JSON.stringify(init)}\n`);
  });
}

// 旁路观察 host 子进程已建立的非回环 TCP 连接。注意：这只是观察，不能单独证明
// host「没有访问公网的企图」——真正的拦截由 pi-offline-guard.mjs 的 preload 守卫做，
// 这里补的是「有没有已经连出去」。
function nonLoopbackConnections(pid) {
  if (!pid) return [];
  const found = [];
  let output;
  if (process.platform === "win32") {
    const result = spawnSync("netstat", ["-ano", "-p", "TCP"], { encoding: "utf8" });
    if (result.error || result.status !== 0) {
      throw new Error("无法检查 host 的网络连接（缺 netstat），不能声称无公网访问");
    }
    output = String(result.stdout ?? "");
    for (const line of output.split("\n")) {
      if (!line.includes("ESTABLISHED") && !line.includes("ESTAB")) continue;
      const parts = line.trim().split(/\s+/);
      if (parts.length < 5 || parts[4] !== String(pid)) continue;
      const remote = parts[2] ?? "";
      if (!/^(127\.0\.0\.1|\[::1\]|::1):/.test(remote)) found.push(remote);
    }
    return found;
  }
  if (process.platform === "darwin") {
    // macOS 没有 ss：用 lsof 按 pid 过滤已建立的 TCP 连接。
    const result = spawnSync(
      "lsof",
      ["-nP", "-iTCP", "-sTCP:ESTABLISHED", "-a", "-p", String(pid)],
      { encoding: "utf8" },
    );
    if (result.error) {
      throw new Error("无法检查 host 的网络连接（缺 lsof），不能声称无公网访问");
    }
    // 没有匹配项、或进程在两次采样之间退出时 lsof 返回 1；这不是工具缺失。
    if (result.status !== 0 && result.status !== 1) {
      throw new Error(
        `lsof 检查 host 的网络连接失败（退出码 ${result.status}）：${String(result.stderr ?? "").trim()}`,
      );
    }
    output = String(result.stdout ?? "");
    for (const line of output.split("\n")) {
      // 形如 `TCP 127.0.0.1:53211->127.0.0.1:8080 (ESTABLISHED)` 或
      // `TCP [::1]:53211->[::1]:8080 (ESTABLISHED)`；取 -> 右侧的远端地址。
      const match = line.match(/(\S+)->(\S+)/);
      if (!match) continue;
      const remote = match[2];
      if (!/^(127\.0\.0\.1|\[::1\]|::1):/.test(remote)) found.push(remote);
    }
    return found;
  }
  const result = spawnSync("ss", ["-tnp"], { encoding: "utf8" });
  if (result.error || result.status !== 0) {
    throw new Error("无法检查 host 的网络连接（缺 ss），不能声称无公网访问");
  }
  output = String(result.stdout ?? "");
  for (const line of output.split("\n")) {
    if (!line.includes("ESTABLISHED") && !line.includes("ESTAB")) continue;
    if (!line.includes(`pid=${pid},`)) continue;
    const parts = line.trim().split(/\s+/);
    const remote = parts[4] ?? "";
    if (!/^(127\.0\.0\.1|\[::1\]|::1):/.test(remote)) found.push(remote);
  }
  return found;
}

function assertDelivery(result, label, want = expected) {
  if (result.code !== 0) throw new Error(`${label}退出码 ${result.code}\nstderr:\n${result.err}`);
  const submitted = result.lines.find((line) => line.type === "submitted");
  const completed = result.lines.find((line) => line.type === "completed");
  if (!submitted) throw new Error(`${label}没有 submitted 事件\n${JSON.stringify(result.lines)}`);
  if (!completed || completed.submitted !== true) throw new Error(`${label}没有 completed(submitted)`);
  if (submitted.content !== want) throw new Error(`${label}交付内容不符：${submitted.content}`);
  return { submitted, completed };
}

let ok = false;
try {
  const first = await runHost(initMessage(false));
  const delivered = assertDelivery(first, "首次运行");
  if (modelCalls !== 4) throw new Error(`模型调用次数异常：${modelCalls}`);
  const receipt = JSON.parse(fs.readFileSync(path.join(runDir, "receipt.json"), "utf8"));
  if (receipt.runId !== "offline-run-1" || receipt.kind !== "document") throw new Error("回执绑定不对");
  if (createHash("sha256").update(expected, "utf8").digest("hex") !== receipt.sha256) {
    throw new Error("回执哈希与快照不符");
  }
  const stateDir = path.join(runDir, "session");
  if (!fs.statSync(stateDir).isDirectory() || fs.readdirSync(stateDir).length === 0) {
    throw new Error("Durable JSONL 没有落盘");
  }
  if (delivered.submitted.sha256 !== receipt.sha256) throw new Error("submitted 与回执哈希不一致");

  const callsBefore = modelCalls;
  const second = await runHost(initMessage(true));
  assertDelivery(second, "回执重放");
  if (modelCalls !== callsBefore) throw new Error("恢复回执时不应再调用模型");
  const reopened = fs.readdirSync(stateDir);
  if (reopened.length === 0) throw new Error("重开后 Durable JSONL 目录为空");

  // 分阶段新流程：同一份 host、同一套交付权限，按材料提炼→提纲→分节→合稿→交付走一遍。
  phased = true;
  phasedCallIndex = 0;
  const phasedRoot = path.join(work, "phased-workspace");
  const phasedRun = path.join(work, "phased-run");
  for (const sub of ["input/materials", "work", "skills/gongwen-markdown", "skills/pi-skills/gongwen-draft"]) {
    fs.mkdirSync(path.join(phasedRoot, sub), { recursive: true });
  }
  fs.mkdirSync(phasedRun, { recursive: true });
  fs.mkdirSync(path.join(phasedRun, "tmp"), { recursive: true });
  fs.writeFileSync(path.join(phasedRoot, "input", "current.md"), "# 当前正文\n\n原有内容。\n");
  fs.writeFileSync(path.join(phasedRoot, "input", "request.md"), "按材料写一份简短通知。\n");
  fs.writeFileSync(path.join(phasedRoot, "input", "materials", "01.md"), phasedMaterial);
  fs.writeFileSync(
    path.join(phasedRoot, "input", "materials", "index.json"),
    JSON.stringify([
      {
        file: "input/materials/01.md",
        title: "防火检查要求",
        source: "离线验收夹具",
        role: "fact",
        sha256: phasedMaterialSha,
        bytes: Buffer.byteLength(phasedMaterial, "utf8"),
      },
    ]),
  );
  fs.writeFileSync(path.join(phasedRoot, "skills", "gongwen-markdown", "SKILL.md"), "# 公文语法\n用一级标题写题名。\n");
  fs.writeFileSync(
    path.join(phasedRoot, "skills", "pi-skills", "gongwen-draft", "SKILL.md"),
    "# 起草\n按宿主每一步的说明做，中间用 record_progress 登记。\n",
  );
  const beforePhased = modelCalls;
  const phasedResult = await runHost(
    initMessage(false, {
      workspaceRoot: phasedRoot,
      runDir: phasedRun,
      requestId: "offline-phased-1",
      runId: "offline-phased-run-1",
      contextWindow: 32768,
      request: "按材料写一份简短通知，说明十月开展一次防火检查。\n" + "背景说明。".repeat(300),
      strategy: { preference: "phased", capability: {} },
      materials: [
        {
          file: "input/materials/01.md",
          title: "防火检查要求",
          source: "离线验收夹具",
          role: "fact",
          sha256: phasedMaterialSha,
          bytes: Buffer.byteLength(phasedMaterial, "utf8"),
        },
      ],
    }),
  );
  assertDelivery(phasedResult, "分阶段运行", phasedExpected);
  const phasedEvents = phasedResult.lines.map((line) => line.type);
  if (!phasedEvents.includes("strategy")) throw new Error("分阶段运行没有 strategy 事件");
  const progress = JSON.parse(fs.readFileSync(path.join(phasedRun, "progress.json"), "utf8"));
  const units = progress.records.map((record) => record.unit).sort();
  const wantUnits = ["distill:input/materials/01.md#1", "final", "outline", "section:1"].sort();
  if (JSON.stringify(units) !== JSON.stringify(wantUnits)) {
    throw new Error(`分阶段登记不完整：${JSON.stringify(units)}`);
  }
  if (modelCalls <= beforePhased) throw new Error("分阶段运行没有真实调用模型");

  ok = true;
  process.stdout.write(
    `PASS 无公网验收：Node ${manifest.node.version} · host ${manifest.host.version} · ${manifest.files.length} 文件 · 常规模型调用 ${callsBefore} 次 · 回执重放 0 次新调用 · 分阶段模型调用 ${modelCalls - beforePhased} 次\n` +
      "说明：工作区里的技能是验收专用简化夹具；真实技能包由应用内嵌资源与 cargo test skill_pack 覆盖，本脚本不冒充真实技能验收。\n",
  );
} catch (error) {
  process.stderr.write(`FAIL: ${error?.message ?? error}\n`);
} finally {
  server.close();
  if (!args.keep && !ok) {
    // 失败时保留现场方便排查。
  } else if (!args.keep) {
    fs.rmSync(work, { recursive: true, force: true });
  }
}
process.exit(ok ? 0 : 1);
