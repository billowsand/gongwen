//! macOS 环境监控：SystemConfiguration/getifaddrs 与 IOKit，均运行在后端线程的 CFRunLoop 上。

use super::{
    DeviceClass, DeviceIdentity, DeviceInfo, DeviceSnapshot, MonitorEvent, NetworkInterface,
    NetworkInterfaceKind, NetworkSnapshot,
};
use std::collections::BTreeMap;
use std::ffi::{CStr, CString, c_char, c_int, c_uint, c_void};
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

type KernReturn = c_int;
type MachPort = c_uint;
type IoObject = c_uint;
type IoService = IoObject;
type IoIterator = IoObject;
type IoNotificationPort = *mut c_void;
type CfIndex = isize;
type CfTypeRef = *const c_void;
type CfAllocatorRef = *const c_void;
type CfStringRef = *const c_void;
type CfArrayRef = *const c_void;
type CfDictionaryRef = *const c_void;
type CfMutableDictionaryRef = *mut c_void;
type CfRunLoopRef = *mut c_void;
type CfRunLoopSourceRef = *mut c_void;
type CfRunLoopTimerRef = *mut c_void;
type CfRunLoopMode = CfStringRef;
type ScDynamicStoreRef = *mut c_void;
type ScNetworkInterfaceRef = *const c_void;
type CfAbsoluteTime = f64;
type CfTimeInterval = f64;

const K_IO_RETURN_SUCCESS: KernReturn = 0;
const CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
const CF_NUMBER_SINT64_TYPE: CfIndex = 4;
const AF_INET: u8 = 2;
const AF_INET6: u8 = 30;
const IFF_UP: c_uint = 0x0001;
const IFF_LOOPBACK: c_uint = 0x0008;
const IFF_RUNNING: c_uint = 0x0040;
const HEARTBEAT_SECONDS: CfTimeInterval = 5.0;

#[repr(C)]
struct IfAddrs {
    next: *mut IfAddrs,
    name: *mut c_char,
    flags: c_uint,
    addr: *mut SockAddr,
    netmask: *mut SockAddr,
    dst_addr: *mut SockAddr,
    data: *mut c_void,
}

#[repr(C)]
struct SockAddr {
    len: u8,
    family: u8,
    data: [c_char; 14],
}

#[repr(C)]
struct InAddr {
    addr: u32,
}

#[repr(C)]
struct SockAddrIn {
    len: u8,
    family: u8,
    port: u16,
    addr: InAddr,
    zero: [c_char; 8],
}

#[repr(C)]
struct In6Addr {
    addr: [u8; 16],
}

#[repr(C)]
struct SockAddrIn6 {
    len: u8,
    family: u8,
    port: u16,
    flow_info: u32,
    addr: In6Addr,
    scope_id: u32,
}

#[repr(C)]
struct ScDynamicStoreContext {
    version: CfIndex,
    info: *mut c_void,
    retain: Option<unsafe extern "C" fn(*const c_void) -> *const c_void>,
    release: Option<unsafe extern "C" fn(*const c_void)>,
    copy_description: Option<unsafe extern "C" fn(*const c_void) -> CfStringRef>,
}

#[repr(C)]
struct CfRunLoopTimerContext {
    version: CfIndex,
    info: *mut c_void,
    retain: Option<unsafe extern "C" fn(*const c_void) -> *const c_void>,
    release: Option<unsafe extern "C" fn(*const c_void)>,
    copy_description: Option<unsafe extern "C" fn(*const c_void) -> CfStringRef>,
}

// SAFETY: 下列声明与 macOS CoreFoundation C ABI 一致；返回的 retained 对象均由 CfObject 释放。
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFRunLoopDefaultMode: CfRunLoopMode;

    fn CFRelease(value: CfTypeRef);
    fn CFGetTypeID(value: CfTypeRef) -> usize;
    fn CFStringGetTypeID() -> usize;
    fn CFNumberGetTypeID() -> usize;
    fn CFBooleanGetTypeID() -> usize;
    fn CFStringCreateWithCString(
        allocator: CfAllocatorRef,
        value: *const c_char,
        encoding: u32,
    ) -> CfStringRef;
    fn CFStringGetLength(value: CfStringRef) -> CfIndex;
    fn CFStringGetMaximumSizeForEncoding(length: CfIndex, encoding: u32) -> CfIndex;
    fn CFStringGetCString(
        value: CfStringRef,
        buffer: *mut c_char,
        buffer_size: CfIndex,
        encoding: u32,
    ) -> u8;
    fn CFStringCompare(left: CfStringRef, right: CfStringRef, options: usize) -> CfIndex;
    fn CFNumberGetValue(value: CfTypeRef, kind: CfIndex, output: *mut i64) -> u8;
    fn CFBooleanGetValue(value: CfTypeRef) -> u8;
    fn CFDictionaryGetValue(dictionary: CfDictionaryRef, key: *const c_void) -> *const c_void;
    fn CFArrayCreate(
        allocator: CfAllocatorRef,
        values: *const *const c_void,
        count: CfIndex,
        callbacks: *const c_void,
    ) -> CfArrayRef;
    fn CFArrayGetCount(array: CfArrayRef) -> CfIndex;
    fn CFArrayGetValueAtIndex(array: CfArrayRef, index: CfIndex) -> *const c_void;
    fn CFRunLoopGetCurrent() -> CfRunLoopRef;
    fn CFRunLoopAddSource(run_loop: CfRunLoopRef, source: CfRunLoopSourceRef, mode: CfRunLoopMode);
    fn CFRunLoopRemoveSource(
        run_loop: CfRunLoopRef,
        source: CfRunLoopSourceRef,
        mode: CfRunLoopMode,
    );
    fn CFRunLoopAddTimer(run_loop: CfRunLoopRef, timer: CfRunLoopTimerRef, mode: CfRunLoopMode);
    fn CFRunLoopRunInMode(
        mode: CfRunLoopMode,
        seconds: CfTimeInterval,
        return_after_source_handled: bool,
    ) -> c_int;
    fn CFAbsoluteTimeGetCurrent() -> CfAbsoluteTime;
    fn CFRunLoopTimerCreate(
        allocator: CfAllocatorRef,
        fire_date: CfAbsoluteTime,
        interval: CfTimeInterval,
        flags: usize,
        order: CfIndex,
        callout: Option<unsafe extern "C" fn(CfRunLoopTimerRef, *mut c_void)>,
        context: *mut CfRunLoopTimerContext,
    ) -> CfRunLoopTimerRef;
    fn CFRunLoopTimerInvalidate(timer: CfRunLoopTimerRef);
}

