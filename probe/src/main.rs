#![no_main]
#![no_std]

extern crate alloc;

mod efi_extra;
#[allow(dead_code)]
mod input_test;
mod smbios;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;

use uefi::boot::{self, OpenProtocolAttributes, OpenProtocolParams, SearchType};
use uefi::data_types::Guid;
use uefi::prelude::*;
use uefi::proto::console::gop::GraphicsOutput;
use uefi::proto::console::pointer::Pointer;
use uefi::proto::console::text::Input;
use uefi::proto::device_path::text::{AllowShortcuts, DevicePathToText, DisplayOnly};
use uefi::proto::device_path::DevicePath;
use uefi::proto::loaded_image::LoadedImage;
use uefi::proto::media::block::BlockIO;
use uefi::proto::media::file::{File, FileAttribute, FileInfo, FileMode, FileType};
use uefi::proto::media::fs::SimpleFileSystem;
use uefi::runtime::{self, VariableVendor};
use uefi::table::cfg;
use uefi::{cstr16, guid, system, Handle, Identify};

use efi_extra::{AbsolutePointer, SimpleTextInputEx, UsbIo};

const DTB_TABLE_GUID: Guid = guid!("b1b621d5-f19c-41a5-830b-d9152c69aae0");
const MEMORY_ATTRIBUTES_GUID: Guid = guid!("dcfa911d-26eb-469f-a220-38b7dc461220");
const LOG_FILENAME: &uefi::CStr16 = cstr16!("\\GAOKUN-PROBE.TXT");

fn print_screen(s: &str) {
    system::with_stdout(|stdout| {
        let _ = stdout.write_str(s);
    });
}

fn read_aarch64_sysregs() -> (u64, u64, u64, u64) {
    #[cfg(target_arch = "aarch64")]
    unsafe {
        let current_el: u64;
        let midr: u64;
        let mpidr: u64;
        let cntfrq: u64;
        core::arch::asm!("mrs {}, CurrentEL", out(reg) current_el, options(nomem, nostack));
        core::arch::asm!("mrs {}, MIDR_EL1", out(reg) midr, options(nomem, nostack));
        core::arch::asm!("mrs {}, MPIDR_EL1", out(reg) mpidr, options(nomem, nostack));
        core::arch::asm!("mrs {}, CNTFRQ_EL0", out(reg) cntfrq, options(nomem, nostack));
        ((current_el >> 2) & 0x3, midr, mpidr, cntfrq)
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        (0, 0, 0, 0)
    }
}

fn format_device_path(handle: Handle) -> String {
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
        return String::from("<no DevicePath>");
    };

    let Ok(dpt_handle) = boot::get_handle_for_protocol::<DevicePathToText>() else {
        return String::from("<no DevicePathToText>");
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
        return String::from("<open DevicePathToText failed>");
    };

    match dpt.convert_device_path_to_text(&dp, DisplayOnly(false), AllowShortcuts(false)) {
        Ok(pool_str) => format!("{}", &*pool_str),
        Err(e) => format!("<dp_to_text err: {:?}>", e.status()),
    }
}

fn known_cfg_guid_name(g: Guid) -> &'static str {
    if g == cfg::ACPI2_GUID {
        "ACPI 2.0 (RSDP)"
    } else if g == cfg::ACPI_GUID {
        "ACPI 1.0"
    } else if g == cfg::SMBIOS3_GUID {
        "SMBIOS 3.0"
    } else if g == cfg::SMBIOS_GUID {
        "SMBIOS 2.x"
    } else if g == DTB_TABLE_GUID {
        "DeviceTree (DTB)"
    } else if g == MEMORY_ATTRIBUTES_GUID {
        "MemoryAttributes"
    } else if g == cfg::PROPERTIES_TABLE_GUID {
        "PropertiesTable"
    } else if g == cfg::LZMA_COMPRESS_GUID {
        "LZMA"
    } else {
        "Other"
    }
}

