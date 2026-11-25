use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::ffi::c_void;
use core::ptr;

use uefi::boot::{
    self, AllocateType, LoadImageSource, MemoryType, OpenProtocolAttributes, OpenProtocolParams,
};
use uefi::proto::device_path::DevicePath;
use uefi::proto::loaded_image::LoadedImage;
use uefi::proto::media::file::{File, FileAttribute, FileInfo, FileMode, FileType};
use uefi::proto::media::fs::SimpleFileSystem;
use uefi::proto::BootPolicy;
use uefi::runtime::{self, ResetType, VariableAttributes, VariableVendor};
use uefi::{cstr16, CStr16, Handle, Status};
use uefi_raw::Status as RawStatus;

use crate::config::BootConfig;
use crate::discover::{BootEntry, BootPayload, El2Drivers};
use crate::efi_extra::{
    LoadFile2Protocol, DEVICE_PATH_PROTOCOL_GUID, DTB_TABLE_GUID, LINUX_EFI_INITRD_MEDIA_GUID,
    LOAD_FILE2_PROTOCOL_GUID,
};

/// Build a concatenated UEFI DevicePath: `<partition DevicePath nodes> + Media::FilePath + End`.
fn build_file_device_path(fs_handle: Handle, efi_path: &str) -> Result<Vec<u8>, String> {
    let base_dp = unsafe {
        boot::open_protocol::<DevicePath>(
            OpenProtocolParams {
                handle: fs_handle,
                agent: boot::image_handle(),
                controller: None,
            },
            OpenProtocolAttributes::GetProtocol,
        )
    }
    .map_err(|e| format!("Failed to open partition DevicePath: {:?}", e.status()))?;

    let mut bytes = Vec::with_capacity(256);

    // Copy all nodes from base partition DevicePath up to (not including) End-of-Entire-Device-Path
    let raw_ptr = base_dp.as_ffi_ptr() as *const u8;
    let mut off = 0usize;
    loop {
        if off + 4 > 4096 {
            return Err(String::from("Partition DevicePath exceeds 4096 bytes"));
        }
        let node_type = unsafe { *raw_ptr.add(off) };
        let node_sub = unsafe { *raw_ptr.add(off + 1) };
        let node_len =
            u16::from_le_bytes(unsafe { [*raw_ptr.add(off + 2), *raw_ptr.add(off + 3)] }) as usize;
        if node_len < 4 || off + node_len > 4096 {
            return Err(String::from("Malformed partition DevicePath"));
        }
        if node_type == 0x7F && node_sub == 0xFF {
            break;
        }
        let slice = unsafe { core::slice::from_raw_parts(raw_ptr.add(off), node_len) };
        bytes.extend_from_slice(slice);
        off += node_len;
    }

    // Append Media::FilePath node (Type=0x04, SubType=0x04)
    let mut utf16: Vec<u16> = efi_path.encode_utf16().collect();
    utf16.push(0); // NUL terminator
    let node_len = 4 + utf16.len() * 2;
    if node_len > u16::MAX as usize {
        return Err(String::from("EFI path too long"));
    }
    bytes.push(0x04); // MEDIA_DEVICE_PATH
    bytes.push(0x04); // MEDIA_FILEPATH_DP
    bytes.extend_from_slice(&(node_len as u16).to_le_bytes());
    for unit in utf16 {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }

    // End of Entire Device Path (Type=0x7F, SubType=0xFF, Length=4)
    bytes.extend_from_slice(&[0x7F, 0xFF, 0x04, 0x00]);
    Ok(bytes)
}

