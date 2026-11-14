use std::collections::BTreeMap;
use std::ffi::{CString, c_char, c_void};
use std::ptr;
use std::sync::Mutex;

use anyhow::{Context, Result, ensure};

use super::{Drive, Media, SECTOR_SIZE};

type Cf = *const c_void;
type Device = Cf;
type Track = Cf;
type Burn = Cf;
type OsStatus = i32;

const CF_UTF8: u32 = 0x0800_0100;
const CF_SIGNED_64: isize = 4;
const INVALID_DATA: OsStatus = 0x8002_0063u32 as i32;
const UNSUPPORTED: OsStatus = 0x8002_0067u32 as i32;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(value: Cf);
    fn CFArrayGetCount(array: Cf) -> isize;
    fn CFArrayGetValueAtIndex(array: Cf, index: isize) -> Cf;
    fn CFDictionaryGetValue(dict: Cf, key: Cf) -> Cf;
    fn CFDictionaryCreate(
        allocator: Cf,
        keys: *const Cf,
        values: *const Cf,
        count: isize,
        key_callbacks: Cf,
        value_callbacks: Cf,
    ) -> Cf;
    fn CFNumberCreate(allocator: Cf, kind: isize, value: Cf) -> Cf;
    fn CFNumberGetValue(number: Cf, kind: isize, value: *mut i64) -> u8;
    fn CFStringGetCString(string: Cf, buffer: *mut c_char, length: isize, encoding: u32) -> u8;
    fn CFEqual(left: Cf, right: Cf) -> u8;
    static kCFBooleanTrue: Cf;
    static kCFBooleanFalse: Cf;
    // Only their addresses are passed to CFDictionaryCreate.
    static kCFTypeDictionaryKeyCallBacks: u8;
    static kCFTypeDictionaryValueCallBacks: u8;
}

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IORegistryEntryFromPath(port: u32, path: *const c_char) -> u32;
    fn IORegistryEntryGetRegistryEntryID(entry: u32, id: *mut u64) -> i32;
    fn IOObjectRelease(entry: u32) -> i32;
}

#[link(name = "DiscRecording", kind = "framework")]
unsafe extern "C" {
    fn DRCopyDeviceArray() -> Cf;
    fn DRDeviceCopyInfo(device: Device) -> Cf;
    fn DRDeviceCopyStatus(device: Device) -> Cf;
    fn DRDeviceAcquireExclusiveAccess(device: Device) -> OsStatus;
    fn DRDeviceReleaseExclusiveAccess(device: Device);
    fn DRBurnCreate(device: Device) -> Burn;
    fn DRBurnSetProperties(burn: Burn, properties: Cf);
    fn DRBurnWriteLayout(burn: Burn, track: Track) -> OsStatus;
    fn DRBurnCopyStatus(burn: Burn) -> Cf;
    fn DRTrackCreate(
        properties: Cf,
        callback: unsafe extern "C" fn(Track, u32, *mut c_void) -> OsStatus,
    ) -> Track;

    static kDRDeviceIORegistryEntryPathKey: Cf;
    static kDRDeviceProductNameKey: Cf;
    static kDRDeviceVendorNameKey: Cf;
    static kDRDeviceMediaInfoKey: Cf;
    static kDRDeviceMediaTypeKey: Cf;
    static kDRDeviceMediaIsBlankKey: Cf;
    static kDRDeviceMediaBlocksFreeKey: Cf;
    static kDRDeviceMediaTypeCDR: Cf;
    static kDRDeviceMediaTypeDVDR: Cf;
    static kDRDeviceMediaTypeDVDPlusR: Cf;
    static kDRTrackLengthKey: Cf;
    static kDRBlockSizeKey: Cf;
    static kDRBlockTypeKey: Cf;
    static kDRDataFormKey: Cf;
    static kDRSessionFormatKey: Cf;
    static kDRTrackModeKey: Cf;
    static kDRVerificationTypeKey: Cf;
    static kDRVerificationTypeReceiveData: Cf;
    static kDRBurnAppendableKey: Cf;
    static kDRBurnOverwriteDiscKey: Cf;
    static kDRBurnVerifyDiscKey: Cf;
    static kDRBurnFailureActionKey: Cf;
    static kDRBurnFailureActionNone: Cf;
    static kDRSynchronousBehaviorKey: Cf;
    static kDRStatusStateKey: Cf;
    static kDRStatusStateDone: Cf;
}