// SAFETY: 下列声明与 macOS SystemConfiguration C ABI 一致。
#[link(name = "SystemConfiguration", kind = "framework")]
unsafe extern "C" {
    static kSCNetworkInterfaceTypeEthernet: CfStringRef;
    static kSCNetworkInterfaceTypeIEEE80211: CfStringRef;
    static kSCNetworkInterfaceTypeWWAN: CfStringRef;
    static kSCNetworkInterfaceTypeIPSec: CfStringRef;
    static kSCNetworkInterfaceTypePPP: CfStringRef;
    static kSCNetworkInterfaceTypeL2TP: CfStringRef;

    fn SCNetworkInterfaceCopyAll() -> CfArrayRef;
    fn SCNetworkInterfaceGetBSDName(interface: ScNetworkInterfaceRef) -> CfStringRef;
    fn SCNetworkInterfaceGetInterfaceType(interface: ScNetworkInterfaceRef) -> CfStringRef;
    fn SCDynamicStoreCreate(
        allocator: CfAllocatorRef,
        name: CfStringRef,
        callout: Option<unsafe extern "C" fn(ScDynamicStoreRef, CfArrayRef, *mut c_void)>,
        context: *mut ScDynamicStoreContext,
    ) -> ScDynamicStoreRef;
    fn SCDynamicStoreSetNotificationKeys(
        store: ScDynamicStoreRef,
        keys: CfArrayRef,
        patterns: CfArrayRef,
    ) -> u8;
    fn SCDynamicStoreCreateRunLoopSource(
        allocator: CfAllocatorRef,
        store: ScDynamicStoreRef,
        order: CfIndex,
    ) -> CfRunLoopSourceRef;
    fn SCDynamicStoreCopyValue(store: ScDynamicStoreRef, key: CfStringRef) -> CfTypeRef;
}

// SAFETY: 下列声明与 macOS IOKit C ABI 一致；io_object_t 句柄均按所有权释放。
#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOServiceMatching(name: *const c_char) -> CfMutableDictionaryRef;
    fn IOServiceGetMatchingServices(
        main_port: MachPort,
        matching: CfMutableDictionaryRef,
        iterator: *mut IoIterator,
    ) -> KernReturn;
    fn IOServiceAddMatchingNotification(
        port: IoNotificationPort,
        notification_type: *const c_char,
        matching: CfMutableDictionaryRef,
        callback: Option<unsafe extern "C" fn(*mut c_void, IoIterator)>,
        refcon: *mut c_void,
        notification: *mut IoIterator,
    ) -> KernReturn;
    fn IONotificationPortCreate(main_port: MachPort) -> IoNotificationPort;
    fn IONotificationPortGetRunLoopSource(port: IoNotificationPort) -> CfRunLoopSourceRef;
    fn IONotificationPortDestroy(port: IoNotificationPort);
    fn IOIteratorNext(iterator: IoIterator) -> IoObject;
    fn IOObjectRelease(object: IoObject) -> KernReturn;
    fn IOObjectConformsTo(object: IoObject, class_name: *const c_char) -> c_int;
    fn IORegistryEntryGetRegistryEntryID(entry: IoObject, id: *mut u64) -> KernReturn;
    fn IORegistryEntryCreateCFProperties(
        entry: IoObject,
        properties: *mut CfMutableDictionaryRef,
        allocator: CfAllocatorRef,
        options: u32,
    ) -> KernReturn;
    fn IORegistryEntryGetChildIterator(
        entry: IoObject,
        plane: *const c_char,
        iterator: *mut IoIterator,
    ) -> KernReturn;
    fn IORegistryEntryGetParentEntry(
        entry: IoObject,
        plane: *const c_char,
        parent: *mut IoObject,
    ) -> KernReturn;
}

// SAFETY: getifaddrs/freeifaddrs 是 macOS libc 的稳定 C 接口。
unsafe extern "C" {
    fn getifaddrs(result: *mut *mut IfAddrs) -> c_int;
    fn freeifaddrs(addresses: *mut IfAddrs);
}

/// 仅包装 Create/Copy 规则返回的 retained CoreFoundation 对象。
struct CfObject(CfTypeRef);

impl CfObject {
    fn retained(value: CfTypeRef) -> Option<Self> {
        if value.is_null() {
            None
        } else {
            Some(Self(value))
        }
    }

    fn pointer(&self) -> CfTypeRef {
        self.0
    }
}

impl Drop for CfObject {
    fn drop(&mut self) {
        // SAFETY: CfObject 只持有 retained 且非空的 CoreFoundation 对象，且只释放一次。
        unsafe { CFRelease(self.0) };
    }
}

struct IoHandle(IoObject);

impl IoHandle {
    fn new(value: IoObject) -> Option<Self> {
        if value == 0 { None } else { Some(Self(value)) }
    }
}

impl Drop for IoHandle {
    fn drop(&mut self) {
        // SAFETY: IOIteratorNext 或输出参数交付一个非零 retained io_object_t，此处仅释放一次。
        unsafe {
            IOObjectRelease(self.0);
        }
    }
}

struct IfAddrsGuard(*mut IfAddrs);

