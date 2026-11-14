//! Linux 网络与 USB 设备监控。
//!
//! 只使用内核接口和 libc ABI：rtnetlink/kobject uevent 负责通知，sysfs、
//! getifaddrs 与 proc 路由表负责生成可重放的完整快照，不依赖 libudev。

use super::{
    DeviceClass, DeviceIdentity, DeviceInfo, DeviceSnapshot, MonitorEvent, NetworkInterface,
    NetworkInterfaceKind, NetworkSnapshot,
};
use eframe::egui;
use std::collections::BTreeMap;
use std::ffi::{CStr, c_void};
use std::fs;
use std::io;
use std::os::raw::{c_char, c_int, c_uint};
use std::path::Path;
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::thread;
use std::time::{Duration, Instant};

const AF_NETLINK: c_int = 16;
const AF_INET: u16 = 2;
const AF_INET6: u16 = 10;
const SOCK_DGRAM: c_int = 2;
const SOCK_RAW: c_int = 3;
const SOCK_NONBLOCK: c_int = 0o0004000;
const SOCK_CLOEXEC: c_int = 0o02000000;
const NETLINK_ROUTE: c_int = 0;
const NETLINK_KOBJECT_UEVENT: c_int = 15;

const RTMGRP_LINK: u32 = 0x0001;
const RTMGRP_IPV4_IFADDR: u32 = 0x0010;
const RTMGRP_IPV4_ROUTE: u32 = 0x0040;
const RTMGRP_IPV6_IFADDR: u32 = 0x0100;
const RTMGRP_IPV6_ROUTE: u32 = 0x0400;

const IFF_UP: c_uint = 0x0001;
const IFF_LOOPBACK: c_uint = 0x0008;
const IFF_RUNNING: c_uint = 0x0040;
const IFF_LOWER_UP: c_uint = 0x1_0000;
const RTF_UP: u32 = 0x0001;

const HEARTBEAT: Duration = Duration::from_secs(5);
const IDLE_SLEEP: Duration = Duration::from_millis(50);

#[repr(C)]
struct SockAddr {
    sa_family: u16,
    sa_data: [c_char; 14],
}

#[repr(C)]
struct InAddr {
    s_addr: u32,
}

#[repr(C)]
struct SockAddrIn {
    sin_family: u16,
    sin_port: u16,
    sin_addr: InAddr,
    sin_zero: [u8; 8],
}

#[repr(C)]
struct In6Addr {
    s6_addr: [u8; 16],
}

#[repr(C)]
struct SockAddrIn6 {
    sin6_family: u16,
    sin6_port: u16,
    sin6_flowinfo: u32,
    sin6_addr: In6Addr,
    sin6_scope_id: u32,
}

#[repr(C)]
struct IfAddrs {
    ifa_next: *mut IfAddrs,
    ifa_name: *mut c_char,
    ifa_flags: c_uint,
    ifa_addr: *mut SockAddr,
    ifa_netmask: *mut SockAddr,
    ifa_ifu: *mut SockAddr,
    ifa_data: *mut c_void,
}

struct IfAddrsGuard(*mut IfAddrs);

impl Drop for IfAddrsGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: 非空指针由成功的getifaddrs返回，并且只在guard析构时释放一次。
            unsafe { freeifaddrs(self.0) }
        }
    }
}

#[repr(C)]
struct SockAddrNl {
    nl_family: u16,
    nl_pad: u16,
    nl_pid: u32,
    nl_groups: u32,
}

unsafe extern "C" {
    fn getifaddrs(ifap: *mut *mut IfAddrs) -> c_int;
    fn freeifaddrs(ifa: *mut IfAddrs);
    fn socket(domain: c_int, socket_type: c_int, protocol: c_int) -> c_int;
    fn bind(fd: c_int, address: *const SockAddr, length: u32) -> c_int;
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
                (&raw const address).cast::<SockAddr>(),
                size_of::<SockAddrNl>() as u32,
            )
        };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(socket)
    }

    fn route() -> io::Result<Self> {
        Self::open(
            NETLINK_ROUTE,
            SOCK_RAW,
            RTMGRP_LINK
                | RTMGRP_IPV4_IFADDR
                | RTMGRP_IPV4_ROUTE
                | RTMGRP_IPV6_IFADDR
                | RTMGRP_IPV6_ROUTE,
        )
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
                changed = true;
                continue;
            }
            if received == 0 {
                return Ok(changed);
            }
            let error = io::Error::last_os_error();
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

#[derive(Default)]
struct InterfaceState {
    flags: c_uint,
    non_loopback_address: bool,
}

