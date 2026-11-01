import { test } from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

// 本测试用本机 HTTP 桩模拟 OpenAI 兼容端点，让生产入口 src/main.mjs 走真实的
// Pi Durable + openai-completions 路径；不访问公网，也不使用 faux provider。

function sse(res, payload) {
  res.write(`data: ${JSON.stringify(payload)}\n\n`);
}

function completionChunk(choices, usage) {
  return {
    id: "chatcmpl-stub",
    object: "chat.completion.chunk",
    created: Math.floor(Date.now() / 1000),
    model: "stub-model",
    choices,
    ...(usage ? { usage } : {}),
  };
}

function toolCallResponse(res, name, args) {
  sse(
    res,
    completionChunk([
      {
        index: 0,
        delta: {
          role: "assistant",
          tool_calls: [
            {
              index: 0,
              id: "call-1",
              type: "function",
              function: { name, arguments: JSON.stringify(args) },
            },
          ],
        },
        finish_reason: null,
      },
    ]),
  );
  sse(
    res,
    completionChunk([{ index: 0, delta: {}, finish_reason: "tool_calls" }], {
      prompt_tokens: 10,
      completion_tokens: 5,
      total_tokens: 15,
    }),
  );
  res.write("data: [DONE]\n\n");
  res.end();
}

async function startStub() {
  const requests = [];
  const server = http.createServer((req, res) => {
    if (!req.url.endsWith("/chat/completions")) {
      res.writeHead(404).end();
      return;
    }
    let body = "";
    req.on("data", (chunk) => {
      body += chunk;
    });
    req.on("end", () => {
      const parsed = JSON.parse(body || "{}");
      requests.push(parsed);
      res.writeHead(200, {
        "content-type": "text/event-stream",
        "cache-control": "no-cache",
      });
      const hasToolResult = (parsed.messages ?? []).some((message) => message.role === "tool");
      const toolNames = (parsed.messages ?? []).flatMap((message) =>
        (message.tool_calls ?? []).map((call) => call.function?.name),
      );
      if (!hasToolResult) {
        toolCallResponse(res, "write", {
          path: "work/final.md",
          content: "# 关于测试的通知\n\n请各单位抓好落实。\n",
        });
      } else if (!toolNames.includes("submit_document")) {
        toolCallResponse(res, "submit_document", {
          path: "work/final.md",
          summary: "按测试要求写好通知",
        });
      } else {
        sse(
          res,
          completionChunk([{ index: 0, delta: { role: "assistant", content: "已提交。" }, finish_reason: null }]),
        );
        sse(
          res,
          completionChunk([{ index: 0, delta: {}, finish_reason: "stop" }], {
            prompt_tokens: 10,
            completion_tokens: 3,
            total_tokens: 13,
          }),
        );
        res.write("data: [DONE]\n\n");
        res.end();
      }
    });
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const { port } = server.address();
  return { server, requests, baseUrl: `http://127.0.0.1:${port}/v1` };
}

test("the production host path submits through a local OpenAI-compatible endpoint", async () => {
  const workspaceRoot = fs.mkdtempSync(path.join(os.tmpdir(), "gongwen-pi-e2e-"));
  fs.mkdirSync(path.join(workspaceRoot, "input"), { recursive: true });
  fs.mkdirSync(path.join(workspaceRoot, "work"), { recursive: true });
  fs.mkdirSync(path.join(workspaceRoot, "skills", "gongwen-markdown"), { recursive: true });
  fs.writeFileSync(path.join(workspaceRoot, "input", "current.md"), "# 当前\n");
  fs.writeFileSync(path.join(workspaceRoot, "input", "request.md"), "写一份通知。\n");
  const deliveryDir = path.join(workspaceRoot, "..", `${path.basename(workspaceRoot)}-run`);
  fs.mkdirSync(deliveryDir, { recursive: true });
  const { server, requests, baseUrl } = await startStub();
  const key = "sk-stub-secret-9876";
  try {
    const hostDir = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
    const child = spawn(process.execPath, [path.join(hostDir, "src", "main.mjs")], {
      cwd: hostDir,
      stdio: ["pipe", "pipe", "pipe"],
    });
    let out = "";
    let err = "";
    child.stdout.on("data", (chunk) => {
      out += chunk;
    });
    child.stderr.on("data", (chunk) => {
      err += chunk;
    });
    child.stdin.write(
      `${JSON.stringify({
        type: "init",
        protocol: 2,
        requestId: "req-e2e",
        runId: "run-e2e",
        workspace: {
          root: workspaceRoot,
          state: path.join(deliveryDir, "session.sqlite"),
          delivery: deliveryDir,
          current: "input/current.md",
          request: "input/request.md",
          skills: "skills/gongwen-markdown",
        },
        model: {
          baseUrl,
          apiKey: key,
          modelId: "stub-model",
          contextWindow: 8192,
          maxTokens: 1024,
          reasoning: false,
        },
        task: { request: "写一份通知。", kind: "通知", systemDate: "2026-11-01" },
      })}\n`,
    );
    const exitCode = await new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        child.kill();
        reject(new Error(`host 超时。stdout:\n${out}\nstderr:\n${err}`));
      }, 60_000);
      child.on("exit", (code) => {
        clearTimeout(timer);
        resolve(code);
      });
    });
    const events = out
      .trim()
      .split("\n")
      .filter(Boolean)
      .map((line) => JSON.parse(line));
    const submitted = events.find((event) => event.type === "submitted");
    assert.ok(submitted, `应当有 submitted 事件：${out}\n${err}`);
    assert.match(submitted.content, /抓好落实/);
    assert.equal(submitted.path, "work/final.md");
    assert.ok(events.some((event) => event.type === "completed" && event.submitted === true));
    assert.equal(exitCode, 0);
    assert.ok(requests.length >= 2);
    assert.ok(!out.includes(key) && !err.includes(key), "密钥不能出现在协议或诊断里");
  } finally {
    server.close();
  }
});
