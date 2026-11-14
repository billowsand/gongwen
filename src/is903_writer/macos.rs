use super::{Target, burn, helper, recheck, valid_serial, verify};
use anyhow::{Context, Result, ensure};
use std::collections::HashMap;
use std::ffi::{CStr, c_char, c_int, c_void};
use std::fs::File;
use std::os::fd::{AsRawFd, FromRawFd};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::ptr::{self, NonNull};
use std::time::{Duration, Instant};

const INSTALL: &str = "/Library/PrivilegedHelperTools/org.gongwen.is903";
const HELPER: &str = "/Library/PrivilegedHelperTools/org.gongwen.is903/gongwen-is903";
const LIBRARY: &CStr = c"/Library/PrivilegedHelperTools/org.gongwen.is903/libusb-1.0.0.dylib";

#[derive(Default)]
struct Node<'a> {
    depth: usize,
    class: &'a str,
    registry_id: Option<u64>,
    props: HashMap<&'a str, &'a str>,
}

impl Node<'_> {
    fn number(&self, key: &str) -> Option<u64> {
        let value = *self.props.get(key)?;
        match value.strip_prefix("0x") {
            Some(hex) => u64::from_str_radix(hex, 16).ok(),
            None => value.parse().ok(),
        }
    }
}

pub(super) fn scan() -> Result<Vec<Target>> {
    let output = Command::new("/usr/sbin/ioreg")
        .env_clear()
        .args([
            "-p",
            "IOService",
            "-l",
            "-w",
            "0",
            "-r",
            "-c",
            "IOUSBHostDevice",
        ])
        .output()
        .context("无法读取 IORegistry")?;
    ensure!(output.status.success(), "ioreg 退出状态：{}", output.status);
    let tree = std::str::from_utf8(&output.stdout).context("无法解析 IORegistry 输出")?;
    let mut nodes = Vec::<Node<'_>>::new();
    for line in tree.lines() {
        let text = line.trim_start_matches([' ', '|']);
        if let Some((_, class)) = text
            .strip_prefix("+-o ")
            .and_then(|s| s.split_once("<class "))
        {
            nodes.push(Node {
                depth: line.len() - text.len(),
                class: class.split(',').next().unwrap_or_default(),
                registry_id: class
                    .split_once(", id 0x")
                    .and_then(|(_, rest)| rest.split_once(','))
                    .and_then(|(hex, _)| u64::from_str_radix(hex, 16).ok()),
                ..Node::default()
            });
        } else if let Some(node) = nodes.last_mut()
            && let Some((key, value)) = text.strip_prefix('"').and_then(|s| s.split_once("\" = "))
        {
            node.props.insert(key, value.trim().trim_matches('"'));
        }
    }
    let mut devices = Vec::new();
    for (index, usb) in nodes.iter().enumerate() {
        if usb.class != "IOUSBHostDevice"
            || usb.number("idVendor") != Some(0x1f75)
            || usb.number("idProduct") != Some(0x0903)
        {
            continue;
        }
        let Some(serial) = usb
            .props
            .get("USB Serial Number")
            .or_else(|| usb.props.get("kUSBSerialNumberString"))
            .or_else(|| usb.props.get("Serial Number"))
            .copied()
            .filter(|serial| valid_serial(serial))
        else {
            continue;
        };
        let (Some(revision), Some(session), Some(location)) = (
            usb.number("bcdDevice"),
            usb.number("sessionID"),
            usb.number("locationID"),
        ) else {
            continue;
        };
        for media in nodes[index + 1..]
            .iter()
            .take_while(|node| node.depth > usb.depth && node.class != "IOUSBHostDevice")
        {
            if media.class == "IOCDMedia"
                && media.props.get("Whole") == Some(&"Yes")
                && media.props.get("Writable") == Some(&"No")
                && let Some(media_registry_id) = media.registry_id
                && let Some(&disk) = media.props.get("BSD Name")
                && disk.strip_prefix("disk").is_some_and(|suffix| {
                    !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit())
                })
            {
                devices.push(Target {
                    session,
                    serial: serial.to_owned(),
                    disk: disk.to_owned(),
                    revision,
                    location,
                    media_registry_id,
                });
            }
        }
    }
    devices.sort_by(|a, b| a.disk.cmp(&b.disk));
    Ok(devices)
}

fn unmount(target: &Target) -> Result<()> {
    recheck(target)?;
    let status = Command::new("/usr/sbin/diskutil")
        .env_clear()
        .args(["unmountDisk", &target.disk])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("无法启动 diskutil")?;
    ensure!(status.success(), "无法卸载 IS903，请关闭正在使用光盘的窗口");
    recheck(target)
}