impl Drop for IfAddrsGuard {
    fn drop(&mut self) {
        // SAFETY: 指针由成功的 getifaddrs 返回，在遍历结束后仅释放一次。
        unsafe { freeifaddrs(self.0) };
    }
}

struct MonitorState {
    sender: mpsc::Sender<MonitorEvent>,
    wake: eframe::egui::Context,
    closed: bool,
    device_events_ready: bool,
}

impl MonitorState {
    fn send(&mut self, event: MonitorEvent) {
        if self.closed {
            return;
        }
        if self.sender.send(event).is_err() {
            self.closed = true;
            return;
        }
        self.wake.request_repaint();
    }
}

fn emit_network(state: &mut MonitorState) {
    state.send(MonitorEvent::Network(collect_network()));
}

fn emit_devices(state: &mut MonitorState) {
    state.send(MonitorEvent::Devices(collect_devices()));
}

// SAFETY: info 在 run 内指向固定堆地址的 MonitorState；CFRunLoop 只在该线程串行回调。
unsafe extern "C" fn network_changed(
    _store: ScDynamicStoreRef,
    _changed_keys: CfArrayRef,
    info: *mut c_void,
) {
    // SAFETY: info 非空时指向 run 内仍存活的 MonitorState，且回调仅在此线程发生。
    let state = unsafe { (info as *mut MonitorState).as_mut() };
    if let Some(state) = state {
        emit_network(state);
    }
}

// SAFETY: refcon 的生命周期覆盖通知注册；迭代器只由回调排空，注册句柄统一释放。
unsafe extern "C" fn device_changed(refcon: *mut c_void, iterator: IoIterator) {
    loop {
        // SAFETY: iterator 是此通知注册持有的有效迭代器，回调只读取并释放返回对象。
        let object = unsafe { IOIteratorNext(iterator) };
        let Some(object) = IoHandle::new(object) else {
            break;
        };
        drop(object);
    }
    // SAFETY: refcon 非空时指向仍存活的 MonitorState，通知先于此状态销毁。
    let state = unsafe { (refcon as *mut MonitorState).as_mut() };
    if let Some(state) = state
        && state.device_events_ready
    {
        emit_devices(state);
    }
}

// SAFETY: info 指向 run 的 MonitorState；定时器先于 MonitorState 释放。
unsafe extern "C" fn heartbeat(_timer: CfRunLoopTimerRef, info: *mut c_void) {
    // SAFETY: info 非空时指向仍存活的 MonitorState，timer 销毁前不会再回调。
    let state = unsafe { (info as *mut MonitorState).as_mut() };
    if let Some(state) = state {
        emit_network(state);
        emit_devices(state);
    }
}

struct NetworkNotifications {
    run_loop: CfRunLoopRef,
    _store: CfObject,
    source: CfObject,
}

impl NetworkNotifications {
    fn install(run_loop: CfRunLoopRef, state: *mut MonitorState) -> Result<Self, String> {
        let name = cf_string(c"GongwenAssistant.NetworkMonitor")
            .ok_or_else(|| "无法创建网络监控名称".to_owned())?;
        let mut context = ScDynamicStoreContext {
            version: 0,
            info: state.cast(),
            retain: None,
            release: None,
            copy_description: None,
        };
        // SAFETY: name 在调用期间有效；context 由 SystemConfiguration 复制，info 生命周期覆盖 store。
        let store = unsafe {
            SCDynamicStoreCreate(
                ptr::null(),
                name.pointer(),
                Some(network_changed),
                &mut context,
            )
        };
        let store =
            CfObject::retained(store).ok_or_else(|| "SCDynamicStoreCreate 失败".to_owned())?;
        let pattern_text = [
            c"State:/Network/Interface/.*/Link",
            c"State:/Network/Interface/.*/IPv4",
            c"State:/Network/Interface/.*/IPv6",
            c"State:/Network/Global/.*",
        ];
        let patterns = pattern_text
            .iter()
            .map(|text| cf_string(text).ok_or_else(|| "无法创建网络通知键".to_owned()))
            .collect::<Result<Vec<_>, _>>()?;
        let pattern_array =
            cf_array(&patterns).ok_or_else(|| "无法创建网络通知键数组".to_owned())?;
        // SAFETY: store 与数组有效；设置会复制键，局部 CFString 随后可安全释放。
        if unsafe {
            SCDynamicStoreSetNotificationKeys(
                store.pointer() as ScDynamicStoreRef,
                ptr::null(),
                pattern_array.pointer(),
            )
        } == 0
        {
            return Err("SCDynamicStoreSetNotificationKeys 失败".to_owned());
        }
        // SAFETY: store 有效，返回的 run-loop source 按 Create 规则由 CfObject 持有。
        let source = unsafe {
            SCDynamicStoreCreateRunLoopSource(ptr::null(), store.pointer() as ScDynamicStoreRef, 0)
        };
        let source = CfObject::retained(source)
            .ok_or_else(|| "SCDynamicStoreCreateRunLoopSource 失败".to_owned())?;
        // SAFETY: run_loop 属于当前线程，source 有效且 default mode 非空。
        unsafe {
            CFRunLoopAddSource(
                run_loop,
                source.pointer() as CfRunLoopSourceRef,
                kCFRunLoopDefaultMode,
            )
        };
        Ok(Self {
            run_loop,
            _store: store,
            source,
        })
    }
}

impl Drop for NetworkNotifications {
    fn drop(&mut self) {
        // SAFETY: source 此前加入该 run loop；先移除，随后字段按 RAII 释放。
        unsafe {
            CFRunLoopRemoveSource(
                self.run_loop,
                self.source.pointer() as CfRunLoopSourceRef,
                kCFRunLoopDefaultMode,
            )
        };
    }
}

struct DeviceNotifications {
    run_loop: CfRunLoopRef,
    port: IoNotificationPort,
    source: CfRunLoopSourceRef,
    iterators: Vec<IoHandle>,
}

