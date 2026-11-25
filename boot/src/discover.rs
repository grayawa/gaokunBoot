use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use uefi::boot::{self, OpenProtocolAttributes, OpenProtocolParams, SearchType};
use uefi::proto::loaded_image::LoadedImage;
use uefi::proto::media::file::{File, FileAttribute, FileMode, FileType};
use uefi::proto::media::fs::SimpleFileSystem;
use uefi::{cstr16, CStr16, Handle, Identify};

use crate::config::{AndroidSlot, BootConfig, BootMethod};
use crate::efi_extra::format_device_path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OsKind {
    Windows,
    Android,
    Linux,
    Tools,
    Generic,
}

impl OsKind {
    pub fn label(self) -> &'static str {
        match self {
            OsKind::Windows => "WINDOWS",
            OsKind::Android => "ANDROID",
            OsKind::Linux => "LINUX",
            OsKind::Tools => "TOOLS",
            OsKind::Generic => "EFI",
        }
    }
}

#[derive(Clone, Debug)]
pub enum BootPayload {
    Efi {
        path: String,
        options: String,
    },
    Linux {
        kernel: String,
        initrd: Option<String>,
        devicetree: Option<String>,
        el2_devicetree: Option<String>,
        options: String,
    },
}

/// A single boot method variant (either Chainload or Direct EFI Stub), optionally supporting Slot A/B.
#[derive(Clone, Debug)]
pub struct MethodVariant {
    pub label: String,
    pub version: String,
    pub source_id: String,
    pub slot_a: BootPayload,
    pub slot_b: Option<BootPayload>,
    pub drivers: Vec<String>,
}

impl MethodVariant {
    pub fn has_ab(&self) -> bool {
        self.slot_b.is_some()
    }

    pub fn payload_for_slot(&self, slot: AndroidSlot) -> &BootPayload {
        if slot == AndroidSlot::B {
            if let Some(ref b) = self.slot_b {
                return b;
            }
        }
        &self.slot_a
    }
}

/// Discovered EL2 UEFI helper drivers (slbounce + optional qebspil) on the ESP.
#[derive(Clone, Debug, Default)]
pub struct El2Drivers {
    pub fs_handle: Option<Handle>,
    pub slbounce: Option<String>,
    pub qebspil: Option<String>,
}

impl El2Drivers {
    pub fn is_ready(&self) -> bool {
        self.fs_handle.is_some() && self.slbounce.is_some()
    }
}

#[derive(Clone, Debug)]
pub struct BootEntry {
    pub id: String,
    pub title: String,
    pub subtitle: String,
    pub sort_key: String,
    pub kind: OsKind,
    pub volume_label: String,
    pub fs_handle: Handle,
    /// Optional Chainload boot variant (e.g., Windows bootmgfw, Android gk3boot.efi, Linux GRUB).
    pub chainload: Option<MethodVariant>,
    /// Optional Direct ARM64 EFI Stub variant (e.g., crDroid Image + DTB + initrd, Linux vmlinuz).
    pub direct: Option<MethodVariant>,
}

impl BootEntry {
    pub fn has_both_methods(&self) -> bool {
        self.chainload.is_some() && self.direct.is_some()
    }

    pub fn active_method(&self, cfg: &BootConfig) -> BootMethod {
        match (&self.chainload, &self.direct) {
            (Some(_), Some(_)) => match self.kind {
                OsKind::Android => cfg.android_method,
                _ => cfg.linux_method,
            },
            (Some(_), None) => BootMethod::Chainload,
            (None, Some(_)) => BootMethod::DirectStub,
            (None, None) => BootMethod::Chainload,
        }
    }

    pub fn active_variant(&self, cfg: &BootConfig) -> &MethodVariant {
        match self.active_method(cfg) {
            BootMethod::Chainload => self
                .chainload
                .as_ref()
                .or(self.direct.as_ref())
                .expect("BootEntry has at least one variant"),
            BootMethod::DirectStub => self
                .direct
                .as_ref()
                .or(self.chainload.as_ref())
                .expect("BootEntry has at least one variant"),
        }
    }