fn read_volume_file(fs_handle: Handle, efi_path: &str) -> Result<Vec<u8>, String> {
    let mut sfs = unsafe {
        boot::open_protocol::<SimpleFileSystem>(
            OpenProtocolParams {
                handle: fs_handle,
                agent: boot::image_handle(),
                controller: None,
            },
            OpenProtocolAttributes::GetProtocol,
        )
    }
    .map_err(|e| format!("Open SimpleFileSystem failed: {:?}", e.status()))?;

    let mut root = sfs
        .open_volume()
        .map_err(|e| format!("open_volume failed: {:?}", e.status()))?;

    let mut path_buf = [0u16; 256];
    let cpath = CStr16::from_str_with_buf(efi_path, &mut path_buf)
        .map_err(|_| format!("Invalid UTF-16 path: {}", efi_path))?;

    let handle = root
        .open(cpath, FileMode::Read, FileAttribute::empty())
        .map_err(|e| format!("Failed to open {}: {:?}", efi_path, e.status()))?;

    let FileType::Regular(mut file) = handle
        .into_type()
        .map_err(|e| format!("into_type failed: {:?}", e.status()))?
    else {
        return Err(format!("{} is a directory, not a file", efi_path));
    };

    // Query exact file size via FileInfo
    let mut info_buf = [0u8; 512];
    let file_size = match file.get_info::<FileInfo>(&mut info_buf) {
        Ok(info) => info.file_size() as usize,
        Err(_) => 0,
    };

    let mut data = Vec::with_capacity(file_size);
    if file_size > 0 {
        data.resize(file_size, 0);
        let mut total = 0usize;
        while total < file_size {
            let n = file
                .read(&mut data[total..])
                .map_err(|e| format!("Read {} failed: {:?}", efi_path, e.status()))?;
            if n == 0 {
                break;
            }
            total += n;
        }
        data.truncate(total);
    } else {
        let mut chunk = [0u8; 8192];
        loop {
            let n = file
                .read(&mut chunk)
                .map_err(|e| format!("Read {} failed: {:?}", efi_path, e.status()))?;
            if n == 0 {
                break;
            }
            data.extend_from_slice(&chunk[..n]);
        }
    }

    Ok(data)
}

/// Wrapper struct containing both the UEFI LoadFile2Protocol vtable and our initrd bytes.
/// `#[repr(C)]` ensures `*mut LoadFile2Protocol` is identical to `*mut InitrdLoadFile2`.
#[repr(C)]
struct InitrdLoadFile2 {
    proto: LoadFile2Protocol,
    data: Vec<u8>,
}

unsafe extern "efiapi" fn initrd_load_file(
    this: *mut LoadFile2Protocol,
    _file_path: *const c_void,
    boot_policy: u8,
    buffer_size: *mut usize,
    buffer: *mut c_void,
) -> RawStatus {
    if boot_policy != 0 {
        return RawStatus::UNSUPPORTED;
    }
    if this.is_null() || buffer_size.is_null() {
        return RawStatus::INVALID_PARAMETER;
    }
    let wrapper = &*(this as *const InitrdLoadFile2);
    let needed = wrapper.data.len();
    if buffer.is_null() || *buffer_size < needed {
        *buffer_size = needed;
        return RawStatus::BUFFER_TOO_SMALL;
    }
    ptr::copy_nonoverlapping(wrapper.data.as_ptr(), buffer as *mut u8, needed);
    *buffer_size = needed;
    RawStatus::SUCCESS
}

struct InstalledInitrd {
    handle: Handle,
    dp_ptr: *mut [u8; 24],
    lf2_ptr: *mut InitrdLoadFile2,
}

impl InstalledInitrd {
    fn install(data: Vec<u8>) -> Result<Self, String> {
        // 20-byte Vendor Media Device Path + 4-byte End node
        let mut dp = Box::new([0u8; 24]);
        dp[0] = 0x04; // MEDIA_DEVICE_PATH
        dp[1] = 0x03; // MEDIA_VENDOR_DP
        dp[2] = 20;
        dp[3] = 0;
        dp[4..20].copy_from_slice(&LINUX_EFI_INITRD_MEDIA_GUID.to_bytes());
        dp[20] = 0x7F;
        dp[21] = 0xFF;
        dp[22] = 0x04;
        dp[23] = 0x00;
        let dp_ptr = Box::into_raw(dp);

        let lf2 = Box::new(InitrdLoadFile2 {
            proto: LoadFile2Protocol {
                load_file: initrd_load_file,
            },
            data,
        });
        let lf2_ptr = Box::into_raw(lf2);

        let handle = unsafe {
            boot::install_protocol_interface(None, &DEVICE_PATH_PROTOCOL_GUID, dp_ptr as *const c_void)
        }
        .map_err(|e| {
            unsafe {
                drop(Box::from_raw(dp_ptr));
                drop(Box::from_raw(lf2_ptr));
            }
            format!("Install initrd DevicePath failed: {:?}", e.status())
        })?;

        if let Err(e) = unsafe {
            boot::install_protocol_interface(
                Some(handle),
                &LOAD_FILE2_PROTOCOL_GUID,
                lf2_ptr as *const c_void,
            )
        } {
            let _ = unsafe {
                boot::uninstall_protocol_interface(
                    handle,
                    &DEVICE_PATH_PROTOCOL_GUID,
                    dp_ptr as *const c_void,
                )
            };
            unsafe {
                drop(Box::from_raw(dp_ptr));
                drop(Box::from_raw(lf2_ptr));
            }
            return Err(format!("Install initrd LoadFile2 failed: {:?}", e.status()));
        }

        Ok(Self {
            handle,
            dp_ptr,
            lf2_ptr,
        })
    }