impl DeviceNotifications {
    fn install(run_loop: CfRunLoopRef, state: *mut MonitorState) -> Result<Self, String> {
        // SAFETY: master port 0 是 kIOMasterPortDefault；返回的 notification port 由本结构销毁。
        let port = unsafe { IONotificationPortCreate(0) };
        if port.is_null() {
            return Err("IONotificationPortCreate 失败".to_owned());
        }
        // SAFETY: port 有效，source 由 notification port 持有，不额外 CFRelease。
        let source = unsafe { IONotificationPortGetRunLoopSource(port) };
        if source.is_null() {
            // SAFETY: port 已创建且尚未转移所有权。
            unsafe { IONotificationPortDestroy(port) };
            return Err("IONotificationPortGetRunLoopSource 失败".to_owned());
        }
        let mut result = Self {
            run_loop,
            port,
            source,
            iterators: Vec::new(),
        };
        // SAFETY: run_loop、source 与 mode 均有效；Drop 会移除 source。
        unsafe { CFRunLoopAddSource(run_loop, source, kCFRunLoopDefaultMode) };
        for service in [c"IOUSBDevice", c"IOMedia"] {
            for notification in [c"IOServiceMatched", c"IOServiceTerminate"] {
                let iterator = register_device_notification(port, notification, service, state)?;
                result.iterators.push(iterator);
            }
        }
        Ok(result)
    }
}

impl Drop for DeviceNotifications {
    fn drop(&mut self) {
        // SAFETY: 先停止 run-loop 回调并释放通知迭代器，最后销毁持有 source 的 port。
        unsafe {
            CFRunLoopRemoveSource(self.run_loop, self.source, kCFRunLoopDefaultMode);
        }
        self.iterators.clear();
        // SAFETY: port 由本结构唯一持有，且所有迭代器已经释放。
        unsafe { IONotificationPortDestroy(self.port) };
    }
}

struct Heartbeat {
    timer: CfRunLoopTimerRef,
}

impl Heartbeat {
    fn install(run_loop: CfRunLoopRef, state: *mut MonitorState) -> Result<Self, String> {
        let mut context = CfRunLoopTimerContext {
            version: 0,
            info: state.cast(),
            retain: None,
            release: None,
            copy_description: None,
        };
        // SAFETY: context 被 timer 复制；info 生命周期覆盖 timer，时间参数为有限的当前时间加五秒。
        let timer = unsafe {
            CFRunLoopTimerCreate(
                ptr::null(),
                CFAbsoluteTimeGetCurrent() + HEARTBEAT_SECONDS,
                HEARTBEAT_SECONDS,
                0,
                0,
                Some(heartbeat),
                &mut context,
            )
        };
        if timer.is_null() {
            return Err("CFRunLoopTimerCreate 失败".to_owned());
        }
        // SAFETY: timer 与当前线程 run loop 有效；Heartbeat::drop 负责 invalidate/release。
        unsafe { CFRunLoopAddTimer(run_loop, timer, kCFRunLoopDefaultMode) };
        Ok(Self { timer })
    }
}

impl Drop for Heartbeat {
    fn drop(&mut self) {
        // SAFETY: invalidate 会将 timer 从所有 run loop 移除；随后释放 Create 持有的引用。
        unsafe {
            CFRunLoopTimerInvalidate(self.timer);
            CFRelease(self.timer);
        }
    }
}

pub(crate) fn run(
    sender: mpsc::Sender<MonitorEvent>,
    stop: Arc<AtomicBool>,
    wake: eframe::egui::Context,
) {
    let mut state = Box::new(MonitorState {
        sender,
        wake,
        closed: false,
        device_events_ready: false,
    });
    let state_pointer = (&mut *state) as *mut MonitorState;

    emit_network(&mut state);
    emit_devices(&mut state);

    // SAFETY: 当前平台线程已有（或可由 CoreFoundation 创建）线程专属 run loop。
    let run_loop = unsafe { CFRunLoopGetCurrent() };
    if run_loop.is_null() {
        state.send(MonitorEvent::Network(Err(
            "CFRunLoopGetCurrent 失败".to_owned()
        )));
        state.send(MonitorEvent::Devices(Err(
            "CFRunLoopGetCurrent 失败".to_owned()
        )));
        return;
    }

    let network_notifications = match NetworkNotifications::install(run_loop, state_pointer) {
        Ok(value) => Some(value),
        Err(error) => {
            state.send(MonitorEvent::Network(Err(error)));
            None
        }
    };
    let device_notifications = match DeviceNotifications::install(run_loop, state_pointer) {
        Ok(value) => Some(value),
        Err(error) => {
            state.send(MonitorEvent::Devices(Err(error)));
            None
        }
    };
    state.device_events_ready = true;
    if device_notifications.is_some() {
        // 通知注册期间的初始迭代只用于武装通知，此处重新对账一次。
        emit_devices(&mut state);
    }

    let heartbeat = match Heartbeat::install(run_loop, state_pointer) {
        Ok(value) => value,
        Err(error) => {
            state.send(MonitorEvent::Network(Err(error.clone())));
            state.send(MonitorEvent::Devices(Err(error)));
            return;
        }
    };

    while !stop.load(Ordering::Acquire) && !state.closed {
        // SAFETY: 所有 source/timer 都在当前 run loop 上；100ms 上限保证 stop 能及时生效。
        unsafe {
            CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.1, true);
        }
    }

    drop(heartbeat);
    drop(device_notifications);
    drop(network_notifications);
}

fn cf_string(value: &CStr) -> Option<CfObject> {
    // SAFETY: value 是 NUL 结尾的 C 字符串，UTF-8 编码常量有效；返回对象按 Create 规则持有。
    let string =
        unsafe { CFStringCreateWithCString(ptr::null(), value.as_ptr(), CF_STRING_ENCODING_UTF8) };
    CfObject::retained(string)
}

