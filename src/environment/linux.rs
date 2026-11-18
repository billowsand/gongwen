//! Linux 外设监控。
//!
//! 只使用内核接口和 libc ABI：kobject uevent 负责通知，sysfs 负责生成可重放的完整快照，不依赖 libudev。

use super::{DeviceClass, DeviceIdentity, DeviceInfo, DeviceSnapshot, TimedEvent};
use eframe::egui;
use std::collections::BTreeMap;
use std::ffi::c_void;
use std::fs;
use std::io;
use std::os::raw::c_int;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::thread;
use std::time::{Duration, Instant};

const AF_NETLINK: c_int = 16;
const SOCK_DGRAM: c_int = 2;
const SOCK_NONBLOCK: c_int = 0o0004000;
const SOCK_CLOEXEC: c_int = 0o02000000;
const NETLINK_KOBJECT_UEVENT: c_int = 15;

const HEARTBEAT: Duration = Duration::from_secs(5);
const FALLBACK_HEARTBEAT: Duration = Duration::from_secs(1);
const IDLE_SLEEP: Duration = Duration::from_millis(50);
const ENOBUFS: i32 = 105;

#[repr(C)]
struct SockAddrNl {
    nl_family: u16,
    nl_pad: u16,
    nl_pid: u32,
    nl_groups: u32,
}

unsafe extern "C" {
    fn socket(domain: c_int, socket_type: c_int, protocol: c_int) -> c_int;
    fn bind(fd: c_int, address: *const SockAddrNl, length: u32) -> c_int;
    fn recv(fd: c_int, buffer: *mut c_void, length: usize, flags: c_int) -> isize;
    fn close(fd: c_int) -> c_int;
}

struct NetlinkSocket {
    fd: c_int,
}