fn ipv4_is_usable_non_loopback(address: &SockAddrIn) -> bool {
    // to_ne_bytes保留内存中的网络字节序，因此首字节可直接判断127/8与0.0.0.0。
    let first = address.sin_addr.s_addr.to_ne_bytes()[0];
    first != 0 && first != 127
}

fn ipv6_is_usable_non_loopback(address: &SockAddrIn6) -> bool {
    let bytes = address.sin6_addr.s6_addr;
    let unspecified = bytes.iter().all(|byte| *byte == 0);
    let loopback = bytes[..15].iter().all(|byte| *byte == 0) && bytes[15] == 1;
    !unspecified && !loopback
}

fn read_hex(path: &Path) -> io::Result<u32> {
    let value = fs::read_to_string(path)?;
    u32::from_str_radix(value.trim().trim_start_matches("0x"), 16)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn interface_kind(name: &str, path: &Path, loopback: bool) -> NetworkInterfaceKind {
    if loopback {
        return NetworkInterfaceKind::Other;
    }
    if path.join("wireless").exists() || path.join("phy80211").exists() {
        return NetworkInterfaceKind::Wifi;
    }
    if name.starts_with("wwan")
        || name.starts_with("rmnet")
        || name.starts_with("ppp")
        || name.starts_with("qmi")
    {
        return NetworkInterfaceKind::Cellular;
    }
    if path.join("tun_flags").exists()
        || name.starts_with("tun")
        || name.starts_with("tap")
        || name.starts_with("wg")
        || name.starts_with("vpn")
    {
        return NetworkInterfaceKind::Vpn;
    }
    let is_virtual = path
        .canonicalize()
        .is_ok_and(|real| real.to_string_lossy().contains("/virtual/"));
    let ethernet_type = read_hex(&path.join("type")).ok() == Some(1);
    if ethernet_type && !is_virtual {
        NetworkInterfaceKind::Ethernet
    } else {
        NetworkInterfaceKind::Other
    }
}

fn interface_link_up(path: &Path, flags: c_uint, loopback: bool) -> bool {
    if loopback {
        return flags & IFF_UP != 0;
    }
    match fs::read_to_string(path.join("carrier")) {
        Ok(value) if value.trim() == "1" => true,
        Ok(value) if value.trim() == "0" => false,
        _ => flags & (IFF_LOWER_UP | IFF_RUNNING) != 0,
    }
}

fn collect_network() -> Result<NetworkSnapshot, String> {
    let mut states = BTreeMap::<String, InterfaceState>::new();
    let mut head = ptr::null_mut();
    // SAFETY: head是有效的输出指针；成功后立即用guard保证所有返回路径都释放链表。
    if unsafe { getifaddrs(&raw mut head) } != 0 {
        return Err(format!("getifaddrs失败: {}", io::Error::last_os_error()));
    }
    let _guard = IfAddrsGuard(head);
    let mut current = head;
    while !current.is_null() {
        // SAFETY: current来自getifaddrs链表，在guard存活期间节点及其字段有效。
        let interface = unsafe { &*current };
        if !interface.ifa_name.is_null() {
            // SAFETY: ifa_name是内核提供的NUL结尾字符串，生命周期覆盖本次读取。
            let name = unsafe { CStr::from_ptr(interface.ifa_name) }
                .to_string_lossy()
                .into_owned();
            let state = states.entry(name).or_default();
            state.flags = interface.ifa_flags;
            if !interface.ifa_addr.is_null() {
                // SAFETY: ifa_addr非空，先读取共有的sa_family，再按族转换到对应布局。
                let family = unsafe { (*interface.ifa_addr).sa_family };
                let usable = match family {
                    AF_INET => {
                        // SAFETY: family为AF_INET，getifaddrs保证地址对象按sockaddr_in布局。
                        let address = unsafe { &*interface.ifa_addr.cast::<SockAddrIn>() };
                        ipv4_is_usable_non_loopback(address)
                    }
                    AF_INET6 => {
                        // SAFETY: family为AF_INET6，getifaddrs保证地址对象按sockaddr_in6布局。
                        let address = unsafe { &*interface.ifa_addr.cast::<SockAddrIn6>() };
                        ipv6_is_usable_non_loopback(address)
                    }
                    _ => false,
                };
                state.non_loopback_address |= usable;
            }
        }
        current = interface.ifa_next;
    }

    let sysfs = Path::new("/sys/class/net");
    let entries =
        fs::read_dir(sysfs).map_err(|error| format!("无法枚举{}: {error}", sysfs.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("读取网络接口目录失败: {error}"))?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let path = entry.path();
        let state = states.entry(name).or_default();
        if let Ok(flags) = read_hex(&path.join("flags")) {
            state.flags = flags;
        }
    }

    let mut interfaces = Vec::with_capacity(states.len());
    for (name, state) in states {
        let path = sysfs.join(&name);
        let loopback = name == "lo" || state.flags & IFF_LOOPBACK != 0;
        let kind = interface_kind(&name, &path, loopback);
        let link_up = interface_link_up(&path, state.flags, loopback);
        interfaces.push(NetworkInterface {
            id: name,
            kind,
            loopback,
            link_up,
            non_loopback_address: state.non_loopback_address,
        });
    }
    Ok(NetworkSnapshot {
        interfaces,
        default_route: default_route()?,
    })
}

fn parse_ipv4_default_route(table: &str) -> bool {
    table.lines().skip(1).any(|line| {
        let mut fields = line.split_whitespace();
        let (Some(interface), Some(destination), Some(_gateway), Some(flags)) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            return false;
        };
        let Some(mask) = fields.nth(3) else {
            return false;
        };
        if interface == "lo" {
            return false;
        }
        let destination = u32::from_str_radix(destination, 16);
        let flags = u32::from_str_radix(flags, 16);
        let mask = u32::from_str_radix(mask, 16);
        matches!((destination, flags, mask), (Ok(0), Ok(flags), Ok(0)) if flags & RTF_UP != 0)
    })
}