fn probe_cpu_and_firmware(out: &mut String) {
    let _ = writeln!(out, "=== 1. CPU & UEFI FIRMWARE ===");
    let (el, midr, mpidr, cntfrq) = read_aarch64_sysregs();
    let _ = writeln!(out, "  CurrentEL: EL{}", el);
    let _ = writeln!(
        out,
        "  MIDR_EL1:  0x{:016X} (impl=0x{:02X} part=0x{:03X})",
        midr,
        (midr >> 24) & 0xFF,
        (midr >> 4) & 0xFFF
    );
    let _ = writeln!(out, "  MPIDR_EL1: 0x{:016X}", mpidr);
    let _ = writeln!(out, "  CNTFRQ:    {} Hz", cntfrq);

    let rev = system::uefi_revision();
    let vendor = system::firmware_vendor();
    let fw_rev = system::firmware_revision();
    let _ = writeln!(out, "  UEFI Rev:  {}.{}", rev.major(), rev.minor());
    let _ = writeln!(out, "  FW Vendor: {}", vendor);
    let _ = writeln!(out, "  FW Rev:    0x{:08X}", fw_rev);

    // Common EFI global variables
    let mut buf = [0u8; 64];
    for (label, name) in [
        ("SecureBoot", cstr16!("SecureBoot")),
        ("SetupMode", cstr16!("SetupMode")),
        ("BootCurrent", cstr16!("BootCurrent")),
        ("BootOrder", cstr16!("BootOrder")),
    ] {
        match runtime::get_variable(name, &VariableVendor::GLOBAL_VARIABLE, &mut buf) {
            Ok((bytes, _attrs)) => {
                let _ = writeln!(out, "  Var {:<11}: {:02X?}", label, bytes);
            }
            Err(e) => {
                let _ = writeln!(out, "  Var {:<11}: <err {:?}>", label, e.status());
            }
        }
    }

    // Configuration tables + SMBIOS3 / SMBIOS2 parse
    let _ = writeln!(out, "\n=== 2. CONFIGURATION TABLES & SMBIOS ===");
    let (cfg_dump, smbios3_addr, smbios2_addr) = system::with_config_table(|tables| {
        let mut s = String::new();
        let mut sm3: Option<usize> = None;
        let mut sm2: Option<usize> = None;
        for entry in tables {
            let name = known_cfg_guid_name(entry.guid);
            let _ = writeln!(
                s,
                "  {}  addr=0x{:016X}  ({})",
                entry.guid,
                entry.address as usize,
                name
            );
            if entry.guid == cfg::SMBIOS3_GUID {
                sm3 = Some(entry.address as usize);
            } else if entry.guid == cfg::SMBIOS_GUID {
                sm2 = Some(entry.address as usize);
            }
        }
        (s, sm3, sm2)
    });
    out.push_str(&cfg_dump);

    if let Some(addr) = smbios3_addr {
        smbios::dump_smbios3(addr, out);
    } else if let Some(addr) = smbios2_addr {
        smbios::dump_smbios2(addr, out);
    } else {
        let _ = writeln!(out, "  No SMBIOS table found.");
    }
}