impl NetlinkSocket {
    fn open(protocol: c_int, socket_type: c_int, groups: u32) -> io::Result<Self> {
        // SAFETY: 参数是Linux定义的常量；成功后fd由NetlinkSocket独占管理。
        let fd = unsafe {
            socket(
                AF_NETLINK,
                socket_type | SOCK_NONBLOCK | SOCK_CLOEXEC,
                protocol,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let socket = Self { fd };
        let address = SockAddrNl {
            nl_family: AF_NETLINK as u16,
            nl_pad: 0,
            nl_pid: 0,
            nl_groups: groups,
        };
        // SAFETY: address指向布局与sockaddr_nl一致的初始化对象，长度准确；socket仍有效。
        let result = unsafe {
            bind(
                socket.fd,
                &raw const address,
                size_of::<SockAddrNl>() as u32,
            )
        };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(socket)
    }

    fn uevents() -> io::Result<Self> {
        Self::open(NETLINK_KOBJECT_UEVENT, SOCK_DGRAM, 1)
    }

    /// 排空通知；消息内容由权威快照接口重新读取，无需信任uevent载荷。
    fn drain(&self) -> io::Result<bool> {
        let mut changed = false;
        let mut buffer = [0_u8; 8192];
        loop {
            // SAFETY: fd有效，buffer可写且长度与实际分配一致；非阻塞socket不会等待UI。
            let received = unsafe {
                recv(
                    self.fd,
                    buffer.as_mut_ptr().cast::<c_void>(),
                    buffer.len(),
                    0,
                )
            };
            if received > 0 {
                // 忽略网络接口事件；USB 网卡仍可通过 USB 外设事件触发复核。
                changed |= !buffer[..received as usize]
                    .split(|byte| *byte == 0)
                    .any(|field| field == b"SUBSYSTEM=net");
                continue;
            }
            if received == 0 {
                return Ok(changed);
            }
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(ENOBUFS) {
                return Ok(true);
            }
            match error.kind() {
                io::ErrorKind::WouldBlock => return Ok(changed),
                io::ErrorKind::Interrupted => continue,
                _ => return Err(error),
            }
        }
    }
}

impl Drop for NetlinkSocket {
    fn drop(&mut self) {
        // SAFETY: fd由socket创建且仅在此关闭一次；关闭返回值无需影响RAII析构。
        unsafe {
            close(self.fd);
        }
    }
}

fn read_hex(path: &Path) -> io::Result<u32> {
    let value = fs::read_to_string(path)?;
    u32::from_str_radix(value.trim().trim_start_matches("0x"), 16)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn read_optional(path: &Path) -> io::Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(value) => Ok(Some(value.trim().to_owned())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn usb_device_identity(device: &Path) -> Option<DeviceIdentity> {
    if read_hex(&device.join("idVendor")).ok()? != 0x1f75
        || read_hex(&device.join("idProduct")).ok()? != 0x0903
    {
        return None;
    }
    let serial = read_optional(&device.join("serial")).ok().flatten()?;
    if serial.is_empty() {
        return None;
    }
    let revision = read_optional(&device.join("bcdDevice")).ok().flatten()?;
    let revision = u64::from_str_radix(revision.trim_start_matches("0x"), 16).ok()?;
    Some(DeviceIdentity { serial, revision })
}

fn usb_identity(root: &Path, interface_name: &str) -> Option<DeviceIdentity> {
    let device_name = interface_name.split(':').next()?;
    usb_device_identity(&root.join(device_name))
}

fn block_usb_details(path: &Path) -> Option<(String, Option<DeviceIdentity>)> {
    let real = path.canonicalize().ok()?;
    let usb = real
        .ancestors()
        .find(|node| node.join("idVendor").is_file())?;
    Some((format!("usb:{}", usb.display()), usb_device_identity(usb)))
}

fn usb_device_class(path: &Path, class: u32, protocol: Option<u32>) -> DeviceClass {
    let interface_name = read_optional(&path.join("interface"))
        .ok()
        .flatten()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if interface_name.contains("mtp") || interface_name.contains("ptp") {
        return DeviceClass::PortableMedia;
    }
    match class {
        0x08 => DeviceClass::RemovableStorage,
        0x03 => DeviceClass::HumanInterface,
        0x06 if protocol == Some(1) => DeviceClass::PortableMedia,
        0x06 | 0x0e => DeviceClass::Imaging,
        _ => DeviceClass::Other,
    }
}

fn is_usb_block_device(path: &Path) -> bool {
    path.canonicalize()
        .is_ok_and(|real| real.to_string_lossy().contains("/usb"))
}

fn collect_devices() -> Result<DeviceSnapshot, String> {
    let usb_root = Path::new("/sys/bus/usb/devices");
    let entries = fs::read_dir(usb_root)
        .map_err(|error| format!("无法枚举{}: {error}", usb_root.display()))?;
    let mut devices = Vec::new();
    let mut identities = BTreeMap::<String, Option<DeviceIdentity>>::new();

    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("读取USB设备目录失败: {error}")),
        };
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        // USB接口目录名形如“1-2:1.0”；父目录描述整条USB设备。
        if !name.contains(':') {
            continue;
        }
        let path = entry.path();
        let class = match read_optional(&path.join("bInterfaceClass")) {
            Ok(Some(value)) => match u32::from_str_radix(value.trim_start_matches("0x"), 16) {
                Ok(class) => class,
                Err(error) => {
                    return Err(format!("解析USB接口{name}类别失败: {error}"));
                }
            },
            Ok(None) => continue,
            Err(error) => return Err(format!("读取USB接口{name}类别失败: {error}")),
        };
        // 类 08 只说明“支持大容量传输”，空读卡器/空光驱也常驻；
        // 是否有可读介质由下面的块设备与 size 判定。
        if class == 0x08 {
            continue;
        }
        let protocol = read_optional(&path.join("bInterfaceProtocol"))
            .ok()
            .flatten()
            .and_then(|value| u32::from_str_radix(value.trim_start_matches("0x"), 16).ok());
        let device_name = name.split(':').next().unwrap_or(name).to_owned();
        let identity = identities
            .entry(device_name)
            .or_insert_with(|| usb_identity(usb_root, name))
            .clone();
        devices.push(DeviceInfo {
            id: format!("usb:{name}"),
            class: usb_device_class(&path, class, protocol),
            identity,
        });
    }

    // 以实际存在的块设备为准：空的 USB 读卡器/光驱没有可读容量，不报警。
    // USB 外置盘即使 removable=0 也按外接存储处理；非 USB 只补充可移动介质。
    let block_root = Path::new("/sys/block");
    let entries = fs::read_dir(block_root)
        .map_err(|error| format!("无法枚举{}: {error}", block_root.display()))?;
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("读取块设备目录失败: {error}")),
        };
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let path = entry.path();
        let size = match read_optional(&path.join("size")) {
            Ok(Some(value)) => value
                .parse::<u64>()
                .map_err(|error| format!("解析块设备{name}容量失败: {error}"))?,
            Ok(None) => continue,
            Err(error) => return Err(format!("读取块设备{name}容量失败: {error}")),
        };
        if size == 0 {
            continue;
        }
        let usb = is_usb_block_device(&path);
        if !usb {
            match read_optional(&path.join("removable")) {
                Ok(Some(value)) if value == "1" => {}
                Ok(_) => continue,
                Err(error) => return Err(format!("读取块设备{name}属性失败: {error}")),
            }
        }
        let details = if usb { block_usb_details(&path) } else { None };
        let (id, identity) = details.unwrap_or_else(|| (format!("block:{name}"), None));
        devices.push(DeviceInfo {
            id,
            class: DeviceClass::RemovableStorage,
            identity,
        });
    }

    devices.sort_by(|left, right| {
        left.id
            .cmp(&right.id)
            .then_with(|| device_class_rank(left.class).cmp(&device_class_rank(right.class)))
    });
    devices.dedup_by(|left, right| left.id == right.id && left.class == right.class);
    Ok(DeviceSnapshot { devices })
}