fn parse_ipv6_default_route(table: &str) -> bool {
    table.lines().any(|line| {
        let mut fields = line.split_whitespace();
        let (Some(destination), Some(prefix)) = (fields.next(), fields.next()) else {
            return false;
        };
        let (Some(flags), Some(interface)) = (fields.nth(6), fields.next()) else {
            return false;
        };
        if interface == "lo" {
            return false;
        }
        let destination = u128::from_str_radix(destination, 16);
        let prefix = u32::from_str_radix(prefix, 16);
        let flags = u32::from_str_radix(flags, 16);
        matches!((destination, prefix, flags), (Ok(0), Ok(0), Ok(flags)) if flags & RTF_UP != 0)
    })
}

fn default_route() -> Result<bool, String> {
    let mut available_tables = 0_u8;
    let mut found = false;
    for (path, ipv6) in [("/proc/net/route", false), ("/proc/net/ipv6_route", true)] {
        match fs::read_to_string(path) {
            Ok(table) => {
                available_tables += 1;
                found |= if ipv6 {
                    parse_ipv6_default_route(&table)
                } else {
                    parse_ipv4_default_route(&table)
                };
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("无法读取{path}: {error}")),
        }
    }
    if available_tables == 0 {
        Err("IPv4/IPv6路由表均不可用".to_owned())
    } else {
        Ok(found)
    }
}

fn read_optional(path: &Path) -> io::Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(value) => Ok(Some(value.trim().to_owned())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn usb_identity(root: &Path, interface_name: &str) -> Option<DeviceIdentity> {
    let device_name = interface_name.split(':').next()?;
    let device = root.join(device_name);
    let serial = read_optional(&device.join("serial")).ok().flatten()?;
    if serial.is_empty() {
        return None;
    }
    let revision = read_optional(&device.join("bcdDevice")).ok().flatten()?;
    let revision = u64::from_str_radix(revision.trim_start_matches("0x"), 16).ok()?;
    Some(DeviceIdentity { serial, revision })
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

    // 补充MMC/SD等非USB可移动块设备；USB块设备已经由接口类08覆盖。
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
        match read_optional(&path.join("removable")) {
            Ok(Some(value)) if value == "1" => {}
            Ok(_) => continue,
            Err(error) => return Err(format!("读取块设备{name}属性失败: {error}")),
        }
        if !is_usb_block_device(&path) {
            devices.push(DeviceInfo {
                id: format!("block:{name}"),
                class: DeviceClass::RemovableStorage,
                identity: None,
            });
        }
    }

    devices.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(DeviceSnapshot { devices })
}

fn send(sender: &Sender<MonitorEvent>, wake: &egui::Context, event: MonitorEvent) -> bool {
    let sent = sender.send(event).is_ok();
    wake.request_repaint();
    sent
}

fn send_network(
    sender: &Sender<MonitorEvent>,
    wake: &egui::Context,
    monitor_available: bool,
) -> bool {
    let event = if monitor_available {
        MonitorEvent::Network(collect_network())
    } else {
        MonitorEvent::Network(Err("rtnetlink事件监控不可用".to_owned()))
    };
    send(sender, wake, event)
}

