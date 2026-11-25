use alloc::format;
use alloc::string::String;
use core::ffi::c_void;

use uefi::boot::{self, OpenProtocolAttributes, OpenProtocolParams};
use uefi::data_types::Guid;
use uefi::proto::device_path::text::{AllowShortcuts, DevicePathToText, DisplayOnly};
use uefi::proto::device_path::DevicePath;
use uefi::proto::unsafe_protocol;
use uefi::table::cfg;
use uefi::{guid, system, Handle};
use uefi_raw::Status as RawStatus;

/// Linux ARM64 Device Tree UEFI Configuration Table GUID
pub const DTB_TABLE_GUID: Guid = guid!("b1b621d5-f19c-41a5-830b-d9152c69aae0");

/// Linux EFI Stub Initrd Media Vendor Device Path GUID
pub const LINUX_EFI_INITRD_MEDIA_GUID: Guid = guid!("5568e427-68fc-4f3d-ac74-ca555231cc68");

/// Standard EFI_DEVICE_PATH_PROTOCOL GUID
pub const DEVICE_PATH_PROTOCOL_GUID: Guid = guid!("09576e91-6d3f-11d2-8e39-00a0c969723b");

/// Standard EFI_LOAD_FILE2_PROTOCOL GUID
pub const LOAD_FILE2_PROTOCOL_GUID: Guid = guid!("4006c0c1-fcb3-403e-996d-4a6c8724e06d");

/// Custom vendor GUID for gaokunBoot NVRAM settings
pub const GAOKUN_VENDOR_GUID: Guid = guid!("6a0c4b10-8280-4a30-9e00-67616f6b756e");

/// Qualcomm / Huawei MateBook E Go hardware Power button scan code (discovered by gaokun-probe)
pub const SCAN_GAOKUN_POWER: u16 = 0x0102;

/// UEFI 2.10 §12.7 EFI_ABSOLUTE_POINTER_MODE
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct AbsolutePointerMode {
    pub absolute_min_x: u64,
    pub absolute_min_y: u64,
    pub absolute_min_z: u64,
    pub absolute_max_x: u64,
    pub absolute_max_y: u64,
    pub absolute_max_z: u64,
    pub attributes: u32,
}

/// UEFI 2.10 §12.7 EFI_ABSOLUTE_POINTER_STATE
#[repr(C)]
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct AbsolutePointerState {
    pub current_x: u64,
    pub current_y: u64,
    pub current_z: u64,
    pub active_buttons: u32,
}

/// UEFI 2.10 §12.7 EFI_ABSOLUTE_POINTER_PROTOCOL
#[repr(C)]
#[unsafe_protocol("8d59d32b-c655-4ae9-9b15-f25904992a43")]
pub struct AbsolutePointer {
    pub reset: unsafe extern "efiapi" fn(this: *mut Self, extended_verification: u8) -> RawStatus,
    pub get_state: unsafe extern "efiapi" fn(
        this: *mut Self,
        state: *mut AbsolutePointerState,
    ) -> RawStatus,
    pub wait_for_input: *mut c_void,
    pub mode: *const AbsolutePointerMode,
}

impl AbsolutePointer {
    pub fn reset_device(&mut self, extended: bool) -> RawStatus {
        unsafe { (self.reset)(self as *mut Self, if extended { 1 } else { 0 }) }
    }

    pub fn mode(&self) -> Option<AbsolutePointerMode> {
        if self.mode.is_null() {
            None
        } else {
            Some(unsafe { *self.mode })
        }
    }

    pub fn read_state(&mut self) -> Result<Option<AbsolutePointerState>, RawStatus> {
        let mut st = AbsolutePointerState::default();
        let status = unsafe { (self.get_state)(self as *mut Self, &mut st) };
        if status == RawStatus::SUCCESS {
            Ok(Some(st))
        } else if status == RawStatus::NOT_READY {
            Ok(None)
        } else {
            Err(status)
        }
    }
}

/// Safely call UEFI BootServices->CheckEvent on a firmware-owned EFI_EVENT without closing it.
pub fn check_raw_event(event: *mut c_void) -> RawStatus {
    if event.is_null() {
        return RawStatus::INVALID_PARAMETER;
    }
    let Some(st) = uefi::table::system_table_raw() else {
        return RawStatus::UNSUPPORTED;
    };
    unsafe {
        let bs = (*st.as_ptr()).boot_services;
        if bs.is_null() {
            return RawStatus::UNSUPPORTED;
        }
        ((*bs).check_event)(event)
    }
}

