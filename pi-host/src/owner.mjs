//! 同一轮 Pi 工作区的独占所有权。
//!
//! Durable 的 SQLite 同一时刻只能有一个写者；界面上的 busy 标记或一个不会回收的
//! `exists` 文件都不够：前一个进程崩溃后标记会一直留着，而后一个进程死了标记也不会
//! 自己消失。这里用 Node 内置 `node:sqlite` 在一个**独立的锁库**上开 `BEGIN EXCLUSIVE`
//! 事务，并一直持有到进程结束：SQLite 自己走 OS 文件锁（Windows 上 LockFileEx，
//! Unix 上 fcntl），进程一死由操作系统释放；第二个 host 拿到的是 SQLITE_BUSY。
//!
//! 锁库与 Durable 的 `session.sqlite` 分开：独占事务不会挡住 Durable 的写入。

import { DatabaseSync } from "node:sqlite";

export class OwnerError extends Error {
  constructor(message, code = "owner_busy") {
    super(message);
    this.name = "OwnerError";
    this.code = code;
  }
}

/**
 * 取得运行目录的所有权。成功返回 `{ release }`；另一个 host 持有锁时抛
 * [`OwnerError`]，消息可直接给用户看，不动数据库。
 */
export function acquireOwner(lockPath, { runId, pid = process.pid } = {}) {
  let db;
  try {
    db = new DatabaseSync(lockPath);
  } catch (error) {
    throw new OwnerError(`打不开 Pi 工作区占用锁（${lockPath}）：${error.message}`, "owner_open");
  }
  try {
    // 不等：另一个进程正在用就立刻失败，不把界面卡在这里。
    db.exec("PRAGMA busy_timeout = 0");
    db.exec("BEGIN EXCLUSIVE");
    db.exec(
      "CREATE TABLE IF NOT EXISTS owner (pid INTEGER NOT NULL, run_id TEXT NOT NULL, started_at TEXT NOT NULL)",
    );
    db.prepare("DELETE FROM owner").run();
    db.prepare("INSERT INTO owner (pid, run_id, started_at) VALUES (?, ?, ?)").run(
      pid,
      String(runId ?? ""),
      new Date().toISOString(),
    );
  } catch (error) {
    try {
      db.close();
    } catch {
      // 关闭失败不覆盖原始错误。
    }
    const message = String(error?.message ?? error);
    if (/SQLITE_BUSY|database is locked/i.test(message)) {
      throw new OwnerError(
        "另一个窗口正在用这一轮的 Pi 工作区（Durable 库已被占用）。请等它结束或关掉那个窗口再试。",
        "owner_busy",
      );
    }
    throw new OwnerError(`占用 Pi 工作区失败：${message}`, "owner_failed");
  }
  let released = false;
  return {
    release() {
      if (released) return;
      released = true;
      try {
        // 回滚就结束独占事务；进程异常退出时由 SQLite / OS 回收。
        db.exec("ROLLBACK");
      } catch {
        // 事务可能已被回滚。
      }
      try {
        db.close();
      } catch {
        // 关库失败不影响进程退出。
      }
    },
  };
}
