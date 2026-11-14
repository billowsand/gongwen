use super::{Scsi, Target, burn, helper, recheck, valid_serial, verify};
use anyhow::{Context, Result, ensure};
use std::ffi::{c_int, c_ulong, c_void};
use std::fs::{self, File, OpenOptions};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[cfg(feature = "ai-workbench")]
const HELPER: &str = "/usr/libexec/gongwen-assistant/gongwen-is903";
#[cfg(not(feature = "ai-workbench"))]
const HELPER: &str = "/usr/libexec/gongwen-assistant-no-ai/gongwen-is903";
const DRIVER: &str = "/sys/bus/usb/drivers/usb-storage";

struct Connection {
    target: Target,
    usb: PathBuf,
    interface: PathBuf,
    generic: Option<PathBuf>,
    device_number: String,
}

fn attr(path: &Path, name: &str) -> Option<String> {
    Some(fs::read_to_string(path.join(name)).ok()?.trim().to_owned())
}

fn decimal(path: &Path, name: &str) -> Option<u64> {
    attr(path, name)?.parse().ok()
}

fn optical_name(name: &str, prefix: &str) -> bool {
    name.strip_prefix(prefix).is_some_and(|suffix| {
        !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
    })
}

fn inspect(entry: &Path) -> Option<Connection> {
    let disk = entry.file_name()?.to_str()?;
    if !optical_name(disk, "sr") {
        return None;
    }
    let scsi = fs::canonicalize(entry.join("device")).ok()?;
    if attr(&scsi, "type")?.as_str() != "5"
        || scsi.file_name()?.to_str()?.rsplit(':').next()? != "0"
    {
        return None;
    }
    let usb = scsi
        .ancestors()
        .find(|node| node.join("idVendor").is_file())?;
    if attr(usb, "idVendor")?.as_str() != "1f75" || attr(usb, "idProduct")?.as_str() != "0903" {
        return None;
    }
    let interface = scsi
        .ancestors()
        .find(|node| node.join("bInterfaceNumber").is_file())?;
    if attr(interface, "bInterfaceNumber")?.as_str() != "00" {
        return None;
    }
    let serial = attr(usb, "serial")?;
    if !valid_serial(&serial) {
        return None;
    }
    let revision = u64::from_str_radix(&attr(usb, "bcdDevice")?, 16).ok()?;
    let bus = u32::try_from(decimal(usb, "busnum")?).ok()?;
    let devnum = u32::try_from(decimal(usb, "devnum")?).ok()?;
    let device_number = attr(entry, "dev")?;
    let generic = fs::read_dir(scsi.join("scsi_generic"))
        .ok()
        .and_then(|entries| {
            entries.flatten().find_map(|entry| {
                optical_name(entry.file_name().to_str()?, "sg")
                    .then(|| Path::new("/dev").join(entry.file_name()))
            })
        });
    Some(Connection {
        target: Target {
            session: u64::from(bus) << 32 | u64::from(devnum),
            serial,
            disk: format!("/dev/{disk}"),
            revision,
            // USB address is unique per bus until disconnect. The sysfs path keeps its physical port distinct.
            location: stable_location(usb),
        },
        usb: usb.to_owned(),
        interface: interface.to_owned(),
        generic,
        device_number,
    })
}

fn stable_location(path: &Path) -> u64 {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut hash = DefaultHasher::new();
    path.hash(&mut hash);
    hash.finish()
}

fn connections() -> Result<Vec<Connection>> {
    let mut devices: Vec<_> = fs::read_dir("/sys/block")?
        .flatten()
        .filter_map(|entry| inspect(&entry.path()))
        .collect();
    devices.sort_by(|a, b| a.target.disk.cmp(&b.target.disk));
    Ok(devices)
}

pub(super) fn scan() -> Result<Vec<Target>> {
    Ok(connections()?
        .into_iter()
        .map(|device| device.target)
        .collect())
}

fn current(target: &Target) -> Result<Connection> {
    let mut matches = connections()?
        .into_iter()
        .filter(|device| device.target.serial == target.serial);
    let device = matches.next().context("IS903 已拔出")?;
    ensure!(
        device.target == *target && matches.next().is_none(),
        "IS903 已换盘或序列号冲突"
    );
    Ok(device)
}

