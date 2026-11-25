use core::ffi::c_void;
use uefi::proto::unsafe_protocol;
use uefi_raw::Status as RawStatus;

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
#[derive(Default, Debug, Clone, Copy)]
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

/// UEFI 2.10 §17.2.4 USB_DEVICE_DESCRIPTOR
#[repr(C)]
#[derive(Default, Debug, Clone, Copy)]
pub struct UsbDeviceDescriptor {
    pub length: u8,
    pub descriptor_type: u8,
    pub bcd_usb: u16,
    pub device_class: u8,
    pub device_sub_class: u8,
    pub device_protocol: u8,
    pub max_packet_size0: u8,
    pub id_vendor: u16,
    pub id_product: u16,
    pub bcd_device: u16,
    pub str_manufacturer: u8,
    pub str_product: u8,
    pub str_serial_number: u8,
    pub num_configurations: u8,
}

/// UEFI 2.10 §17.2.4 USB_INTERFACE_DESCRIPTOR
#[repr(C)]
#[derive(Default, Debug, Clone, Copy)]
pub struct UsbInterfaceDescriptor {
    pub length: u8,
    pub descriptor_type: u8,
    pub interface_number: u8,
    pub alternate_setting: u8,
    pub num_endpoints: u8,
    pub interface_class: u8,
    pub interface_sub_class: u8,
    pub interface_protocol: u8,
    pub interface: u8,
}

/// UEFI 2.10 §17.2.4 EFI_USB_IO_PROTOCOL (read-only descriptor subset)
#[repr(C)]
#[unsafe_protocol("2b2f68d6-0cd2-44cf-8e8b-bba20b1b5b75")]
pub struct UsbIo {
    pub control_transfer: *mut c_void,
    pub bulk_transfer: *mut c_void,
    pub async_interrupt_transfer: *mut c_void,
    pub sync_interrupt_transfer: *mut c_void,
    pub isochronous_transfer: *mut c_void,
    pub async_isochronous_transfer: *mut c_void,
    pub get_device_descriptor: unsafe extern "efiapi" fn(
        this: *mut Self,
        descriptor: *mut UsbDeviceDescriptor,
    ) -> RawStatus,
    pub get_config_descriptor: *mut c_void,
    pub get_interface_descriptor: unsafe extern "efiapi" fn(
        this: *mut Self,
        descriptor: *mut UsbInterfaceDescriptor,
    ) -> RawStatus,
    pub get_endpoint_descriptor: *mut c_void,
    pub get_string_descriptor: *mut c_void,
    pub get_supported_languages: *mut c_void,
    pub port_reset: *mut c_void,
}

impl UsbIo {
    pub fn device_descriptor(&mut self) -> Result<UsbDeviceDescriptor, RawStatus> {
        let mut d = UsbDeviceDescriptor::default();
        let st = unsafe { (self.get_device_descriptor)(self as *mut Self, &mut d) };
        if st == RawStatus::SUCCESS {
            Ok(d)
        } else {
            Err(st)
        }
    }

    pub fn interface_descriptor(&mut self) -> Result<UsbInterfaceDescriptor, RawStatus> {
        let mut d = UsbInterfaceDescriptor::default();
        let st = unsafe { (self.get_interface_descriptor)(self as *mut Self, &mut d) };
        if st == RawStatus::SUCCESS {
            Ok(d)
        } else {
            Err(st)
        }
    }
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