    fn uninstall(self) {
        let _ = unsafe {
            boot::uninstall_protocol_interface(
                self.handle,
                &LOAD_FILE2_PROTOCOL_GUID,
                self.lf2_ptr as *const c_void,
            )
        };
        let _ = unsafe {
            boot::uninstall_protocol_interface(
                self.handle,
                &DEVICE_PATH_PROTOCOL_GUID,
                self.dp_ptr as *const c_void,
            )
        };
        unsafe {
            drop(Box::from_raw(self.dp_ptr));
            drop(Box::from_raw(self.lf2_ptr));
        }
    }
}

struct InstalledDtb {
    ptr: core::ptr::NonNull<u8>,
    pages: usize,
}

impl InstalledDtb {
    fn install(dtb_bytes: &[u8]) -> Result<Self, String> {
        // Verify Flattened Device Tree (FDT) big-endian magic 0xD00DFEED
        if dtb_bytes.len() < 40 || dtb_bytes[0..4] != [0xD0, 0x0D, 0xFE, 0xED] {
            return Err(String::from("Invalid DTB file (missing FDT magic 0xD00DFEED)"));
        }
        let pages = dtb_bytes.len().div_ceil(4096).max(1);
        let ptr = boot::allocate_pages(AllocateType::AnyPages, MemoryType::ACPI_RECLAIM, pages)
            .map_err(|e| format!("Allocate DTB pages failed: {:?}", e.status()))?;
        unsafe {
            ptr::copy_nonoverlapping(dtb_bytes.as_ptr(), ptr.as_ptr(), dtb_bytes.len());
            boot::install_configuration_table(&DTB_TABLE_GUID, ptr.as_ptr() as *const c_void)
                .map_err(|e| {
                    let _ = boot::free_pages(ptr, pages);
                    format!("Install DTB config table failed: {:?}", e.status())
                })?;
        }
        Ok(Self { ptr, pages })
    }

    fn uninstall(self) {
        unsafe {
            let _ = boot::install_configuration_table(&DTB_TABLE_GUID, ptr::null());
            let _ = boot::free_pages(self.ptr, self.pages);
        }
    }
}

fn load_and_start_efi(fs_handle: Handle, efi_path: &str, options: &str) -> Result<(), String> {
    let dp_bytes = build_file_device_path(fs_handle, efi_path)?;
    let dp_ref = unsafe { DevicePath::from_ffi_ptr(dp_bytes.as_ptr() as *const _) };

    let child = boot::load_image(
        boot::image_handle(),
        LoadImageSource::FromDevicePath {
            device_path: dp_ref,
            boot_policy: BootPolicy::ExactMatch,
        },
    )
    .map_err(|e| format!("LoadImage({}) failed: {:?}", efi_path, e.status()))?;

    // Keep UTF-16 options buffer alive for the entire duration of start_image!
    let mut opts_u16: Vec<u16> = Vec::new();
    if !options.is_empty() {
        opts_u16.extend(options.encode_utf16());
        opts_u16.push(0);
        let mut li = unsafe {
            boot::open_protocol::<LoadedImage>(
                OpenProtocolParams {
                    handle: child,
                    agent: boot::image_handle(),
                    controller: None,
                },
                OpenProtocolAttributes::GetProtocol,
            )
        }
        .map_err(|e| format!("Open child LoadedImage failed: {:?}", e.status()))?;

        unsafe {
            li.set_load_options(opts_u16.as_ptr() as *const u8, (opts_u16.len() * 2) as u32);
        }
    }

    let res = boot::start_image(child);
    let _ = boot::unload_image(child);
    drop(opts_u16);

    match res {
        Ok(()) => Ok(()),
        Err(e) => Err(format!("StartImage({}) returned: {:?}", efi_path, e.status())),
    }
}