fn cf_array(values: &[CfObject]) -> Option<CfObject> {
    let pointers = values.iter().map(CfObject::pointer).collect::<Vec<_>>();
    // SAFETY: pointers 在调用期间连续有效；无 callbacks 的临时数组不取得元素所有权。
    let array = unsafe {
        CFArrayCreate(
            ptr::null(),
            pointers.as_ptr(),
            pointers.len() as CfIndex,
            ptr::null(),
        )
    };
    CfObject::retained(array)
}

fn cf_string_value(value: CfStringRef) -> Option<String> {
    if value.is_null() {
        return None;
    }
    // SAFETY: value 已检查非空且由调用方保证为 CFString；类型检查避免错误属性类型。
    if unsafe { CFGetTypeID(value) != CFStringGetTypeID() } {
        return None;
    }
    // SAFETY: value 是有效 CFString。
    let length = unsafe { CFStringGetLength(value) };
    // SAFETY: 同一字符串与 UTF-8 编码用于计算最大缓冲区。
    let size = unsafe { CFStringGetMaximumSizeForEncoding(length, CF_STRING_ENCODING_UTF8) }
        .checked_add(1)?;
    let mut buffer = vec![0_u8; usize::try_from(size).ok()?];
    // SAFETY: buffer 有 size 字节可写，字符串有效；成功后缓冲区保证 NUL 结尾。
    if unsafe {
        CFStringGetCString(
            value,
            buffer.as_mut_ptr().cast(),
            size,
            CF_STRING_ENCODING_UTF8,
        )
    } == 0
    {
        return None;
    }
    let length = buffer.iter().position(|byte| *byte == 0)?;
    String::from_utf8(buffer[..length].to_vec()).ok()
}

fn cf_strings_equal(left: CfStringRef, right: CfStringRef) -> bool {
    !left.is_null() && !right.is_null() &&
        // SAFETY: 两个非空值都是 SystemConfiguration 返回的 CFString。
        unsafe { CFStringCompare(left, right, 0) == 0 }
}

fn dictionary_value(dictionary: CfDictionaryRef, key: &CStr) -> *const c_void {
    let Some(key) = cf_string(key) else {
        return ptr::null();
    };
    // SAFETY: dictionary 为有效 CFDictionary，key 在调用期间存活；返回值为 borrowed。
    unsafe { CFDictionaryGetValue(dictionary, key.pointer()) }
}

fn property_i64(dictionary: CfDictionaryRef, key: &CStr) -> Option<i64> {
    let value = dictionary_value(dictionary, key);
    if value.is_null() {
        return None;
    }
    // SAFETY: 仅在类型确认为 CFNumber 后按 CFNumber 读取。
    if unsafe { CFGetTypeID(value) != CFNumberGetTypeID() } {
        return None;
    }
    let mut output = 0_i64;
    // SAFETY: output 是 SInt64 的可写位置，value 为有效 CFNumber。
    (unsafe { CFNumberGetValue(value, CF_NUMBER_SINT64_TYPE, &mut output) } != 0).then_some(output)
}

fn property_bool(dictionary: CfDictionaryRef, key: &CStr) -> bool {
    let value = dictionary_value(dictionary, key);
    if value.is_null() {
        return false;
    }
    // SAFETY: 先比较动态类型，再调用对应的 CFBoolean/CFNumber 读取函数。
    unsafe {
        if CFGetTypeID(value) == CFBooleanGetTypeID() {
            CFBooleanGetValue(value) != 0
        } else if CFGetTypeID(value) == CFNumberGetTypeID() {
            let mut output = 0_i64;
            CFNumberGetValue(value, CF_NUMBER_SINT64_TYPE, &mut output) != 0 && output != 0
        } else {
            false
        }
    }
}

fn property_string(dictionary: CfDictionaryRef, key: &CStr) -> Option<String> {
    cf_string_value(dictionary_value(dictionary, key))
}

fn registry_id(entry: IoObject) -> Result<u64, String> {
    let mut id = 0_u64;
    // SAFETY: entry 在调用期间有效，id 是可写输出位置。
    let result = unsafe { IORegistryEntryGetRegistryEntryID(entry, &mut id) };
    if result == K_IO_RETURN_SUCCESS {
        Ok(id)
    } else {
        Err(format!(
            "IORegistryEntryGetRegistryEntryID 失败: 0x{result:08x}"
        ))
    }
}

fn properties(entry: IoObject) -> Result<CfObject, String> {
    let mut dictionary = ptr::null_mut();
    // SAFETY: entry 有效，dictionary 为输出位置；成功后按 Create 规则持有。
    let result =
        unsafe { IORegistryEntryCreateCFProperties(entry, &mut dictionary, ptr::null(), 0) };
    if result != K_IO_RETURN_SUCCESS {
        return Err(format!(
            "IORegistryEntryCreateCFProperties 失败: 0x{result:08x}"
        ));
    }
    CfObject::retained(dictionary).ok_or_else(|| "IOKit 返回空属性字典".to_owned())
}

fn matching_services(service: &CStr) -> Result<Option<IoHandle>, String> {
    // SAFETY: service 为静态 NUL 结尾字符串；matching dictionary 交由 GetMatchingServices 消耗。
    let matching = unsafe { IOServiceMatching(service.as_ptr()) };
    if matching.is_null() {
        return Err(format!(
            "IOServiceMatching({}) 失败",
            service.to_string_lossy()
        ));
    }
    let mut iterator = 0;
    // SAFETY: matching 有效且所有权被消耗，iterator 是有效输出位置。
    let result = unsafe { IOServiceGetMatchingServices(0, matching, &mut iterator) };
    if result != K_IO_RETURN_SUCCESS {
        return Err(format!("IOServiceGetMatchingServices 失败: 0x{result:08x}"));
    }
    Ok(IoHandle::new(iterator))
}

