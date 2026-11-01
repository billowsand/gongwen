//! 无公网验收专用 preload：在导入 host 之前拦截一切对外连接。
//!
//! 只允许 127.0.0.1 / ::1 / localhost；任何其它 net / tls / dns / fetch 目标都写进
//! `GONGWEN_PI_OFFLINE_AUDIT`（JSONL）并抛错。它是验收夹具，不是生产权限开关：
//! 只在 `node --import scripts/pi-offline-guard.mjs` 时生效，应用运行时从不加载。

import fs from "node:fs";
import net from "node:net";
import tls from "node:tls";
import dns from "node:dns";
import dnsPromises from "node:dns/promises";
import { syncBuiltinESMExports } from "node:module";

const auditPath = process.env.GONGWEN_PI_OFFLINE_AUDIT;
const allowed = new Set(["127.0.0.1", "::1", "0:0:0:0:0:0:0:1", "localhost"]);

function record(entry) {
  if (!auditPath) return;
  try {
    fs.appendFileSync(auditPath, `${JSON.stringify({ ...entry, at: Date.now() })}\n`);
  } catch {
    // 审计文件写不进去时仍然拦截；验收脚本会因为缺少审计记录而失败。
  }
}

function normalize(host) {
  return String(host ?? "")
    .trim()
    .replace(/^\[/, "")
    .replace(/\]$/, "")
    .toLowerCase();
}

function check(kind, host) {
  const target = normalize(host);
  if (!allowed.has(target)) {
    record({ kind, host: target, denied: true });
    throw new Error(`无公网验收：拦截 ${kind} 到 ${target || "(空目标)"}`);
  }
  record({ kind, host: target, denied: false });
}

function replace(target, name, factory) {
  const original = target[name];
  if (typeof original !== "function") return;
  try {
    target[name] = factory(original);
  } catch {
    // 某些内建属性不可写；验收脚本会检查审计记录是否真的覆盖到连接。
  }
}

record({ kind: "guard", host: "started", denied: false });

replace(net, "connect", (original) => function wrappedNetConnect(...args) {
  let host;
  if (args[0] && typeof args[0] === "object") host = args[0].host ?? args[0].hostname;
  else host = args[1];
  check("net.connect", host ?? "127.0.0.1");
  return original.apply(this, args);
});
replace(net, "createConnection", (original) => function wrappedCreateConnection(...args) {
  let host;
  if (args[0] && typeof args[0] === "object") host = args[0].host ?? args[0].hostname;
  else host = args[1];
  check("net.createConnection", host ?? "127.0.0.1");
  return original.apply(this, args);
});
replace(net.Socket.prototype, "connect", (original) => function wrappedSocketConnect(...args) {
  const first = Array.isArray(args[0]) ? args[0][0] : args[0];
  const host = first && typeof first === "object" ? first.host ?? first.hostname : args[1];
  check("Socket.connect", host ?? "127.0.0.1");
  return original.apply(this, args);
});

replace(tls, "connect", (original) => function wrappedTlsConnect(...args) {
  const options = args[0] && typeof args[0] === "object" ? args[0] : {};
  check("tls.connect", options.host ?? options.servername ?? "127.0.0.1");
  return original.apply(this, args);
});

replace(dns, "lookup", (original) => function wrappedLookup(hostname, ...rest) {
  check("dns.lookup", hostname);
  return original.call(this, hostname, ...rest);
});
for (const name of ["resolve", "resolve4", "resolve6", "resolveAny", "resolveCname", "resolveMx", "resolveTxt"]) {
  replace(dns, name, (original) => function wrappedResolve(hostname, ...rest) {
    check(`dns.${name}`, hostname);
    return original.call(this, hostname, ...rest);
  });
}
for (const name of ["lookup", "resolve", "resolve4", "resolve6", "resolveAny"]) {
  replace(dnsPromises, name, (original) => function wrappedResolve(hostname, ...rest) {
    check(`dns.promises.${name}`, hostname);
    return original.call(this, hostname, ...rest);
  });
}

const originalFetch = globalThis.fetch;
if (typeof originalFetch === "function") {
  globalThis.fetch = function wrappedFetch(input, init) {
    let hostname = "";
    try {
      hostname = new URL(typeof input === "string" ? input : input?.url ?? String(input)).hostname;
    } catch {
      hostname = "";
    }
    check("fetch", hostname);
    return originalFetch.call(this, input, init);
  };
}
// 命名导入也必须看到包装后的实现，避免 import { connect } 绕过验收守卫。
syncBuiltinESMExports();