fn mounted(connection: &Connection) -> Result<bool> {
    Ok(fs::read_to_string("/proc/self/mountinfo")?
        .lines()
        .any(|line| line.split_whitespace().nth(2) == Some(connection.device_number.as_str())))
}

fn unmount(connection: &Connection) -> Result<()> {
    if mounted(connection)? {
        let result = Command::new("/usr/bin/udisksctl")
            .env_clear()
            .args([
                "unmount",
                "--no-user-interaction",
                "-b",
                &connection.target.disk,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .context("无法启动 udisksctl")?;
        ensure!(
            result.success() && !mounted(connection)?,
            "无法卸载 IS903，请关闭正在使用光盘的程序"
        );
    }
    Ok(())
}

fn bind_again(connection: &Connection) -> Result<Target> {
    let driver = Path::new(DRIVER);
    let interface = connection
        .interface
        .file_name()
        .context("IS903 接口名称缺失")?
        .as_encoded_bytes();
    let interface_name = std::ffi::OsStr::new(std::str::from_utf8(interface)?);
    let found = current(&connection.target)?;
    ensure!(
        found.interface == connection.interface
            && fs::canonicalize(driver.join(interface_name))? == connection.interface,
        "IS903 USB 接口已变化"
    );
    fs::write(driver.join("unbind"), interface).context("无法解除 IS903 内核驱动绑定")?;
    let bus = decimal(&connection.usb, "busnum");
    let device = decimal(&connection.usb, "devnum");
    ensure!(
        bus.zip(device)
            .is_some_and(|(bus, dev)| bus << 32 | dev == connection.target.session)
            && attr(&connection.usb, "serial").as_deref()
                == Some(connection.target.serial.as_str()),
        "IS903 解绑期间已拔出或换盘"
    );
    let bind_result = fs::write(driver.join("bind"), interface);
    bind_result.context("镜像已发送，但无法重新绑定 IS903 内核驱动")?;

    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let mut matches = connections()?
            .into_iter()
            .filter(|c| c.target.serial == connection.target.serial);
        if let Some(found) = matches.next() {
            ensure!(
                matches.next().is_none()
                    && found.target.session == connection.target.session
                    && found.target.location == connection.target.location
                    && found.target.revision == connection.target.revision
                    && found.usb == connection.usb,
                "重新绑定的 IS903 身份不一致"
            );
            return Ok(found.target);
        }
        ensure!(Instant::now() < deadline, "等待 IS903 重新绑定超时");
        std::thread::sleep(Duration::from_millis(250));
    }
}

unsafe extern "C" {
    fn ioctl(fd: c_int, command: c_ulong, ...) -> c_int;
    fn flock(fd: c_int, operation: c_int) -> c_int;
}

#[repr(C)]
#[derive(Default)]
struct SgRequest {
    interface_id: c_int,
    direction: c_int,
    command_length: u8,
    sense_capacity: u8,
    vectors: u16,
    data_length: u32,
    data: *mut u8,
    cdb: *const u8,
    sense: *mut u8,
    timeout_ms: u32,
    flags: u32,
    pack_id: c_int,
    user: *mut c_void,
    status: u8,
    masked_status: u8,
    message_status: u8,
    sense_length: u8,
    host_status: u16,
    driver_status: u16,
    residue: c_int,
    duration: u32,
    info: u32,
}

struct Drive(File);

impl Drive {
    fn open(connection: &Connection) -> Result<Self> {
        let path = connection
            .generic
            .as_ref()
            .context("找不到 IS903 SCSI 通用节点，请加载 sg 模块")?;
        let drive = OpenOptions::new().read(true).write(true).open(path)?;
        let latest = current(&connection.target)?;
        ensure!(
            latest.generic.as_ref() == Some(path),
            "IS903 SCSI 节点已变化"
        );
        // Verify the open fd and current sysfs generic device correspond to the same major/minor.
        use std::os::unix::fs::MetadataExt;
        let name = path.file_name().context("SCSI 节点名称缺失")?;
        let want = attr(&Path::new("/sys/class/scsi_generic").join(name), "dev")
            .context("IS903 SCSI 节点已消失")?;
        let raw = drive.metadata()?.rdev();
        let major = ((raw >> 8) & 0xfff) | ((raw >> 32) & 0xfffff000);
        let minor = (raw & 0xff) | ((raw >> 12) & 0xffffff00);
        ensure!(
            want == format!("{major}:{minor}"),
            "打开的 SCSI 节点与目标不匹配"
        );
        Ok(Self(drive))
    }
}

impl Scsi for Drive {
    fn transfer(&self, command: &[u8], input: bool, data: &mut [u8]) -> Result<()> {
        let mut sense = [0; 32];
        let mut request = SgRequest {
            interface_id: i32::from(b'S'),
            direction: if input { -3 } else { -2 },
            command_length: u8::try_from(command.len())?,
            sense_capacity: 32,
            data_length: u32::try_from(data.len())?,
            data: data.as_mut_ptr(),
            cdb: command.as_ptr(),
            sense: sense.as_mut_ptr(),
            timeout_ms: 30_000,
            ..Default::default()
        };
        // SAFETY: The open sg descriptor and all buffers in the C-layout request remain live during the synchronous call.
        let result = unsafe { ioctl(self.0.as_raw_fd(), 0x2285, &mut request as *mut SgRequest) };
        if result < 0 {
            return Err(std::io::Error::last_os_error()).context("IS903 SCSI 直通失败");
        }
        ensure!(
            request.status == 0
                && request.host_status == 0
                && request.driver_status == 0
                && request.residue == 0,
            "IS903 SCSI 命令 {:02x} 失败（状态 {} / {} / {}，剩余 {} 字节）",
            command[0],
            request.status,
            request.host_status,
            request.driver_status,
            request.residue
        );
        Ok(())
    }
}

fn burn_and_verify(
    target: &Target,
    image: &[u8],
    authorize: &dyn Fn() -> Result<()>,
) -> Result<()> {
    let connection = current(target)?;
    let lock = File::open(HELPER)?;
    ensure!(
        // SAFETY: The File owns a live descriptor and the flags request an exclusive nonblocking lock.
        unsafe { flock(lock.as_raw_fd(), 2 | 4) } == 0,
        "已有一项 IS903 刻录正在进行"
    );
    let driver = Path::new(DRIVER);
    let interface_name = connection
        .interface
        .file_name()
        .context("IS903 接口名称缺失")?;
    ensure!(
        fs::canonicalize(driver.join(interface_name))? == connection.interface,
        "IS903 未绑定 usb-storage 驱动"
    );
    unmount(&connection)?;
    let drive = Drive::open(&connection)?;
    burn(&drive, image, &|| {
        recheck(target)?;
        ensure!(!mounted(&connection)?, "IS903 光盘已重新挂载");
        authorize()
    })?;
    drop(drive);
    let refreshed = bind_again(&connection).context("IS903 写入后刷新失败")?;
    verify(&Drive::open(&current(&refreshed)?)?, image)
}

pub(super) fn write(
    target: &Target,
    image: &[u8],
    authorize: &dyn Fn() -> Result<()>,
) -> Result<()> {
    helper::trusted_file(Path::new(HELPER))?;
    authorize()?;
    let mut child = Command::new("/usr/bin/pkexec")
        .args([HELPER, helper::ARG])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("无法请求 Linux 设备写入授权")?;
    let mut input = child.stdout.take().context("无法打开 IS903 助手输出")?;
    let mut output = child.stdin.take().context("无法打开 IS903 助手输入")?;
    let result = helper::send(target, image, authorize, &mut input, &mut output);
    drop(output);
    drop(input);
    let status = child.wait()?;
    result?;
    ensure!(status.success(), "IS903 助手异常退出（{status}）");
    Ok(())
}

pub(crate) fn helper_main() -> Option<i32> {
    helper::run(
        HELPER,
        4,
        || helper::trusted_file(Path::new(HELPER)),
        burn_and_verify,
    )
}
