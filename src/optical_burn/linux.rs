use std::ffi::{CStr, CString, c_char, c_int, c_uint, c_void};
use std::fs;
use std::os::fd::{AsFd, IntoRawFd};
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::ptr::{self, NonNull};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use zeroize::Zeroizing;

use super::{Drive, Media, SECTOR_SIZE};

type Handle = *mut c_void;
const DISC_BLANK: c_int = 1;
const DISC_FULL: c_int = 4;
const DRIVE_IDLE: c_int = 0;
const DRIVE_MMC: c_int = 1;
const SOURCE_OK: c_int = 0;
const MODE1: c_int = 1 << 2;
const POS_END: c_uint = 100;
const WRITE_NONE: c_int = 4;
const REASONS_LEN: usize = 4096;
static LIBBURN: Mutex<()> = Mutex::new(());

// libburn.h uses one unsigned-int allocation unit for the eleven capability bitfields.
#[repr(C)]
struct DriveInfo {
    vendor: [c_char; 9],
    product: [c_char; 17],
    revision: [c_char; 5],
    location: [c_char; 17],
    capabilities: c_uint,
    buffer_size: c_int,
    block_types: [c_int; 4],
    drive: Handle,
}

unsafe extern "C" {
    fn dlopen(name: *const c_char, flags: c_int) -> Handle;
    fn dlsym(library: Handle, name: *const c_char) -> Handle;
    fn dlclose(library: Handle) -> c_int;
}

struct Library(NonNull<c_void>);

impl Library {
    fn open() -> Result<Self> {
        // libburn uses detached workers. Keep its code mapped even after a job
        // goes idle, since a worker can still be returning through library code.
        const RTLD_NOW: c_int = 2;
        const RTLD_NODELETE: c_int = 0x1000;
        // SAFETY: The library name is a C constant.
        NonNull::new(unsafe { dlopen(c"libburn.so.4".as_ptr(), RTLD_NOW | RTLD_NODELETE) })
            .map(Self)
            .context("系统没有可用的 libburn.so.4，请安装发行版的 libburn 运行库")
    }

    fn symbol(&self, name: &CStr) -> Result<Handle> {
        // SAFETY: The name is a C string and this object keeps the library alive.
        let ptr = unsafe { dlsym(self.0.as_ptr(), name.as_ptr()) };
        ensure!(
            !ptr.is_null(),
            "libburn 缺少接口 {}",
            name.to_string_lossy()
        );
        Ok(ptr)
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        // SAFETY: The handle owns one dlopen reference and is still valid.
        unsafe { dlclose(self.0.as_ptr()) };
    }
}

// The function pointer types are from the libburn.so.4 API in libburn.h, on
// both supported 64-bit Linux architectures (including off_t = i64).
macro_rules! api {
    ($($name:ident: fn($($arg:ty),*) $(-> $out:ty)?),* $(,)?) => {
        struct Api {
            $($name: unsafe extern "C" fn($($arg),*) $(-> $out)?,)*
            _library: Library,
        }

        impl Api {
            fn open() -> Result<Self> {
                let library = Library::open()?;
                // SAFETY: Each symbol is resolved under its documented, fixed C signature.
                Ok(Self {
                    // SAFETY: The documented API gives this symbol the function signature declared below.
                    $($name: unsafe {
                        std::mem::transmute::<Handle, unsafe extern "C" fn($($arg),*) $(-> $out)?>
                            (library.symbol(CStr::from_bytes_with_nul(concat!("burn_", stringify!($name), "\0").as_bytes())?)?)
                    },)*
                    _library: library,
                })
            }
        }
    }
}