fn network_interface_kinds() -> Result<BTreeMap<String, NetworkInterfaceKind>, String> {
    // SAFETY: 无参数 CopyAll 返回 retained CFArray。
    let interfaces = unsafe { SCNetworkInterfaceCopyAll() };
    let interfaces = CfObject::retained(interfaces)
        .ok_or_else(|| "SCNetworkInterfaceCopyAll 失败".to_owned())?;
    // SAFETY: interfaces 已确认非空且来自 SCNetworkInterfaceCopyAll。
    let count = unsafe { CFArrayGetCount(interfaces.pointer()) };
    let mut result = BTreeMap::new();
    for index in 0..count {
        // SAFETY: index 在数组范围内，元素按 SystemConfiguration 约定为 SCNetworkInterfaceRef。
        let interface = unsafe { CFArrayGetValueAtIndex(interfaces.pointer(), index) };
        // SAFETY: 元素是有效 SCNetworkInterface，返回字符串为 borrowed。
        let bsd_name = unsafe { cf_string_value(SCNetworkInterfaceGetBSDName(interface)) };
        // SAFETY: 同上，接口类型字符串在数组生命周期内有效。
        let kind = unsafe { SCNetworkInterfaceGetInterfaceType(interface) };
        let Some(name) = bsd_name else {
            continue;
        };
        // SAFETY: 比较的静态常量由 SystemConfiguration 框架提供且进程生命周期内有效。
        let kind = unsafe {
            if cf_strings_equal(kind, kSCNetworkInterfaceTypeIEEE80211) {
                NetworkInterfaceKind::Wifi
            } else if cf_strings_equal(kind, kSCNetworkInterfaceTypeWWAN) {
                NetworkInterfaceKind::Cellular
            } else if cf_strings_equal(kind, kSCNetworkInterfaceTypeIPSec)
                || cf_strings_equal(kind, kSCNetworkInterfaceTypePPP)
                || cf_strings_equal(kind, kSCNetworkInterfaceTypeL2TP)
            {
                NetworkInterfaceKind::Vpn
            } else if cf_strings_equal(kind, kSCNetworkInterfaceTypeEthernet) {
                NetworkInterfaceKind::Ethernet
            } else {
                infer_interface_kind(&name)
            }
        };
        result.insert(name, kind);
    }
    Ok(result)
}

fn infer_interface_kind(name: &str) -> NetworkInterfaceKind {
    if name.starts_with("utun")
        || name.starts_with("tun")
        || name.starts_with("tap")
        || name.starts_with("ppp")
        || name.starts_with("ipsec")
    {
        NetworkInterfaceKind::Vpn
    } else if name.starts_with("wlan")
        || name.starts_with("wifi")
        || name.starts_with("awdl")
        || name.starts_with("llw")
    {
        NetworkInterfaceKind::Wifi
    } else if name.starts_with("rmnet") || name.starts_with("pdp_ip") {
        NetworkInterfaceKind::Cellular
    } else if name.starts_with("en") {
        NetworkInterfaceKind::Ethernet
    } else {
        NetworkInterfaceKind::Other
    }
}

fn collect_network() -> Result<NetworkSnapshot, String> {
    let kinds = network_interface_kinds()?;
    let mut head = ptr::null_mut();
    // SAFETY: head 是有效输出指针；成功后由 IfAddrsGuard 释放整条链表。
    if unsafe { getifaddrs(&mut head) } != 0 {
        return Err(format!(
            "getifaddrs 失败: {}",
            std::io::Error::last_os_error()
        ));
    }
    let _guard = IfAddrsGuard(head);
    let mut interfaces: BTreeMap<String, NetworkInterface> = BTreeMap::new();
    let mut current = head;
    // SAFETY: current 始终指向 getifaddrs 链表节点或空，链表由 guard 保持存活。
    while let Some(address) = unsafe { current.as_ref() } {
        if !address.name.is_null() {
            // SAFETY: ifa_name 由 getifaddrs 提供为 NUL 结尾字符串，生命周期覆盖整个遍历。
            let name = unsafe { CStr::from_ptr(address.name) }
                .to_string_lossy()
                .into_owned();
            let loopback = address.flags & IFF_LOOPBACK != 0;
            let entry = interfaces
                .entry(name.clone())
                .or_insert_with(|| NetworkInterface {
                    id: name.clone(),
                    kind: kinds
                        .get(&name)
                        .copied()
                        .unwrap_or_else(|| infer_interface_kind(&name)),
                    loopback,
                    link_up: false,
                    non_loopback_address: false,
                });
            entry.loopback |= loopback;
            entry.link_up |= address.flags & (IFF_UP | IFF_RUNNING) == (IFF_UP | IFF_RUNNING);
            if !entry.loopback && has_non_loopback_address(address.addr) {
                entry.non_loopback_address = true;
            }
        }
        current = address.next;
    }

    let name = cf_string(c"GongwenAssistant.RouteLookup")
        .ok_or_else(|| "无法创建路由查询名称".to_owned())?;
    // SAFETY: 无 callback 的查询 store 不需要 context；name 在调用期间有效。
    let store = unsafe { SCDynamicStoreCreate(ptr::null(), name.pointer(), None, ptr::null_mut()) };
    let store = CfObject::retained(store).ok_or_else(|| "无法创建默认路由查询 store".to_owned())?;
    for interface in interfaces
        .values_mut()
        .filter(|interface| !interface.loopback)
    {
        let Ok(key) = CString::new(format!("State:/Network/Interface/{}/Link", interface.id))
        else {
            continue;
        };
        let Some(key) = cf_string(&key) else {
            continue;
        };
        // SAFETY: store/key 有效，Link 实体按 SystemConfiguration 约定是含 Active 布尔值的字典。
        let value =
            unsafe { SCDynamicStoreCopyValue(store.pointer() as ScDynamicStoreRef, key.pointer()) };
        if let Some(value) = CfObject::retained(value) {
            // 物理接口以 Link.Active 为准；没有 Link 实体的 VPN 等虚拟接口保留 getifaddrs 标志。
            interface.link_up = property_bool(value.pointer(), c"Active");
        }
    }
    let default_route = [c"State:/Network/Global/IPv4", c"State:/Network/Global/IPv6"]
        .into_iter()
        .any(|key| {
            let Some(key) = cf_string(key) else {
                return false;
            };
            // SAFETY: store/key 有效；CopyValue 返回对象按 Copy 规则立即由 CfObject 释放。
            let value = unsafe {
                SCDynamicStoreCopyValue(store.pointer() as ScDynamicStoreRef, key.pointer())
            };
            CfObject::retained(value).is_some()
        });
    Ok(NetworkSnapshot {
        interfaces: interfaces.into_values().collect(),
        default_route,
    })
}

