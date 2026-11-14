//! libusb BOT transport for the fixed interface zero on IS903.
use crate::is903_writer::{Scsi, Target};
use anyhow::{Context, Result, bail, ensure};
use std::cell::Cell;
use std::ffi::{CStr, c_int, c_void};
use std::ptr::{self, NonNull};

type Opaque = *mut c_void;

#[repr(C)]
#[derive(Default)]
struct Descriptor {
    length: u8,
    kind: u8,
    usb: u16,
    class: u8,
    subclass: u8,
    protocol: u8,
    packet: u8,
    vid: u16,
    pid: u16,
    revision: u16,
    manufacturer: u8,
    product: u8,
    serial: u8,
    configs: u8,
}

#[repr(C)]
struct Endpoint {
    length: u8,
    kind: u8,
    address: u8,
    attributes: u8,
    packet_size: u16,
    interval: u8,
    refresh: u8,
    sync_address: u8,
    extra: *const u8,
    extra_len: c_int,
}

#[repr(C)]
struct Alternate {
    length: u8,
    kind: u8,
    number: u8,
    setting: u8,
    count: u8,
    class: u8,
    subclass: u8,
    protocol: u8,
    label: u8,
    endpoints: *const Endpoint,
    extra: *const u8,
    extra_len: c_int,
}

#[repr(C)]
struct Interface {
    alternates: *const Alternate,
    count: c_int,
}

#[repr(C)]
struct Config {
    length: u8,
    kind: u8,
    total_length: u16,
    count: u8,
    value: u8,
    label: u8,
    attributes: u8,
    power: u8,
    interfaces: *const Interface,
    extra: *const u8,
    extra_len: c_int,
}

unsafe extern "C" {
    fn dlopen(path: *const std::ffi::c_char, mode: c_int) -> Opaque;
    fn dlsym(handle: Opaque, symbol: *const std::ffi::c_char) -> Opaque;
    fn dlclose(handle: Opaque) -> c_int;
}

struct Library(NonNull<c_void>);

impl Drop for Library {
    fn drop(&mut self) {
        // SAFETY: This object owns the loader reference and lives longer than all resolved functions.
        unsafe {
            dlclose(self.0.as_ptr());
        }
    }
}

macro_rules! functions {
    ($($name:ident: $ty:ty),+ $(,)?) => {
        pub(super) struct Api {
            $($name: $ty,)+
            _library: Library,
        }

        impl Api {
            pub(super) fn load() -> Result<Self> {
                // The path and every ancestor have already passed trusted_install.
                // SAFETY: The path is a NUL-terminated constant in a verified root-owned installation.
                let handle = unsafe { dlopen(super::LIBRARY.as_ptr(), 0x2 | 0x4) };
                let library = Library(NonNull::new(handle).context("无法加载已安装的 IS903 libusb")?);
                Ok(Self {
                    $($name: {
                        let name = CStr::from_bytes_with_nul(concat!("libusb_", stringify!($name), "\0").as_bytes())?;
                        // SAFETY: The handle is still open and name is a NUL-terminated C string.
                        let symbol = unsafe { dlsym(handle, name.as_ptr()) };
                        ensure!(!symbol.is_null(), "IS903 libusb 缺少 {}，需 1.0.30 或更新版本", name.to_string_lossy());
                        // The symbol signatures are declared from the libusb public C ABI.
                        // SAFETY: The non-null C function symbol has the exact signature declared from the libusb header.
                        unsafe { std::mem::transmute::<Opaque, $ty>(symbol) }
                    },)+
                    _library: library,
                })
            }
        }
    }
}

functions! {
    init: unsafe extern "C" fn(*mut Opaque) -> c_int,
    exit: unsafe extern "C" fn(Opaque),
    get_device_list: unsafe extern "C" fn(Opaque, *mut *mut Opaque) -> isize,
    free_device_list: unsafe extern "C" fn(*mut Opaque, c_int),
    get_session_data: unsafe extern "C" fn(Opaque) -> usize,
    get_device_descriptor: unsafe extern "C" fn(Opaque, *mut Descriptor) -> c_int,
    get_active_config_descriptor: unsafe extern "C" fn(Opaque, *mut *const Config) -> c_int,
    free_config_descriptor: unsafe extern "C" fn(*const Config),
    open: unsafe extern "C" fn(Opaque, *mut Opaque) -> c_int,
    close: unsafe extern "C" fn(Opaque),
    get_string_descriptor_ascii: unsafe extern "C" fn(Opaque, u8, *mut u8, c_int) -> c_int,
    set_auto_detach_kernel_driver: unsafe extern "C" fn(Opaque, c_int) -> c_int,
    claim_interface: unsafe extern "C" fn(Opaque, c_int) -> c_int,
    release_interface: unsafe extern "C" fn(Opaque, c_int) -> c_int,
    bulk_transfer: unsafe extern "C" fn(Opaque, u8, *mut u8, c_int, *mut c_int, u32) -> c_int,
}

