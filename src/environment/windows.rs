use super::{
    DeviceClass, DeviceIdentity, DeviceInfo, DeviceSnapshot, MonitorEvent, NetworkInterface,
    NetworkInterfaceKind, NetworkSnapshot,
};
use eframe::egui;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::c_void;
use std::mem::{size_of, size_of_val};
use std::net::{Ipv4Addr, Ipv6Addr};
use std::ptr::{self, addr_of, addr_of_mut};
use std::slice;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};
use windows_sys::Win32::{
    Devices::{
        DeviceAndDriverInstallation::{
            CM_DEVCAP_UNIQUEID, CM_DRP_CAPABILITIES, CM_DRP_COMPATIBLEIDS, CM_DRP_HARDWAREID,
            CM_DRP_REMOVAL_POLICY, CM_GET_DEVICE_INTERFACE_LIST_PRESENT,
            CM_Get_DevNode_Registry_PropertyW, CM_Get_Device_IDW,
            CM_Get_Device_Interface_List_SizeW, CM_Get_Device_Interface_ListW,
            CM_Get_Device_Interface_PropertyW, CM_Get_Parent, CM_Locate_DevNodeW,
            CM_REMOVAL_POLICY_EXPECT_NO_REMOVAL, CR_BUFFER_SMALL, CR_NO_SUCH_DEVICE_INTERFACE,
            CR_NO_SUCH_DEVNODE, CR_NO_SUCH_VALUE, CR_SUCCESS,
        },
        HumanInterfaceDevice::GUID_DEVINTERFACE_HID,
        PortableDevices::GUID_DEVINTERFACE_WPD,
        Properties::{DEVPKEY_Device_InstanceId, DEVPROP_TYPE_STRING},
        Usb::GUID_DEVINTERFACE_USB_DEVICE,
    },
    Foundation::{HANDLE, HINSTANCE, HWND},
    NetworkManagement::{
        IpHelper::{
            CancelMibChangeNotify2, FreeMibTable, GetIfTable2, GetIpForwardTable2,
            GetUnicastIpAddressTable, IF_TYPE_ETHERNET_CSMACD, IF_TYPE_FASTETHER,
            IF_TYPE_FASTETHER_FX, IF_TYPE_GIGABITETHERNET, IF_TYPE_IEEE80211, IF_TYPE_PPP,
            IF_TYPE_PPPMULTILINKBUNDLE, IF_TYPE_PROP_VIRTUAL, IF_TYPE_PROP_WIRELESS_P2P,
            IF_TYPE_SOFTWARE_LOOPBACK, IF_TYPE_TUNNEL, IF_TYPE_WWANPP, IF_TYPE_WWANPP2,
            MIB_IF_TABLE2, MIB_IPFORWARD_ROW2, MIB_IPFORWARD_TABLE2, MIB_NOTIFICATION_TYPE,
            MIB_UNICASTIPADDRESS_ROW, MIB_UNICASTIPADDRESS_TABLE, NotifyIpInterfaceChange,
            NotifyRouteChange2, NotifyUnicastIpAddressChange,
        },
        Ndis::{IfOperStatusUp, NET_IF_ACCESS_LOOPBACK},
    },
    Networking::WinSock::{AF_INET, AF_INET6, AF_UNSPEC},
    System::{
        Ioctl::{GUID_DEVINTERFACE_CDROM, GUID_DEVINTERFACE_DISK},
        LibraryLoader::GetModuleHandleW,
    },
    UI::WindowsAndMessaging::{
        CREATESTRUCTW, CreateWindowExW, DBT_DEVTYP_DEVICEINTERFACE,
        DEV_BROADCAST_DEVICEINTERFACE_W, DEVICE_NOTIFY_ALL_INTERFACE_CLASSES,
        DEVICE_NOTIFY_WINDOW_HANDLE, DefWindowProcW, DestroyWindow, DispatchMessageW,
        GWLP_USERDATA, GetWindowLongPtrW, HDEVNOTIFY, HWND_MESSAGE, MSG, PM_REMOVE, PeekMessageW,
        RegisterClassExW, RegisterDeviceNotificationW, SetWindowLongPtrW, TranslateMessage,
        UnregisterClassW, UnregisterDeviceNotification, WM_DEVICECHANGE, WM_NCCREATE, WM_NCDESTROY,
        WNDCLASSEXW,
    },
};
use windows_sys::core::GUID;