struct Owned(Cf);

impl Owned {
    fn from_copy(value: Cf) -> Option<Self> {
        (!value.is_null()).then_some(Self(value))
    }

    fn number(value: i64) -> Result<Self> {
        // SAFETY: CF copies one i64, matching kCFNumberSInt64Type.
        Self::from_copy(unsafe {
            CFNumberCreate(ptr::null(), CF_SIGNED_64, ptr::from_ref(&value).cast())
        })
        .context("无法分配刻录数字属性")
    }

    fn dict<const N: usize>(keys: [Cf; N], values: [Cf; N]) -> Result<Self> {
        // SAFETY: Both arrays contain N live CoreFoundation references and CF retains them.
        Self::from_copy(unsafe {
            CFDictionaryCreate(
                ptr::null(),
                keys.as_ptr(),
                values.as_ptr(),
                N as isize,
                ptr::addr_of!(kCFTypeDictionaryKeyCallBacks).cast(),
                ptr::addr_of!(kCFTypeDictionaryValueCallBacks).cast(),
            )
        })
        .context("无法分配刻录属性表")
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: self owns one Create/Copy reference and has not released it.
        unsafe { CFRelease(self.0) }
    }
}

fn equal(value: Cf, expected: Cf) -> bool {
    // SAFETY: Callers pass a borrowed CF value or NULL and a live expected constant.
    !value.is_null() && unsafe { CFEqual(value, expected) != 0 }
}

fn cf_text(value: Cf) -> Option<String> {
    let mut bytes = [0u8; 4096];
    if value.is_null()
        // SAFETY: For a valid CFString the call writes at most the supplied array length.
        || unsafe {
            CFStringGetCString(
                value,
                bytes.as_mut_ptr().cast(),
                bytes.len() as isize,
                CF_UTF8,
            ) == 0
        }
    {
        return None;
    }
    let end = bytes.iter().position(|byte| *byte == 0)?;
    std::str::from_utf8(&bytes[..end]).ok().map(str::to_owned)
}

fn registry_id(path: &str) -> Option<u64> {
    let path = CString::new(path).ok()?;
    // SAFETY: The path is NUL-terminated; the returned object is released exactly once.
    let entry = unsafe { IORegistryEntryFromPath(0, path.as_ptr()) };
    if entry == 0 {
        return None;
    }
    let mut id = 0;
    // SAFETY: entry is a live IORegistry object and id has the expected uint64 layout.
    let status = unsafe { IORegistryEntryGetRegistryEntryID(entry, &mut id) };
    // SAFETY: The local entry reference is owned and has not been released.
    unsafe { IOObjectRelease(entry) };
    (status == 0 && id != 0).then_some(id)
}