pub fn launch_entry(
    entry: &BootEntry,
    cfg: &BootConfig,
    el2_drivers: &El2Drivers,
) -> Result<(), String> {
    let variant = entry.active_variant(cfg);

    // 1. Optional per-entry UEFI drivers
    for drv_path in &variant.drivers {
        load_and_start_efi(entry.fs_handle, drv_path, "")?;
    }

    // 2. Resolve active payload (respecting Boot Method + Android Slot A/B selection)
    let payload = entry.active_payload(cfg);

    // 3. Optional EL2 boot drivers (qebspil + slbounce) for Linux / EFI-stub kernels
    let use_el2 = entry.active_supports_el2(cfg) && cfg.linux_el2;
    if use_el2 {
        let Some(drv_fs) = el2_drivers.fs_handle else {
            return Err(String::from(
                "EL2 enabled, but slbounce.efi not found in \\EFI\\gaokunBoot\\drivers\\",
            ));
        };
        let Some(ref slbounce_path) = el2_drivers.slbounce else {
            return Err(String::from(
                "EL2 enabled, but slbounce.efi not found in \\EFI\\gaokunBoot\\drivers\\",
            ));
        };
        // Pre-load DSP co-processor loader if present, then hook ExitBootServices via slbounce
        if let Some(ref qebspil_path) = el2_drivers.qebspil {
            load_and_start_efi(drv_fs, qebspil_path, "")?;
        }
        load_and_start_efi(drv_fs, slbounce_path, "")?;
    }

    // 4. Boot primary payload
    match payload {
        BootPayload::Efi { path, options } => load_and_start_efi(entry.fs_handle, path, options),
        BootPayload::Linux {
            kernel,
            initrd,
            devicetree,
            el2_devicetree,
            options,
        } => {
            let chosen_dtb = if use_el2 && el2_devicetree.is_some() {
                el2_devicetree.as_deref()
            } else {
                devicetree.as_deref()
            };

            let installed_dtb = if let Some(dtb_path) = chosen_dtb {
                let dtb_bytes = read_volume_file(entry.fs_handle, dtb_path)?;
                Some(InstalledDtb::install(&dtb_bytes)?)
            } else {
                None
            };

            let installed_initrd = if let Some(initrd_path) = initrd {
                match read_volume_file(entry.fs_handle, initrd_path)
                    .and_then(InstalledInitrd::install)
                {
                    Ok(h) => Some(h),
                    Err(e) => {
                        if let Some(d) = installed_dtb {
                            d.uninstall();
                        }
                        return Err(e);
                    }
                }
            } else {
                None
            };

            let res = load_and_start_efi(entry.fs_handle, kernel, options);

            if let Some(i) = installed_initrd {
                i.uninstall();
            }
            if let Some(d) = installed_dtb {
                d.uninstall();
            }
            res
        }
    }
}

pub fn reboot_to_firmware_setup() -> Result<(), String> {
    const EFI_OS_INDICATIONS_BOOT_TO_FW_UI: u64 = 0x0000_0000_0000_0001;
    let mut cur: u64 = 0;
    let mut buf = [0u8; 8];
    if let Ok((bytes, _)) = runtime::get_variable(
        cstr16!("OsIndications"),
        &VariableVendor::GLOBAL_VARIABLE,
        &mut buf,
    ) {
        if bytes.len() == 8 {
            cur = u64::from_le_bytes(buf);
        }
    }
    let next = (cur | EFI_OS_INDICATIONS_BOOT_TO_FW_UI).to_le_bytes();
    let attrs = VariableAttributes::NON_VOLATILE
        | VariableAttributes::BOOTSERVICE_ACCESS
        | VariableAttributes::RUNTIME_ACCESS;
    runtime::set_variable(
        cstr16!("OsIndications"),
        &VariableVendor::GLOBAL_VARIABLE,
        attrs,
        &next,
    )
    .map_err(|e| format!("Failed to set OsIndications: {:?}", e.status()))?;
    runtime::reset(ResetType::COLD, Status::SUCCESS, None);
}

pub fn reboot() -> ! {
    runtime::reset(ResetType::COLD, Status::SUCCESS, None);
}

pub fn shutdown() -> ! {
    runtime::reset(ResetType::SHUTDOWN, Status::SUCCESS, None);
}