/// Huawei MateBook E Go (gaokun3) I2cTouchPanel.efi private wakeup protocol.
/// Discovered in SetupMouse.efi: calling `init(this)` initializes touch panel GPIOs + polling timer
/// so standard EFI_ABSOLUTE_POINTER_PROTOCOL starts delivering live touch coordinates!
#[repr(C)]
#[unsafe_protocol("8d59d323-c654-4ae7-7b15-f25904992a42")]
pub struct HuaweiTouchInitProtocol {
    pub initialized: u8,
    pub _pad: [u8; 7],
    pub init: unsafe extern "efiapi" fn(this: *mut Self) -> RawStatus,
    pub hw_init: unsafe extern "efiapi" fn(this: *mut Self) -> RawStatus,
}

/// Wake up the MateBook E Go touchscreen hardware via I2cTouchPanel.efi's TouchDeviceInit protocol.
/// Returns `Some(true)` if initialized successfully, `Some(false)` on error, `None` if protocol absent.
pub fn init_huawei_touchscreen() -> Option<bool> {
    let handle = boot::get_handle_for_protocol::<HuaweiTouchInitProtocol>().ok()?;
    let mut proto = unsafe {
        boot::open_protocol::<HuaweiTouchInitProtocol>(
            OpenProtocolParams {
                handle,
                agent: boot::image_handle(),
                controller: None,
            },
            OpenProtocolAttributes::GetProtocol,
        )
    }
    .ok()?;

    let ptr: *mut HuaweiTouchInitProtocol = &mut *proto;
    let status = unsafe { ((*ptr).init)(ptr) };
    Some(status == RawStatus::SUCCESS)
}

/// UEFI 2.10 §12.2 EFI_INPUT_KEY
#[repr(C)]
#[derive(Default, Debug, Clone, Copy)]
pub struct InputKey {
    pub scan_code: u16,
    pub unicode_char: u16,
}

/// UEFI 2.10 §12.2 EFI_KEY_STATE
#[repr(C)]
#[derive(Default, Debug, Clone, Copy)]
pub struct KeyState {
    pub key_shift_state: u32,
    pub key_toggle_state: u8,
}

/// UEFI 2.10 §12.2 EFI_KEY_DATA
#[repr(C)]
#[derive(Default, Debug, Clone, Copy)]
pub struct KeyData {
    pub key: InputKey,
    pub key_state: KeyState,
}

/// UEFI 2.10 §12.2 EFI_SIMPLE_TEXT_INPUT_EX_PROTOCOL
#[repr(C)]
#[unsafe_protocol("dd9e7534-7762-4698-8c14-f58517a625aa")]
pub struct SimpleTextInputEx {
    pub reset: unsafe extern "efiapi" fn(this: *mut Self, extended_verification: u8) -> RawStatus,
    pub read_key_stroke_ex:
        unsafe extern "efiapi" fn(this: *mut Self, key_data: *mut KeyData) -> RawStatus,
    pub wait_for_key_ex: *mut c_void,
    pub set_state: unsafe extern "efiapi" fn(this: *mut Self, key_toggle_state: *const u8) -> RawStatus,
    pub register_key_notify: *mut c_void,
    pub unregister_key_notify: *mut c_void,
}

impl SimpleTextInputEx {
    pub fn read_key_stroke_ex(&mut self) -> Result<Option<KeyData>, RawStatus> {
        let mut data = KeyData::default();
        let status = unsafe { (self.read_key_stroke_ex)(self as *mut Self, &mut data) };
        if status == RawStatus::SUCCESS {
            Ok(Some(data))
        } else if status == RawStatus::NOT_READY {
            Ok(None)
        } else {
            Err(status)
        }
    }
}

/// UEFI 2.10 §13.2 EFI_LOAD_FILE2_PROTOCOL
#[repr(C)]
pub struct LoadFile2Protocol {
    pub load_file: unsafe extern "efiapi" fn(
        this: *mut LoadFile2Protocol,
        file_path: *const c_void,
        boot_policy: u8,
        buffer_size: *mut usize,
        buffer: *mut c_void,
    ) -> RawStatus,
}

pub fn read_current_el() -> u8 {
    #[cfg(target_arch = "aarch64")]
    unsafe {
        let el: u64;
        core::arch::asm!("mrs {}, CurrentEL", out(reg) el, options(nomem, nostack));
        ((el >> 2) & 0x3) as u8
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        0
    }
}