api! {
    initialize: fn() -> c_int,
    finish: fn(),
    drive_scan_and_grab: fn(*mut *mut DriveInfo, *mut c_char, c_int) -> c_int,
    drive_info_free: fn(*mut DriveInfo),
    drive_release: fn(Handle, c_int),
    drive_get_drive_role: fn(Handle) -> c_int,
    drive_get_status: fn(Handle, Handle) -> c_int,
    drive_wrote_well: fn(Handle) -> c_int,
    drive_re_assess: fn(Handle, c_int) -> c_int,
    drive_cancel: fn(Handle),
    disc_get_status: fn(Handle) -> c_int,
    disc_get_profile: fn(Handle, *mut c_int, *mut c_char) -> c_int,
    disc_available_space: fn(Handle, Handle) -> i64,
    disc_create: fn() -> Handle,
    disc_free: fn(Handle),
    disc_add_session: fn(Handle, Handle, c_uint) -> c_int,
    session_create: fn() -> Handle,
    session_free: fn(Handle),
    session_add_track: fn(Handle, Handle, c_uint) -> c_int,
    track_create: fn() -> Handle,
    track_free: fn(Handle),
    track_define_data: fn(Handle, c_int, c_int, c_int, c_int),
    track_set_source: fn(Handle, Handle) -> c_int,
    fd_source_new: fn(c_int, c_int, i64) -> Handle,
    source_free: fn(Handle),
    write_opts_new: fn(Handle) -> Handle,
    write_opts_free: fn(Handle),
    write_opts_set_multi: fn(Handle, c_int),
    write_opts_auto_write_type: fn(Handle, Handle, *mut c_char, c_int) -> c_int,
    precheck_write: fn(Handle, Handle, *mut c_char, c_int) -> c_int,
    disc_write: fn(Handle, Handle),
    read_data: fn(Handle, i64, *mut c_char, i64, *mut i64, c_int) -> c_int,
}

struct BurnLibrary(Api);

impl BurnLibrary {
    fn open() -> Result<Self> {
        let api = Api::open()?;
        // SAFETY: The Api keeps the library loaded and LIBBURN serializes initialization.
        ensure!(unsafe { (api.initialize)() } != 0, "无法初始化 libburn");
        Ok(Self(api))
    }
}

impl Drop for BurnLibrary {
    fn drop(&mut self) {
        // SAFETY: The guard owns the successful initialization and keeps the library loaded.
        unsafe { (self.0.finish)() };
    }
}

struct Owned {
    ptr: Handle,
    free: unsafe extern "C" fn(Handle),
}

impl Owned {
    fn new(ptr: Handle, free: unsafe extern "C" fn(Handle)) -> Result<Self> {
        ensure!(!ptr.is_null(), "无法初始化光盘刻录对象");
        Ok(Self { ptr, free })
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: Each object is paired with its documented destructor and freed only here.
        unsafe { (self.free)(self.ptr) };
    }
}

struct LockedDrive<'a> {
    api: &'a Api,
    info: NonNull<DriveInfo>,
    ptr: Handle,
}

impl<'a> LockedDrive<'a> {
    fn acquire(api: &'a Api, path: &str) -> Result<Self> {
        let mut name = CString::new(path)?.into_bytes_with_nul();
        let mut info = ptr::null_mut();
        // SAFETY: libburn fills the drive info array and grabs only this device.
        let status = unsafe { (api.drive_scan_and_grab)(&mut info, name.as_mut_ptr().cast(), 0) };
        ensure!(status > 0, "无法独占刻录机 {path}");
        let info = NonNull::new(info).context("无法读取刻录机信息")?;
        let drive = Self {
            api,
            info,
            // SAFETY: A successful scan returned the populated first drive-info entry.
            ptr: unsafe { info.as_ref().drive },
        };
        ensure!(!drive.ptr.is_null(), "未取得刻录机句柄");
        ensure!(
            // SAFETY: The preceding non-null check and the grabbed drive guard keep this pointer live.
            unsafe { (api.drive_get_drive_role)(drive.ptr) } == DRIVE_MMC,
            "设备不是实体光驱"
        );
        Ok(drive)
    }