fn probe_display(out: &mut String) {
    let _ = writeln!(out, "\n=== 3. DISPLAY (GOP & CONSOLE) ===");

    let console_dump = system::with_stdout(|stdout| {
        let mut s = String::new();
        if let Ok(Some(mode)) = stdout.current_mode() {
            let _ = writeln!(
                s,
                "  ConsoleOut Current Text Mode: {} cols x {} rows",
                mode.columns(),
                mode.rows()
            );
        }
        let mut modes_str = String::new();
        for (i, m) in stdout.modes().enumerate() {
            let _ = write!(modes_str, "[{}:{}x{}] ", i, m.columns(), m.rows());
        }
        let _ = writeln!(s, "  ConsoleOut Modes: {}", modes_str);
        s
    });
    out.push_str(&console_dump);

    match boot::locate_handle_buffer(SearchType::ByProtocol(&GraphicsOutput::GUID)) {
        Ok(handles) => {
            let _ = writeln!(out, "  GOP Handle count: {}", handles.len());
            for (idx, &handle) in handles.iter().enumerate() {
                let dp = format_device_path(handle);
                let _ = writeln!(out, "  GOP[{}] DP: {}", idx, dp);
                match unsafe {
                    boot::open_protocol::<GraphicsOutput>(
                        OpenProtocolParams {
                            handle,
                            agent: boot::image_handle(),
                            controller: None,
                        },
                        OpenProtocolAttributes::GetProtocol,
                    )
                } {
                    Ok(mut gop) => {
                        let cur = gop.current_mode_info();
                        let mut fb = gop.frame_buffer();
                        let _ = writeln!(
                            out,
                            "    Current: {}x{}, stride={}, fmt={:?}, fb=0x{:X} ({} bytes)",
                            cur.resolution().0,
                            cur.resolution().1,
                            cur.stride(),
                            cur.pixel_format(),
                            fb.as_mut_ptr() as usize,
                            fb.size()
                        );
                        if let Some(mask) = cur.pixel_bitmask() {
                            let _ = writeln!(
                                out,
                                "    Bitmask: R=0x{:08X} G=0x{:08X} B=0x{:08X}",
                                mask.red, mask.green, mask.blue
                            );
                        }
                        let _ = writeln!(out, "    Supported modes:");
                        for (m_idx, mode) in gop.modes().enumerate() {
                            let info = mode.info();
                            let _ = writeln!(
                                out,
                                "      Mode {:>2}: {}x{}, stride={}, fmt={:?}",
                                m_idx,
                                info.resolution().0,
                                info.resolution().1,
                                info.stride(),
                                info.pixel_format()
                            );
                        }
                    }
                    Err(e) => {
                        let _ = writeln!(out, "    Failed to open GOP: {:?}", e.status());
                    }
                }
            }
        }
        Err(e) => {
            let _ = writeln!(out, "  Locate GOP failed: {:?}", e.status());
        }
    }
}

