//! 单实例：程序已开着时，再双击 `.gwx`（或再点一次图标）不开第二个窗口，
//! 把要导入的稿件包转交给已开的窗口。
//!
//! 不开网络端口（本机端口可能触发防火墙提示）、不引入新依赖：主实例在配置目录
//! 持有 `instance.lock` 的独占锁，进程结束时系统自动释放，崩溃也不会留下残留锁；
//! 后起的进程拿不到锁，就把请求写进 `inbox/` 后退出，主实例的后台线程每 300 ms
//! 看一次收件目录。两个进程也就不会同时写同一个稿件库和标签记录。
//! 方案见 `docs/gwx-package-design.md` 第四节。

use anyhow::{Context, Result};
use eframe::egui;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 跳过单实例检查、照旧另开一个窗口的命令行参数。
pub const NEW_WINDOW_FLAG: &str = "--new-window";

const LOCK_FILE: &str = "instance.lock";
const INBOX_DIR: &str = "inbox";
const REQUEST_EXT: &str = "req";
const PARTIAL_EXT: &str = "tmp";
const POLL_INTERVAL: Duration = Duration::from_millis(300);
/// 收件目录里放了这么久还没被取走的请求（主实例当时卡死后被强杀之类），
/// 下次启动时直接丢掉，免得过了很久突然弹出口令框。
const STALE_AFTER: Duration = Duration::from_secs(10 * 60);
/// 一条请求只是几行路径，超过这个大小的不是本程序写的，不读。
const MAX_REQUEST_BYTES: u64 = 1024 * 1024;

/// 启动时争主实例的结果。
pub enum Startup {
    /// 自己就是主实例，照常建窗口。
    Primary(Instance),
    /// 已有实例在跑，请求已转交，本进程直接退出。
    Forwarded,
}

/// 主实例：持有锁文件直到进程结束。
pub struct Instance {
    _lock: File,
    inbox: PathBuf,
}

/// 在配置目录争主实例。锁或收件目录用不了（只读盘、网络盘不支持文件锁）时返回错误，
/// 调用方照旧开一个独立窗口。
pub fn start(imports: &[PathBuf]) -> Result<Startup> {
    start_in(&crate::storage::config_dir()?, imports)
}

fn start_in(dir: &Path, imports: &[PathBuf]) -> Result<Startup> {
    fs::create_dir_all(dir).with_context(|| format!("无法创建 {}", dir.display()))?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join(LOCK_FILE))
        .context("无法打开实例锁文件")?;
    let inbox = dir.join(INBOX_DIR);
    match lock.try_lock() {
        Ok(()) => {
            clear_stale(&inbox, SystemTime::now());
            Ok(Startup::Primary(Instance { _lock: lock, inbox }))
        }
        Err(fs::TryLockError::WouldBlock) => {
            forward(&inbox, imports)?;
            foreground::allow_any();
            Ok(Startup::Forwarded)
        }
        Err(fs::TryLockError::Error(error)) => Err(error).context("无法锁定实例锁文件"),
    }
}

impl Instance {
    /// 起后台线程盯着收件目录：每条请求是一组要导入的路径（空表示只要把窗口唤到前台），
    /// 送进返回的通道并让界面重绘一帧。通道收端被丢掉后线程自己结束。
    pub fn watch(&self, ctx: egui::Context) -> Receiver<Vec<PathBuf>> {
        let (tx, rx) = mpsc::channel();
        let inbox = self.inbox.clone();
        thread::spawn(move || {
            loop {
                let requests = take_requests(&inbox);
                if !requests.is_empty() {
                    for request in requests {
                        if tx.send(request).is_err() {
                            return;
                        }
                    }
                    ctx.request_repaint();
                }
                thread::sleep(POLL_INTERVAL);
            }
        });
        rx
    }
}

/// 把请求写进收件目录：先写 `.tmp` 再改名，主实例只读完整的 `.req`。
/// 文件名以纳秒时间戳开头，主实例按名字排序就是到达顺序。
fn forward(inbox: &Path, imports: &[PathBuf]) -> Result<()> {
    fs::create_dir_all(inbox).with_context(|| format!("无法创建 {}", inbox.display()))?;
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let stem = format!("{nanos:024}-{}", uuid::Uuid::new_v4().simple());
    let partial = inbox.join(format!("{stem}.{PARTIAL_EXT}"));
    let body = imports
        .iter()
        .map(|path| path.to_string_lossy())
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(&partial, body).context("无法写入转交请求")?;
    fs::rename(&partial, inbox.join(format!("{stem}.{REQUEST_EXT}")))
        .context("无法提交转交请求")?;
    Ok(())
}

/// 取走收件目录里的全部请求（读完即删），按到达顺序返回。
fn take_requests(inbox: &Path) -> Vec<Vec<PathBuf>> {
    let Ok(entries) = fs::read_dir(inbox) else {
        return Vec::new();
    };
    let mut files = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == REQUEST_EXT))
        .collect::<Vec<_>>();
    files.sort();
    files
        .into_iter()
        .filter_map(|file| {
            let body = crate::file_limits::read_bytes(&file, MAX_REQUEST_BYTES, "转交请求");
            let _ = fs::remove_file(&file);
            let body = String::from_utf8_lossy(&body.ok()?).into_owned();
            Some(
                body.lines()
                    .filter(|line| !line.is_empty())
                    .map(PathBuf::from)
                    .collect(),
            )
        })
        .collect()
}