struct UsbContext<'a> {
    lib: &'a Api,
    raw: Opaque,
}

impl Drop for UsbContext<'_> {
    fn drop(&mut self) {
        // SAFETY: This object exclusively owns a live libusb context, and handles close first.
        unsafe {
            (self.lib.exit)(self.raw);
        }
    }
}

struct DeviceList<'a> {
    lib: &'a Api,
    raw: *mut Opaque,
}

impl Drop for DeviceList<'_> {
    fn drop(&mut self) {
        // SAFETY: The list came from libusb_get_device_list and is freed exactly once.
        unsafe {
            (self.lib.free_device_list)(self.raw, 1);
        }
    }
}

struct DeviceConfig<'a> {
    lib: &'a Api,
    raw: NonNull<Config>,
}

impl Drop for DeviceConfig<'_> {
    fn drop(&mut self) {
        // SAFETY: The descriptor came from libusb_get_active_config_descriptor and is freed once.
        unsafe {
            (self.lib.free_config_descriptor)(self.raw.as_ptr());
        }
    }
}

fn success(code: c_int, operation: &str) -> Result<()> {
    ensure!(code == 0, "{operation}失败（libusb {code}）");
    Ok(())
}

impl Api {
    fn endpoints(&self, target: Opaque) -> Result<(u8, u8)> {
        let mut raw = ptr::null();
        success(
            // SAFETY: target points to a libusb device retained by the device list, and raw is a writable out-pointer.
            unsafe { (self.get_active_config_descriptor)(target, &mut raw) },
            "查询 USB 端点",
        )?;
        let config = DeviceConfig {
            lib: self,
            raw: NonNull::new(raw.cast_mut()).context("libusb 未返回设备配置")?,
        };
        // SAFETY: The live DeviceConfig owns this non-null libusb descriptor until the enclosing function returns.
        let cfg = unsafe { config.raw.as_ref() };
        for interface in descriptor_slice(cfg.interfaces, cfg.count.into())? {
            for alternate in
                descriptor_slice(interface.alternates, usize::try_from(interface.count)?)?
            {
                if (
                    alternate.number,
                    alternate.setting,
                    alternate.class,
                    alternate.subclass,
                    alternate.protocol,
                ) != (0, 0, 8, 6, 0x50)
                {
                    continue;
                }
                let mut input = None;
                let mut output = None;
                for endpoint in descriptor_slice(alternate.endpoints, alternate.count.into())? {
                    if endpoint.attributes & 3 == 2 {
                        let direction = if endpoint.address & 0x80 == 0 {
                            &mut output
                        } else {
                            &mut input
                        };
                        ensure!(
                            direction.replace(endpoint.address).is_none(),
                            "USB BOT 端点重复"
                        );
                    }
                }
                if let (Some(input), Some(output)) = (input, output) {
                    return Ok((input, output));
                }
            }
        }
        bail!("目标没有指定的 USB BOT 接口")
    }

    pub(super) fn claim(&self, target: &Target) -> Result<Drive<'_>> {
        let mut raw = ptr::null_mut();
        // SAFETY: raw is writable storage for a single libusb context pointer.
        success(unsafe { (self.init)(&mut raw) }, "初始化 USB")?;
        let context = UsbContext { lib: self, raw };
        let mut list = ptr::null_mut();
        // SAFETY: The initialized context is alive and list is a writable pointer slot.
        let count = unsafe { (self.get_device_list)(context.raw, &mut list) };
        ensure!(count >= 0, "无法扫描 USB 设备（libusb {count}）");
        let devices = DeviceList {
            lib: self,
            raw: list,
        };
        for &device in descriptor_slice(devices.raw, count as usize)? {
            // SAFETY: The retained libusb list guarantees a live device for each iteration.
            if unsafe { (self.get_session_data)(device) } as u64 != target.session {
                continue;
            }
            let mut descriptor = Descriptor::default();
            success(
                // SAFETY: The retained device and writable C-layout descriptor live through this call.
                unsafe { (self.get_device_descriptor)(device, &mut descriptor) },
                "读取 USB 标识",
            )?;
            ensure!(
                (
                    descriptor.vid,
                    descriptor.pid,
                    u64::from(descriptor.revision)
                ) == (0x1f75, 0x0903, target.revision)
                    && descriptor.serial != 0,
                "USB 标识与已认证设备不一致"
            );
            let (input, output) = self.endpoints(device)?;
            let mut handle = ptr::null_mut();
            success(
                // SAFETY: The device list retains device, and handle is a writable pointer slot.
                unsafe { (self.open)(device, &mut handle) },
                "打开 IS903 USB 设备",
            )?;
            let mut drive = Drive {
                context,
                handle: NonNull::new(handle).context("libusb 未返回设备句柄")?,
                input,
                output,
                tag: Cell::new(0),
                claimed: false,
            };
            let mut serial = [0; 256];
            // SAFETY: The open handle is live; the mutable buffer is writable for the specified 256 bytes.
            let got = unsafe {
                (self.get_string_descriptor_ascii)(
                    handle,
                    descriptor.serial,
                    serial.as_mut_ptr(),
                    serial.len() as c_int,
                )
            };
            ensure!(
                got >= 0 && serial.get(..got as usize) == Some(target.serial.as_bytes()),
                "USB 序列号与已认证设备不一致"
            );
            success(
                // SAFETY: The handle is open and the argument is an allowed enable flag.
                unsafe { (self.set_auto_detach_kernel_driver)(handle, 1) },
                "设置 USB 接管",
            )?;
            success(
                // SAFETY: The handle refers to the verified USB device with interface zero.
                unsafe { (self.claim_interface)(handle, 0) },
                "接管 IS903 USB 接口",
            )?;
            drive.claimed = true;
            return Ok(drive);
        }
        bail!("指定的 IS903 USB 会话已消失")
    }
}