fn probe_input_inventory(out: &mut String) {
    let _ = writeln!(out, "\n=== 4. INPUT PROTOCOLS INVENTORY ===");

    // SimpleTextInput
    match boot::locate_handle_buffer(SearchType::ByProtocol(&Input::GUID)) {
        Ok(handles) => {
            let _ = writeln!(out, "  SimpleTextInput handles: {}", handles.len());
            for (i, &h) in handles.iter().enumerate() {
                let _ = writeln!(out, "    TextIn[{}] DP: {}", i, format_device_path(h));
            }
        }
        Err(e) => {
            let _ = writeln!(out, "  SimpleTextInput locate err: {:?}", e.status());
        }
    }

    // SimpleTextInputEx
    match boot::locate_handle_buffer(SearchType::ByProtocol(&SimpleTextInputEx::GUID)) {
        Ok(handles) => {
            let _ = writeln!(out, "  SimpleTextInputEx handles: {}", handles.len());
            for (i, &h) in handles.iter().enumerate() {
                let _ = writeln!(out, "    TextEx[{}] DP: {}", i, format_device_path(h));
            }
        }
        Err(e) => {
            let _ = writeln!(out, "  SimpleTextInputEx locate err: {:?}", e.status());
        }
    }

    // AbsolutePointer (Touchscreen)
    match boot::locate_handle_buffer(SearchType::ByProtocol(&AbsolutePointer::GUID)) {
        Ok(handles) => {
            let _ = writeln!(out, "  AbsolutePointer handles: {}", handles.len());
            for (i, &h) in handles.iter().enumerate() {
                let _ = writeln!(out, "    AbsPtr[{}] DP: {}", i, format_device_path(h));
                if let Ok(ap) = unsafe {
                    boot::open_protocol::<AbsolutePointer>(
                        OpenProtocolParams {
                            handle: h,
                            agent: boot::image_handle(),
                            controller: None,
                        },
                        OpenProtocolAttributes::GetProtocol,
                    )
                } {
                    if let Some(m) = ap.mode() {
                        let _ = writeln!(
                            out,
                            "      Mode: X=[{}..{}] Y=[{}..{}] Z=[{}..{}] attrs=0x{:X}",
                            m.absolute_min_x,
                            m.absolute_max_x,
                            m.absolute_min_y,
                            m.absolute_max_y,
                            m.absolute_min_z,
                            m.absolute_max_z,
                            m.attributes
                        );
                    }
                }
                if let Ok(protos) = boot::protocols_per_handle(h) {
                    let _ = writeln!(out, "      Protocols on AbsPtr[{}]: {:?}", i, &*protos);
                }
            }
        }
        Err(e) => {
            let _ = writeln!(out, "  AbsolutePointer locate err: {:?} (no touch protocol)", e.status());
        }
    }

    // UsbIo interfaces (reveals magnetic keyboard/touchpad HID interfaces!)
    match boot::locate_handle_buffer(SearchType::ByProtocol(&UsbIo::GUID)) {
        Ok(handles) => {
            let _ = writeln!(out, "  UsbIo handles: {}", handles.len());
            for (i, &h) in handles.iter().enumerate() {
                let dp = format_device_path(h);
                if let Ok(mut usb) = unsafe {
                    boot::open_protocol::<UsbIo>(
                        OpenProtocolParams {
                            handle: h,
                            agent: boot::image_handle(),
                            controller: None,
                        },
                        OpenProtocolAttributes::GetProtocol,
                    )
                } {
                    let dev = usb.device_descriptor();
                    let ifc = usb.interface_descriptor();
                    match (dev, ifc) {
                        (Ok(d), Ok(f)) => {
                            let _ = writeln!(
                                out,
                                "    USB[{}] VID:PID={:04X}:{:04X} ifc={} cls={:02X}/{:02X}/{:02X} eps={} DP: {}",
                                i,
                                d.id_vendor,
                                d.id_product,
                                f.interface_number,
                                f.interface_class,
                                f.interface_sub_class,
                                f.interface_protocol,
                                f.num_endpoints,
                                dp
                            );
                        }
                        _ => {
                            let _ = writeln!(out, "    USB[{}] <desc err> DP: {}", i, dp);
                        }
                    }
                }
            }
        }
        Err(e) => {
            let _ = writeln!(out, "  UsbIo locate err: {:?}", e.status());
        }
    }

    // SimplePointer (Mouse / Touchpad)
    match boot::locate_handle_buffer(SearchType::ByProtocol(&Pointer::GUID)) {
        Ok(handles) => {
            let _ = writeln!(out, "  SimplePointer handles: {}", handles.len());
            for (i, &h) in handles.iter().enumerate() {
                let _ = writeln!(out, "    RelPtr[{}] DP: {}", i, format_device_path(h));
                if let Ok(sp) = unsafe {
                    boot::open_protocol::<Pointer>(
                        OpenProtocolParams {
                            handle: h,
                            agent: boot::image_handle(),
                            controller: None,
                        },
                        OpenProtocolAttributes::GetProtocol,
                    )
                } {
                    let m = sp.mode();
                    let _ = writeln!(
                        out,
                        "      Mode: res=({}, {}, {}) left={} right={}",
                        m.resolution[0],
                        m.resolution[1],
                        m.resolution[2],
                        m.has_button[0],
                        m.has_button[1]
                    );
                }
            }
        }
        Err(e) => {
            let _ = writeln!(out, "  SimplePointer locate err: {:?}", e.status());
        }
    }
}

