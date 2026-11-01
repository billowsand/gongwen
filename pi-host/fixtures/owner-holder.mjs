//! 测试专用持锁进程：取到独占锁后写信号文件并一直活着，直到被杀。
//! 用法：`node fixtures/owner-holder.mjs <lockPath> <signalPath>`

import fs from "node:fs";
import { acquireOwner } from "../src/owner.mjs";

const [lockPath, signalPath] = process.argv.slice(2);
if (!lockPath || !signalPath) {
  process.stderr.write("用法：owner-holder.mjs <lockPath> <signalPath>\n");
  process.exit(2);
}
try {
  acquireOwner(lockPath, { runId: "holder", pid: process.pid });
} catch (error) {
  process.stderr.write(`${error?.code ?? "error"}:${error?.message ?? error}\n`);
  process.exit(3);
}
fs.writeFileSync(signalPath, String(process.pid));
// 持锁不放：由操作系统在进程死亡时释放。
setInterval(() => {}, 1_000);