    pub fn active_payload(&self, cfg: &BootConfig) -> &BootPayload {
        self.active_variant(cfg).payload_for_slot(cfg.android_slot)
    }

    pub fn active_supports_ab(&self, cfg: &BootConfig) -> bool {
        self.active_variant(cfg).has_ab()
    }

    pub fn active_supports_el2(&self, cfg: &BootConfig) -> bool {
        matches!(self.active_payload(cfg), BootPayload::Linux { .. })
    }

    pub fn mode_label(&self, cfg: &BootConfig) -> &'static str {
        match self.active_payload(cfg) {
            BootPayload::Efi { .. } => "UEFI Chainload",
            BootPayload::Linux { .. } => "ARM64 Linux EFI Stub",
        }
    }

    pub fn chainload_label(&self) -> &str {
        self.chainload
            .as_ref()
            .map(|v| v.label.as_str())
            .unwrap_or("Chainload")
    }

    pub fn direct_label(&self) -> &str {
        self.direct
            .as_ref()
            .map(|v| v.label.as_str())
            .unwrap_or("Direct Stub")
    }
}

/// Normalize forward/back slashes to UEFI backslash path starting with `\`.
pub fn to_efi_path(path: &str) -> String {
    let trimmed = path.trim();
    let mut out = String::with_capacity(trimmed.len() + 1);
    if !trimmed.starts_with('\\') && !trimmed.starts_with('/') {
        out.push('\\');
    }
    for ch in trimmed.chars() {
        if ch == '/' {
            out.push('\\');
        } else {
            out.push(ch);
        }
    }
    out
}

fn file_exists(root: &mut uefi::proto::media::file::Directory, efi_path: &str) -> bool {
    let mut buf = [0u16; 256];
    let Ok(cpath) = CStr16::from_str_with_buf(efi_path, &mut buf) else {
        return false;
    };
    root.open(cpath, FileMode::Read, FileAttribute::empty()).is_ok()
}

