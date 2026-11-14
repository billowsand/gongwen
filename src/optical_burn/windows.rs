use anyhow::{Context, Result, ensure};
use windows::Win32::Foundation::{VARIANT_FALSE, VARIANT_TRUE};
use windows::Win32::Storage::Imapi::{
    IBurnVerification, IDiscFormat2Data, IDiscMaster2, IDiscRecorder2, IDiscRecorder2Ex,
    IMAPI_BURN_VERIFICATION_FULL, IMAPI_FORMAT2_DATA_MEDIA_STATE_FINALIZED, IMAPI_MEDIA_TYPE_CDR,
    IMAPI_MEDIA_TYPE_DVDDASHR, IMAPI_MEDIA_TYPE_DVDPLUSR, MsftDiscFormat2Data, MsftDiscMaster2,
    MsftDiscRecorder2,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows::Win32::UI::Shell::SHCreateMemStream;
use windows::core::{BSTR, Interface};

use super::{Drive, Media};

struct Com;

impl Com {
    fn initialize() -> Result<Self> {
        // SAFETY: scan and burn run on worker threads. Every successful COM initialization
        // (including S_FALSE) gets matched by one CoUninitialize on the same thread.
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok() }
            .context("无法初始化 Windows 刻录接口")?;
        Ok(Self)
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        // SAFETY: The successful initialization represented by this guard was on this same thread.
        unsafe { CoUninitialize() }
    }
}

fn recorder(id: &BSTR) -> Result<IDiscRecorder2> {
    // SAFETY: Caller initialized COM and passes the ID obtained from IDiscMaster2.
    unsafe {
        let recorder: IDiscRecorder2 =
            CoCreateInstance(&MsftDiscRecorder2, None, CLSCTX_INPROC_SERVER)?;
        recorder.InitializeDiscRecorder(id)?;
        Ok(recorder)
    }
}

fn writer(recorder: &IDiscRecorder2) -> Result<IDiscFormat2Data> {
    // SAFETY: COM is live and recorder is an initialized IMAPI2 object.
    unsafe {
        let writer: IDiscFormat2Data =
            CoCreateInstance(&MsftDiscFormat2Data, None, CLSCTX_INPROC_SERVER)?;
        ensure!(
            writer.IsRecorderSupported(recorder)? == VARIANT_TRUE,
            "设备不支持实体数据光盘刻录"
        );
        ensure!(
            writer.IsCurrentMediaSupported(recorder)? == VARIANT_TRUE,
            "刻录机中的介质不可用"
        );
        writer.SetRecorder(recorder)?;
        Ok(writer)
    }
}

fn inspect(recorder: &IDiscRecorder2, writer: &IDiscFormat2Data) -> Result<Drive> {
    // SAFETY: Both objects are initialized and refer to the same recorder.
    unsafe {
        ensure!(
            writer.MediaPhysicallyBlank()? == VARIANT_TRUE,
            "请插入空白一次写入光盘"
        );
        let media = match writer.CurrentPhysicalMediaType()? {
            IMAPI_MEDIA_TYPE_CDR => Media::CdR,
            IMAPI_MEDIA_TYPE_DVDDASHR => Media::DvdR,
            IMAPI_MEDIA_TYPE_DVDPLUSR => Media::DvdPlusR,
            _ => anyhow::bail!("只接受空白 CD-R、DVD-R 或 DVD+R"),
        };
        let sectors = writer.FreeSectorsOnMedia()?;
        ensure!(sectors > 0, "无法确认空白光盘容量");
        let id = recorder.ActiveDiscRecorder()?.to_string();
        ensure!(!id.is_empty(), "无法取得刻录机标识");
        let vendor = recorder.VendorId()?.to_string();
        let product = recorder.ProductId()?.to_string();
        let revision = recorder.ProductRevision()?.to_string();
        let number = recorder.LegacyDeviceNumber()?;
        ensure!(number >= 0, "无法取得刻录机编号");
        Ok(Drive {
            label: format!(
                "{} {} {} · {} · CD-ROM {number}",
                vendor.trim(),
                product.trim(),
                revision.trim(),
                media.label()
            ),
            id,
            connection: number as u64,
            media,
            sectors: sectors as u64,
        })
    }
}

