//! 单实例转交：请求只在 loopback 管道和内存中传递，不再写入路径收件文件。
//! endpoint 仅包含临时端口与本轮随机认证令牌；不含业务路径、应用密码或数据密钥。
use anyhow::{Context, Result};
use eframe::egui;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver},
};
use std::thread;
use std::time::Duration;

pub const NEW_WINDOW_FLAG: &str = "--new-window";
const LOCK_FILE: &str = "instance.lock";
const ENDPOINT_FILE: &str = "instance.endpoint";
const MAX_REQUEST_BYTES: u64 = 1024 * 1024;

pub enum Startup {
    Primary(Instance),
    Forwarded,
}

pub struct Instance {
    _lock: File,
    endpoint: PathBuf,
    running: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
    requests: Mutex<Option<Receiver<Vec<PathBuf>>>>,
}

impl Drop for Instance {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        let _ = fs::remove_file(&self.endpoint);
    }
}

pub fn start(imports: &[PathBuf]) -> Result<Startup> {
    start_in(&crate::storage::config_dir()?, imports)
}

fn start_in(dir: &Path, imports: &[PathBuf]) -> Result<Startup> {
    fs::create_dir_all(dir)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join(LOCK_FILE))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        lock.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    let endpoint = dir.join(ENDPOINT_FILE);
    match lock.try_lock() {
        Ok(()) => {
            // 旧版明文收件箱不再消费；清理失败不能装作风险已消失。
            let inbox = dir.join("inbox");
            if inbox.is_dir() {
                for entry in fs::read_dir(&inbox)? {
                    let entry = entry?;
                    if entry.file_type()?.is_file() {
                        crate::vault::erase_file_checked(&entry.path())?;
                    }
                }
                fs::remove_dir(&inbox).context("旧明文转交目录清理失败")?;
            }
            let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
            listener.set_nonblocking(true)?;
            let token = uuid::Uuid::new_v4().simple().to_string();
            let record = format!("{}\n{}", listener.local_addr()?.port(), token);
            // 仅公开本轮寻址信息；复制磁盘也拿不到任何已转交的路径。
            crate::storage::atomic_write(&endpoint, |file| {
                file.write_all(record.as_bytes())?;
                Ok(())
            })?;
            let running = Arc::new(AtomicBool::new(true));
            let alive = running.clone();
            let (tx, rx) = mpsc::channel();
            let worker = thread::spawn(move || {
                while alive.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((mut stream, peer)) if peer.ip().is_loopback() => {
                            let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
                            let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
                            if let Ok(paths) = receive(&mut stream, &token) {
                                if tx.send(paths).is_err() {
                                    break;
                                }
                                let _ = stream.write_all(b"OK");
                            }
                        }
                        _ => thread::sleep(Duration::from_millis(25)),
                    }
                }
            });
            Ok(Startup::Primary(Instance {
                _lock: lock,
                endpoint,
                running,
                worker: Some(worker),
                requests: Mutex::new(Some(rx)),
            }))
        }
        Err(fs::TryLockError::WouldBlock) => {
            forward(&endpoint, imports)?;
            foreground::allow_any();
            Ok(Startup::Forwarded)
        }
        Err(fs::TryLockError::Error(error)) => Err(error).context("无法锁定实例锁文件"),
    }
}

fn receive(stream: &mut TcpStream, token: &str) -> Result<Vec<PathBuf>> {
    let mut length = [0u8; 4];
    stream.read_exact(&mut length)?;
    let length = u32::from_le_bytes(length) as u64;
    anyhow::ensure!(length <= MAX_REQUEST_BYTES, "转交请求过大");
    let mut body = vec![0u8; length as usize];
    stream.read_exact(&mut body)?;
    let (received, paths): (String, Vec<PathBuf>) = serde_json::from_slice(&body)?;
    anyhow::ensure!(received == token, "转交认证失败");
    Ok(paths)
}

fn forward(endpoint: &Path, imports: &[PathBuf]) -> Result<()> {
    let record = crate::file_limits::read_bytes(endpoint, 128, "实例地址")?;
    let record = String::from_utf8(record)?;
    let (port, token) = record.split_once('\n').context("实例地址格式错误")?;
    let port: u16 = port.parse()?;
    let mut stream = TcpStream::connect_timeout(
        &SocketAddrV4::new(Ipv4Addr::LOCALHOST, port).into(),
        Duration::from_secs(1),
    )?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    let body = serde_json::to_vec(&(token, imports))?;
    anyhow::ensure!(body.len() as u64 <= MAX_REQUEST_BYTES, "转交请求过大");
    stream.write_all(&(body.len() as u32).to_le_bytes())?;
    stream.write_all(&body)?;
    let mut ack = [0u8; 2];
    stream.read_exact(&mut ack)?;
    anyhow::ensure!(&ack == b"OK", "转交未确认");
    Ok(())
}

impl Instance {
    pub fn watch(&self, ctx: egui::Context) -> Receiver<Vec<PathBuf>> {
        let (tx, rx) = mpsc::channel();
        let requests = self.requests.lock().unwrap().take();
        if let Some(requests) = requests {
            thread::spawn(move || {
                while let Ok(request) = requests.recv() {
                    if tx.send(request).is_err() {
                        break;
                    }
                    ctx.request_repaint();
                }
            });
        }
        rx
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
    #[test]
    fn transfer_is_memory_only_and_new_primary_can_start_after_drop() {
        let dir = tempfile::tempdir().unwrap();
        let Startup::Primary(primary) = start_in(dir.path(), &[]).unwrap() else {
            panic!()
        };
        let rx = primary.watch(egui::Context::default());
        let paths = vec![PathBuf::from("/某单位/关于人事调整的请示.gwx")];
        assert!(matches!(
            start_in(dir.path(), &paths).unwrap(),
            Startup::Forwarded
        ));
        assert_eq!(rx.recv_timeout(Duration::from_secs(3)).unwrap(), paths);
        for entry in fs::read_dir(dir.path()).unwrap() {
            let bytes = fs::read(entry.unwrap().path()).unwrap();
            assert!(
                !bytes
                    .windows("人事调整".len())
                    .any(|v| v == "人事调整".as_bytes())
            );
        }
        assert!(!dir.path().join("inbox").exists());
        drop(primary);
        assert!(matches!(
            start_in(dir.path(), &[]).unwrap(),
            Startup::Primary(_)
        ));
    }

    #[test]
    fn wrong_token_cannot_inject_a_request() {
        let dir = tempfile::tempdir().unwrap();
        let Startup::Primary(primary) = start_in(dir.path(), &[]).unwrap() else {
            panic!()
        };
        let rx = primary.watch(egui::Context::default());
        let record = fs::read_to_string(&primary.endpoint).unwrap();
        fs::write(
            dir.path().join("fake"),
            format!("{}\nwrong", record.split_once('\n').unwrap().0),
        )
        .unwrap();
        assert!(forward(&dir.path().join("fake"), &[]).is_err());
        assert!(rx.try_recv().is_err());
    }
}