fn read_small_text_file(
    root: &mut uefi::proto::media::file::Directory,
    efi_path: &str,
) -> Option<String> {
    let mut buf = [0u16; 256];
    let cpath = CStr16::from_str_with_buf(efi_path, &mut buf).ok()?;
    let fh = root
        .open(cpath, FileMode::Read, FileAttribute::empty())
        .ok()?;
    let FileType::Regular(mut rf) = fh.into_type().ok()? else {
        return None;
    };

    let mut out = Vec::new();
    let mut chunk = [0u8; 512];
    loop {
        match rf.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                out.extend_from_slice(&chunk[..n]);
                if out.len() > 32 * 1024 {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    String::from_utf8(out).ok()
}

fn volume_friendly_name(dp: &str, is_boot_vol: bool) -> String {
    let lower = dp.to_ascii_lowercase();
    if lower.contains("nvme(") {
        if lower.contains("hd(1,") {
            return String::from("NVMe ESP");
        }
        return String::from("NVMe Volume");
    }
    if lower.contains("usb(") {
        return if is_boot_vol {
            String::from("USB Boot Media")
        } else {
            String::from("USB Drive")
        };
    }
    if lower.contains("ufs(") {
        return String::from("UFS ESP");
    }
    String::from("UEFI Volume")
}

fn should_skip_volume(dp: &str) -> bool {
    let lower = dp.to_ascii_lowercase();
    lower.contains("memorymapped(")
        || lower.starts_with("fv(")
        || lower.contains("7cce9c94-983f-4d0a-8143-b6c05545b223")
}

fn classify_entry(filename: &str, title: &str, options: &str, payload: &BootPayload) -> OsKind {
    let joined = format!(
        "{} {} {}",
        filename.to_ascii_lowercase(),
        title.to_ascii_lowercase(),
        options.to_ascii_lowercase()
    );
    if joined.contains("fastboot")
        || joined.contains("tool")
        || joined.contains("recovery")
        || joined.contains("shell")
    {
        return OsKind::Tools;
    }
    if joined.contains("windows") || joined.contains("bootmgfw") {
        return OsKind::Windows;
    }
    if joined.contains("android")
        || joined.contains("crdroid")
        || joined.contains("gk3boot")
        || joined.contains("lineage")
    {
        return OsKind::Android;
    }
    if joined.contains("linux")
        || joined.contains("arch")
        || joined.contains("fedora")
        || joined.contains("ubuntu")
        || joined.contains("debian")
        || joined.contains("grub")
    {
        return OsKind::Linux;
    }
    match payload {
        BootPayload::Linux { .. } => OsKind::Linux,
        BootPayload::Efi { .. } => OsKind::Generic,
    }
}

fn detect_slot(filename: &str, options: &str) -> Option<AndroidSlot> {
    let fl = filename.to_ascii_lowercase();
    if options.contains("gk3.hint=a")
        || options.contains("slot_suffix=_a")
        || fl.contains("-android-a")
    {
        Some(AndroidSlot::A)
    } else if options.contains("gk3.hint=b")
        || options.contains("slot_suffix=_b")
        || fl.contains("-android-b")
    {
        Some(AndroidSlot::B)
    } else {
        None
    }
}

fn ab_pair_key(filename: &str, kind: OsKind) -> Option<String> {
    if kind != OsKind::Android {
        return None;
    }
    let fl = filename.to_ascii_lowercase();
    if fl.starts_with("gk3boot-android-") {
        return Some(String::from("pair:gk3boot-android"));
    }
    if let Some(pos) = fl.find("-android-") {
        return Some(format!("pair:{}-android", &fl[..pos]));
    }
    None
}

fn find_el2_dtb(
    root: &mut uefi::proto::media::file::Directory,
    dtb_path: &str,
) -> Option<String> {
    if let Some(stem) = dtb_path.strip_suffix(".dtb") {
        let candidate = format!("{}-el2.dtb", stem);
        if file_exists(root, &candidate) {
            return Some(candidate);
        }
    }
    None
}

fn infer_method_label(title: &str, payload: &BootPayload) -> String {
    match payload {
        BootPayload::Efi { path, .. } => {
            let low = path.to_ascii_lowercase();
            if low.contains("gk3boot") {
                String::from("gk3boot")
            } else if low.contains("grub") {
                String::from("GRUB")
            } else if low.contains("systemd") {
                String::from("systemd-boot")
            } else if low.contains("bootmgfw") {
                String::from("Windows BootMgr")
            } else {
                String::from("EFI Chainload")
            }
        }
        BootPayload::Linux { .. } => {
            let low = title.to_ascii_lowercase();
            if low.contains("crdroid") {
                String::from("Direct (crDroid)")
            } else {
                String::from("Direct Stub")
            }
        }
    }
}

#[derive(Clone)]
struct ParsedRawEntry {
    entry: BootEntry,
    slot: Option<AndroidSlot>,
    pair_key: Option<String>,
}

fn parse_bls_entry(
    root: &mut uefi::proto::media::file::Directory,
    filename: &str,
    text: &str,
    fs_handle: Handle,
    volume_label: &str,
) -> Option<ParsedRawEntry> {
    let mut title = String::new();
    let mut version = String::new();
    let mut sort_key = String::new();
    let mut efi_path: Option<String> = None;
    let mut linux_path: Option<String> = None;
    let mut initrd_path: Option<String> = None;
    let mut dtb_path: Option<String> = None;
    let mut el2_dtb_path: Option<String> = None;
    let mut options = String::new();
    let mut drivers: Vec<String> = Vec::new();

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(2, |c: char| c.is_ascii_whitespace());
        let key = parts.next()?.trim();
        let val = parts.next().unwrap_or("").trim();
        if val.is_empty() {
            continue;
        }
        match key {
            "title" => title = String::from(val),
            "version" => version = String::from(val),
            "sort-key" => sort_key = String::from(val),
            "efi" => efi_path = Some(to_efi_path(val)),
            "linux" => linux_path = Some(to_efi_path(val)),
            "initrd" => initrd_path = Some(to_efi_path(val)),
            "devicetree" => dtb_path = Some(to_efi_path(val)),
            "el2-devicetree" => el2_dtb_path = Some(to_efi_path(val)),
            "driver" => drivers.push(to_efi_path(val)),
            "options" => {
                if !options.is_empty() {
                    options.push(' ');
                }
                options.push_str(val);
            }
            _ => {}
        }
    }

    let is_linux_stub = linux_path.is_some();
    let payload = if let Some(linux) = linux_path {
        if el2_dtb_path.is_none() {
            if let Some(ref d) = dtb_path {
                el2_dtb_path = find_el2_dtb(root, d);
            }
        }
        BootPayload::Linux {
            kernel: linux,
            initrd: initrd_path,
            devicetree: dtb_path,
            el2_devicetree: el2_dtb_path,
            options: options.clone(),
        }
    } else {
        let efi = efi_path?;
        BootPayload::Efi {
            path: efi,
            options: options.clone(),
        }
    };

    if title.is_empty() {
        title = String::from(filename.trim_end_matches(".conf"));
    }

    let clean_title = title.replace(['\u{2014}', '\u{2013}'], "-");
    let kind = classify_entry(filename, &clean_title, &options, &payload);
    let slot = detect_slot(filename, &options);
    let method_label = infer_method_label(&clean_title, &payload);

    let variant = MethodVariant {
        label: method_label,
        version: version.clone(),
        source_id: String::from(filename),
        slot_a: payload,
        slot_b: None,
        drivers,
    };

    let subtitle = if !version.is_empty() {
        format!("{} | {}", version, volume_label)
    } else {
        String::from(volume_label)
    };

    let entry = BootEntry {
        id: String::from(filename),
        title: clean_title,
        subtitle,
        sort_key: if sort_key.is_empty() {
            String::from("zzzz")
        } else {
            sort_key
        },
        kind,
        volume_label: String::from(volume_label),
        fs_handle,
        chainload: if !is_linux_stub { Some(variant.clone()) } else { None },
        direct: if is_linux_stub { Some(variant) } else { None },
    };

    let pair_key = ab_pair_key(filename, kind);
    Some(ParsedRawEntry {
        entry,
        slot,
        pair_key,
    })
}

fn scan_bls_dir(
    root: &mut uefi::proto::media::file::Directory,
    dir_path: &CStr16,
    dir_prefix: &str,
    fs_handle: Handle,
    volume_label: &str,
    out: &mut Vec<ParsedRawEntry>,
) {
    let Ok(dh) = root.open(dir_path, FileMode::Read, FileAttribute::empty()) else {
        return;
    };
    let Ok(FileType::Dir(mut dir)) = dh.into_type() else {
        return;
    };

    let mut conf_names: Vec<String> = Vec::new();
    let mut buf = [0u8; 512];
    while let Ok(Some(info)) = dir.read_entry(&mut buf) {
        let name = format!("{}", info.file_name());
        if name.ends_with(".conf") && !name.starts_with('.') {
            conf_names.push(name);
        }
    }

    for fname in conf_names {
        let full = format!("{}\\{}", dir_prefix, fname);
        if let Some(text) = read_small_text_file(root, &full) {
            if let Some(parsed) = parse_bls_entry(root, &fname, &text, fs_handle, volume_label) {
                out.push(parsed);
            }
        }
    }
}

fn probe_el2_drivers_on_volume(
    root: &mut uefi::proto::media::file::Directory,
    fs_handle: Handle,
    el2: &mut El2Drivers,
) {
    if el2.slbounce.is_none() {
        for cand in [
            "\\EFI\\gaokunBoot\\drivers\\slbounce.efi",
            "\\EFI\\systemd\\drivers\\slbounce.efi",
            "\\EFI\\slbounce.efi",
            "\\slbounce.efi",
        ] {
            if file_exists(root, cand) {
                el2.slbounce = Some(String::from(cand));
                el2.fs_handle = Some(fs_handle);
                break;
            }
        }
    }
    if el2.qebspil.is_none() {
        for cand in [
            "\\EFI\\gaokunBoot\\drivers\\qebspil.efi",
            "\\EFI\\systemd\\drivers\\qebspil.efi",
            "\\EFI\\qebspil.efi",
            "\\qebspil.efi",
        ] {
            if file_exists(root, cand) {
                el2.qebspil = Some(String::from(cand));
                if el2.fs_handle.is_none() {
                    el2.fs_handle = Some(fs_handle);
                }
                break;
            }
        }
    }
}

fn clean_paired_title(title: &str) -> String {
    for suffix in [" - slot _a", " - slot _b", " (slot _a)", " (slot _b)"] {
        if let Some(stripped) = title.strip_suffix(suffix) {
            return String::from(stripped.trim());
        }
    }
    String::from(title)
}

/// Step 1 of grouping: merge paired Android Slot A + Slot B entries into single MethodVariants.
fn merge_ab_entries(raw: Vec<ParsedRawEntry>) -> Vec<BootEntry> {
    let mut merged: Vec<BootEntry> = Vec::new();
    let mut paired_indices: Vec<(String, usize)> = Vec::new();

    for item in raw {
        if let (Some(ref key), Some(slot)) = (&item.pair_key, item.slot) {
            if let Some(&(_, idx)) = paired_indices.iter().find(|(k, _)| k == key) {
                let target = &mut merged[idx];
                // Attach Slot B payload onto the matching method variant (chainload or direct)
                if let (Some(ref mut dst), Some(src)) = (&mut target.chainload, item.entry.chainload)
                {
                    match slot {
                        AndroidSlot::B => dst.slot_b = Some(src.slot_a),
                        AndroidSlot::A => {
                            let prev = core::mem::replace(&mut dst.slot_a, src.slot_a);
                            dst.slot_b = Some(prev);
                            dst.source_id = src.source_id;
                            target.id = item.entry.id;
                        }
                    }
                } else if let (Some(ref mut dst), Some(src)) =
                    (&mut target.direct, item.entry.direct)
                {
                    match slot {
                        AndroidSlot::B => dst.slot_b = Some(src.slot_a),
                        AndroidSlot::A => {
                            let prev = core::mem::replace(&mut dst.slot_a, src.slot_a);
                            dst.slot_b = Some(prev);
                            dst.source_id = src.source_id;
                            target.id = item.entry.id;
                        }
                    }
                }
                target.title = clean_paired_title(&target.title);
                target.subtitle = format!("A/B Dual-Slot | {}", target.volume_label);
                continue;
            } else {
                let idx = merged.len();
                let mut e = item.entry;
                e.title = clean_paired_title(&e.title);
                merged.push(e);
                paired_indices.push((key.clone(), idx));
                continue;
            }
        }
        merged.push(item.entry);
    }

    merged
}

/// Step 2 of grouping: merge Chainload + Direct EFI Stub entries of the same OS (Android / Linux)
/// into ONE clean sidebar card so users switch Boot Method cleanly in the right-hand OS Settings pane.
fn merge_os_methods(entries: Vec<BootEntry>) -> Vec<BootEntry> {
    let mut out: Vec<BootEntry> = Vec::new();

    for entry in entries {
        let mergeable_kind = matches!(entry.kind, OsKind::Android | OsKind::Linux);
        if mergeable_kind {
            if let Some(existing) = out.iter_mut().find(|e| {
                e.kind == entry.kind
                    && e.fs_handle == entry.fs_handle
                    && ((e.chainload.is_some() && e.direct.is_none() && entry.direct.is_some())
                        || (e.direct.is_some() && e.chainload.is_none() && entry.chainload.is_some()))
            }) {
                // Combine the two complementary boot methods onto one unified OS card!
                if existing.chainload.is_none() {
                    existing.chainload = entry.chainload;
                }
                if existing.direct.is_none() {
                    existing.direct = entry.direct;
                }
                // Prefer the more descriptive title (e.g. "crDroid 16.0 (gaokun3)" over generic "Android")
                if existing.title.eq_ignore_ascii_case("android")
                    && !entry.title.eq_ignore_ascii_case("android")
                {
                    existing.title = entry.title;
                }
                if existing.kind == OsKind::Android {
                    existing.id = String::from("android");
                    existing.sort_key = String::from("01android");
                } else if existing.kind == OsKind::Linux {
                    existing.sort_key = existing.sort_key.clone().min(entry.sort_key);
                }
                existing.subtitle = format!("Dual-Method | {}", existing.volume_label);
                continue;
            }
        }
        out.push(entry);
    }

    out
}

pub struct DiscoveryResult {
    pub grouped_entries: Vec<BootEntry>,
    pub raw_entries: Vec<BootEntry>,
    pub esp_config_text: Option<String>,
    pub el2_drivers: El2Drivers,
}

/// Discover all bootable OS entries + EL2 helper drivers across attached UEFI filesystems.
/// Returns both `grouped_entries` (clean 1-card-per-OS view) and `raw_entries` (every raw entry).
pub fn discover_entries() -> DiscoveryResult {
    let mut base_entries: Vec<BootEntry> = Vec::new();
    let mut raw_bls: Vec<ParsedRawEntry> = Vec::new();
    let mut esp_config_text: Option<String> = None;
    let mut el2_drivers = El2Drivers::default();

    let boot_dev_handle: Option<Handle> = unsafe {
        boot::open_protocol::<LoadedImage>(
            OpenProtocolParams {
                handle: boot::image_handle(),
                agent: boot::image_handle(),
                controller: None,
            },
            OpenProtocolAttributes::GetProtocol,
        )
    }
    .ok()
    .and_then(|li| li.device());

    let Ok(handles) = boot::locate_handle_buffer(SearchType::ByProtocol(&SimpleFileSystem::GUID))
    else {
        return DiscoveryResult {
            grouped_entries: Vec::new(),
            raw_entries: Vec::new(),
            esp_config_text,
            el2_drivers,
        };
    };

    let mut windows_count = 0usize;

    for &handle in handles.iter() {
        let dp = format_device_path(handle);
        if should_skip_volume(&dp) {
            continue;
        }
        let is_boot_vol = boot_dev_handle == Some(handle);
        let vol_label = volume_friendly_name(&dp, is_boot_vol);

        let Ok(mut sfs) = (unsafe {
            boot::open_protocol::<SimpleFileSystem>(
                OpenProtocolParams {
                    handle,
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

        if esp_config_text.is_none() {
            esp_config_text = read_small_text_file(&mut root, "\\EFI\\gaokunBoot\\config.txt");
        }

        probe_el2_drivers_on_volume(&mut root, handle, &mut el2_drivers);

        // 1. Windows Boot Manager
        if file_exists(&mut root, "\\EFI\\Microsoft\\Boot\\bootmgfw.efi") {
            let is_recovery = file_exists(&mut root, "\\sources\\boot.wim")
                || file_exists(&mut root, "\\EFI\\OneKeyLog.txt");
            windows_count += 1;
            let (id, title, subtitle, kind, sort_key) = if is_recovery {
                (
                    format!("windows-recovery-{}", windows_count),
                    String::from("Windows Recovery (WinRE)"),
                    format!("Recovery Partition | {}", vol_label),
                    OsKind::Tools,
                    String::from("9winre"),
                )
            } else {
                (
                    if windows_count == 1 {
                        String::from("windows")
                    } else {
                        format!("windows-{}", windows_count)
                    },
                    String::from("Windows 11 ARM64"),
                    format!("Microsoft Boot Manager | {}", vol_label),
                    OsKind::Windows,
                    String::from("00windows"),
                )
            };

            let variant = MethodVariant {
                label: String::from("Windows BootMgr"),
                version: String::from("EFI Boot Manager"),
                source_id: id.clone(),
                slot_a: BootPayload::Efi {
                    path: String::from("\\EFI\\Microsoft\\Boot\\bootmgfw.efi"),
                    options: String::new(),
                },
                slot_b: None,
                drivers: Vec::new(),
            };

            base_entries.push(BootEntry {
                id,
                title,
                subtitle,
                sort_key,
                kind,
                volume_label: vol_label.clone(),
                fs_handle: handle,
                chainload: Some(variant),
                direct: None,
            });
        }

        // 2. Standard systemd-boot / BLS entries
        scan_bls_dir(
            &mut root,
            cstr16!("\\loader\\entries"),
            "\\loader\\entries",
            handle,
            &vol_label,
            &mut raw_bls,
        );

        // 3. Custom gaokunBoot entries
        scan_bls_dir(
            &mut root,
            cstr16!("\\EFI\\gaokunBoot\\entries"),
            "\\EFI\\gaokunBoot\\entries",
            handle,
            &vol_label,
            &mut raw_bls,
        );

        // 4. Optional Linux GRUB chainloaders (only if present on ESP)
        for (distro, grub_path) in [
            ("Arch Linux", "\\EFI\\arch\\grubaa64.efi"),
            ("Fedora Linux", "\\EFI\\fedora\\grubaa64.efi"),
            ("Ubuntu", "\\EFI\\ubuntu\\grubaa64.efi"),
            ("Debian", "\\EFI\\debian\\grubaa64.efi"),
            ("GRUB Linux", "\\EFI\\grub\\grubaa64.efi"),
        ] {
            if file_exists(&mut root, grub_path) {
                let id = format!("grub-{}", distro.to_ascii_lowercase().replace(' ', "-"));
                let variant = MethodVariant {
                    label: String::from("GRUB"),
                    version: String::from("GRUB2 ARM64"),
                    source_id: String::from(grub_path),
                    slot_a: BootPayload::Efi {
                        path: String::from(grub_path),
                        options: String::new(),
                    },
                    slot_b: None,
                    drivers: Vec::new(),
                };
                raw_bls.push(ParsedRawEntry {
                    entry: BootEntry {
                        id,
                        title: String::from(distro),
                        subtitle: format!("GRUB2 Chainload | {}", vol_label),
                        sort_key: String::from("02linux"),
                        kind: OsKind::Linux,
                        volume_label: vol_label.clone(),
                        fs_handle: handle,
                        chainload: Some(variant),
                        direct: None,
                    },
                    slot: None,
                    pair_key: None,
                });
            }
        }
    }

    // Build Raw list (every individual discovered entry)
    let mut raw_entries = base_entries.clone();
    for item in &raw_bls {
        raw_entries.push(item.entry.clone());
    }
    raw_entries.sort_by(|a, b| a.sort_key.cmp(&b.sort_key).then_with(|| a.id.cmp(&b.id)));

    // Build Grouped list (merge A/B slots + merge Chainload/Direct Stub per OS!)
    let mut grouped_entries = base_entries;
    grouped_entries.extend(merge_os_methods(merge_ab_entries(raw_bls)));
    grouped_entries.sort_by(|a, b| a.sort_key.cmp(&b.sort_key).then_with(|| a.id.cmp(&b.id)));

    DiscoveryResult {
        grouped_entries,
        raw_entries,
        esp_config_text,
        el2_drivers,
    }
}
