use alloc::format;
use alloc::string::String;
use core::fmt::Write;

use uefi::cstr16;
use uefi::runtime::{self, VariableAttributes, VariableVendor};

use crate::efi_extra::GAOKUN_VENDOR_GUID;

const CFG_VAR_NAME: &uefi::CStr16 = cstr16!("GaokunBootCfg");
const CFG_VENDOR: VariableVendor = VariableVendor(GAOKUN_VENDOR_GUID);

/// Allowed user-configurable auto-boot timeouts (seconds). 0 = Off (wait forever).
pub const TIMEOUT_CHOICES: [u8; 4] = [0, 3, 5, 10];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AndroidSlot {
    A,
    B,
}

impl AndroidSlot {
    pub fn as_str(self) -> &'static str {
        match self {
            AndroidSlot::A => "a",
            AndroidSlot::B => "b",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            AndroidSlot::A => "Slot A (_a)",
            AndroidSlot::B => "Slot B (_b)",
        }
    }

    pub fn toggle(self) -> Self {
        match self {
            AndroidSlot::A => AndroidSlot::B,
            AndroidSlot::B => AndroidSlot::A,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootMethod {
    Chainload,
    DirectStub,
}

impl BootMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            BootMethod::Chainload => "chainload",
            BootMethod::DirectStub => "direct",
        }
    }

    pub fn toggle(self) -> Self {
        match self {
            BootMethod::Chainload => BootMethod::DirectStub,
            BootMethod::DirectStub => BootMethod::Chainload,
        }
    }
}

#[derive(Clone, Debug)]
pub struct BootConfig {
    /// Entry ID to highlight/auto-boot by default.
    /// Special values: "" (first entry), "@last" (remember last booted entry).
    pub default_id: String,
    /// Auto-boot timeout in seconds (0 = disabled).
    pub timeout_secs: u8,
    /// Last entry ID booted by the user.
    pub last_booted_id: String,
    /// Preferred Android A/B slot.
    pub android_slot: AndroidSlot,
    /// Preferred Android boot method when both Chainload (gk3boot) and Direct Stub exist.
    pub android_method: BootMethod,
    /// Preferred Linux boot method when both Direct Stub and Chainload (GRUB/EFI) exist.
    pub linux_method: BootMethod,
    /// Whether to boot Linux / EFI-stub entries in EL2 (via slbounce + optional qebspil).
    pub linux_el2: bool,
    /// Pointer speed level: 1 (Slow) .. 5 (Turbo). Default 2 (0.60x).
    pub ptr_speed: u8,
    /// Whether to merge duplicate OS entries (Chainload + Direct Stub & A/B slots) into single cards.
    pub group_entries: bool,
}

impl Default for BootConfig {
    fn default() -> Self {
        Self {
            default_id: String::new(),
            timeout_secs: 5,
            last_booted_id: String::new(),
            android_slot: AndroidSlot::A,
            android_method: BootMethod::Chainload,
            linux_method: BootMethod::DirectStub,
            linux_el2: false,
            ptr_speed: 2,
            group_entries: true,
        }
    }
}

impl BootConfig {
    pub fn load() -> Self {
        let mut cfg = Self::default();
        let mut buf = [0u8; 1024];
        if let Ok((bytes, _attrs)) = runtime::get_variable(CFG_VAR_NAME, &CFG_VENDOR, &mut buf) {
            if let Ok(text) = core::str::from_utf8(bytes) {
                cfg.apply_text(text);
            }
        }
        cfg
    }

    pub fn apply_text(&mut self, text: &str) {
        for raw_line in text.lines() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                let k = k.trim();
                let v = v.trim();
                match k {
                    "default" => self.default_id = String::from(v),
                    "timeout" => {
                        if let Ok(n) = v.parse::<u8>() {
                            self.timeout_secs = n.min(60);
                        }
                    }
                    "last" => self.last_booted_id = String::from(v),
                    "android_slot" => {
                        self.android_slot = if v.eq_ignore_ascii_case("b") {
                            AndroidSlot::B
                        } else {
                            AndroidSlot::A
                        };
                    }
                    "android_method" => {
                        self.android_method = if v.eq_ignore_ascii_case("direct") {
                            BootMethod::DirectStub
                        } else {
                            BootMethod::Chainload
                        };
                    }
                    "linux_method" => {
                        self.linux_method = if v.eq_ignore_ascii_case("chainload") {
                            BootMethod::Chainload
                        } else {
                            BootMethod::DirectStub
                        };
                    }
                    "linux_el2" => {
                        self.linux_el2 = v == "1" || v.eq_ignore_ascii_case("true");
                    }
                    "ptr_speed" => {
                        if let Ok(n) = v.parse::<u8>() {
                            self.ptr_speed = n.clamp(1, 5);
                        }
                    }
                    "group_entries" => {
                        self.group_entries = !(v == "0" || v.eq_ignore_ascii_case("false"));
                    }
                    _ => {}
                }
            }
        }
    }

    pub fn serialize(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(s, "default={}", self.default_id);
        let _ = writeln!(s, "timeout={}", self.timeout_secs);
        let _ = writeln!(s, "last={}", self.last_booted_id);
        let _ = writeln!(s, "android_slot={}", self.android_slot.as_str());
        let _ = writeln!(s, "android_method={}", self.android_method.as_str());
        let _ = writeln!(s, "linux_method={}", self.linux_method.as_str());
        let _ = writeln!(s, "linux_el2={}", if self.linux_el2 { 1 } else { 0 });
        let _ = writeln!(s, "ptr_speed={}", self.ptr_speed);
        let _ = writeln!(s, "group_entries={}", if self.group_entries { 1 } else { 0 });
        s
    }

    pub fn save(&self) -> Result<(), String> {
        let data = self.serialize();
        let attrs = VariableAttributes::NON_VOLATILE
            | VariableAttributes::BOOTSERVICE_ACCESS
            | VariableAttributes::RUNTIME_ACCESS;
        runtime::set_variable(CFG_VAR_NAME, &CFG_VENDOR, attrs, data.as_bytes())
            .map_err(|e| format!("NVRAM save failed: {:?}", e.status()))
    }

    pub fn cycle_timeout(&mut self) {
        let cur_pos = TIMEOUT_CHOICES
            .iter()
            .position(|&x| x == self.timeout_secs)
            .unwrap_or(0);
        self.timeout_secs = TIMEOUT_CHOICES[(cur_pos + 1) % TIMEOUT_CHOICES.len()];
    }

    pub fn timeout_label(&self) -> String {
        if self.timeout_secs == 0 {
            String::from("Off (Manual)")
        } else {
            format!("{}s", self.timeout_secs)
        }
    }

    pub fn cycle_ptr_speed(&mut self) {
        self.ptr_speed = if self.ptr_speed >= 5 {
            1
        } else {
            self.ptr_speed + 1
        };
    }

    pub fn ptr_speed_label(&self) -> &'static str {
        match self.ptr_speed {
            1 => "1/5 - Precision (0.35x)",
            2 => "2/5 - Smooth (0.60x)",
            3 => "3/5 - Balanced (1.00x)",
            4 => "4/5 - Fast (1.50x)",
            _ => "5/5 - Turbo (2.00x)",
        }
    }

    /// Fixed-point multiplier (numerator / 20) for sub-pixel pointer scaling.
    pub fn ptr_speed_num(&self) -> i32 {
        match self.ptr_speed {
            1 => 7,  // 0.35x
            2 => 12, // 0.60x
            3 => 20, // 1.00x
            4 => 30, // 1.50x
            _ => 40, // 2.00x
        }
    }
}