fn inspect(device: Device) -> Option<Drive> {
    // SAFETY: The caller retains the device array while this reference is in use. Types
    // of the looked-up values follow the DiscRecording SDK dictionaries.
    unsafe {
        let info = Owned::from_copy(DRDeviceCopyInfo(device))?;
        let status = Owned::from_copy(DRDeviceCopyStatus(device))?;
        let id = cf_text(CFDictionaryGetValue(
            info.0,
            kDRDeviceIORegistryEntryPathKey,
        ))?;
        let connection = registry_id(&id)?;
        let media_info = CFDictionaryGetValue(status.0, kDRDeviceMediaInfoKey);
        if media_info.is_null()
            || !equal(
                CFDictionaryGetValue(media_info, kDRDeviceMediaIsBlankKey),
                kCFBooleanTrue,
            )
        {
            return None;
        }
        let media_type = CFDictionaryGetValue(media_info, kDRDeviceMediaTypeKey);
        let media = [
            (kDRDeviceMediaTypeCDR, Media::CdR),
            (kDRDeviceMediaTypeDVDR, Media::DvdR),
            (kDRDeviceMediaTypeDVDPlusR, Media::DvdPlusR),
        ]
        .into_iter()
        .find_map(|(value, media)| equal(media_type, value).then_some(media))?;
        let free = CFDictionaryGetValue(media_info, kDRDeviceMediaBlocksFreeKey);
        let mut sectors = 0;
        if free.is_null() || CFNumberGetValue(free, CF_SIGNED_64, &mut sectors) == 0 || sectors <= 0
        {
            return None;
        }
        let vendor =
            cf_text(CFDictionaryGetValue(info.0, kDRDeviceVendorNameKey)).unwrap_or_default();
        let product =
            cf_text(CFDictionaryGetValue(info.0, kDRDeviceProductNameKey)).unwrap_or_default();
        Some(Drive {
            label: format!(
                "{} {} · {} · {:x}",
                vendor.trim(),
                product.trim(),
                media.label(),
                connection
            ),
            id,
            connection,
            media,
            sectors: sectors as u64,
        })
    }
}

fn visit<T>(mut visitor: impl FnMut(Device, Drive) -> Result<Option<T>>) -> Result<Option<T>> {
    // SAFETY: DRCopyDeviceArray returns owned CFArray whose entries live through visitor.
    let devices = Owned::from_copy(unsafe { DRCopyDeviceArray() }).context("无法枚举光盘刻录机")?;
    // SAFETY: devices owns a live CFArray throughout this loop.
    for index in 0..unsafe { CFArrayGetCount(devices.0) } {
        // SAFETY: index is within the array and devices keeps the element live.
        let device = unsafe { CFArrayGetValueAtIndex(devices.0, index) };
        if let Some(drive) = inspect(device)
            && let Some(result) = visitor(device, drive)?
        {
            return Ok(Some(result));
        }
    }
    Ok(None)
}

pub(super) fn scan() -> Result<Vec<Drive>> {
    let mut drives = Vec::new();
    visit::<()>(|_, drive| {
        drives.push(drive);
        Ok(None)
    })?;
    Ok(drives)
}

struct Exclusive(Device);

impl Drop for Exclusive {
    fn drop(&mut self) {
        // SAFETY: This guard is created only after successful acquisition of a live device.
        unsafe { DRDeviceReleaseExclusiveAccess(self.0) }
    }
}

pub(super) fn burn(target: &Drive, iso: &[u8], authorize: &dyn Fn() -> Result<()>) -> Result<()> {
    visit(|device, drive| {
        if drive != *target {
            return Ok(None);
        }
        // SAFETY: visit retains the device object; a successful acquisition is guarded below.
        let status = unsafe { DRDeviceAcquireExclusiveAccess(device) };
        ensure!(status == 0, "无法独占刻录机（{status:#x}）");
        let _lock = Exclusive(device);
        write(device, target, iso, authorize)?;
        Ok(Some(()))
    })?
    .context("找不到之前选定且装有空白盘的刻录机")
}