fn has_non_loopback_address(address: *mut SockAddr) -> bool {
    if address.is_null() {
        return false;
    }
    // SAFETY: getifaddrs 的 ifa_addr 非空时指向 sockaddr，先读取公共 family 字段。
    match unsafe { (*address).family } {
        AF_INET => {
            // SAFETY: family 为 AF_INET，因此实际对象是 sockaddr_in。
            let bytes = unsafe { (*(address as *const SockAddrIn)).addr.addr.to_ne_bytes() };
            bytes != [0; 4] && bytes[0] != 127
        }
        AF_INET6 => {
            // SAFETY: family 为 AF_INET6，因此实际对象是 sockaddr_in6。
            let bytes = unsafe { (*(address as *const SockAddrIn6)).addr.addr };
            let loopback = bytes[..15].iter().all(|byte| *byte == 0) && bytes[15] == 1;
            bytes != [0; 16] && !loopback
        }
        _ => false,
    }
}

struct UsbClasses {
    classes: Vec<DeviceClass>,
    only_hub: bool,
}

impl UsbClasses {
    fn add(&mut self, dictionary: CfDictionaryRef) {
        let class = property_i64(dictionary, c"bDeviceClass")
            .or_else(|| property_i64(dictionary, c"bInterfaceClass"));
        let subclass = property_i64(dictionary, c"bDeviceSubClass")
            .or_else(|| property_i64(dictionary, c"bInterfaceSubClass"));
        let protocol = property_i64(dictionary, c"bDeviceProtocol")
            .or_else(|| property_i64(dictionary, c"bInterfaceProtocol"));
        let label = property_string(dictionary, c"USB Interface Name")
            .or_else(|| property_string(dictionary, c"kUSBString"))
            .or_else(|| property_string(dictionary, c"USB Product Name"))
            .or_else(|| property_string(dictionary, c"kUSBProductString"))
            .unwrap_or_default()
            .to_ascii_uppercase();
        let explicitly_portable = label.contains("MTP") || label.contains("PTP");

        let Some(class) = class else {
            return;
        };
        match class {
            0 => {}
            3 => self.push(DeviceClass::HumanInterface),
            6 => {
                self.push(DeviceClass::Imaging);
                if (subclass == Some(1) && protocol == Some(1)) || explicitly_portable {
                    self.push(DeviceClass::PortableMedia);
                }
            }
            8 => self.push(DeviceClass::RemovableStorage),
            9 => {}
            14 => self.push(DeviceClass::Imaging),
            _ => self.push(DeviceClass::Other),
        }
        if explicitly_portable {
            self.push(DeviceClass::PortableMedia);
        }
        self.only_hub = class == 9 && self.classes.is_empty();
    }