fn device_class_rank(class: DeviceClass) -> u8 {
    match class {
        DeviceClass::RemovableStorage => 0,
        DeviceClass::PortableMedia => 1,
        DeviceClass::HumanInterface => 2,
        DeviceClass::Imaging => 3,
        DeviceClass::Other => 4,
    }
}

fn send(
    sender: &Sender<TimedEvent>,
    wake: &egui::Context,
    event: Result<DeviceSnapshot, String>,
) -> bool {
    let sent = sender.send((Instant::now(), event)).is_ok();
    wake.request_repaint();
    sent
}

fn snapshot_with_retry<T>(mut collect: impl FnMut() -> Result<T, String>) -> Result<T, String> {
    let mut last = None;
    for attempt in 0..3 {
        match collect() {
            Ok(value) => return Ok(value),
            Err(error) => {
                last = Some(error);
                if attempt < 2 {
                    thread::sleep(Duration::from_millis(40));
                }
            }
        }
    }
    Err(last.unwrap_or_else(|| "采集失败".to_owned()))
}

fn send_devices(sender: &Sender<TimedEvent>, wake: &egui::Context) -> bool {
    send(sender, wake, snapshot_with_retry(collect_devices))
}

pub(crate) fn run(sender: Sender<TimedEvent>, stop: Arc<AtomicBool>, wake: egui::Context) {
    // 先订阅再枚举，确保枚举期间发生的热插拔也会触发第二次对账。
    let mut uevent_socket = NetlinkSocket::uevents().ok();

    if !send_devices(&sender, &wake) {
        return;
    }
    let mut last_devices = Instant::now();
    let mut device_dirty = false;

    while !stop.load(Ordering::Acquire) {
        let now = Instant::now();
        let uevent_result = uevent_socket.as_ref().map(NetlinkSocket::drain);
        match uevent_result {
            Some(Ok(true)) => device_dirty = true,
            Some(Ok(false)) | None => {}
            Some(Err(_)) => {
                uevent_socket = None;
                device_dirty = true;
            }
        }

        let device_interval = if uevent_socket.is_some() {
            HEARTBEAT
        } else {
            FALLBACK_HEARTBEAT
        };
        if device_dirty || now.duration_since(last_devices) >= device_interval {
            if !send_devices(&sender, &wake) {
                return;
            }
            device_dirty = false;
            last_devices = now;
        }

        thread::sleep(IDLE_SLEEP);
    }
}