const HEARTBEAT: Duration = Duration::from_secs(5);
const POLL_INTERVAL: Duration = Duration::from_millis(25);
const MAX_DEVICE_PATHS: u32 = 1024 * 1024;
const GUID_DEVINTERFACE_IMAGE: GUID = GUID::from_u128(0x6bdd1fc6_810f_11d0_bec7_08002be2092f);

struct MibTable(*mut c_void);

impl Drop for MibTable {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: The IP Helper API allocated this table and ownership has not been released elsewhere.
            unsafe { FreeMibTable(self.0) };
        }
    }
}

struct MibNotification(HANDLE);

impl Drop for MibNotification {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: This is a live notification handle owned by this guard; cancellation is ignored during teardown.
            unsafe {
                CancelMibChangeNotify2(self.0);
            }
        }
    }
}

fn mark_dirty(context: *const c_void) {
    if let Some(dirty) = (!context.is_null()).then(|| {
        // SAFETY: Every network callback receives an Arc<AtomicBool> pointer that outlives notification cancellation.
        unsafe { &*context.cast::<AtomicBool>() }
    }) {
        dirty.store(true, Ordering::Release);
    }
}

unsafe extern "system" fn ip_interface_changed(
    context: *const c_void,
    _row: *const windows_sys::Win32::NetworkManagement::IpHelper::MIB_IPINTERFACE_ROW,
    _kind: MIB_NOTIFICATION_TYPE,
) {
    mark_dirty(context);
}

unsafe extern "system" fn unicast_address_changed(
    context: *const c_void,
    _row: *const MIB_UNICASTIPADDRESS_ROW,
    _kind: MIB_NOTIFICATION_TYPE,
) {
    mark_dirty(context);
}

unsafe extern "system" fn route_changed(
    context: *const c_void,
    _row: *const MIB_IPFORWARD_ROW2,
    _kind: MIB_NOTIFICATION_TYPE,
) {
    mark_dirty(context);
}

fn register_network_notifications(dirty: &Arc<AtomicBool>) -> Result<Vec<MibNotification>, String> {
    let context = Arc::as_ptr(dirty).cast::<c_void>();
    let mut registrations = Vec::with_capacity(3);
    let mut handle = ptr::null_mut();
    // SAFETY: The callback has the required ABI; its context remains alive until all returned guards are cancelled.
    let status = unsafe {
        NotifyIpInterfaceChange(
            AF_UNSPEC,
            Some(ip_interface_changed),
            context,
            false,
            &mut handle,
        )
    };
    if status != 0 {
        return Err(format!("无法注册网络接口通知（{status}）"));
    }
    registrations.push(MibNotification(handle));

    handle = ptr::null_mut();
    // SAFETY: The callback and context have the same lifetime guarantees as the interface notification above.
    let status = unsafe {
        NotifyUnicastIpAddressChange(
            AF_UNSPEC,
            Some(unicast_address_changed),
            context,
            false,
            &mut handle,
        )
    };
    if status != 0 {
        return Err(format!("无法注册 IP 地址通知（{status}）"));
    }
    registrations.push(MibNotification(handle));

    handle = ptr::null_mut();
    // SAFETY: The callback and context have the same lifetime guarantees as the interface notification above.
    let status =
        unsafe { NotifyRouteChange2(AF_UNSPEC, Some(route_changed), context, false, &mut handle) };
    if status != 0 {
        return Err(format!("无法注册路由通知（{status}）"));
    }
    registrations.push(MibNotification(handle));
    Ok(registrations)
}

fn address_is_non_loopback(address: &MIB_UNICASTIPADDRESS_ROW) -> bool {
    // SAFETY: si_family selects the corresponding active union member in the OS-provided socket address.
    let family = unsafe { address.Address.si_family };
    match family {
        AF_INET => {
            // SAFETY: AF_INET establishes that Ipv4 is the active SOCKADDR_INET member.
            let raw = unsafe { address.Address.Ipv4.sin_addr.S_un.S_addr };
            let ip = Ipv4Addr::from(raw.to_ne_bytes());
            !ip.is_loopback() && !ip.is_unspecified()
        }
        AF_INET6 => {
            // SAFETY: AF_INET6 establishes that Ipv6 is active and its Byte union member is the address storage.
            let bytes = unsafe { address.Address.Ipv6.sin6_addr.u.Byte };
            let ip = Ipv6Addr::from(bytes);
            if let Some(mapped) = ip.to_ipv4_mapped() {
                return !mapped.is_loopback() && !mapped.is_unspecified();
            }
            !ip.is_loopback() && !ip.is_unspecified()
        }
        _ => false,
    }
}

