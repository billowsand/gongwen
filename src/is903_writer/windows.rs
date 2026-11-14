use super::{Scsi, Target, burn, valid_serial, verify};
use anyhow::{Context, Result, ensure};
use std::ffi::c_void;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::mem::{offset_of, size_of, size_of_val};
use std::ptr::{self, addr_of_mut};
use std::time::{Duration, Instant};
use windows_sys::Win32::{
    Devices::{
        DeviceAndDriverInstallation::{
            CM_DEVCAP_UNIQUEID, CM_DRP_CAPABILITIES, CM_DRP_HARDWAREID,
            CM_GET_DEVICE_INTERFACE_LIST_PRESENT, CM_Get_DevNode_PropertyW,
            CM_Get_DevNode_Registry_PropertyW, CM_Get_Device_IDW,
            CM_Get_Device_Interface_List_SizeW, CM_Get_Device_Interface_ListW,
            CM_Get_Device_Interface_PropertyW, CM_Get_Parent, CM_Locate_DevNodeW, CR_SUCCESS,
        },
        Properties::{
            DEVPKEY_Device_InstanceId, DEVPKEY_Device_LastArrivalDate, DEVPROP_TYPE_FILETIME,
            DEVPROP_TYPE_STRING,
        },
    },
    Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE},
    Storage::{
        FileSystem::{
            CreateFileW, FILE_DEVICE_CD_ROM, FILE_SHARE_READ, FILE_SHARE_WRITE, GetDriveTypeW,
            GetLogicalDrives, OPEN_EXISTING,
        },
        IscsiDisc::{IOCTL_SCSI_PASS_THROUGH_DIRECT, SCSI_PASS_THROUGH_DIRECT},
    },
    System::{
        IO::DeviceIoControl,
        Ioctl::{
            FSCTL_DISMOUNT_VOLUME, FSCTL_LOCK_VOLUME, FSCTL_UNLOCK_VOLUME, GUID_DEVINTERFACE_CDROM,
            IOCTL_STORAGE_GET_DEVICE_NUMBER, STORAGE_DEVICE_NUMBER,
        },
    },
};

const DRIVE_CDROM: u32 = 5;

struct Drive(HANDLE);

impl Drive {
    fn open(path: &[u16], access: u32) -> Result<Self> {
        // SAFETY: The path is a NUL-terminated UTF-16 buffer; optional pointers are null.
        let handle = unsafe {
            CreateFileW(
                path.as_ptr(),
                access,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                ptr::null(),
                OPEN_EXISTING,
                0,
                ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error()).context("无法打开 IS903 光驱");
        }
        Ok(Self(handle))
    }

    fn volume(disk: &str, access: u32) -> Result<Self> {
        ensure!(
            disk.len() == 2 && disk.as_bytes()[0].is_ascii_uppercase() && disk.ends_with(':'),
            "Windows 光驱盘符无效"
        );
        Self::open(&wide(&format!(r"\\.\{disk}")), access)
    }

    fn ioctl(
        &self,
        code: u32,
        input: *const c_void,
        input_len: u32,
        output: *mut c_void,
        output_len: u32,
    ) -> Result<u32> {
        let mut returned = 0;
        // SAFETY: The volume handle is live; the caller supplied the command-specific buffers and exact lengths.
        let result = unsafe {
            DeviceIoControl(
                self.0,
                code,
                input,
                input_len,
                output,
                output_len,
                &mut returned,
                ptr::null_mut(),
            )
        };
        if result == 0 {
            return Err(std::io::Error::last_os_error())
                .with_context(|| format!("光驱控制 {code:#x} 失败"));
        }
        Ok(returned)
    }

    fn control(&self, code: u32) -> Result<()> {
        self.ioctl(code, ptr::null(), 0, ptr::null_mut(), 0)?;
        Ok(())
    }

    fn number(&self) -> Result<u32> {
        let mut number = STORAGE_DEVICE_NUMBER::default();
        let size = size_of_val(&number) as u32;
        let received = self.ioctl(
            IOCTL_STORAGE_GET_DEVICE_NUMBER,
            ptr::null(),
            0,
            addr_of_mut!(number).cast(),
            size,
        )?;
        ensure!(
            received >= size && number.DeviceType == FILE_DEVICE_CD_ROM,
            "设备不是 CD-ROM"
        );
        Ok(number.DeviceNumber)
    }