fn send_devices(
    sender: &Sender<MonitorEvent>,
    wake: &egui::Context,
    monitor_available: bool,
) -> bool {
    let event = if monitor_available {
        MonitorEvent::Devices(collect_devices())
    } else {
        MonitorEvent::Devices(Err("kernel uevent设备事件监控不可用".to_owned()))
    };
    send(sender, wake, event)
}

pub(crate) fn run(sender: Sender<MonitorEvent>, stop: Arc<AtomicBool>, wake: egui::Context) {
    // 先订阅再枚举，确保枚举期间发生的热插拔/地址变化也会触发第二次对账。
    let mut route_socket = NetlinkSocket::route().ok();
    let mut uevent_socket = NetlinkSocket::uevents().ok();

    // 启动时先给出真实枚举结果；订阅失败随后置为异常，绝不让Ok快照覆盖该错误。
    if !send_network(&sender, &wake, true) || !send_devices(&sender, &wake, true) {
        return;
    }
    if route_socket.is_none()
        && !send(
            &sender,
            &wake,
            MonitorEvent::Network(Err("无法订阅rtnetlink".to_owned())),
        )
    {
        return;
    }
    if uevent_socket.is_none()
        && !send(
            &sender,
            &wake,
            MonitorEvent::Devices(Err("无法订阅kernel uevent".to_owned())),
        )
    {
        return;
    }
    let mut last_network = Instant::now();
    let mut last_devices = last_network;
    let mut network_dirty = false;
    let mut device_dirty = false;

    while !stop.load(Ordering::Acquire) {
        let now = Instant::now();
        let route_result = route_socket.as_ref().map(NetlinkSocket::drain);
        match route_result {
            Some(Ok(changed)) => network_dirty |= changed,
            Some(Err(error)) => {
                route_socket = None;
                network_dirty = false;
                if !send(
                    &sender,
                    &wake,
                    MonitorEvent::Network(Err(format!("rtnetlink接收失败: {error}"))),
                ) {
                    return;
                }
            }
            None => {}
        }
        let uevent_result = uevent_socket.as_ref().map(NetlinkSocket::drain);
        match uevent_result {
            Some(Ok(true)) => {
                device_dirty = true;
            }
            Some(Ok(false)) | None => {}
            Some(Err(error)) => {
                uevent_socket = None;
                device_dirty = false;
                if !send(
                    &sender,
                    &wake,
                    MonitorEvent::Devices(Err(format!("kernel uevent接收失败: {error}"))),
                ) {
                    return;
                }
            }
        }

        if network_dirty || now.duration_since(last_network) >= HEARTBEAT {
            if !send_network(&sender, &wake, route_socket.is_some()) {
                return;
            }
            network_dirty = false;
            last_network = now;
        }
        if device_dirty || now.duration_since(last_devices) >= HEARTBEAT {
            if !send_devices(&sender, &wake, uevent_socket.is_some()) {
                return;
            }
            device_dirty = false;
            last_devices = now;
        }

        thread::sleep(IDLE_SLEEP);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipv4_default_route_requires_up_flag_and_zero_mask() {
        let up = "Iface Destination Gateway Flags RefCnt Use Metric Mask MTU Window IRTT\n\
                  eth0 00000000 0102A8C0 0003 0 0 0 00000000 0 0 0\n";
        let down = "Iface Destination Gateway Flags RefCnt Use Metric Mask MTU Window IRTT\n\
                    eth0 00000000 0102A8C0 0002 0 0 0 00000000 0 0 0\n";
        let subnet = "Iface Destination Gateway Flags RefCnt Use Metric Mask MTU Window IRTT\n\
                      eth0 0002A8C0 00000000 0001 0 0 0 0000FFFF 0 0 0\n";
        assert!(parse_ipv4_default_route(up));
        assert!(!parse_ipv4_default_route(down));
        assert!(!parse_ipv4_default_route(subnet));
    }

    #[test]
    fn ipv6_default_route_ignores_loopback() {
        let zeros = "00000000000000000000000000000000";
        let default =
            format!("{zeros} 00 {zeros} 00 {zeros} 00000000 00000000 00000000 00000001 eth0\n");
        let loopback =
            format!("{zeros} 00 {zeros} 00 {zeros} 00000000 00000000 00000000 00000001 lo\n");
        assert!(parse_ipv6_default_route(&default));
        assert!(!parse_ipv6_default_route(&loopback));
    }
}