pub fn format_device_path(handle: Handle) -> String {
    let Ok(dp) = (unsafe {
        boot::open_protocol::<DevicePath>(
            OpenProtocolParams {
                handle,
                agent: boot::image_handle(),
                controller: None,
            },
            OpenProtocolAttributes::GetProtocol,
        )
    }) else {
        return String::new();
    };

    let Ok(dpt_handle) = boot::get_handle_for_protocol::<DevicePathToText>() else {
        return String::new();
    };

    let Ok(dpt) = (unsafe {
        boot::open_protocol::<DevicePathToText>(
            OpenProtocolParams {
                handle: dpt_handle,
                agent: boot::image_handle(),
                controller: None,
            },
            OpenProtocolAttributes::GetProtocol,
        )
    }) else {
        return String::new();
    };

    match dpt.convert_device_path_to_text(&dp, DisplayOnly(true), AllowShortcuts(true)) {
        Ok(pool_str) => format!("{}", &*pool_str),
        Err(_) => String::new(),
    }
}

/// Read basic SMBIOS 2.x/3.0 ProductName and BIOS Version for the header device-info display.
pub fn read_device_summary() -> (String, String) {
    let (sm3, sm2) = system::with_config_table(|tables| {
        let mut a3 = None;
        let mut a2 = None;
        for t in tables {
            if t.guid == cfg::SMBIOS3_GUID {
                a3 = Some(t.address as usize);
            } else if t.guid == cfg::SMBIOS_GUID {
                a2 = Some(t.address as usize);
            }
        }
        (a3, a2)
    });

    let (table_addr, table_len) = if let Some(a) = sm3 {
        let ep = unsafe { core::slice::from_raw_parts(a as *const u8, 24) };
        if &ep[0..5] == b"_SM3_" {
            let len = u32::from_le_bytes([ep[12], ep[13], ep[14], ep[15]]) as usize;
            let addr = u64::from_le_bytes([
                ep[16], ep[17], ep[18], ep[19], ep[20], ep[21], ep[22], ep[23],
            ]) as usize;
            (addr, len)
        } else {
            (0, 0)
        }
    } else if let Some(a) = sm2 {
        let ep = unsafe { core::slice::from_raw_parts(a as *const u8, 31) };
        if &ep[0..4] == b"_SM_" {
            let len = u16::from_le_bytes([ep[22], ep[23]]) as usize;
            let addr = u32::from_le_bytes([ep[24], ep[25], ep[26], ep[27]]) as usize;
            (addr, len)
        } else {
            (0, 0)
        }
    } else {
        (0, 0)
    };

    let mut product = String::from("MateBook E Go (GK-W7X)");
    let mut bios_ver = String::from("UEFI");

    if table_addr == 0 || !(4..=1024 * 1024).contains(&table_len) {
        return (product, bios_ver);
    }

    let data = unsafe { core::slice::from_raw_parts(table_addr as *const u8, table_len) };
    let mut off = 0usize;
    while off + 4 <= data.len() {
        let ty = data[off];
        let len = data[off + 1] as usize;
        if len < 4 || off + len > data.len() {
            break;
        }
        let formatted = &data[off..off + len];
        let mut s_off = off + len;
        let s_start = s_off;
        while s_off + 1 < data.len() {
            if data[s_off] == 0 && data[s_off + 1] == 0 {
                s_off += 2;
                break;
            }
            s_off += 1;
        }
        let s_area = &data[s_start..s_off.min(data.len())];
        if ty == 0 && len > 0x05 {
            if let Some(v) = smbios_str(s_area, formatted[0x05]) {
                bios_ver = format!("BIOS {}", v);
            }
        } else if ty == 1 && len > 0x05 {
            if let Some(p) = smbios_str(s_area, formatted[0x05]) {
                product = format!("HUAWEI {}", p);
            }
        } else if ty == 127 {
            break;
        }
        off = s_off;
    }

    (product, bios_ver)
}

fn smbios_str(area: &[u8], idx: u8) -> Option<&str> {
    if idx == 0 {
        return None;
    }
    let mut cur = 1u8;
    let mut start = 0usize;
    for (i, &b) in area.iter().enumerate() {
        if b == 0 {
            if cur == idx {
                let s = core::str::from_utf8(&area[start..i]).ok()?.trim();
                return if s.is_empty() { None } else { Some(s) };
            }
            cur = cur.saturating_add(1);
            start = i + 1;
            if start < area.len() && area[start] == 0 {
                break;
            }
        }
    }
    None
}