fn interface_kind(kind: u32) -> NetworkInterfaceKind {
    match kind {
        IF_TYPE_ETHERNET_CSMACD
        | IF_TYPE_FASTETHER
        | IF_TYPE_FASTETHER_FX
        | IF_TYPE_GIGABITETHERNET => NetworkInterfaceKind::Ethernet,
        IF_TYPE_IEEE80211 | IF_TYPE_PROP_WIRELESS_P2P => NetworkInterfaceKind::Wifi,
        IF_TYPE_WWANPP | IF_TYPE_WWANPP2 => NetworkInterfaceKind::Cellular,
        IF_TYPE_PPP | IF_TYPE_PPPMULTILINKBUNDLE | IF_TYPE_TUNNEL | IF_TYPE_PROP_VIRTUAL => {
            NetworkInterfaceKind::Vpn
        }
        _ => NetworkInterfaceKind::Other,
    }
}

fn network_snapshot() -> Result<NetworkSnapshot, String> {
    let mut raw_interfaces = ptr::null_mut();
    // SAFETY: raw_interfaces is writable and receives an IP-Helper-owned table on success.
    let status = unsafe { GetIfTable2(&mut raw_interfaces) };
    let interfaces = MibTable(raw_interfaces.cast());
    if status != 0 || interfaces.0.is_null() {
        return Err(format!("GetIfTable2 失败（{status}）"));
    }

    let mut raw_addresses = ptr::null_mut();
    // SAFETY: raw_addresses is writable and AF_UNSPEC requests both IP address families.
    let status = unsafe { GetUnicastIpAddressTable(AF_UNSPEC, &mut raw_addresses) };
    let addresses = MibTable(raw_addresses.cast());
    if status != 0 || addresses.0.is_null() {
        return Err(format!("GetUnicastIpAddressTable 失败（{status}）"));
    }

    let mut raw_routes = ptr::null_mut();
    // SAFETY: raw_routes is writable and AF_UNSPEC requests both IP route families.
    let status = unsafe { GetIpForwardTable2(AF_UNSPEC, &mut raw_routes) };
    let routes = MibTable(raw_routes.cast());
    if status != 0 || routes.0.is_null() {
        return Err(format!("GetIpForwardTable2 失败（{status}）"));
    }

    // SAFETY: Each successful table remains allocated by its guard; NumEntries describes its flexible array tail.
    let (interface_rows, address_rows, route_rows) = unsafe {
        let interface_table = &*interfaces.0.cast::<MIB_IF_TABLE2>();
        let address_table = &*addresses.0.cast::<MIB_UNICASTIPADDRESS_TABLE>();
        let route_table = &*routes.0.cast::<MIB_IPFORWARD_TABLE2>();
        (
            slice::from_raw_parts(
                interface_table.Table.as_ptr(),
                interface_table.NumEntries as usize,
            ),
            slice::from_raw_parts(
                address_table.Table.as_ptr(),
                address_table.NumEntries as usize,
            ),
            slice::from_raw_parts(route_table.Table.as_ptr(), route_table.NumEntries as usize),
        )
    };

    let mut addressed = BTreeSet::new();
    for row in address_rows
        .iter()
        .filter(|row| address_is_non_loopback(row))
    {
        // SAFETY: Value reads the complete LUID union without depending on its bitfield representation.
        let luid = unsafe { row.InterfaceLuid.Value };
        addressed.insert((luid, row.InterfaceIndex));
    }

    let mut result = NetworkSnapshot::default();
    for row in interface_rows {
        // SAFETY: Value reads the complete LUID union supplied by GetIfTable2.
        let luid = unsafe { row.InterfaceLuid.Value };
        let loopback =
            row.Type == IF_TYPE_SOFTWARE_LOOPBACK || row.AccessType == NET_IF_ACCESS_LOOPBACK;
        let non_loopback_address = !loopback
            && (addressed.contains(&(luid, row.InterfaceIndex))
                || addressed.contains(&(luid, 0))
                || addressed.contains(&(0, row.InterfaceIndex)));
        result.interfaces.push(NetworkInterface {
            id: if luid == 0 {
                format!("if-{}", row.InterfaceIndex)
            } else {
                format!("{luid:016x}")
            },
            kind: interface_kind(row.Type),
            loopback,
            link_up: row.OperStatus == IfOperStatusUp,
            non_loopback_address,
        });
    }
    result
        .interfaces
        .sort_by(|left, right| left.id.cmp(&right.id));

    result.default_route = route_rows.iter().any(|route| {
        if route.Loopback || route.DestinationPrefix.PrefixLength != 0 {
            return false;
        }
        // SAFETY: Reading the family does not access any inactive union member.
        let family = unsafe { route.DestinationPrefix.Prefix.si_family };
        family == AF_INET || family == AF_INET6
    });
    Ok(result)
}