fn write(
    device: Device,
    target: &Drive,
    iso: &[u8],
    authorize: &dyn Fn() -> Result<()>,
) -> Result<()> {
    let blocks = Owned::number((iso.len() / SECTOR_SIZE) as i64)?;
    let block_size = Owned::number(SECTOR_SIZE as i64)?;
    // Standard Mode 1 data blocks from DRCoreTrack.h. DVD data has a distinct track mode.
    let block_type = Owned::number(8)?;
    let data_form = Owned::number(16)?;
    let session_format = Owned::number(0)?;
    let track_mode = Owned::number(if target.media == Media::CdR { 4 } else { 5 })?;
    // SAFETY: The framework constants are CFString keys and all numeric values are live.
    let track_props = unsafe {
        Owned::dict(
            [
                kDRTrackLengthKey,
                kDRBlockSizeKey,
                kDRBlockTypeKey,
                kDRDataFormKey,
                kDRSessionFormatKey,
                kDRTrackModeKey,
                kDRVerificationTypeKey,
            ],
            [
                blocks.0,
                block_size.0,
                block_type.0,
                data_form.0,
                session_format.0,
                track_mode.0,
                kDRVerificationTypeReceiveData,
            ],
        )?
    };
    // SAFETY: The properties are a valid dictionary and produce has the SDK callback ABI.
    let track = Owned::from_copy(unsafe { DRTrackCreate(track_props.0, produce) })
        .context("无法创建数据轨")?;
    // SAFETY: The caller keeps its borrowed device live for this entire synchronous burn.
    let burn = Owned::from_copy(unsafe { DRBurnCreate(device) }).context("无法启动刻录")?;
    // SAFETY: The framework constants and CFBoolean values are live CF references.
    let burn_props = unsafe {
        Owned::dict(
            [
                kDRBurnAppendableKey,
                kDRBurnOverwriteDiscKey,
                kDRBurnVerifyDiscKey,
                kDRSynchronousBehaviorKey,
                kDRBurnFailureActionKey,
            ],
            [
                kCFBooleanFalse,
                kCFBooleanFalse,
                kCFBooleanTrue,
                kCFBooleanTrue,
                kDRBurnFailureActionNone,
            ],
        )?
    };
    // SAFETY: The burn and the burn properties dictionary are both valid owned CF objects.
    unsafe { DRBurnSetProperties(burn.0, burn_props.0) };
    let source = SourceGuard::register(track.0, iso)?;
    ensure!(
        inspect(device).as_ref() == Some(target),
        "刻录机或空白盘状态已变化"
    );
    authorize()?;
    // SAFETY: Synchronous burn is configured above. The SourceGuard keeps the borrowed
    // image live and available to all callbacks until the framework has returned.
    let result = unsafe { DRBurnWriteLayout(burn.0, track.0) };
    ensure!(
        result == 0,
        "光盘刻录失败（{result:#x}），请将此光盘按可能含明文处置"
    );
    let status =
        // SAFETY: burn is still a live framework burn object.
        Owned::from_copy(unsafe { DRBurnCopyStatus(burn.0) }).context("无法取得刻录结果")?;
    ensure!(
        // SAFETY: status is a valid burn status dictionary and both keys are SDK constants.
        unsafe {
            equal(
                CFDictionaryGetValue(status.0, kDRStatusStateKey),
                kDRStatusStateDone,
            )
        },
        "光盘刻录或回读未成功，请将此光盘按可能含明文处置"
    );
    ensure!(
        source.verified() == Some(iso.len()),
        "刻录机未完整回读镜像，请将此光盘按可能含明文处置"
    );
    Ok(())
}

#[repr(C)]
struct ProductionInfo {
    buffer: *mut u8,
    requested: u32,
    actual: u32,
    flags: u32,
    block_size: u32,
    address: u64,
}

struct Source {
    address: usize,
    length: usize,
    verified: usize,
}

static SOURCES: Mutex<BTreeMap<usize, Source>> = Mutex::new(BTreeMap::new());

struct SourceGuard<'a> {
    track: usize,
    _iso: &'a [u8],
}