    fn dismount(&self) -> Result<()> {
        self.control(FSCTL_LOCK_VOLUME)?;
        let result = self.control(FSCTL_DISMOUNT_VOLUME);
        let release = self.control(FSCTL_UNLOCK_VOLUME);
        result?;
        release
    }
}

impl Drop for Drive {
    fn drop(&mut self) {
        // SAFETY: This object exclusively owns the live handle returned by CreateFileW.
        unsafe {
            CloseHandle(self.0);
        }
    }
}

#[repr(C)]
struct Packet {
    request: SCSI_PASS_THROUGH_DIRECT,
    sense: [u8; 32],
}

impl Scsi for Drive {
    fn transfer(&self, command: &[u8], input: bool, data: &mut [u8]) -> Result<()> {
        let length = u32::try_from(data.len())?;
        let mut packet = Packet {
            request: SCSI_PASS_THROUGH_DIRECT {
                Length: size_of::<SCSI_PASS_THROUGH_DIRECT>() as u16,
                CdbLength: u8::try_from(command.len())?,
                SenseInfoLength: 32,
                DataIn: u8::from(input),
                DataTransferLength: length,
                TimeOutValue: 30,
                DataBuffer: data.as_mut_ptr().cast(),
                SenseInfoOffset: offset_of!(Packet, sense) as u32,
                ..Default::default()
            },
            sense: [0; 32],
        };
        packet.request.Cdb[..command.len()].copy_from_slice(command);
        let raw = addr_of_mut!(packet).cast::<c_void>();
        let size = size_of::<Packet>() as u32;
        let returned = self.ioctl(IOCTL_SCSI_PASS_THROUGH_DIRECT, raw, size, raw, size)?;
        ensure!(
            returned >= u32::from(packet.request.Length)
                && packet.request.ScsiStatus == 0
                && packet.request.DataTransferLength == length,
            "IS903 SCSI 命令 {:02x} 失败或传输不完整（状态 {:02x}）",
            command[0],
            packet.request.ScsiStatus
        );
        Ok(())
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn interface_paths() -> Result<Vec<Vec<u16>>> {
    let mut len = 0;
    // SAFETY: The class GUID and writable length output are valid; null device ID requests all CD interfaces.
    let mut status = unsafe {
        CM_Get_Device_Interface_List_SizeW(
            &mut len,
            &GUID_DEVINTERFACE_CDROM,
            ptr::null(),
            CM_GET_DEVICE_INTERFACE_LIST_PRESENT,
        )
    };
    ensure!(
        status == CR_SUCCESS && len <= 64 * 1024,
        "无法读取 Windows 光驱列表（{status}）"
    );
    if len <= 1 {
        return Ok(Vec::new());
    }
    let mut result = vec![0; len as usize];
    // SAFETY: The buffer contains len writable u16 elements, and the GUID remains alive for the call.
    status = unsafe {
        CM_Get_Device_Interface_ListW(
            &GUID_DEVINTERFACE_CDROM,
            ptr::null(),
            result.as_mut_ptr(),
            len,
            CM_GET_DEVICE_INTERFACE_LIST_PRESENT,
        )
    };
    ensure!(
        status == CR_SUCCESS,
        "无法读取 Windows 光驱列表（{status}）"
    );
    Ok(result
        .split(|&unit| unit == 0)
        .take_while(|entry| !entry.is_empty())
        .map(|entry| entry.iter().copied().chain(std::iter::once(0)).collect())
        .collect())
}

fn instance(path: &[u16]) -> Option<u32> {
    let mut id = [0u16; 512];
    let mut property_type = 0;
    let mut length = size_of_val(&id) as u32;
    // SAFETY: path is NUL-terminated UTF-16 and id holds the advertised number of writable bytes.
    let status = unsafe {
        CM_Get_Device_Interface_PropertyW(
            path.as_ptr(),
            &DEVPKEY_Device_InstanceId,
            &mut property_type,
            id.as_mut_ptr().cast(),
            &mut length,
            0,
        )
    };
    if status != CR_SUCCESS || property_type != DEVPROP_TYPE_STRING {
        return None;
    }
    let mut node = 0;
    // SAFETY: The successful string lookup left a NUL-terminated UTF-16 instance ID and node is writable.
    (unsafe { CM_Locate_DevNodeW(&mut node, id.as_ptr(), 0) } == CR_SUCCESS).then_some(node)
}

fn usb_parent(mut node: u32) -> Option<u32> {
    for _ in 0..12 {
        let mut id = [0u16; 512];
        // SAFETY: id has the number of writable UTF-16 elements passed to Configuration Manager.
        if unsafe { CM_Get_Device_IDW(node, id.as_mut_ptr(), id.len() as u32, 0) } != CR_SUCCESS {
            return None;
        }
        let instance = String::from_utf16(id.split(|&c| c == 0).next()?)
            .ok()?
            .to_ascii_uppercase();
        let mut parts = instance.split('\\');
        if parts.next() == Some("USB") && parts.next() == Some("VID_1F75&PID_0903") {
            return Some(node);
        }
        let mut parent = 0;
        // SAFETY: parent is writable storage for one devnode value.
        if unsafe { CM_Get_Parent(&mut parent, node, 0) } != CR_SUCCESS {
            break;
        }
        node = parent;
    }
    None
}

fn identity(node: u32) -> Option<(String, u64, u64)> {
    let mut capabilities = 0u32;
    let mut amount = size_of_val(&capabilities) as u32;
    // SAFETY: capabilities is writable and amount gives the size of its u32 storage.
    let unique = unsafe {
        CM_Get_DevNode_Registry_PropertyW(
            node,
            CM_DRP_CAPABILITIES,
            ptr::null_mut(),
            addr_of_mut!(capabilities).cast(),
            &mut amount,
            0,
        )
    };
    if unique != CR_SUCCESS || capabilities & CM_DEVCAP_UNIQUEID == 0 {
        return None;
    }
    let mut id = [0u16; 512];
    // SAFETY: id has the number of writable UTF-16 elements passed to Configuration Manager.
    if unsafe { CM_Get_Device_IDW(node, id.as_mut_ptr(), id.len() as u32, 0) } != CR_SUCCESS {
        return None;
    }
    let instance = String::from_utf16(id.split(|&c| c == 0).next()?).ok()?;
    let serial = instance.rsplit('\\').next()?.to_owned();
    if !valid_serial(&serial) {
        return None;
    }
    let mut hardware = [0u16; 512];
    let mut amount = size_of_val(&hardware) as u32;
    // SAFETY: hardware has the number of writable bytes advertised through amount.
    let id_status = unsafe {
        CM_Get_DevNode_Registry_PropertyW(
            node,
            CM_DRP_HARDWAREID,
            ptr::null_mut(),
            hardware.as_mut_ptr().cast(),
            &mut amount,
            0,
        )
    };
    if id_status != CR_SUCCESS {
        return None;
    }
    let revision = hardware
        .split(|&c| c == 0)
        .filter_map(|id| {
            let id = String::from_utf16(id).ok()?.to_ascii_uppercase();
            u64::from_str_radix(id.strip_prefix(r"USB\VID_1F75&PID_0903&REV_")?, 16).ok()
        })
        .next()?;
    let mut arrived = 0u64;
    let mut amount = size_of_val(&arrived) as u32;
    let mut property_type = 0;
    // SAFETY: The property key is static and arrived is writable storage for the advertised 64-bit FILETIME.
    let arrival_status = unsafe {
        CM_Get_DevNode_PropertyW(
            node,
            &DEVPKEY_Device_LastArrivalDate,
            &mut property_type,
            addr_of_mut!(arrived).cast(),
            &mut amount,
            0,
        )
    };
    // Require a connection generation so a reconnect between selection and write
    // does not authorize a newly bound volume, even if PnP reuses its devnode.
    if arrival_status != CR_SUCCESS || property_type != DEVPROP_TYPE_FILETIME || arrived == 0 {
        return None;
    }
    Some((serial, revision, arrived))
}

fn connections() -> Result<Vec<(Target, u32)>> {
    // SAFETY: GetLogicalDrives has no arguments or preconditions.
    let mask = unsafe { GetLogicalDrives() };
    let letters: Vec<_> = (0..26)
        .filter(|index| mask & (1 << index) != 0)
        .filter_map(|index| {
            let letter = (b'A' + index) as char;
            let disk = format!("{letter}:");
            let root = wide(&format!("{disk}\\"));
            // SAFETY: root is a live NUL-terminated UTF-16 root path.
            if unsafe { GetDriveTypeW(root.as_ptr()) } != DRIVE_CDROM {
                return None;
            }
            let number = Drive::volume(&disk, 0).ok()?.number().ok()?;
            Some((disk, number))
        })
        .collect();
    let mut devices = Vec::new();
    for path in interface_paths()? {
        let Some(parent) = instance(&path).and_then(usb_parent) else {
            continue;
        };
        let Some((serial, revision, session)) = identity(parent) else {
            continue;
        };
        let Some(number) = Drive::open(&path, 0)
            .ok()
            .and_then(|drive| drive.number().ok())
        else {
            continue;
        };
        for (disk, _) in letters.iter().filter(|(_, candidate)| *candidate == number) {
            let mut location = DefaultHasher::new();
            path.hash(&mut location);
            number.hash(&mut location);
            devices.push((
                Target {
                    serial: serial.clone(),
                    disk: disk.clone(),
                    session,
                    location: location.finish(),
                    revision,
                },
                number,
            ));
        }
    }
    devices.sort_by(|(first, _), (second, _)| first.disk.cmp(&second.disk));
    Ok(devices)
}

pub(super) fn scan() -> Result<Vec<Target>> {
    Ok(connections()?
        .into_iter()
        .map(|(target, _)| target)
        .collect())
}

fn recheck_open_volume(drive: &Drive, target: &Target) -> Result<()> {
    let number = drive.number()?;
    let connections = connections()?;
    let mut matching = connections
        .iter()
        .filter(|(device, _)| device.serial == target.serial);
    ensure!(
        matching
            .next()
            .is_some_and(|(device, expected)| device == target && *expected == number)
            && matching.next().is_none(),
        "打开的光驱与已认证的 IS903 不一致"
    );
    Ok(())
}

/// 写入和收尾命令已经成功后，介质会变成只读，IS903 也可能重新枚举。
/// 回读校验先试旧句柄，失败再等新的连接并用只读句柄重试；不能因为
/// 写后的卷卸载/重新枚举失败就把已经写好的盘报成「刻录失败」。
fn verify_after_burn(target: &Target, drive: Drive, image: &[u8]) -> Result<()> {
    // 介质已封盘时锁卷/卸载可能直接失败；它只影响卷缓存，不影响 SCSI 回读。
    let _ = drive.dismount();
    let old_error = if recheck_open_volume(&drive, target).is_ok() {
        verify(&drive, image).err()
    } else {
        None
    };
    drop(drive);

    let mut errors = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        match connections() {
            Ok(devices) => {
                let mut matching = devices
                    .iter()
                    .filter(|(device, _)| device.serial == target.serial);
                let candidate = matching.next();
                if candidate.is_some() && matching.next().is_some() {
                    errors.push(anyhow::anyhow!("IS903 序列号冲突"));
                } else if let Some((device, _)) = candidate {
                    if device.revision != target.revision {
                        errors.push(anyhow::anyhow!("刷新后的 IS903 版本不一致"));
                    } else {
                        match Drive::volume(&device.disk, GENERIC_READ) {
                            Ok(drive) => match recheck_open_volume(&drive, device) {
                                Ok(()) => match verify(&drive, image) {
                                    Ok(()) => return Ok(()),
                                    Err(error) => errors.push(error),
                                },
                                Err(error) => errors.push(error),
                            },
                            Err(error) => errors.push(error),
                        }
                    }
                } else {
                    errors.push(anyhow::anyhow!("未找到 IS903 光驱"));
                }
            }
            Err(error) => errors.push(error),
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    Err(errors
        .pop()
        .or(old_error)
        .unwrap_or_else(|| anyhow::anyhow!("等待 IS903 光盘重新枚举超时")))
    .context("IS903 已写入，但回读校验未通过；介质可能已有明文")
}

pub(super) fn write(
    target: &Target,
    image: &[u8],
    authorize: &dyn Fn() -> Result<()>,
) -> Result<()> {
    static WRITE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _serial = WRITE_LOCK
        .try_lock()
        .map_err(|_| anyhow::anyhow!("已有一项 IS903 刻录正在进行"))?;
    let drive = Drive::volume(&target.disk, GENERIC_READ | GENERIC_WRITE)?;
    recheck_open_volume(&drive, target)?;
    drive
        .dismount()
        .context("无法卸载 IS903，请关闭正在使用光盘的程序")?;
    burn(&drive, image, &|| {
        recheck_open_volume(&drive, target)?;
        authorize()
    })?;
    verify_after_burn(target, drive, image)
}