    fn inspect(&self, path: &str, connection: u64) -> Result<Drive> {
        // SAFETY: The drive remains grabbed for all of these calls.
        unsafe {
            ensure!(
                (self.api.disc_get_status)(self.ptr) == DISC_BLANK,
                "请插入空白一次写入光盘"
            );
            let mut profile = 0;
            let mut profile_name = [0; 80];
            ensure!(
                (self.api.disc_get_profile)(self.ptr, &mut profile, profile_name.as_mut_ptr()) > 0,
                "无法识别光盘类型"
            );
            let media = match profile {
                0x09 => Media::CdR,
                0x11 => Media::DvdR,
                0x1b => Media::DvdPlusR,
                _ => anyhow::bail!("只接受空白 CD-R、DVD-R 或 DVD+R"),
            };
            let sectors =
                (self.api.disc_available_space)(self.ptr, ptr::null_mut()) / SECTOR_SIZE as i64;
            ensure!(sectors > 0, "无法确认空白光盘容量");
            let info = self.info.as_ref();
            Ok(Drive {
                id: path.to_owned(),
                label: format!(
                    "{} {} · {} · {path}",
                    field(&info.vendor),
                    field(&info.product),
                    media.label()
                ),
                connection,
                media,
                sectors: sectors as u64,
            })
        }
    }
}

impl Drop for LockedDrive<'_> {
    fn drop(&mut self) {
        // SAFETY: The drive was successfully grabbed, is idle, and owns this info array.
        unsafe {
            if !self.ptr.is_null() {
                (self.api.drive_release)(self.ptr, 0);
            }
            (self.api.drive_info_free)(self.info.as_ptr());
        }
    }
}

fn field(chars: &[c_char]) -> String {
    let bytes: Vec<u8> = chars
        .iter()
        .take_while(|c| **c != 0)
        .map(|c| c.to_ne_bytes()[0])
        .collect();
    String::from_utf8_lossy(&bytes).trim().to_owned()
}

fn connection(path: &Path) -> Result<u64> {
    let node = fs::symlink_metadata(path)?;
    ensure!(node.file_type().is_block_device(), "刻录目标不是块设备");
    // Linux diskseq changes whenever a previously removed drive reuses the same node.
    let name = path.file_name().context("设备路径无效")?;
    let directory = Path::new("/sys/class/block").join(name);
    let device = fs::read_to_string(directory.join("dev"))?;
    let (major, minor) = device.trim().split_once(':').context("设备号无效")?;
    let major = major.parse::<u64>()?;
    let minor = minor.parse::<u64>()?;
    let rdev = node.rdev();
    let node_major = ((rdev >> 8) & 0xfff) | ((rdev >> 32) & !0xfff);
    let node_minor = (rdev & 0xff) | ((rdev >> 12) & !0xff);
    ensure!(
        node_major == major && node_minor == minor,
        "光驱设备节点已变化"
    );
    Ok(fs::read_to_string(directory.join("diskseq"))?
        .trim()
        .parse()?)
}

pub(super) fn scan() -> Result<Vec<Drive>> {
    let _guard = LIBBURN
        .lock()
        .map_err(|_| anyhow::anyhow!("刻录状态不可用"))?;
    let library = BurnLibrary::open()?;
    let mut drives = Vec::new();
    for item in fs::read_dir("/sys/class/block")? {
        let name = item?.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name
            .strip_prefix("sr")
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        {
            continue;
        }
        let path = PathBuf::from("/dev").join(name);
        if let Some(path) = path.to_str()
            && let Ok(sequence) = connection(Path::new(path))
            && let Ok(locked) = LockedDrive::acquire(&library.0, path)
            && let Ok(drive) = locked.inspect(path, sequence)
        {
            drives.push(drive);
        }
    }
    drives.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(drives)
}

pub(super) fn burn(target: &Drive, iso: &[u8], authorize: &dyn Fn() -> Result<()>) -> Result<()> {
    let _guard = LIBBURN
        .lock()
        .map_err(|_| anyhow::anyhow!("刻录状态不可用"))?;
    let library = BurnLibrary::open()?;
    let path = Path::new(&target.id);
    ensure!(connection(path)? == target.connection, "刻录机连接已变化");
    let drive = LockedDrive::acquire(&library.0, &target.id)?;
    ensure!(
        drive.inspect(&target.id, connection(path)?)? == *target,
        "刻录机或空白盘状态已变化"
    );
    write(&drive, target, iso, authorize)
}