pub(super) fn scan() -> Result<Vec<Drive>> {
    let _com = Com::initialize()?;
    // SAFETY: COM is live; the master and each recorder are dropped before _com.
    unsafe {
        let master: IDiscMaster2 = CoCreateInstance(&MsftDiscMaster2, None, CLSCTX_INPROC_SERVER)?;
        ensure!(
            master.IsSupportedEnvironment()? == VARIANT_TRUE,
            "Windows 刻录接口不可用"
        );
        let mut drives = Vec::new();
        for index in 0..master.Count()? {
            let id = master.get_Item(index)?;
            if let Ok(recorder) = recorder(&id)
                && let Ok(writer) = writer(&recorder)
                && let Ok(drive) = inspect(&recorder, &writer)
            {
                drives.push(drive);
            }
        }
        Ok(drives)
    }
}

struct Exclusive<'a>(&'a IDiscRecorder2);

impl Drop for Exclusive<'_> {
    fn drop(&mut self) {
        // SAFETY: The successful acquisition represented by this guard has not been released.
        let _ = unsafe { self.0.ReleaseExclusiveAccess() };
    }
}

struct MediaNotifications<'a>(&'a IDiscRecorder2);

impl Drop for MediaNotifications<'_> {
    fn drop(&mut self) {
        // SAFETY: This guard owns one outstanding DisableMcn reference on the live recorder.
        let _ = unsafe { self.0.EnableMcn() };
    }
}

struct TrayLock(IDiscRecorder2Ex);

impl TrayLock {
    fn acquire(recorder: &IDiscRecorder2) -> Result<Self> {
        let device = recorder.cast::<IDiscRecorder2Ex>()?;
        set_tray_lock(&device, true)?;
        Ok(Self(device))
    }
}

impl Drop for TrayLock {
    fn drop(&mut self) {
        let _ = set_tray_lock(&self.0, false);
    }
}

fn set_tray_lock(device: &IDiscRecorder2Ex, locked: bool) -> Result<()> {
    // Standard MMC PREVENT ALLOW MEDIUM REMOVAL. Some failed requests return
    // a successful HRESULT carrying sense data, so inspect the buffer as well.
    let command = [0x1e, 0, 0, 0, u8::from(locked), 0];
    let mut sense = [0; 18];
    // SAFETY: Both buffers remain valid through the synchronous MMC request.
    unsafe { device.SendCommandNoData(&command, &mut sense, 10) }
        .context("无法设置刻录机托盘锁")?;
    ensure!(sense.iter().all(|byte| *byte == 0), "刻录机拒绝设置托盘锁");
    Ok(())
}

pub(super) fn burn(target: &Drive, iso: &[u8], authorize: &dyn Fn() -> Result<()>) -> Result<()> {
    let _com = Com::initialize()?;
    let recorder = recorder(&BSTR::from(target.id.as_str())).context("找不到之前选定的刻录机")?;
    let client = BSTR::from("Gongwen");
    // SAFETY: COM and recorder are live for the entire synchronous write and automatic verify.
    unsafe {
        recorder
            .AcquireExclusiveAccess(VARIANT_FALSE, &client)
            .context("无法独占刻录机")?;
        let _lock = Exclusive(&recorder);
        let _tray = TrayLock::acquire(&recorder)?;
        recorder.DisableMcn().context("无法暂停介质变更通知")?;
        let _notifications = MediaNotifications(&recorder);
        let writer = writer(&recorder)?;
        writer.SetClientName(&client)?;
        writer.SetForceOverwrite(VARIANT_FALSE)?;
        writer.SetForceMediaToBeClosed(VARIANT_TRUE)?;
        let verification = writer
            .cast::<IBurnVerification>()
            .context("刻录环境不支持回读校验")?;
        verification.SetBurnVerificationLevel(IMAPI_BURN_VERIFICATION_FULL)?;
        ensure!(
            u32::try_from(iso.len()).is_ok(),
            "光盘镜像超过 Windows 内存流上限"
        );
        let stream = SHCreateMemStream(Some(iso)).context("无法创建刻录内存流")?;
        ensure!(
            inspect(&recorder, &writer)? == *target,
            "刻录机或空白盘状态已变化"
        );
        authorize()?;
        writer
            .Write(&stream)
            .context("光盘刻录或回读校验失败，请将此光盘按可能含明文处置")?;
        ensure!(
            writer.CurrentMediaStatus()?.0 & IMAPI_FORMAT2_DATA_MEDIA_STATE_FINALIZED.0 != 0,
            "刻录后无法确认光盘已封盘，请将此光盘按可能含明文处置"
        );
    }
    Ok(())
}