    fn push(&mut self, class: DeviceClass) {
        if !self.classes.contains(&class) {
            self.classes.push(class);
        }
    }
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

fn insert_device(
    devices: &mut BTreeMap<(String, u8), DeviceInfo>,
    id: String,
    class: DeviceClass,
    identity: Option<DeviceIdentity>,
) {
    let key = (id.clone(), class_rank(class));
    devices
        .entry(key)
        .and_modify(|device| {
            if device.identity.is_none() {
                device.identity = identity.clone();
            }
        })
        .or_insert(DeviceInfo {
            id,
            class,
            identity,
        });
}

fn usb_identity(dictionary: CfDictionaryRef) -> Option<DeviceIdentity> {
    let serial = property_string(dictionary, c"USB Serial Number")
        .or_else(|| property_string(dictionary, c"kUSBSerialNumberString"))?
        .trim()
        .to_owned();
    if serial.is_empty() {
        return None;
    }
    let revision = property_i64(dictionary, c"bcdDevice")?;
    (revision >= 0).then_some(DeviceIdentity {
        serial,
        revision: revision as u64,
    })
}

fn collect_usb_device(
    device: IoService,
    devices: &mut BTreeMap<(String, u8), DeviceInfo>,
) -> Result<(), String> {
    let id = format!("usb:{}", registry_id(device)?);
    let dictionary = properties(device)?;
    let identity = usb_identity(dictionary.pointer());
    let mut classes = UsbClasses {
        classes: Vec::new(),
        only_hub: property_i64(dictionary.pointer(), c"bDeviceClass") == Some(9),
    };
    classes.add(dictionary.pointer());
    collect_usb_children(device, 0, &mut classes)?;
    if classes.classes.is_empty() && !classes.only_hub {
        classes.push(DeviceClass::Other);
    }
    for class in classes.classes {
        let identity = (class == DeviceClass::RemovableStorage)
            .then(|| identity.clone())
            .flatten();
        insert_device(devices, id.clone(), class, identity);
    }
    Ok(())
}

fn collect_usb_children(
    parent: IoService,
    depth: u32,
    classes: &mut UsbClasses,
) -> Result<(), String> {
    if depth >= 16 {
        return Ok(());
    }
    let mut raw_iterator = 0;
    // SAFETY: parent 有效，IOService plane 名称为静态 C 字符串，iterator 为有效输出位置。
    let result = unsafe {
        IORegistryEntryGetChildIterator(parent, c"IOService".as_ptr(), &mut raw_iterator)
    };
    if result != K_IO_RETURN_SUCCESS {
        return Err(format!(
            "IORegistryEntryGetChildIterator 失败: 0x{result:08x}"
        ));
    }
    let Some(iterator) = IoHandle::new(raw_iterator) else {
        return Ok(());
    };
    loop {
        // SAFETY: iterator 有效，返回对象由 IoHandle 释放。
        let child = unsafe { IOIteratorNext(iterator.0) };
        let Some(child) = IoHandle::new(child) else {
            break;
        };
        // Hub 下游的 IOUSBDevice 会由顶层迭代单独处理，不能并入当前复合设备。
        // SAFETY: child 有效，IOObjectConformsTo 包含 IOUSBDevice 子类。
        if unsafe { IOObjectConformsTo(child.0, c"IOUSBDevice".as_ptr()) } != 0 {
            continue;
        }
        let dictionary = properties(child.0)?;
        classes.add(dictionary.pointer());
        collect_usb_children(child.0, depth + 1, classes)?;
    }
    Ok(())
}

fn collect_usb_devices(devices: &mut BTreeMap<(String, u8), DeviceInfo>) -> Result<(), String> {
    let Some(iterator) = matching_services(c"IOUSBDevice")? else {
        return Ok(());
    };
    loop {
        // SAFETY: iterator 有效，返回对象由 IoHandle 释放。
        let device = unsafe { IOIteratorNext(iterator.0) };
        let Some(device) = IoHandle::new(device) else {
            break;
        };
        collect_usb_device(device.0, devices)?;
    }
    Ok(())
}

fn usb_ancestor(media: IoObject) -> Result<Option<(String, Option<DeviceIdentity>)>, String> {
    let mut current = media;
    let mut current_owner = None;
    for _ in 0..32 {
        let mut parent = 0;
        // SAFETY: current 由 media 或 current_owner 保证存活，parent 是有效输出位置。
        let result =
            unsafe { IORegistryEntryGetParentEntry(current, c"IOService".as_ptr(), &mut parent) };
        if result != K_IO_RETURN_SUCCESS {
            return if current_owner.is_none() {
                Ok(None)
            } else {
                Err(format!(
                    "IORegistryEntryGetParentEntry 失败: 0x{result:08x}"
                ))
            };
        }
        let Some(parent) = IoHandle::new(parent) else {
            return Ok(None);
        };
        // SAFETY: parent 有效，IOObjectConformsTo 会检查 IOUSBDevice 及其子类。
        if unsafe { IOObjectConformsTo(parent.0, c"IOUSBDevice".as_ptr()) } != 0 {
            let id = format!("usb:{}", registry_id(parent.0)?);
            let dictionary = properties(parent.0)?;
            return Ok(Some((id, usb_identity(dictionary.pointer()))));
        }
        current = parent.0;
        // 新 owner 替换旧 owner 后，current 指向仍存活的 parent；旧祖先不再使用。
        current_owner = Some(parent);
    }
    Ok(None)
}

fn collect_removable_media(devices: &mut BTreeMap<(String, u8), DeviceInfo>) -> Result<(), String> {
    let Some(iterator) = matching_services(c"IOMedia")? else {
        return Ok(());
    };
    loop {
        // SAFETY: iterator 有效，返回对象由 IoHandle 释放。
        let media = unsafe { IOIteratorNext(iterator.0) };
        let Some(media) = IoHandle::new(media) else {
            break;
        };
        let dictionary = properties(media.0)?;
        let leaf = property_bool(dictionary.pointer(), c"Leaf");
        let removable = property_bool(dictionary.pointer(), c"Removable")
            || property_bool(dictionary.pointer(), c"Ejectable")
            || property_bool(dictionary.pointer(), c"External");
        if !leaf || !removable {
            continue;
        }
        let (id, identity) = match usb_ancestor(media.0)? {
            Some(value) => value,
            None => (format!("media:{}", registry_id(media.0)?), None),
        };
        insert_device(devices, id, DeviceClass::RemovableStorage, identity);
    }
    Ok(())
}

fn collect_devices() -> Result<DeviceSnapshot, String> {
    let mut devices = BTreeMap::new();
    collect_usb_devices(&mut devices)?;
    collect_removable_media(&mut devices)?;
    Ok(DeviceSnapshot {
        devices: devices.into_values().collect(),
    })
}

fn register_device_notification(
    port: IoNotificationPort,
    notification: &CStr,
    service: &CStr,
    state: *mut MonitorState,
) -> Result<IoHandle, String> {
    // SAFETY: service 为静态 C 字符串，matching dictionary 由 AddMatchingNotification 消耗。
    let matching = unsafe { IOServiceMatching(service.as_ptr()) };
    if matching.is_null() {
        return Err(format!(
            "IOServiceMatching({}) 失败",
            service.to_string_lossy()
        ));
    }
    let mut iterator = 0;
    // SAFETY: port/matching 有效，state 生命周期覆盖注册，iterator 是有效输出位置。
    let result = unsafe {
        IOServiceAddMatchingNotification(
            port,
            notification.as_ptr(),
            matching,
            Some(device_changed),
            state.cast(),
            &mut iterator,
        )
    };
    if result != K_IO_RETURN_SUCCESS {
        return Err(format!(
            "IOServiceAddMatchingNotification 失败: 0x{result:08x}"
        ));
    }
    IoHandle::new(iterator).ok_or_else(|| "IOKit 返回空通知迭代器".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_native_objects_do_not_reach_release() {
        assert!(CfObject::retained(ptr::null()).is_none());
        assert!(IoHandle::new(0).is_none());
    }
}