fn write(
    drive: &LockedDrive<'_>,
    target: &Drive,
    iso: &[u8],
    authorize: &dyn Fn() -> Result<()>,
) -> Result<()> {
    use std::io::Write;

    let api = drive.api;
    // SAFETY: All libburn objects are kept alive until the worker is idle. Libburn owns
    // the read end after successful creation and frees it with the source.
    unsafe {
        let disc = Owned::new((api.disc_create)(), api.disc_free)?;
        let session = Owned::new((api.session_create)(), api.session_free)?;
        let track = Owned::new((api.track_create)(), api.track_free)?;
        let opts = Owned::new((api.write_opts_new)(drive.ptr), api.write_opts_free)?;
        let (reader, mut writer) = std::io::pipe()?;
        // Borrowed FD stays with Rust on failure and transfers to libburn on success.
        use std::os::fd::AsRawFd;
        let source = Owned::new(
            (api.fd_source_new)(reader.as_fd().as_raw_fd(), -1, iso.len() as i64),
            api.source_free,
        )?;
        let _ = reader.into_raw_fd();
        let pad = if target.media == Media::CdR {
            300 * 1024
        } else {
            0
        };
        (api.track_define_data)(track.ptr, 0, pad, 1, MODE1);
        ensure!(
            (api.track_set_source)(track.ptr, source.ptr) == SOURCE_OK,
            "无法绑定内存镜像数据源"
        );
        ensure!(
            (api.session_add_track)(session.ptr, track.ptr, POS_END) > 0,
            "无法添加光盘数据轨"
        );
        ensure!(
            (api.disc_add_session)(disc.ptr, session.ptr, POS_END) > 0,
            "无法添加光盘会话"
        );
        (api.write_opts_set_multi)(opts.ptr, 0);
        let mut reasons = [0; REASONS_LEN];
        ensure!(
            (api.write_opts_auto_write_type)(opts.ptr, disc.ptr, reasons.as_mut_ptr(), 0)
                != WRITE_NONE,
            "刻录机不支持此盘片的单会话刻录"
        );
        ensure!(
            (api.precheck_write)(opts.ptr, disc.ptr, reasons.as_mut_ptr(), 1) > 0,
            "刻录前检查失败，光盘可能容量不足"
        );
        ensure!(
            drive.inspect(&target.id, connection(Path::new(&target.id))?)? == *target,
            "刻录机或空白盘状态已变化"
        );
        authorize()?;
        std::thread::scope(|scope| -> Result<()> {
            // The producer owns the write end. A writer blocked by lack of consumer
            // is unblocked when libburn's read FD is cancelled/closed.
            let producer = std::thread::Builder::new()
                .spawn_scoped(scope, move || writer.write_all(iso))
                .context("无法启动刻录供数任务")?;
            (api.disc_write)(opts.ptr, disc.ptr);
            let limit = Instant::now() + Duration::from_secs(60 * 60);
            let mut cancelled = false;
            while (api.drive_get_status)(drive.ptr, ptr::null_mut()) != DRIVE_IDLE {
                if !cancelled && Instant::now() >= limit {
                    (api.drive_cancel)(drive.ptr);
                    cancelled = true;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            let success = !cancelled && (api.drive_wrote_well)(drive.ptr) == 1;
            // Drop all libburn references to the pipe before joining the producer.
            drop(opts);
            drop(track);
            drop(session);
            drop(disc);
            drop(source);
            let sent = producer
                .join()
                .map_err(|_| anyhow::anyhow!("内存供数任务中断"))?;
            ensure!(
                success && sent.is_ok(),
                "光盘刻录失败，请将此光盘按可能含明文处置"
            );
            Ok(())
        })?;
        ensure!(
            (api.drive_re_assess)(drive.ptr, 0) > 0
                && (api.disc_get_status)(drive.ptr) == DISC_FULL,
            "刻录后无法验证光盘已封盘，请将此光盘按可能含明文处置"
        );
        let mut readback = Zeroizing::new(vec![0u8; 32 * SECTOR_SIZE]);
        for (index, expected) in iso.chunks(readback.len()).enumerate() {
            let mut actual = 0;
            let address = (index * readback.len()) as i64;
            ensure!(
                (api.read_data)(
                    drive.ptr,
                    address,
                    readback.as_mut_ptr().cast(),
                    expected.len() as i64,
                    &mut actual,
                    0
                ) > 0
                    && actual == expected.len() as i64
                    && &readback[..expected.len()] == expected,
                "光盘回读校验失败，请将此光盘按可能含明文处置"
            );
        }
    }
    Ok(())
}