// Only descriptors supplied by libusb are passed here. Empty arrays may be null.
fn descriptor_slice<'a, T>(pointer: *const T, count: usize) -> Result<&'a [T]> {
    if count == 0 {
        return Ok(&[]);
    }
    ensure!(!pointer.is_null(), "libusb 描述符缺失");
    // SAFETY: The libusb descriptor array has count initialized elements and stays alive for its consumers.
    Ok(unsafe { std::slice::from_raw_parts(pointer, count) })
}

pub(super) struct Drive<'a> {
    context: UsbContext<'a>,
    handle: NonNull<c_void>,
    input: u8,
    output: u8,
    tag: Cell<u32>,
    claimed: bool,
}

impl Drive<'_> {
    pub(super) fn release(mut self) -> Result<()> {
        self.release_interface()
    }

    fn release_interface(&mut self) -> Result<()> {
        if self.claimed {
            self.claimed = false;
            success(
                // SAFETY: The live handle has a successful claim on interface zero.
                unsafe { (self.context.lib.release_interface)(self.handle.as_ptr(), 0) },
                "释放 IS903 USB 接口",
            )?;
        }
        Ok(())
    }

    fn bulk(&self, endpoint: u8, data: &mut [u8]) -> Result<()> {
        let mut transferred = 0;
        let length = c_int::try_from(data.len())?;
        // SAFETY: The handle and endpoint belong to this claim; data and transferred are live writable buffers.
        let status = unsafe {
            (self.context.lib.bulk_transfer)(
                self.handle.as_ptr(),
                endpoint,
                data.as_mut_ptr(),
                length,
                &mut transferred,
                30_000,
            )
        };
        success(status, "IS903 USB 数据传输")?;
        ensure!(transferred == length, "IS903 USB 数据传输不完整");
        Ok(())
    }
}

impl Drop for Drive<'_> {
    fn drop(&mut self) {
        let _ = self.release_interface();
        // SAFETY: The retained handle came from libusb_open and is closed once after releasing its claim.
        unsafe {
            (self.context.lib.close)(self.handle.as_ptr());
        }
    }
}

impl Scsi for Drive<'_> {
    fn transfer(&self, command: &[u8], input: bool, data: &mut [u8]) -> Result<()> {
        ensure!((1..=16).contains(&command.len()), "IS903 USB 命令长度无效");
        let tag = self.tag.get().wrapping_add(1);
        self.tag.set(tag);
        let mut wrapper = [0; 31];
        wrapper[..4].copy_from_slice(b"USBC");
        wrapper[4..8].copy_from_slice(&tag.to_le_bytes());
        wrapper[8..12].copy_from_slice(&u32::try_from(data.len())?.to_le_bytes());
        wrapper[12] = if input { 0x80 } else { 0 };
        wrapper[14] = command.len() as u8;
        wrapper[15..15 + command.len()].copy_from_slice(command);
        self.bulk(self.output, &mut wrapper)?;
        self.bulk(if input { self.input } else { self.output }, data)?;
        let mut status = [0; 13];
        self.bulk(self.input, &mut status)?;
        ensure!(
            &status[..4] == b"USBS" && status[4..8] == tag.to_le_bytes() && status[8..] == [0; 5],
            "IS903 USB 命令 0x{:02x} 状态无效",
            command[0]
        );
        Ok(())
    }
}