fn probe_storage(out: &mut String) {
    let _ = writeln!(out, "\n=== 5. STORAGE, FILESYSTEMS & LOADED IMAGE ===");

    let mut boot_dev_handle: Option<Handle> = None;
    if let Ok(li) = unsafe {
        boot::open_protocol::<LoadedImage>(
            OpenProtocolParams {
                handle: boot::image_handle(),
                agent: boot::image_handle(),
                controller: None,
            },
            OpenProtocolAttributes::GetProtocol,
        )
    } {
        boot_dev_handle = li.device();
        if let Some(dev) = boot_dev_handle {
            let _ = writeln!(out, "  LoadedImage Device DP: {}", format_device_path(dev));
        }
    }

    // SimpleFileSystem volumes (read-only top-level peek)
    if let Ok(handles) = boot::locate_handle_buffer(SearchType::ByProtocol(&SimpleFileSystem::GUID))
    {
        let _ = writeln!(out, "  SimpleFileSystem volumes: {}", handles.len());
        for (i, &h) in handles.iter().enumerate() {
            let is_boot = boot_dev_handle == Some(h);
            let _ = writeln!(
                out,
                "    FS[{}]{} DP: {}",
                i,
                if is_boot { " [BOOT]" } else { "" },
                format_device_path(h)
            );
            if let Ok(mut sfs) = unsafe {
                boot::open_protocol::<SimpleFileSystem>(
                    OpenProtocolParams {
                        handle: h,
                        agent: boot::image_handle(),
                        controller: None,
                    },
                    OpenProtocolAttributes::GetProtocol,
                )
            } {
                let dp_str = format_device_path(h);
                let is_fv = dp_str.starts_with("Fv(");
                if let Ok(mut root) = sfs.open_volume() {
                    let mut names: Vec<String> = Vec::new();
                    let mut fv_hits: Vec<String> = Vec::new();
                    let mut buf = [0u8; 512];
                    while let Ok(Some(info)) = root.read_entry(&mut buf) {
                        let name = format!("{}", info.file_name());
                        if name != "." && name != ".." {
                            if is_fv {
                                let low = name.to_ascii_lowercase();
                                if low.contains("touch")
                                    || low.contains("himax")
                                    || low.contains("thp")
                                    || low.contains("spi")
                                    || low.contains("i2c")
                                    || low.contains("hid")
                                    || low.contains("key")
                                    || low.contains("ptr")
                                    || low.contains("mouse")
                                    || low.contains("tp")
                                {
                                    fv_hits.push(name.clone());
                                }
                            }
                            if names.len() < 12 {
                                names.push(name);
                            } else if names.len() == 12 {
                                names.push(String::from("..."));
                            }
                        }
                    }
                    let _ = writeln!(out, "      Root entries: {:?}", names);
                    if is_fv {
                        let _ = writeln!(out, "      FV Touch/Input/Bus DXEs: {:?}", fv_hits);
                    }

                    // If this volume looks like an ESP with EFI/ or loader/, peek inside (read-only)
                    for sub in [
                        cstr16!("\\EFI"),
                        cstr16!("\\EFI\\gk3boot"),
                        cstr16!("\\loader"),
                        cstr16!("\\loader\\entries"),
                    ] {
                        if let Ok(sub_handle) =
                            root.open(sub, FileMode::Read, FileAttribute::empty())
                        {
                            if let Ok(FileType::Dir(mut dir)) = sub_handle.into_type() {
                                let mut sub_names: Vec<String> = Vec::new();
                                while let Ok(Some(info)) = dir.read_entry(&mut buf) {
                                    let n = format!("{}", info.file_name());
                                    if n != "." && n != ".." {
                                        sub_names.push(n);
                                    }
                                    if sub_names.len() >= 20 {
                                        sub_names.push(String::from("..."));
                                        break;
                                    }
                                }
                                let _ = writeln!(out, "      {} entries: {:?}", sub, sub_names);

                                // If this is \loader\entries, dump contents of each .conf file (read-only)
                                if sub == cstr16!("\\loader\\entries") {
                                    for fname in &sub_names {
                                        if fname.ends_with(".conf") {
                                            let mut u16_buf = [0u16; 128];
                                            let full_path = format!("\\loader\\entries\\{}", fname);
                                            if let Ok(cpath) = uefi::CStr16::from_str_with_buf(&full_path, &mut u16_buf) {
                                                if let Ok(fh) = root.open(cpath, FileMode::Read, FileAttribute::empty()) {
                                                    if let Ok(FileType::Regular(mut rf)) = fh.into_type() {
                                                        let mut content = [0u8; 1024];
                                                        if let Ok(n) = rf.read(&mut content) {
                                                            let text = core::str::from_utf8(&content[..n]).unwrap_or("<non-utf8>");
                                                            let _ = writeln!(out, "        --- {} ---", fname);
                                                            for l in text.lines() {
                                                                let _ = writeln!(out, "          {}", l);
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // BlockIO summary
    if let Ok(handles) = boot::locate_handle_buffer(SearchType::ByProtocol(&BlockIO::GUID)) {
        let _ = writeln!(out, "  BlockIO handles: {}", handles.len());
        for (i, &h) in handles.iter().enumerate() {
            if let Ok(bio) = unsafe {
                boot::open_protocol::<BlockIO>(
                    OpenProtocolParams {
                        handle: h,
                        agent: boot::image_handle(),
                        controller: None,
                    },
                    OpenProtocolAttributes::GetProtocol,
                )
            } {
                let m = bio.media();
                let blocks = m.last_block().saturating_add(1);
                let size_mb = (blocks.saturating_mul(m.block_size() as u64)) / (1024 * 1024);
                let _ = writeln!(
                    out,
                    "    BLK[{}] {} MiB (blk={}B, rem={}, part={}, ro={}, present={}) DP: {}",
                    i,
                    size_mb,
                    m.block_size(),
                    m.is_removable_media(),
                    m.is_logical_partition(),
                    m.is_read_only(),
                    m.is_media_present(),
                    format_device_path(h)
                );
            }
        }
    }
}

fn read_file_bytes(
    root: &mut uefi::proto::media::file::Directory,
    name: &uefi::CStr16,
) -> Result<Vec<u8>, String> {
    let fh = root
        .open(name, FileMode::Read, FileAttribute::empty())
        .map_err(|e| format!("open {:?}: {:?}", name, e.status()))?;
    let FileType::Regular(mut rf) = fh
        .into_type()
        .map_err(|e| format!("into_type: {:?}", e.status()))?
    else {
        return Err(String::from("not a regular file"));
    };
    let mut info_buf = [0u8; 512];
    let sz = rf
        .get_info::<FileInfo>(&mut info_buf)
        .map(|i| i.file_size() as usize)
        .unwrap_or(0);
    let mut out = Vec::with_capacity(sz);
    let mut chunk = [0u8; 4096];
    loop {
        match rf.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                out.extend_from_slice(&chunk[..n]);
                if out.len() > 8 * 1024 * 1024 {
                    break;
                }
            }
            Err(e) => return Err(format!("read err: {:?}", e.status())),
        }
    }
    Ok(out)
}

fn dump_fv_touch_drivers(out: &mut String) {
    let _ = writeln!(out, "\n=== 7. FV DRIVER EXTRACTION (\\FV_DUMP\\) ===");
    let Ok(mut boot_sfs) = boot::get_image_file_system(boot::image_handle()) else {
        let _ = writeln!(out, "  Failed to open boot volume for writing.");
        return;
    };
    let Ok(mut boot_root) = boot_sfs.open_volume() else {
        let _ = writeln!(out, "  Failed to open boot root.");
        return;
    };

    // Create \FV_DUMP directory on boot USB image
    let Ok(dir_h) = boot_root.open(
        cstr16!("\\FV_DUMP"),
        FileMode::CreateReadWrite,
        FileAttribute::DIRECTORY,
    ) else {
        let _ = writeln!(out, "  Failed to create \\FV_DUMP (read-only media?)");
        return;
    };
    let Ok(FileType::Dir(mut dump_dir)) = dir_h.into_type() else {
        let _ = writeln!(out, "  \\FV_DUMP is not a directory");
        return;
    };

    let targets: &[(&str, &uefi::CStr16)] = &[
        ("TouchPanelInit.efi", cstr16!("TouchPanelInit.efi")),
        ("I2cTouchPanel.efi", cstr16!("I2cTouchPanel.efi")),
        ("SetupMouse.efi", cstr16!("SetupMouse.efi")),
        ("SPI.efi", cstr16!("SPI.efi")),
        ("I2C.efi", cstr16!("I2C.efi")),
        ("UsbMouseDxe.efi", cstr16!("UsbMouseDxe.efi")),
        ("SoftKeyBoard.efi", cstr16!("SoftKeyBoard.efi")),
        ("uefiplat.cfg", cstr16!("uefiplat.cfg")),
    ];

    let Ok(handles) = boot::locate_handle_buffer(SearchType::ByProtocol(&SimpleFileSystem::GUID))
    else {
        return;
    };

    for &h in handles.iter() {
        let dp = format_device_path(h);
        if !dp.starts_with("Fv(") && !dp.starts_with("MemoryMapped(") {
            continue;
        }
        let Ok(mut sfs) = (unsafe {
            boot::open_protocol::<SimpleFileSystem>(
                OpenProtocolParams {
                    handle: h,
                    agent: boot::image_handle(),
                    controller: None,
                },
                OpenProtocolAttributes::GetProtocol,
            )
        }) else {
            continue;
        };
        let Ok(mut root) = sfs.open_volume() else {
            continue;
        };

        for &(label, cname) in targets {
            if let Ok(bytes) = read_file_bytes(&mut root, cname) {
                // Save into \FV_DUMP\<name>
                if let Ok(old) =
                    dump_dir.open(cname, FileMode::ReadWrite, FileAttribute::empty())
                {
                    let _ = old.delete();
                }
                match dump_dir.open(cname, FileMode::CreateReadWrite, FileAttribute::empty()) {
                    Ok(dst_h) => {
                        if let Ok(FileType::Regular(mut wf)) = dst_h.into_type() {
                            match wf
                                .write(&bytes)
                                .map_err(|e| e.status())
                                .and_then(|_| wf.flush().map_err(|e| e.status()))
                            {
                                Ok(()) => {
                                    let msg = format!(
                                        "  [DUMPED] {} ({} bytes) from {}\r\n",
                                        label,
                                        bytes.len(),
                                        dp
                                    );
                                    out.push_str(&msg);
                                    print_screen(&msg);
                                }
                                Err(st) => {
                                    let _ = writeln!(out, "  [ERR] write {}: {:?}", label, st);
                                }
                            }
                        }
                    }
                    Err(e) => {
                        let _ = writeln!(out, "  [ERR] create {}: {:?}", label, e.status());
                    }
                }
            }
        }
    }
}

fn save_report_to_boot_volume(report: &str) -> Result<(), String> {
    let mut sfs = boot::get_image_file_system(boot::image_handle())
        .map_err(|e| format!("get_image_file_system failed: {:?}", e.status()))?;

    let mut root = sfs
        .open_volume()
        .map_err(|e| format!("open_volume failed: {:?}", e.status()))?;

    // If an older file exists, delete it first so size truncates cleanly
    if let Ok(existing) = root.open(LOG_FILENAME, FileMode::ReadWrite, FileAttribute::empty()) {
        let _ = existing.delete();
    }

    let handle = root
        .open(
            LOG_FILENAME,
            FileMode::CreateReadWrite,
            FileAttribute::empty(),
        )
        .map_err(|e| format!("create GAOKUN-PROBE.TXT failed (media read-only?): {:?}", e.status()))?;

    match handle
        .into_type()
        .map_err(|e| format!("into_type failed: {:?}", e.status()))?
    {
        FileType::Regular(mut file) => {
            file.write(report.as_bytes())
                .map_err(|e| format!("write failed: {:?}", e.status()))?;
            file.flush()
                .map_err(|e| format!("flush failed: {:?}", e.status()))?;
            Ok(())
        }
        FileType::Dir(_) => Err(String::from("GAOKUN-PROBE.TXT is a directory")),
    }
}

#[entry]
fn main() -> Status {
    let _ = system::with_stdout(|stdout| stdout.clear());

    print_screen("===================================================\r\n");
    print_screen("  gaokun-probe v0.5 (FV Touch/SPI/Mouse Extractor) \r\n");
    print_screen("===================================================\r\n");
    print_screen("Extracting touch/input DXEs from UEFI FV to \\FV_DUMP\\...\r\n");

    let mut report = String::with_capacity(32 * 1024);
    let _ = writeln!(report, "gaokun-probe v0.5 report");
    let _ = writeln!(report, "========================");

    probe_cpu_and_firmware(&mut report);
    probe_display(&mut report);
    probe_input_inventory(&mut report);
    probe_storage(&mut report);
    dump_fv_touch_drivers(&mut report);

    // Save final full report
    match save_report_to_boot_volume(&report) {
        Ok(()) => {
            print_screen("\r\n===================================================\r\n");
            print_screen("[DONE] FV DXEs saved to \\FV_DUMP\\ and report saved!\r\n");
            print_screen("Returning to UEFI firmware in 5 seconds...\r\n");
            print_screen("===================================================\r\n");
            boot::stall(5_000_000);
        }
        Err(e) => {
            let msg = format!("\r\n[WARN] Save failed: {}\r\n", e);
            print_screen(&msg);
            boot::stall(10_000_000);
        }
    }

    Status::SUCCESS
}