fn refreshed(previous: &Target) -> Result<Target> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let devices = scan()?;
        let mut matches = devices
            .into_iter()
            .filter(|target| target.serial == previous.serial);
        if let Some(device) = matches.next() {
            ensure!(matches.next().is_none(), "IS903 序列号冲突");
            ensure!(
                device.location == previous.location && device.revision == previous.revision,
                "重新枚举的 IS903 身份不一致"
            );
            if device.media_registry_id != previous.media_registry_id {
                return Ok(device);
            }
        }
        ensure!(Instant::now() < deadline, "等待 IS903 光盘重新枚举超时");
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn burn_and_verify(target: &Target, image: &[u8], ready: &dyn Fn() -> Result<()>) -> Result<()> {
    let lib = usb::Api::load()?;
    // One helper at a time, including the intervals while the optical node disappears.
    let lock = File::open(HELPER)?;
    ensure!(
        // SAFETY: The File owns a live descriptor and the flags request an exclusive nonblocking lock.
        unsafe { flock(lock.as_raw_fd(), 2 | 4) } == 0,
        "已有一项 IS903 刻录正在进行"
    );
    unmount(target)?;
    let drive = lib.claim(target)?;
    burn(&drive, image, ready)?;
    drive.release()?;
    let updated = refreshed(target)?;
    unmount(&updated)?;
    let drive = lib.claim(&updated)?;
    verify(&drive, image)?;
    drive.release()?;
    // 回读是只读操作，不会再让光盘重新枚举；这里若继续等 media_registry_id
    // 变化，会把已成功刻录并校验的盘误报成「等待重新枚举超时」。
    Ok(())
}

fn trusted_install() -> Result<()> {
    helper::trusted_file(Path::new(HELPER))?;
    helper::trusted_file(&PathBuf::from(INSTALL).join("libusb-1.0.0.dylib"))
}

#[repr(C)]
struct AuthorizationItem {
    name: *const c_char,
    value_length: usize,
    value: *mut c_void,
    flags: u32,
}

#[repr(C)]
struct AuthorizationItems {
    count: u32,
    items: *mut AuthorizationItem,
}

#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    fn AuthorizationCreate(
        rights: *const AuthorizationItems,
        environment: *const AuthorizationItems,
        flags: u32,
        handle: *mut *mut c_void,
    ) -> i32;
    fn AuthorizationFree(handle: *mut c_void, flags: u32) -> i32;
    fn AuthorizationExecuteWithPrivileges(
        handle: *mut c_void,
        tool: *const c_char,
        flags: u32,
        args: *const *const c_char,
        communications: *mut *mut c_void,
    ) -> i32;
}

unsafe extern "C" {
    fn fclose(stream: *mut c_void) -> c_int;
    fn fileno(stream: *mut c_void) -> c_int;
    fn fcntl(fd: c_int, command: c_int, ...) -> c_int;
    fn flock(fd: c_int, operation: c_int) -> c_int;
}

struct Authorization(NonNull<c_void>);

impl Drop for Authorization {
    fn drop(&mut self) {
        // SAFETY: This object owns the non-null reference returned by AuthorizationCreate.
        unsafe {
            AuthorizationFree(self.0.as_ptr(), 8);
        }
    }
}

fn helper_channel() -> Result<File> {
    trusted_install()?;
    let mut raw = ptr::null_mut();
    let mut right = AuthorizationItem {
        name: c"system.privilege.admin".as_ptr(),
        value_length: 0,
        value: ptr::null_mut(),
        flags: 0,
    };
    let rights = AuthorizationItems {
        count: 1,
        items: &mut right,
    };
    // InteractionAllowed | ExtendRights | PreAuthorize.
    // SAFETY: The rights array and item live through the call; raw is writable storage for the returned reference.
    let status = unsafe { AuthorizationCreate(&rights, ptr::null(), 1 | 2 | 16, &mut raw) };
    ensure!(status == 0, "macOS 管理员授权失败（{status}）");
    let auth = Authorization(NonNull::new(raw).context("系统未返回授权句柄")?);
    let executable = std::ffi::CString::new(HELPER)?;
    let argument = std::ffi::CString::new(helper::ARG)?;
    let args = [argument.as_ptr(), ptr::null()];
    let mut pipe = ptr::null_mut();
    // SAFETY: The authorization reference, C strings, null-terminated argv, and pipe output live through the call.
    let status = unsafe {
        AuthorizationExecuteWithPrivileges(
            auth.0.as_ptr(),
            executable.as_ptr(),
            0,
            args.as_ptr(),
            &mut pipe,
        )
    };
    ensure!(
        status == 0 && !pipe.is_null(),
        "无法启动 IS903 助手（{status}）"
    );
    // The socket created by Authorization Services remains open on the duplicate.
    // SAFETY: Successful Authorization Services execution returned a live standard I/O stream.
    let fd = unsafe { fcntl(fileno(pipe), 67, 0 as c_int) }; // F_DUPFD_CLOEXEC
    // SAFETY: The original stream is owned by this function and not used after this close.
    unsafe {
        fclose(pipe);
    }
    ensure!(fd >= 0, "无法打开 IS903 助手管道");
    // SAFETY: Successful fcntl returned an owned close-on-exec descriptor independent of the closed stream.
    Ok(unsafe { File::from_raw_fd(fd) })
}

pub(super) fn write(
    target: &Target,
    image: &[u8],
    authorize: &dyn Fn() -> Result<()>,
) -> Result<()> {
    authorize()?;
    let mut channel = helper_channel()?;
    let mut input = channel.try_clone()?;
    helper::send(target, image, authorize, &mut input, &mut channel)
}

pub(crate) fn helper_main() -> Option<i32> {
    helper::run(HELPER, 4, trusted_install, burn_and_verify)
}

mod usb;
