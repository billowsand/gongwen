//! 同一轮 Pi 工作区的独占所有权。
//!
//! `host-owner.lock` 只是不含业务数据的锁载体：SQLite 在独立文件上持有
//! `BEGIN EXCLUSIVE`，Windows / Unix 都由 OS 在进程死亡时释放文件锁。
//! Durable 会话另存 JSONL，不在这个锁文件里写 runId、pid、正文或任何业务字段。

import { DatabaseSync } from "node:sqlite";

export class OwnerError extends Error {
  constructor(message, code = "owner_busy") {
    super(message);
    this.name = "OwnerError";
    this.code = code;
  }
}

/** 取得运行目录所有权；另一个 host 持有锁时立即失败，不等待。 */
export function acquireOwner(lockPath, { runId: _runId, pid: _pid } = {}) {
  let db;
  try {
    db = new DatabaseSync(lockPath);
  } catch (error) {
    throw new OwnerError(`打不开 Pi 工作区占用锁（${lockPath}）：${error.message}`, "owner_open");
  }
  try {
    db.exec("PRAGMA busy_timeout = 0");
    db.exec("BEGIN EXCLUSIVE");
    // 只建立无行的锁标记表，不写入运行标识或进程标识等业务内容。
    db.exec("CREATE TABLE IF NOT EXISTS lock_marker (id INTEGER PRIMARY KEY CHECK (id = 0))");
  } catch (error) {
    try {
      db.close();
    } catch {
      // 关闭失败不覆盖原始错误。
    }
    const message = String(error?.message ?? error);
    if (/SQLITE_BUSY|database is locked/i.test(message)) {
      throw new OwnerError(
        "另一个窗口正在用这一轮的 Pi 工作区。请等它结束或关掉那个窗口再试。",
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