/// 删掉放得太久的请求与没写完的半截文件；新近的留给 [`Instance::watch`] 照常处理
/// （转交进程恰好赶上上一个主实例退出时，请求会等到下次启动再处理）。
fn clear_stale(inbox: &Path, now: SystemTime) {
    let Ok(entries) = fs::read_dir(inbox) else {
        return;
    };
    for entry in entries.flatten() {
        let stale = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_none_or(|age| age > STALE_AFTER);
        if stale {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Windows 不让后台进程随便抢前台：主实例收到请求后 `Focus` 自己的窗口，多半只会
/// 让任务栏按钮闪烁。转交进程是用户双击启动的，手里有前台权，退出前把它让出来。
#[cfg(windows)]
#[allow(unsafe_code)]
mod foreground {
    use windows_sys::Win32::UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow};

    pub(super) fn allow_any() {
        // SAFETY: AllowSetForegroundWindow 只接受一个进程号（ASFW_ANY 表示任意进程），
        // 不涉及指针或句柄；失败只是返回 FALSE（本进程本来就没有前台权），忽略即可。
        unsafe {
            AllowSetForegroundWindow(ASFW_ANY);
        }
    }
}

#[cfg(not(windows))]
mod foreground {
    pub(super) fn allow_any() {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn primary(dir: &Path) -> Instance {
        match start_in(dir, &[]).unwrap() {
            Startup::Primary(instance) => instance,
            Startup::Forwarded => panic!("应当成为主实例"),
        }
    }

    #[test]
    fn second_start_forwards_paths_to_the_primary() {
        let temp = tempfile::tempdir().unwrap();
        let instance = primary(temp.path());

        let packages = vec![PathBuf::from("/收文/甲.gwx"), PathBuf::from("/收文/乙.zip")];
        assert!(matches!(
            start_in(temp.path(), &packages).unwrap(),
            Startup::Forwarded
        ));
        // 再点一次图标：空请求，只唤到前台。
        assert!(matches!(
            start_in(temp.path(), &[]).unwrap(),
            Startup::Forwarded
        ));

        assert_eq!(
            take_requests(&instance.inbox),
            vec![packages, Vec::<PathBuf>::new()]
        );
        // 读完即删。
        assert!(take_requests(&instance.inbox).is_empty());

        // 主实例退出（锁释放）后，下一个进程自己成为主实例。
        drop(instance);
        primary(temp.path());
    }

    const PROBE_DIR_VAR: &str = "GONGWEN_INSTANCE_PROBE_DIR";

    /// 子进程探针：只在下面的跨进程测试里被重新拉起时干活，平时直接返回。
    #[test]
    #[ignore]
    fn child_probe() {
        let Some(dir) = std::env::var_os(PROBE_DIR_VAR) else {
            return;
        };
        let role = match start_in(Path::new(&dir), &[PathBuf::from("/子进程.gwx")]).unwrap() {
            Startup::Primary(_) => "primary",
            Startup::Forwarded => "forwarded",
        };
        println!("probe:{role}");
    }

    fn run_probe(dir: &Path) -> String {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "single_instance::tests::child_probe",
                "--ignored",
                "--nocapture",
            ])
            .env(PROBE_DIR_VAR, dir)
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// 同进程两个句柄的锁冲突不足以说明跨进程也成立，这里真起一个子进程。
    #[test]
    fn another_process_forwards_while_the_primary_holds_the_lock() {
        let temp = tempfile::tempdir().unwrap();
        let instance = primary(temp.path());
        assert!(run_probe(temp.path()).contains("probe:forwarded"));
        assert_eq!(
            take_requests(&instance.inbox),
            vec![vec![PathBuf::from("/子进程.gwx")]]
        );
        drop(instance);
        assert!(run_probe(temp.path()).contains("probe:primary"));
    }

    #[test]
    fn half_written_requests_are_not_taken() {
        let temp = tempfile::tempdir().unwrap();
        let inbox = temp.path().join(INBOX_DIR);
        fs::create_dir_all(&inbox).unwrap();
        fs::write(inbox.join("1.tmp"), "/半截.gwx").unwrap();
        assert!(take_requests(&inbox).is_empty());
        assert!(inbox.join("1.tmp").exists());
    }

    #[test]
    fn stale_requests_are_dropped_and_fresh_ones_kept() {
        let temp = tempfile::tempdir().unwrap();
        let inbox = temp.path().join(INBOX_DIR);
        forward(&inbox, &[PathBuf::from("/新.gwx")]).unwrap();

        clear_stale(&inbox, SystemTime::now());
        assert_eq!(take_requests(&inbox), vec![vec![PathBuf::from("/新.gwx")]]);

        forward(&inbox, &[PathBuf::from("/旧.gwx")]).unwrap();
        clear_stale(&inbox, SystemTime::now() + STALE_AFTER * 2);
        assert!(take_requests(&inbox).is_empty());
    }

    #[test]
    fn watcher_delivers_forwarded_requests() {
        let temp = tempfile::tempdir().unwrap();
        let instance = primary(temp.path());
        let rx = instance.watch(egui::Context::default());
        assert!(matches!(
            start_in(temp.path(), &[PathBuf::from("/来函.gwx")]).unwrap(),
            Startup::Forwarded
        ));
        let request = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(request, vec![PathBuf::from("/来函.gwx")]);
    }
}