struct WindowClass {
    name: Vec<u16>,
    instance: HINSTANCE,
}

impl WindowClass {
    fn new() -> Result<Self, String> {
        // SAFETY: A null module name requests the current process module, which outlives this window class.
        let instance = unsafe { GetModuleHandleW(ptr::null()) };
        if instance.is_null() {
            return Err(format!(
                "GetModuleHandleW 失败：{}",
                std::io::Error::last_os_error()
            ));
        }
        let name = wide("GongwenAssistantEnvironmentMonitor");
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(device_window_proc),
            hInstance: instance,
            lpszClassName: name.as_ptr(),
            ..Default::default()
        };
        // SAFETY: class points to a fully initialized structure and name is NUL-terminated for the registration call.
        if unsafe { RegisterClassExW(&class) } == 0 {
            return Err(format!(
                "注册设备通知窗口类失败：{}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(Self { name, instance })
    }
}

impl Drop for WindowClass {
    fn drop(&mut self) {
        // SAFETY: The class was registered for this module and its only window has already been destroyed.
        unsafe {
            UnregisterClassW(self.name.as_ptr(), self.instance);
        }
    }
}

struct HiddenWindow(HWND);

impl HiddenWindow {
    fn new(class: &WindowClass, context: *const c_void) -> Result<Self, String> {
        // SAFETY: The registered class name is NUL-terminated; context is passed to WM_NCCREATE and retained by the window.
        let window = unsafe {
            CreateWindowExW(
                0,
                class.name.as_ptr(),
                class.name.as_ptr(),
                0,
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                ptr::null_mut(),
                class.instance,
                context,
            )
        };
        if window.is_null() {
            return Err(format!(
                "创建设备通知窗口失败：{}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(Self(window))
    }
}

impl Drop for HiddenWindow {
    fn drop(&mut self) {
        // SAFETY: This guard owns the window and is dropped on the same thread that created it.
        unsafe {
            DestroyWindow(self.0);
        }
    }
}

struct DeviceNotification(HDEVNOTIFY);

impl DeviceNotification {
    fn new(window: HWND) -> Result<Self, String> {
        let filter = DEV_BROADCAST_DEVICEINTERFACE_W {
            dbcc_size: size_of::<DEV_BROADCAST_DEVICEINTERFACE_W>() as u32,
            dbcc_devicetype: DBT_DEVTYP_DEVICEINTERFACE,
            ..Default::default()
        };
        // SAFETY: window is live, filter has the advertised size, and all interface classes are requested explicitly.
        let notification = unsafe {
            RegisterDeviceNotificationW(
                window,
                addr_of!(filter).cast(),
                DEVICE_NOTIFY_WINDOW_HANDLE | DEVICE_NOTIFY_ALL_INTERFACE_CLASSES,
            )
        };
        if notification.is_null() {
            return Err(format!(
                "注册设备变更通知失败：{}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(Self(notification))
    }
}

impl Drop for DeviceNotification {
    fn drop(&mut self) {
        // SAFETY: This guard exclusively owns the registration handle and unregisters before destroying its window.
        unsafe {
            UnregisterDeviceNotification(self.0);
        }
    }
}

struct DeviceMonitor {
    _notification: DeviceNotification,
    _window: HiddenWindow,
    _class: WindowClass,
}

impl DeviceMonitor {
    fn new(context: *const c_void) -> Result<Self, String> {
        let class = WindowClass::new()?;
        let window = HiddenWindow::new(&class, context)?;
        let notification = DeviceNotification::new(window.0)?;
        Ok(Self {
            _notification: notification,
            _window: window,
            _class: class,
        })
    }
}

unsafe extern "system" fn device_window_proc(
    window: HWND,
    message: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    match message {
        WM_NCCREATE => {
            if lparam == 0 {
                return 0;
            }
            // SAFETY: WM_NCCREATE supplies a valid CREATESTRUCTW for the duration of this callback.
            let create = unsafe { &*(lparam as *const CREATESTRUCTW) };
            // SAFETY: window is being created and its user-data slot accepts the Arc-backed context pointer.
            unsafe {
                SetWindowLongPtrW(window, GWLP_USERDATA, create.lpCreateParams as isize);
            }
            1
        }
        WM_DEVICECHANGE => {
            // SAFETY: GWLP_USERDATA contains the device_dirty Arc pointer installed during WM_NCCREATE.
            let context = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) } as *const c_void;
            mark_dirty(context);
            let _ = wparam;
            0
        }
        WM_NCDESTROY => {
            // SAFETY: The window is being destroyed, so its Arc-backed callback context must no longer be reachable.
            unsafe {
                SetWindowLongPtrW(window, GWLP_USERDATA, 0);
            }
            // SAFETY: Unhandled non-client destruction processing is delegated with the original arguments.
            unsafe { DefWindowProcW(window, message, wparam, lparam) }
        }
        // SAFETY: All other messages retain the operating system's default window behavior.
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

fn pump_device_messages() {
    for _ in 0..64 {
        let mut message = MSG::default();
        // SAFETY: message is writable; null HWND drains this thread's queue without creating a window dependency.
        if unsafe { PeekMessageW(&mut message, ptr::null_mut(), 0, 0, PM_REMOVE) } == 0 {
            break;
        }
        // SAFETY: PeekMessageW initialized message and this is the queue-owning thread.
        unsafe {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn device_id(node: u32) -> Result<String, u32> {
    let mut id = [0u16; 512];
    // SAFETY: id has the writable UTF-16 capacity passed to Configuration Manager.
    let status = unsafe { CM_Get_Device_IDW(node, id.as_mut_ptr(), id.len() as u32, 0) };
    if status != CR_SUCCESS {
        return Err(status);
    }
    let end = id.iter().position(|&unit| unit == 0).unwrap_or(id.len());
    Ok(String::from_utf16_lossy(&id[..end]))
}

fn interface_instance(path: &[u16]) -> Result<Option<(u32, String)>, String> {
    let mut id = [0u16; 512];
    let mut property_type = 0;
    let mut length = size_of_val(&id) as u32;
    // SAFETY: path is NUL-terminated and id exposes its exact writable byte capacity.
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
    if matches!(
        status,
        CR_NO_SUCH_DEVICE_INTERFACE | CR_NO_SUCH_DEVNODE | CR_NO_SUCH_VALUE
    ) {
        return Ok(None);
    }
    if status != CR_SUCCESS || property_type != DEVPROP_TYPE_STRING {
        return Err(format!("读取设备接口实例失败（{status}）"));
    }
    let end = id.iter().position(|&unit| unit == 0).unwrap_or(id.len());
    let instance = String::from_utf16_lossy(&id[..end]);
    if instance.is_empty() {
        return Err("设备接口返回空实例标识".into());
    }
    let terminated = wide(&instance);
    let mut node = 0;
    // SAFETY: terminated is a live NUL-terminated instance ID and node is writable.
    let status = unsafe { CM_Locate_DevNodeW(&mut node, terminated.as_ptr(), 0) };
    if status == CR_NO_SUCH_DEVNODE {
        return Ok(None);
    }
    if status != CR_SUCCESS {
        return Err(format!("定位设备实例失败（{status}）"));
    }
    Ok(Some((node, instance)))
}

fn interface_instances(guid: &GUID) -> Result<Vec<(u32, String)>, String> {
    for _ in 0..3 {
        let mut length = 0;
        // SAFETY: length is writable and a null device ID requests all present interfaces of this class.
        let status = unsafe {
            CM_Get_Device_Interface_List_SizeW(
                &mut length,
                guid,
                ptr::null(),
                CM_GET_DEVICE_INTERFACE_LIST_PRESENT,
            )
        };
        if status != CR_SUCCESS {
            return Err(format!("读取设备接口列表大小失败（{status}）"));
        }
        if length > MAX_DEVICE_PATHS {
            return Err("设备接口列表异常过大".into());
        }
        if length <= 1 {
            return Ok(Vec::new());
        }
        let mut paths = vec![0u16; length as usize];
        // SAFETY: paths has the writable element count supplied by the size query.
        let status = unsafe {
            CM_Get_Device_Interface_ListW(
                guid,
                ptr::null(),
                paths.as_mut_ptr(),
                length,
                CM_GET_DEVICE_INTERFACE_LIST_PRESENT,
            )
        };
        if status == CR_BUFFER_SMALL {
            continue;
        }
        if status != CR_SUCCESS {
            return Err(format!("读取设备接口列表失败（{status}）"));
        }
        let mut nodes = BTreeMap::new();
        for path in paths
            .split(|&unit| unit == 0)
            .take_while(|path| !path.is_empty())
        {
            let path: Vec<u16> = path.iter().copied().chain(std::iter::once(0)).collect();
            if let Some((node, id)) = interface_instance(&path)? {
                nodes.entry(node).or_insert(id);
            }
        }
        return Ok(nodes.into_iter().collect());
    }
    Err("设备接口列表在枚举期间持续变化".into())
}

fn registry_u32(node: u32, property: u32) -> Result<Option<u32>, String> {
    let mut value = 0u32;
    let mut length = size_of_val(&value) as u32;
    // SAFETY: value is writable u32 storage and length is its exact byte size.
    let status = unsafe {
        CM_Get_DevNode_Registry_PropertyW(
            node,
            property,
            ptr::null_mut(),
            addr_of_mut!(value).cast(),
            &mut length,
            0,
        )
    };
    if status == CR_NO_SUCH_VALUE {
        return Ok(None);
    }
    if status != CR_SUCCESS || length < size_of_val(&value) as u32 {
        return Err(format!("读取设备属性 {property} 失败（{status}）"));
    }
    Ok(Some(value))
}

fn registry_strings(node: u32, property: u32) -> Result<Vec<String>, String> {
    for _ in 0..3 {
        let mut length = 0;
        // SAFETY: A null buffer queries the required byte count and length is writable.
        let status = unsafe {
            CM_Get_DevNode_Registry_PropertyW(
                node,
                property,
                ptr::null_mut(),
                ptr::null_mut(),
                &mut length,
                0,
            )
        };
        if status == CR_NO_SUCH_VALUE || (status == CR_SUCCESS && length == 0) {
            return Ok(Vec::new());
        }
        if status != CR_BUFFER_SMALL && status != CR_SUCCESS {
            return Err(format!(
                "读取设备字符串属性 {property} 大小失败（{status}）"
            ));
        }
        if length > 64 * 1024 {
            return Err(format!("设备字符串属性 {property} 异常过大"));
        }
        let mut buffer = vec![0u16; length.div_ceil(2) as usize + 1];
        let mut actual = size_of_val(buffer.as_slice()) as u32;
        // SAFETY: buffer is writable for actual bytes and its extra UTF-16 element guarantees NUL termination.
        let status = unsafe {
            CM_Get_DevNode_Registry_PropertyW(
                node,
                property,
                ptr::null_mut(),
                buffer.as_mut_ptr().cast(),
                &mut actual,
                0,
            )
        };
        if status == CR_BUFFER_SMALL {
            continue;
        }
        if status != CR_SUCCESS || actual > size_of_val(buffer.as_slice()) as u32 {
            return Err(format!("读取设备字符串属性 {property} 失败（{status}）"));
        }
        let units = actual.div_ceil(2) as usize;
        return Ok(buffer[..units]
            .split(|&unit| unit == 0)
            .filter(|entry| !entry.is_empty())
            .map(|entry| String::from_utf16_lossy(entry))
            .collect());
    }
    Err(format!("设备字符串属性 {property} 在读取期间持续变化"))
}

#[derive(Clone)]
struct UsbDevice {
    node: u32,
    id: String,
}

#[derive(Default)]
struct Ancestry {
    usb: Option<UsbDevice>,
    removable: bool,
}

fn ancestry(mut node: u32) -> Result<Ancestry, String> {
    let mut result = Ancestry::default();
    for _ in 0..16 {
        let id = match device_id(node) {
            Ok(id) => id,
            Err(CR_NO_SUCH_DEVNODE) => return Ok(result),
            Err(status) => return Err(format!("读取设备实例标识失败（{status}）")),
        };
        let upper = id.to_ascii_uppercase();
        if result.usb.is_none() && upper.starts_with("USB\\VID_") {
            result.usb = Some(UsbDevice {
                node,
                id: id.clone(),
            });
            result.removable = true;
        }
        if registry_u32(node, CM_DRP_REMOVAL_POLICY)?
            .is_some_and(|policy| policy != CM_REMOVAL_POLICY_EXPECT_NO_REMOVAL)
        {
            result.removable = true;
        }
        let mut parent = 0;
        // SAFETY: parent is writable storage for one devnode value.
        let status = unsafe { CM_Get_Parent(&mut parent, node, 0) };
        if status == CR_NO_SUCH_DEVNODE || status == CR_NO_SUCH_VALUE {
            break;
        }
        if status != CR_SUCCESS {
            return Err(format!("读取设备父节点失败（{status}）"));
        }
        if parent == node {
            return Err("设备父节点形成循环".into());
        }
        node = parent;
    }
    Ok(result)
}

fn usb_identity(usb: &UsbDevice) -> Option<DeviceIdentity> {
    let capabilities = registry_u32(usb.node, CM_DRP_CAPABILITIES).ok().flatten()?;
    if capabilities & CM_DEVCAP_UNIQUEID == 0 {
        return None;
    }
    let serial = usb.id.rsplit('\\').next()?.trim();
    if serial.is_empty() {
        return None;
    }
    let revision = registry_strings(usb.node, CM_DRP_HARDWAREID)
        .ok()?
        .into_iter()
        .filter_map(|id| {
            let id = id.to_ascii_uppercase();
            let start = id.find("REV_")? + 4;
            let digits = id.get(start..start + 4)?;
            u64::from_str_radix(digits, 16).ok()
        })
        .next()?;
    Some(DeviceIdentity {
        serial: serial.to_owned(),
        revision,
    })
}

fn usb_class(node: u32) -> Result<Option<u8>, String> {
    for id in registry_strings(node, CM_DRP_COMPATIBLEIDS)? {
        let id = id.to_ascii_uppercase();
        if let Some(start) = id.find("CLASS_") {
            if let Some(digits) = id.get(start + 6..start + 8) {
                if let Ok(class) = u8::from_str_radix(digits, 16) {
                    return Ok(Some(class));
                }
            }
        }
    }
    Ok(None)
}

fn class_rank(class: DeviceClass) -> u8 {
    match class {
        DeviceClass::RemovableStorage => 0,
        DeviceClass::PortableMedia => 1,
        DeviceClass::HumanInterface => 2,
        DeviceClass::Imaging => 3,
        DeviceClass::Other => 4,
    }
}

fn add_device(
    devices: &mut BTreeMap<(String, u8), Option<DeviceIdentity>>,
    id: String,
    class: DeviceClass,
    identity: Option<DeviceIdentity>,
) {
    let entry = devices.entry((id, class_rank(class))).or_insert(None);
    if identity.is_some() {
        *entry = identity;
    }
}

fn device_snapshot() -> Result<DeviceSnapshot, String> {
    let mut devices = BTreeMap::new();
    let mut classified_usb = BTreeSet::new();

    for guid in [&GUID_DEVINTERFACE_DISK, &GUID_DEVINTERFACE_CDROM] {
        for (node, id) in interface_instances(guid)? {
            let ancestry = ancestry(node)?;
            if !ancestry.removable {
                continue;
            }
            let identity = ancestry.usb.as_ref().and_then(usb_identity);
            if let Some(usb) = ancestry.usb {
                classified_usb.insert(usb.id);
            }
            add_device(&mut devices, id, DeviceClass::RemovableStorage, identity);
        }
    }

    for (node, id) in interface_instances(&GUID_DEVINTERFACE_WPD)? {
        let usb = ancestry(node)?.usb;
        let identity = usb.as_ref().and_then(usb_identity);
        if let Some(usb) = usb {
            classified_usb.insert(usb.id);
        }
        add_device(&mut devices, id, DeviceClass::PortableMedia, identity);
    }

    for (guid, class) in [
        (&GUID_DEVINTERFACE_HID, DeviceClass::HumanInterface),
        (&GUID_DEVINTERFACE_IMAGE, DeviceClass::Imaging),
    ] {
        for (node, id) in interface_instances(guid)? {
            let Some(usb) = ancestry(node)?.usb else {
                continue;
            };
            let identity = usb_identity(&usb);
            classified_usb.insert(usb.id);
            add_device(&mut devices, id, class, identity);
        }
    }

    for (node, id) in interface_instances(&GUID_DEVINTERFACE_USB_DEVICE)? {
        let class = usb_class(node)?;
        let mapped = match class {
            Some(0x03) => Some(DeviceClass::HumanInterface),
            Some(0x06 | 0x0e) => Some(DeviceClass::Imaging),
            Some(0x08) => Some(DeviceClass::RemovableStorage),
            Some(0x09) => None,
            _ => Some(DeviceClass::Other),
        };
        if mapped == Some(DeviceClass::Other) && classified_usb.contains(&id) {
            continue;
        }
        if let Some(class) = mapped {
            let usb = UsbDevice {
                node,
                id: id.clone(),
            };
            add_device(&mut devices, id, class, usb_identity(&usb));
        }
    }

    Ok(DeviceSnapshot {
        devices: devices
            .into_iter()
            .map(|((id, rank), identity)| DeviceInfo {
                id,
                class: match rank {
                    0 => DeviceClass::RemovableStorage,
                    1 => DeviceClass::PortableMedia,
                    2 => DeviceClass::HumanInterface,
                    3 => DeviceClass::Imaging,
                    _ => DeviceClass::Other,
                },
                identity,
            })
            .collect(),
    })
}

fn send_network(sender: &mpsc::Sender<MonitorEvent>, error: Option<&str>) -> bool {
    let event = match error {
        Some(error) => Err(error.to_owned()),
        None => network_snapshot(),
    };
    sender.send(MonitorEvent::Network(event)).is_ok()
}

fn send_devices(sender: &mpsc::Sender<MonitorEvent>, error: Option<&str>) -> bool {
    let event = match error {
        Some(error) => Err(error.to_owned()),
        None => device_snapshot(),
    };
    sender.send(MonitorEvent::Devices(event)).is_ok()
}

pub(crate) fn run(sender: mpsc::Sender<MonitorEvent>, stop: Arc<AtomicBool>, wake: egui::Context) {
    let network_dirty = Arc::new(AtomicBool::new(true));
    let device_dirty = Arc::new(AtomicBool::new(true));
    let network_registrations = register_network_notifications(&network_dirty);
    let network_error = network_registrations.as_ref().err().cloned();
    let _network_notifications = network_registrations.ok();
    let device_context = Arc::as_ptr(&device_dirty).cast::<c_void>();
    let device_monitor = DeviceMonitor::new(device_context);
    let device_error = device_monitor.as_ref().err().cloned();
    let _device_monitor = device_monitor.ok();

    network_dirty.store(false, Ordering::Release);
    device_dirty.store(false, Ordering::Release);
    let connected = send_network(&sender, None) && send_devices(&sender, None);
    if !connected {
        return;
    }
    if let Some(error) = network_error.as_deref() {
        let _ = send_network(&sender, Some(error));
    }
    if let Some(error) = device_error.as_deref() {
        let _ = send_devices(&sender, Some(error));
    }
    wake.request_repaint();

    let mut last_heartbeat = Instant::now();
    while !stop.load(Ordering::Acquire) {
        pump_device_messages();
        let heartbeat = last_heartbeat.elapsed() >= HEARTBEAT;
        let network_changed = network_dirty.swap(false, Ordering::AcqRel);
        let device_changed = device_dirty.swap(false, Ordering::AcqRel);
        let mut sent = false;
        if network_changed || heartbeat {
            if !send_network(&sender, network_error.as_deref()) {
                return;
            }
            sent = true;
        }
        if device_changed || heartbeat {
            if !send_devices(&sender, device_error.as_deref()) {
                return;
            }
            sent = true;
        }
        if sent {
            wake.request_repaint();
        }
        if heartbeat {
            last_heartbeat = Instant::now();
        }
        thread::sleep(POLL_INTERVAL);
    }
}
