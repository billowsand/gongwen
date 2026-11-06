//! 同一轮 Pi 工作区的独占所有权。
//!
//! `host-owner.lock` 只是不含业务数据的锁载体：SQLite 在独立文件上持有
//! `BEGIN EXCLUSIVE`，Windows / Unix 都由 OS 在进程死亡时释放文件锁。
//! Durable 会话另存 JSONL，不在这个锁文件里写 runId、pid、正文或任何业务字段。

import fs from "node:fs";
import path from "node:path";
import { DatabaseSync } from "node:sqlite";

/** 允许自愈的锁文件名：只有这几个文件名确定不含业务数据。 */
const HEALABLE_LOCK_NAMES = new Set(["host-owner.lock", "host-owner.sqlite"]);
/** SQLite 明确说「这不是数据库」时的错误文本。 */
const NOT_A_DATABASE = /file is not a database|not a database|malformed/i;
/** 加密资源的魔数前缀；锁文件被解锁迁移误封时是这两种之一。 */
const SEALED_PREFIX = "GWA";

export class OwnerError extends Error {
  constructor(message, code = "owner_busy") {
    super(message);
    this.name = "OwnerError";
    this.code = code;
  }
}

/** 只读 4 字节判断是不是被误封的密文锁文件。 */
function looksSealed(lockPath) {
  let fd;
  try {
    fd = fs.openSync(lockPath, "r");
    const head = Buffer.alloc(4);
    const count = fs.readSync(fd, head, 0, 4, 0);
    return head.subarray(0, count).toString("latin1").startsWith(SEALED_PREFIX);
  } catch {
    return false;
  } finally {
    if (fd !== undefined) {
      try {
        fs.closeSync(fd);
      } catch {
        // 关闭失败不影响判断。
      }
    }
  }
}

/**
 * 受控自愈：只在「文件名是已知锁载体」且「文件头是 GWA1/GWA2，或 SQLite 报
 * file is not a database」时删掉重建。锁文件没有业务数据，删掉不会丢东西；
 * 另一个 host 真持锁时拿到的是 SQLITE_BUSY，不会走到这里。
 */
function healLockFile(lockPath, message) {
  if (!HEALABLE_LOCK_NAMES.has(path.basename(lockPath))) return false;
  const damaged = looksSealed(lockPath) || NOT_A_DATABASE.test(String(message ?? ""));
  if (!damaged) return false;
  for (const suffix of ["", "-journal", "-wal", "-shm"]) {
    try {
      fs.unlinkSync(lockPath + suffix);
    } catch {
      // 不存在或删不掉都继续；主文件删不掉时下面回退成 owner_failed。
    }
  }
  return !fs.existsSync(lockPath);
}

/**
 * 取得运行目录所有权；另一个 host 持有锁时立即失败，不等待。
 * 锁文件被旧版迁移误封或损坏时自愈一次，再按正常流程抢锁。
 */
export function acquireOwner(lockPath, _options = {}) {
  return acquireWithHeal(lockPath, false);
}

function acquireWithHeal(lockPath, healed) {
  let db;
  try {
    db = new DatabaseSync(lockPath);
  } catch (error) {
    const message = String(error?.message ?? error);
    if (!healed && healLockFile(lockPath, message)) return acquireWithHeal(lockPath, true);
    throw new OwnerError(`打不开 Pi 工作区占用锁（${lockPath}）：${message}`, "owner_open");
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
    if (!healed && healLockFile(lockPath, message)) return acquireWithHeal(lockPath, true);
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