impl<'a> SourceGuard<'a> {
    fn register(track: Track, iso: &'a [u8]) -> Result<Self> {
        let track = track.addr();
        let mut sources = SOURCES
            .lock()
            .map_err(|_| anyhow::anyhow!("数据轨状态不可用"))?;
        ensure!(!sources.contains_key(&track), "数据轨已在使用");
        sources.insert(
            track,
            Source {
                address: iso.as_ptr().addr(),
                length: iso.len(),
                verified: 0,
            },
        );
        // The callback reconstructs the pointer using its exposed provenance.
        iso.as_ptr().expose_provenance();
        Ok(Self { track, _iso: iso })
    }

    fn verified(&self) -> Option<usize> {
        Some(SOURCES.lock().ok()?.get(&self.track)?.verified)
    }
}

impl Drop for SourceGuard<'_> {
    fn drop(&mut self) {
        SOURCES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.track);
    }
}

unsafe extern "C" fn produce(track: Track, message: u32, param: *mut c_void) -> OsStatus {
    let Ok(mut sources) = SOURCES.lock() else {
        return INVALID_DATA;
    };
    let Some(source) = sources.get_mut(&track.addr()) else {
        return INVALID_DATA;
    };
    match &message.to_be_bytes() {
        b"pre " | b"vdon" | b"post" => 0,
        b"vstr" => {
            source.verified = 0;
            0
        }
        b"esti" if !param.is_null() => {
            // SAFETY: On EstimateLength, DiscRecording provides a writable UInt64.
            unsafe {
                param
                    .cast::<u64>()
                    .write((source.length / SECTOR_SIZE) as u64)
            };
            0
        }
        b"prod" | b"vrfy" | b"prpr" | b"vrpr" if !param.is_null() => {
            // SAFETY: Per DRTrackCallbackProc the parameter is a valid DRTrackProductionInfo
            // and its buffer is valid for requested bytes.
            let info = unsafe { &mut *param.cast::<ProductionInfo>() };
            if info.buffer.is_null() || info.block_size as usize != SECTOR_SIZE {
                return INVALID_DATA;
            }
            let count = info.requested as usize;
            if count == 0 {
                info.actual = 0;
                return 0;
            }
            let action = message.to_be_bytes();
            if &action == b"prpr" {
                // SAFETY: The engine provides an exclusive output buffer of requested bytes.
                unsafe { info.buffer.write_bytes(0, count) };
                info.actual = info.requested;
                return 0;
            }
            if &action == b"vrpr" {
                // SAFETY: For verify, the engine provides an initialized input buffer of requested bytes.
                return if unsafe { std::slice::from_raw_parts(info.buffer, count) }
                    .iter()
                    .all(|b| *b == 0)
                {
                    0
                } else {
                    INVALID_DATA
                };
            }
            let Ok(offset) = usize::try_from(info.address) else {
                return INVALID_DATA;
            };
            let Some(available) = source.length.checked_sub(offset) else {
                return INVALID_DATA;
            };
            if !offset.is_multiple_of(SECTOR_SIZE) || !count.is_multiple_of(SECTOR_SIZE) {
                return INVALID_DATA;
            }
            let length = available.min(count);
            // SAFETY: SourceGuard retains the immutable image until synchronous burn
            // completion. The map lock prevents concurrent removal of the source.
            let ptr = unsafe { ptr::with_exposed_provenance::<u8>(source.address).add(offset) };
            if &action == b"prod" {
                // SAFETY: The callback destination is owned by the engine and disjoint from the source.
                unsafe { ptr.copy_to_nonoverlapping(info.buffer, length) };
                info.actual = length as u32;
            } else {
                if offset != source.verified
                    || length != count
                    // SAFETY: The source guard and index checks above cover this immutable read.
                    || unsafe { std::slice::from_raw_parts(ptr, length) }
                        // SAFETY: The verified callback buffer is initialized for count bytes by the engine.
                        != unsafe { std::slice::from_raw_parts(info.buffer, length) }
                {
                    return INVALID_DATA;
                }
                source.verified += length;
            }
            0
        }
        _ => UNSUPPORTED,
    }
}
