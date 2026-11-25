use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::config::{AndroidSlot, BootConfig, TIMEOUT_CHOICES};
use crate::discover::{BootEntry, BootPayload, El2Drivers, OsKind};
use crate::gfx::{Canvas, Color, FontKind, Rect};

// Palette: Midnight Obsidian + Clean Slate Panels
const BG_MAIN: Color = Color::rgb(10, 14, 22);
const BG_BAR: Color = Color::rgb(14, 20, 32);
const BG_SIDEBAR: Color = Color::rgb(13, 19, 30);
const BG_PANEL: Color = Color::rgb(17, 24, 39);
const BG_CARD: Color = Color::rgb(22, 31, 49);
const BG_CARD_SEL: Color = Color::rgb(28, 44, 72);
const BG_CODE: Color = Color::rgb(11, 16, 27);
const BORDER_SUBTLE: Color = Color::rgb(35, 49, 74);
const BORDER_ACCENT: Color = Color::rgb(56, 189, 248);

const TEXT_PRIMARY: Color = Color::rgb(248, 250, 252);
const TEXT_SECONDARY: Color = Color::rgb(148, 163, 184);
const TEXT_MUTED: Color = Color::rgb(100, 116, 139);

const ACCENT_CYAN: Color = Color::rgb(56, 189, 248);
const ACCENT_GREEN: Color = Color::rgb(52, 211, 153);
const ACCENT_AMBER: Color = Color::rgb(251, 191, 36);
const ACCENT_PURPLE: Color = Color::rgb(167, 139, 250);
const ACCENT_RED: Color = Color::rgb(248, 113, 113);

pub const BUILD_VERSION: &str = concat!("v", env!("CARGO_PKG_VERSION"));

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SidebarSel {
    Entry(usize),
    Settings,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusPane {
    Sidebar,
    SettingsPane,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingFocus {
    Timeout,
    DefaultEntry,
    PtrSpeed,
    BiosSetup,
    Reboot,
    Shutdown,
    BackToMenu,
}

const SETTINGS_ORDER: [SettingFocus; 7] = [
    SettingFocus::Timeout,
    SettingFocus::DefaultEntry,
    SettingFocus::PtrSpeed,
    SettingFocus::BiosSetup,
    SettingFocus::Reboot,
    SettingFocus::Shutdown,
    SettingFocus::BackToMenu,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClickAction {
    SelectEntry(usize),
    OpenSettings,
    BootSelected,
    SetSelectedDefault,
    SetAndroidSlot(AndroidSlot),
    SetLinuxEl2(bool),
    SetTimeout(u8),
    CycleDefault,
    SetPtrSpeed(u8),
    BiosSetup,
    Reboot,
    Shutdown,
    BackToMenu,
}

pub struct UiState {
    pub entry_count: usize,
    pub sidebar_sel: SidebarSel,
    pub last_entry_idx: usize,
    pub focus: FocusPane,
    pub setting_idx: usize,
    pub scroll_top: usize,
    pub device_name: String,
    pub bios_ver: String,
    pub current_el: u8,
    pub touch_ready: bool,
    pub toast: Option<(String, bool)>,
    pub countdown_ms: Option<u64>,
    pub countdown_total_ms: u64,
    pub hit_boxes: Vec<(Rect, ClickAction)>,
    pub progress_bar_rect: Rect,
    pub timer_pill_rect: Rect,
}

impl UiState {
    pub fn new(
        entry_count: usize,
        initial_entry: usize,
        device_name: String,
        bios_ver: String,
        current_el: u8,
        touch_ready: bool,
        timeout_secs: u8,
    ) -> Self {
        let initial = if entry_count > 0 {
            initial_entry.min(entry_count - 1)
        } else {
            0
        };
        let sidebar_sel = if entry_count > 0 {
            SidebarSel::Entry(initial)
        } else {
            SidebarSel::Settings
        };
        let total_ms = (timeout_secs as u64) * 1000;
        let countdown_ms = if timeout_secs > 0 && entry_count > 0 {
            Some(total_ms)
        } else {
            None
        };

        Self {
            entry_count,
            sidebar_sel,
            last_entry_idx: initial,
            focus: FocusPane::Sidebar,
            setting_idx: 0,
            scroll_top: 0,
            device_name,
            bios_ver,
            current_el,
            touch_ready,
            toast: None,
            countdown_ms,
            countdown_total_ms: total_ms,
            hit_boxes: Vec::new(),
            progress_bar_rect: Rect::new(0, 108, 2560, 6),
            timer_pill_rect: Rect::new(2300, 30, 210, 48),
        }
    }

    pub fn cancel_countdown(&mut self) {
        self.countdown_ms = None;
    }

    pub fn current_setting(&self) -> SettingFocus {
        SETTINGS_ORDER[self.setting_idx % SETTINGS_ORDER.len()]
    }

    pub fn move_up(&mut self) {
        match self.focus {
            FocusPane::Sidebar => {
                if self.entry_count == 0 {
                    self.sidebar_sel = SidebarSel::Settings;
                    return;
                }
                match self.sidebar_sel {
                    SidebarSel::Entry(0) => self.sidebar_sel = SidebarSel::Settings,
                    SidebarSel::Entry(i) => {
                        self.sidebar_sel = SidebarSel::Entry(i - 1);
                        self.last_entry_idx = i - 1;
                    }
                    SidebarSel::Settings => {
                        let last = self.entry_count - 1;
                        self.sidebar_sel = SidebarSel::Entry(last);
                        self.last_entry_idx = last;
                    }
                }
            }
            FocusPane::SettingsPane => {
                if self.setting_idx == 0 {
                    self.setting_idx = SETTINGS_ORDER.len() - 1;
                } else {
                    self.setting_idx -= 1;
                }
            }
        }
    }

    pub fn move_down(&mut self) {
        match self.focus {
            FocusPane::Sidebar => {
                if self.entry_count == 0 {
                    self.sidebar_sel = SidebarSel::Settings;
                    return;
                }
                match self.sidebar_sel {
                    SidebarSel::Entry(i) if i + 1 < self.entry_count => {
                        self.sidebar_sel = SidebarSel::Entry(i + 1);
                        self.last_entry_idx = i + 1;
                    }
                    SidebarSel::Entry(_) => self.sidebar_sel = SidebarSel::Settings,
                    SidebarSel::Settings => {
                        self.sidebar_sel = SidebarSel::Entry(0);
                        self.last_entry_idx = 0;
                    }
                }
            }
            FocusPane::SettingsPane => {
                self.setting_idx = (self.setting_idx + 1) % SETTINGS_ORDER.len();
            }
        }
    }

    pub fn hit_test(&self, x: i32, y: i32) -> Option<ClickAction> {
        for &(rect, action) in self.hit_boxes.iter().rev() {
            if rect.contains(x, y) {
                return Some(action);
            }
        }
        None
    }

    fn os_accent(kind: OsKind) -> Color {
        match kind {
            OsKind::Windows => ACCENT_CYAN,
            OsKind::Android => ACCENT_GREEN,
            OsKind::Linux => ACCENT_AMBER,
            OsKind::Tools => ACCENT_PURPLE,
            OsKind::Generic => TEXT_SECONDARY,
        }
    }

    fn default_label(&self, entries: &[BootEntry], cfg: &BootConfig) -> String {
        if cfg.default_id == "@last" {
            return String::from("Remember Last Booted");
        }
        if cfg.default_id.is_empty() {
            return entries
                .first()
                .map(|e| format!("First ({})", e.title))
                .unwrap_or_else(|| String::from("First Entry"));
        }
        entries
            .iter()
            .find(|e| e.id == cfg.default_id)
            .map(|e| e.title.clone())
            .unwrap_or_else(|| cfg.default_id.clone())
    }

    fn is_entry_default(entry: &BootEntry, idx: usize, cfg: &BootConfig) -> bool {
        if cfg.default_id.is_empty() {
            idx == 0
        } else if cfg.default_id == "@last" {
            !cfg.last_booted_id.is_empty() && entry.id == cfg.last_booted_id
        } else {
            entry.id == cfg.default_id
        }
    }

    /// Partial-rect update for countdown progress bar + header timer pill (<1ms Blt, never blocks touch!).
    pub fn render_countdown_only(&self, canvas: &mut Canvas, cfg: &BootConfig) {
        // 1. Thin 6px progress bar
        let r = self.progress_bar_rect;
        canvas.fill_rect(r.x, r.y, r.w, r.h, Color::rgb(20, 30, 48));
        if let Some(rem) = self.countdown_ms {
            if let Some(fill_w) = (rem * r.w as u64).checked_div(self.countdown_total_ms) {
                canvas.fill_rect(r.x, r.y, fill_w as usize, r.h, ACCENT_CYAN);
            }
        }
        canvas.present_rect(r.x, r.y, r.w, r.h);

        // 2. Small top-right countdown pill
        let tp = self.timer_pill_rect;
        canvas.fill_rect(tp.x, tp.y, tp.w, tp.h, BG_BAR);
        self.draw_timer_pill(canvas, cfg);
        canvas.present_rect(tp.x, tp.y, tp.w, tp.h);
    }

    fn draw_timer_pill(&self, canvas: &mut Canvas, cfg: &BootConfig) {
        let tp = self.timer_pill_rect;
        let text = match self.countdown_ms {
            Some(ms) => format!("Auto {}s", ms.div_ceil(1000)),
            None => {
                if cfg.timeout_secs == 0 {
                    String::from("Manual")
                } else {
                    format!("Paused ({}s)", cfg.timeout_secs)
                }
            }
        };
        let active = self.countdown_ms.is_some();
        canvas.fill_rounded_rect(
            tp.x,
            tp.y,
            tp.w,
            tp.h,
            12,
            if active {
                Color::rgb(18, 44, 68)
            } else {
                BG_CARD
            },
        );
        canvas.stroke_rounded_rect(
            tp.x,
            tp.y,
            tp.w,
            tp.h,
            12,
            1,
            if active { ACCENT_CYAN } else { BORDER_SUBTLE },
        );
        let tw = canvas.measure_text(FontKind::Mono, &text);
        canvas.draw_text(
            FontKind::Mono,
            tp.x + tp.w.saturating_sub(tw) / 2,
            tp.y + 10,
            &text,
            if active { ACCENT_CYAN } else { TEXT_SECONDARY },
        );
    }

    /// Render the full wireframe layout:
    ///   Top Bar:  [Logo] gaokunBoot                         Device info & Build info | Timer
    ///   Left:     Main Boot Menu (OS cards only) + bottom-pinned [Settings] button
    ///   Right:    OS specific info & Settings   OR   GaokunBoot Settings
    pub fn render(
        &mut self,
        canvas: &mut Canvas,
        entries: &[BootEntry],
        cfg: &BootConfig,
        el2_drivers: &El2Drivers,
    ) {
        self.hit_boxes.clear();
        let w = canvas.w;
        let h = canvas.h;
        canvas.clear(BG_MAIN);

        let header_h = 108usize;
        let bar_h = 6usize;
        let body_y = header_h + bar_h;
        let body_h = h.saturating_sub(body_y);
        let sidebar_w = (w * 29 / 100).clamp(320, 760).min(w / 2);

        self.progress_bar_rect = Rect::new(0, header_h, w, bar_h);

        // =========================================================================
        // 1. TOP BAR (Logo + Title on Left | Device info & Build info on Right)
        // =========================================================================
        canvas.fill_rect(0, 0, w, header_h, BG_BAR);
        canvas.fill_rect(0, header_h - 1, w, 1, BORDER_SUBTLE);

        // Custom vector logo + brand title
        canvas.draw_brand_logo(32, 20, 68);
        canvas.draw_text(FontKind::Large, 120, 18, "gaokunBoot", TEXT_PRIMARY);

        // Top-right timer pill (fixed box for fast dirty-rect updates!)
        let timer_w = 196usize;
        self.timer_pill_rect = Rect::new(w - 36 - timer_w, 30, timer_w, 48);
        self.draw_timer_pill(canvas, cfg);

        // Device info & Build info right next to timer pill
        let touch_tag = if self.touch_ready {
            "Touch ON"
        } else {
            "Touch OFF"
        };
        let info_str = format!(
            "{}  |  {}  |  EL{}  |  {}  |  gaokunBoot {}",
            self.device_name, self.bios_ver, self.current_el, touch_tag, BUILD_VERSION
        );
        let info_w = canvas.measure_text(FontKind::Mono, &info_str);
        let info_x = self.timer_pill_rect.x.saturating_sub(28 + info_w);
        canvas.draw_text(FontKind::Mono, info_x, 40, &info_str, TEXT_SECONDARY);

        // Thin progress bar right below top bar
        let r = self.progress_bar_rect;
        canvas.fill_rect(r.x, r.y, r.w, r.h, Color::rgb(20, 30, 48));
        if let Some(rem) = self.countdown_ms {
            if let Some(fill_w) = (rem * r.w as u64).checked_div(self.countdown_total_ms) {
                canvas.fill_rect(r.x, r.y, fill_w as usize, r.h, ACCENT_CYAN);
            }
        }

        // =========================================================================
        // 2. LEFT SIDEBAR (Main Boot Menu + Bottom-Pinned Settings Button)
        // =========================================================================
        canvas.fill_rect(0, body_y, sidebar_w, body_h, BG_SIDEBAR);
        canvas.fill_rect(sidebar_w, body_y, 2, body_h, BORDER_SUBTLE);

        let sb_pad = 28usize;
        let sb_inner_w = sidebar_w - sb_pad * 2;

        // Bottom-pinned Settings button geometry
        let settings_btn_h = 98usize;
        let settings_btn_y = h - sb_pad - settings_btn_h;
        let settings_rect = Rect::new(sb_pad, settings_btn_y, sb_inner_w, settings_btn_h);

        // Main Boot Menu container area
        let menu_top = body_y + 28;
        let menu_bottom = settings_btn_y - 28;

        canvas.draw_text(
            FontKind::Mono,
            sb_pad + 6,
            menu_top,
            &format!("MAIN BOOT MENU  ({})", entries.len()),
            TEXT_MUTED,
        );

        let list_start_y = menu_top + 42;
        let card_h = 126usize;
        let card_gap = 16usize;
        let max_visible = ((menu_bottom.saturating_sub(list_start_y)) + card_gap) / (card_h + card_gap);
        let max_visible = max_visible.max(1);

        if let SidebarSel::Entry(idx) = self.sidebar_sel {
            if idx < self.scroll_top {
                self.scroll_top = idx;
            } else if idx >= self.scroll_top + max_visible {
                self.scroll_top = idx + 1 - max_visible;
            }
        }

        let mut cy = list_start_y;
        for (idx, entry) in entries.iter().enumerate().skip(self.scroll_top) {
            if cy + card_h > menu_bottom {
                canvas.draw_text(
                    FontKind::Mono,
                    sb_pad + 12,
                    menu_bottom.saturating_sub(28),
                    "... more entries below ...",
                    TEXT_MUTED,
                );
                break;
            }

            let is_sel = self.sidebar_sel == SidebarSel::Entry(idx);
            let accent = Self::os_accent(entry.kind);
            let card_rect = Rect::new(sb_pad, cy, sb_inner_w, card_h);
            self.hit_boxes.push((card_rect, ClickAction::SelectEntry(idx)));

            canvas.fill_rounded_rect(
                card_rect.x,
                card_rect.y,
                card_rect.w,
                card_rect.h,
                20,
                if is_sel { BG_CARD_SEL } else { BG_CARD },
            );
            if is_sel {
                canvas.stroke_rounded_rect(
                    card_rect.x,
                    card_rect.y,
                    card_rect.w,
                    card_rect.h,
                    20,
                    3,
                    accent,
                );
                canvas.fill_rounded_rect(
                    card_rect.x + 12,
                    card_rect.y + 24,
                    8,
                    card_rect.h - 48,
                    4,
                    accent,
                );
            } else {
                canvas.stroke_rounded_rect(
                    card_rect.x,
                    card_rect.y,
                    card_rect.w,
                    card_rect.h,
                    20,
                    1,
                    BORDER_SUBTLE,
                );
            }

            // Vector OS Icon
            canvas.draw_os_icon(card_rect.x + 32, card_rect.y + 27, 72, entry.kind);

            // Title & live subtitle
            let tx = card_rect.x + 124;
            let is_def = Self::is_entry_default(entry, idx, cfg);
            let def_reserve = if is_def { 112 } else { 20 };
            canvas.draw_text_ellipsis(
                FontKind::Ui,
                tx,
                card_rect.y + 22,
                card_rect.w.saturating_sub(124 + def_reserve),
                &entry.title,
                TEXT_PRIMARY,
            );

            let mut sub = entry.subtitle.clone();
            if entry.supports_ab_slot {
                sub = format!("{} | {}", cfg.android_slot.label(), entry.volume_label);
            }
            if entry.supports_el2 {
                let el_tag = if cfg.linux_el2 { "EL2" } else { "EL1" };
                sub = format!("{} | {}", sub, el_tag);
            }
            canvas.draw_text_ellipsis(
                FontKind::Mono,
                tx,
                card_rect.y + 74,
                card_rect.w.saturating_sub(124 + 20),
                &sub,
                TEXT_SECONDARY,
            );

            if is_def {
                let dx = card_rect.x + card_rect.w - 102;
                canvas.fill_rounded_rect(dx, card_rect.y + 26, 84, 34, 8, Color::rgb(18, 54, 44));
                canvas.stroke_rounded_rect(dx, card_rect.y + 26, 84, 34, 8, 1, ACCENT_GREEN);
                canvas.draw_text(FontKind::Mono, dx + 14, card_rect.y + 29, "DEF", ACCENT_GREEN);
            }

            cy += card_h + card_gap;
        }

        // Divider above pinned Settings button
        canvas.fill_rect(sb_pad, settings_btn_y - 16, sb_inner_w, 1, BORDER_SUBTLE);

        // Bottom-pinned Settings Button (matches wireframe!)
        let settings_active = self.sidebar_sel == SidebarSel::Settings;
        canvas.fill_rounded_rect(
            settings_rect.x,
            settings_rect.y,
            settings_rect.w,
            settings_rect.h,
            20,
            if settings_active {
                BG_CARD_SEL
            } else {
                BG_CARD
            },
        );
        canvas.stroke_rounded_rect(
            settings_rect.x,
            settings_rect.y,
            settings_rect.w,
            settings_rect.h,
            20,
            if settings_active { 3 } else { 1 },
            if settings_active {
                ACCENT_CYAN
            } else {
                BORDER_SUBTLE
            },
        );
        canvas.draw_os_icon(settings_rect.x + 28, settings_rect.y + 21, 56, OsKind::Tools);
        canvas.draw_text(
            FontKind::Ui,
            settings_rect.x + 106,
            settings_rect.y + 26,
            "Settings",
            if settings_active {
                ACCENT_CYAN
            } else {
                TEXT_PRIMARY
            },
        );
        let st_hint = if self.focus == FocusPane::SettingsPane {
            "Active"
        } else {
            "gaokunBoot"
        };
        let shw = canvas.measure_text(FontKind::Mono, st_hint);
        canvas.draw_text(
            FontKind::Mono,
            settings_rect.x + settings_rect.w - shw - 28,
            settings_rect.y + 35,
            st_hint,
            if settings_active {
                ACCENT_CYAN
            } else {
                TEXT_MUTED
            },
        );
        self.hit_boxes.push((settings_rect, ClickAction::OpenSettings));

        // =========================================================================
        // 3. RIGHT MAIN PANE: OS specific info & Settings  OR  GaokunBoot Settings
        // =========================================================================
        let right_x = sidebar_w + 2;
        let right_w = w - right_x;
        let pad = 52usize;
        let px = right_x + pad;
        let py = body_y + 40;
        let pw = right_w - pad * 2;
        let ph = body_h - 40 - 36;

        match self.sidebar_sel {
            SidebarSel::Entry(e_idx) if e_idx < entries.len() => {
                self.render_os_pane(canvas, px, py, pw, ph, e_idx, &entries[e_idx], cfg, el2_drivers);
            }
            _ => {
                self.render_settings_pane(canvas, px, py, pw, ph, entries, cfg);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn render_os_pane(
        &mut self,
        canvas: &mut Canvas,
        px: usize,
        py: usize,
        pw: usize,
        ph: usize,
        e_idx: usize,
        entry: &BootEntry,
        cfg: &BootConfig,
        el2_drivers: &El2Drivers,
    ) {
        let accent = Self::os_accent(entry.kind);
        let is_def = Self::is_entry_default(entry, e_idx, cfg);
        let active_payload = entry.active_payload(cfg.android_slot);

        // Hero Row
        canvas.draw_os_icon(px, py, 112, entry.kind);
        let hx = px + 144;
        canvas.draw_text_ellipsis(
            FontKind::Large,
            hx,
            py - 4,
            pw.saturating_sub(144),
            &entry.title,
            TEXT_PRIMARY,
        );

        let badge_y = py + 70;
        let mut bx = hx;
        bx = Self::draw_pill(canvas, bx, badge_y, entry.kind.label(), accent);
        bx = Self::draw_pill(
            canvas,
            bx + 12,
            badge_y,
            entry.mode_label(cfg.android_slot),
            TEXT_SECONDARY,
        );
        if entry.supports_ab_slot {
            bx = Self::draw_pill(canvas, bx + 12, badge_y, cfg.android_slot.label(), ACCENT_GREEN);
        }
        if is_def {
            let _ = Self::draw_pill(canvas, bx + 12, badge_y, "DEFAULT", ACCENT_CYAN);
        }

        let mut cy = py + 140;
        canvas.fill_rect(px, cy, pw, 2, BORDER_SUBTLE);
        cy += 28;

        // OS-Specific Settings Section (matches wireframe "OS specific info & Settings")
        canvas.draw_text(FontKind::Mono, px, cy, "OS SPECIFIC SETTINGS", TEXT_MUTED);
        cy += 38;

        if entry.supports_ab_slot {
            let row_h = 84usize;
            canvas.fill_rounded_rect(px, cy, pw, row_h, 18, BG_PANEL);
            canvas.stroke_rounded_rect(px, cy, pw, row_h, 18, 1, BORDER_SUBTLE);
            canvas.draw_text(FontKind::Ui, px + 28, cy + 19, "Android Slot  [S]", TEXT_PRIMARY);

            let seg_w = 240usize;
            let seg_h = 58usize;
            let seg_y = cy + 13;
            let r_b = Rect::new(px + pw - 20 - seg_w, seg_y, seg_w, seg_h);
            let r_a = Rect::new(r_b.x - 16 - seg_w, seg_y, seg_w, seg_h);
            Self::draw_seg_ui(
                canvas,
                r_a,
                "Slot A (_a)",
                cfg.android_slot == AndroidSlot::A,
                ACCENT_GREEN,
            );
            Self::draw_seg_ui(
                canvas,
                r_b,
                "Slot B (_b)",
                cfg.android_slot == AndroidSlot::B,
                ACCENT_GREEN,
            );
            self.hit_boxes
                .push((r_a, ClickAction::SetAndroidSlot(AndroidSlot::A)));
            self.hit_boxes
                .push((r_b, ClickAction::SetAndroidSlot(AndroidSlot::B)));
            cy += row_h + 16;
        }

        if entry.supports_el2 {
            let row_h = 84usize;
            canvas.fill_rounded_rect(px, cy, pw, row_h, 18, BG_PANEL);
            canvas.stroke_rounded_rect(px, cy, pw, row_h, 18, 1, BORDER_SUBTLE);
            canvas.draw_text(FontKind::Ui, px + 28, cy + 19, "Privilege Level  [E]", TEXT_PRIMARY);

            let drv_lbl = if el2_drivers.is_ready() {
                ("slbounce.efi Ready", ACCENT_GREEN)
            } else {
                ("Put slbounce.efi in \\EFI\\gaokunBoot\\drivers\\", ACCENT_AMBER)
            };
            let seg_w = 240usize;
            let seg_h = 58usize;
            let seg_y = cy + 13;
            let r_2 = Rect::new(px + pw - 20 - seg_w, seg_y, seg_w, seg_h);
            let r_1 = Rect::new(r_2.x - 16 - seg_w, seg_y, seg_w, seg_h);
            let drv_x = px + 390;
            if r_1.x > drv_x + 40 {
                canvas.draw_text_ellipsis(
                    FontKind::Mono,
                    drv_x,
                    cy + 28,
                    r_1.x.saturating_sub(drv_x + 18),
                    drv_lbl.0,
                    drv_lbl.1,
                );
            }
            Self::draw_seg_ui(canvas, r_1, "EL1 (QHEE)", !cfg.linux_el2, ACCENT_CYAN);
            Self::draw_seg_ui(canvas, r_2, "EL2 (KVM)", cfg.linux_el2, ACCENT_AMBER);
            self.hit_boxes.push((r_1, ClickAction::SetLinuxEl2(false)));
            self.hit_boxes.push((r_2, ClickAction::SetLinuxEl2(true)));
            cy += row_h + 16;
        }

        if !entry.supports_ab_slot && !entry.supports_el2 {
            let row_h = 74usize;
            canvas.fill_rounded_rect(px, cy, pw, row_h, 16, BG_PANEL);
            canvas.stroke_rounded_rect(px, cy, pw, row_h, 16, 1, BORDER_SUBTLE);
            let note = match entry.kind {
                OsKind::Windows => "Windows ARM64 UEFI Chainload  |  Preserves original HUAWEI / Qualcomm ACPI tables",
                OsKind::Tools => "UEFI Utility / Fastboot Tool  |  Direct EFI application launch",
                _ => "Standard UEFI Boot Target",
            };
            canvas.draw_text_ellipsis(FontKind::Mono, px + 26, cy + 23, pw - 52, note, TEXT_SECONDARY);
            cy += row_h + 16;
        }

        // OS Info Card
        cy += 8;
        canvas.draw_text(FontKind::Mono, px, cy, "BOOT TARGET DETAILS", TEXT_MUTED);
        cy += 38;

        let btn_h = 94usize;
        let btn_y = py + ph - btn_h;
        let toast_h = if self.toast.is_some() { 64 } else { 0 };
        let info_bottom = btn_y.saturating_sub(24 + toast_h);

        if info_bottom > cy + 120 {
            let card_h = info_bottom - cy;
            canvas.fill_rounded_rect(px, cy, pw, card_h, 22, BG_PANEL);
            canvas.stroke_rounded_rect(px, cy, pw, card_h, 22, 1, BORDER_SUBTLE);

            let mut iy = cy + 28;
            let ix = px + 32;
            let iw = pw - 64;

            iy = Self::draw_kv(
                canvas,
                ix,
                iy,
                iw,
                "Source",
                &format!("{}  ({})", entry.volume_label, entry.id),
            );
            if !entry.version.is_empty() {
                iy = Self::draw_kv(canvas, ix, iy, iw, "Version", &entry.version);
            }

            let options_str: &str;
            match active_payload {
                BootPayload::Efi { path, options } => {
                    iy = Self::draw_kv(canvas, ix, iy, iw, "EFI Image", path);
                    options_str = options;
                }
                BootPayload::Linux {
                    kernel,
                    initrd,
                    devicetree,
                    el2_devicetree,
                    options,
                } => {
                    iy = Self::draw_kv(canvas, ix, iy, iw, "Kernel", kernel);
                    let dtb_disp = if cfg.linux_el2 && el2_devicetree.is_some() {
                        el2_devicetree.as_deref()
                    } else {
                        devicetree.as_deref()
                    };
                    if let Some(dtb) = dtb_disp {
                        iy = Self::draw_kv(canvas, ix, iy, iw, "DeviceTree", dtb);
                    }
                    if let Some(rd) = initrd {
                        iy = Self::draw_kv(canvas, ix, iy, iw, "Initrd", rd);
                    }
                    options_str = options;
                }
            }

            // Boot options well
            iy += 10;
            let well_bottom = cy + card_h - 24;
            if well_bottom > iy + 64 {
                let wh = well_bottom - iy;
                canvas.fill_rounded_rect(ix, iy, iw, wh, 14, BG_CODE);
                canvas.stroke_rounded_rect(ix, iy, iw, wh, 14, 1, BORDER_SUBTLE);
                canvas.draw_text(FontKind::Mono, ix + 20, iy + 14, "BOOT OPTIONS / CMDLINE", TEXT_MUTED);
                let max_lines = (wh.saturating_sub(58)) / (FontKind::Mono.line_height() + 4);
                let disp = if options_str.is_empty() {
                    "<none - default EFI handoff>"
                } else {
                    options_str
                };
                canvas.draw_wrapped_text(
                    FontKind::Mono,
                    ix + 20,
                    iy + 46,
                    iw - 40,
                    max_lines.max(1),
                    disp,
                    if options_str.is_empty() {
                        TEXT_MUTED
                    } else {
                        TEXT_SECONDARY
                    },
                );
            }
        }

        // Toast Banner
        if let Some((ref msg, is_err)) = self.toast {
            let ty = btn_y - 60;
            Self::draw_toast(canvas, px, ty, pw, msg, is_err);
        }

        // Bottom Touch-Friendly Primary Action Buttons
        let boot_w = (pw - 24) * 62 / 100;
        let def_w = pw - 24 - boot_w;

        let boot_rect = Rect::new(px, btn_y, boot_w, btn_h);
        canvas.fill_rounded_rect(boot_rect.x, boot_rect.y, boot_rect.w, boot_rect.h, 20, accent);
        let blbl = format!("Boot {}   [Power / Enter]", entry.title);
        let blw = canvas.measure_text(FontKind::Ui, &blbl).min(boot_rect.w - 48);
        canvas.draw_text_ellipsis(
            FontKind::Ui,
            boot_rect.x + boot_rect.w.saturating_sub(blw) / 2,
            boot_rect.y + 24,
            boot_rect.w - 48,
            &blbl,
            BG_MAIN,
        );
        self.hit_boxes.push((boot_rect, ClickAction::BootSelected));

        let def_rect = Rect::new(px + boot_w + 24, btn_y, def_w, btn_h);
        canvas.fill_rounded_rect(def_rect.x, def_rect.y, def_rect.w, def_rect.h, 20, BG_CARD);
        canvas.stroke_rounded_rect(
            def_rect.x,
            def_rect.y,
            def_rect.w,
            def_rect.h,
            20,
            2,
            if is_def { ACCENT_GREEN } else { BORDER_ACCENT },
        );
        let dlbl = if is_def {
            "Default Entry  [D]"
        } else {
            "Set as Default  [D]"
        };
        let dlw = canvas.measure_text(FontKind::Ui, dlbl);
        canvas.draw_text(
            FontKind::Ui,
            def_rect.x + def_rect.w.saturating_sub(dlw) / 2,
            def_rect.y + 24,
            dlbl,
            if is_def { ACCENT_GREEN } else { TEXT_PRIMARY },
        );
        self.hit_boxes.push((def_rect, ClickAction::SetSelectedDefault));
    }

    #[allow(clippy::too_many_arguments)]
    fn render_settings_pane(
        &mut self,
        canvas: &mut Canvas,
        px: usize,
        py: usize,
        pw: usize,
        ph: usize,
        entries: &[BootEntry],
        cfg: &BootConfig,
    ) {
        let in_pane = self.focus == FocusPane::SettingsPane;
        let active_setting = self.current_setting();

        // Title
        canvas.draw_text(FontKind::Large, px, py - 4, "GaokunBoot Settings", TEXT_PRIMARY);
        canvas.draw_text(
            FontKind::Mono,
            px,
            py + 70,
            "All changes save automatically to UEFI NVRAM  |  Tap any option or use Vol+/- & Power",
            TEXT_SECONDARY,
        );

        let mut cy = py + 124;
        canvas.fill_rect(px, cy, pw, 2, BORDER_SUBTLE);
        cy += 30;

        // Row 1: Auto-Boot Timeout
        let row_h = 116usize;
        let r1_focus = in_pane && active_setting == SettingFocus::Timeout;
        Self::draw_setting_frame(canvas, Rect::new(px, cy, pw, row_h), r1_focus, ACCENT_CYAN);
        canvas.draw_text(FontKind::Ui, px + 32, cy + 18, "Auto-Boot Timeout  [T]", TEXT_PRIMARY);
        canvas.draw_text(
            FontKind::Mono,
            px + 32,
            cy + 68,
            &format!("Current: {}", cfg.timeout_label()),
            TEXT_SECONDARY,
        );
        let seg_w = 152usize;
        let seg_h = 62usize;
        let seg_gap = 14usize;
        let total_seg_w = TIMEOUT_CHOICES.len() * seg_w + (TIMEOUT_CHOICES.len() - 1) * seg_gap;
        let seg_x0 = px + pw - 28 - total_seg_w;
        for (i, &secs) in TIMEOUT_CHOICES.iter().enumerate() {
            let r = Rect::new(seg_x0 + i * (seg_w + seg_gap), cy + 27, seg_w, seg_h);
            let lbl = if secs == 0 {
                String::from("Off")
            } else {
                format!("{}s", secs)
            };
            Self::draw_seg_ui(canvas, r, &lbl, cfg.timeout_secs == secs, ACCENT_CYAN);
            self.hit_boxes.push((r, ClickAction::SetTimeout(secs)));
        }
        cy += row_h + 22;

        // Row 2: Default Boot Entry
        let r2_focus = in_pane && active_setting == SettingFocus::DefaultEntry;
        Self::draw_setting_frame(canvas, Rect::new(px, cy, pw, row_h), r2_focus, ACCENT_GREEN);
        canvas.draw_text(FontKind::Ui, px + 32, cy + 18, "Default Boot Entry", TEXT_PRIMARY);
        let def_lbl = self.default_label(entries, cfg);
        canvas.draw_text_ellipsis(
            FontKind::Mono,
            px + 32,
            cy + 68,
            pw.saturating_sub(360),
            &format!("Current: {}", def_lbl),
            ACCENT_GREEN,
        );
        let cyc_rect = Rect::new(px + pw - 28 - 280, cy + 27, 280, 62);
        Self::draw_seg_ui(canvas, cyc_rect, "Cycle Default", true, ACCENT_GREEN);
        self.hit_boxes.push((cyc_rect, ClickAction::CycleDefault));
        cy += row_h + 22;

        // Row 3: Touchpad / Mouse Pointer Speed (1..5)
        let r3_focus = in_pane && active_setting == SettingFocus::PtrSpeed;
        Self::draw_setting_frame(canvas, Rect::new(px, cy, pw, row_h), r3_focus, ACCENT_CYAN);
        canvas.draw_text(FontKind::Ui, px + 32, cy + 18, "Touchpad Pointer Speed  [V]", TEXT_PRIMARY);
        canvas.draw_text(
            FontKind::Mono,
            px + 32,
            cy + 68,
            cfg.ptr_speed_label(),
            TEXT_SECONDARY,
        );
        let sp_w = 112usize;
        let sp_gap = 12usize;
        let sp_total = 5 * sp_w + 4 * sp_gap;
        let sp_x0 = px + pw - 28 - sp_total;
        for level in 1..=5u8 {
            let r = Rect::new(
                sp_x0 + ((level - 1) as usize) * (sp_w + sp_gap),
                cy + 27,
                sp_w,
                62,
            );
            let lbl = format!("{}", level);
            Self::draw_seg_ui(canvas, r, &lbl, cfg.ptr_speed == level, ACCENT_CYAN);
            self.hit_boxes.push((r, ClickAction::SetPtrSpeed(level)));
        }
        cy += row_h + 36;

        // Section: Firmware & Power Actions (3 big touch cards side by side!)
        canvas.draw_text(FontKind::Mono, px, cy, "FIRMWARE & POWER", TEXT_MUTED);
        cy += 38;

        let act_gap = 22usize;
        let act_w = (pw - act_gap * 2) / 3;
        let act_h = 136usize;

        let actions = [
            (
                SettingFocus::BiosSetup,
                ClickAction::BiosSetup,
                ACCENT_PURPLE,
                "UEFI BIOS Setup",
                "Enter Huawei BIOS [F2]",
            ),
            (
                SettingFocus::Reboot,
                ClickAction::Reboot,
                ACCENT_AMBER,
                "Reboot Device",
                "UEFI Cold Reset [R]",
            ),
            (
                SettingFocus::Shutdown,
                ClickAction::Shutdown,
                ACCENT_RED,
                "Power Off",
                "ACPI Shutdown [P]",
            ),
        ];

        for (i, (sf, ca, acc, title, sub)) in actions.iter().enumerate() {
            let r = Rect::new(px + i * (act_w + act_gap), cy, act_w, act_h);
            let focused = in_pane && active_setting == *sf;
            canvas.fill_rounded_rect(r.x, r.y, r.w, r.h, 20, if focused { BG_CARD_SEL } else { BG_PANEL });
            canvas.stroke_rounded_rect(r.x, r.y, r.w, r.h, 20, if focused { 3 } else { 2 }, *acc);
            canvas.draw_text(FontKind::Ui, r.x + 28, r.y + 26, title, *acc);
            canvas.draw_text(FontKind::Mono, r.x + 28, r.y + 82, sub, TEXT_SECONDARY);
            self.hit_boxes.push((r, *ca));
        }

        // Bottom Back button + Toast
        let back_h = 84usize;
        let back_y = py + ph - back_h;

        if let Some((ref msg, is_err)) = self.toast {
            Self::draw_toast(canvas, px, back_y - 62, pw, msg, is_err);
        }

        let back_focus = in_pane && active_setting == SettingFocus::BackToMenu;
        let back_rect = Rect::new(px, back_y, 440, back_h);
        canvas.fill_rounded_rect(
            back_rect.x,
            back_rect.y,
            back_rect.w,
            back_rect.h,
            18,
            if back_focus { BG_CARD_SEL } else { BG_CARD },
        );
        canvas.stroke_rounded_rect(
            back_rect.x,
            back_rect.y,
            back_rect.w,
            back_rect.h,
            18,
            if back_focus { 3 } else { 1 },
            if back_focus { ACCENT_CYAN } else { BORDER_SUBTLE },
        );
        canvas.draw_text(
            FontKind::Ui,
            back_rect.x + 36,
            back_rect.y + 19,
            "<  Back to Boot Menu",
            if back_focus { ACCENT_CYAN } else { TEXT_PRIMARY },
        );
        self.hit_boxes.push((back_rect, ClickAction::BackToMenu));

        // Helpful legend at bottom right
        let hint = "Vol+/-: Move   Power/Enter: Apply   ESC: Back";
        let hw = canvas.measure_text(FontKind::Mono, hint);
        canvas.draw_text(FontKind::Mono, px + pw - hw, back_y + 28, hint, TEXT_MUTED);
    }

    pub fn render_launching(&self, canvas: &mut Canvas, title: &str, detail: &str) {
        canvas.clear(BG_MAIN);
        let box_w = 1040usize.min(canvas.w - 120);
        let box_h = 280usize;
        let bx = (canvas.w - box_w) / 2;
        let by = (canvas.h - box_h) / 2;
        canvas.fill_rounded_rect(bx, by, box_w, box_h, 28, BG_PANEL);
        canvas.stroke_rounded_rect(bx, by, box_w, box_h, 28, 3, ACCENT_CYAN);

        let heading = format!("Launching {}...", title);
        let hw = canvas.measure_text(FontKind::Large, &heading);
        canvas.draw_text(
            FontKind::Large,
            bx + box_w.saturating_sub(hw) / 2,
            by + 64,
            &heading,
            TEXT_PRIMARY,
        );
        let dw = canvas.measure_text(FontKind::Mono, detail);
        canvas.draw_text_ellipsis(
            FontKind::Mono,
            bx + box_w.saturating_sub(dw.min(box_w - 80)) / 2,
            by + 166,
            box_w - 80,
            detail,
            TEXT_SECONDARY,
        );
        canvas.present_full();
    }

    fn draw_setting_frame(canvas: &mut Canvas, rect: Rect, focused: bool, accent: Color) {
        canvas.fill_rounded_rect(
            rect.x,
            rect.y,
            rect.w,
            rect.h,
            20,
            if focused { BG_CARD_SEL } else { BG_PANEL },
        );
        canvas.stroke_rounded_rect(
            rect.x,
            rect.y,
            rect.w,
            rect.h,
            20,
            if focused { 3 } else { 1 },
            if focused { accent } else { BORDER_SUBTLE },
        );
    }

    fn draw_seg_ui(canvas: &mut Canvas, rect: Rect, label: &str, active: bool, accent: Color) {
        if active {
            canvas.fill_rounded_rect(rect.x, rect.y, rect.w, rect.h, 14, accent);
        } else {
            canvas.fill_rounded_rect(rect.x, rect.y, rect.w, rect.h, 14, BG_CARD);
            canvas.stroke_rounded_rect(rect.x, rect.y, rect.w, rect.h, 14, 1, BORDER_SUBTLE);
        }
        let tw = canvas.measure_text(FontKind::Ui, label);
        let y_off = rect.h.saturating_sub(FontKind::Ui.line_height()) / 2;
        canvas.draw_text(
            FontKind::Ui,
            rect.x + rect.w.saturating_sub(tw) / 2,
            rect.y + y_off,
            label,
            if active { BG_MAIN } else { TEXT_PRIMARY },
        );
    }

    fn draw_toast(canvas: &mut Canvas, x: usize, y: usize, w: usize, msg: &str, is_err: bool) {
        let (bg, fg) = if is_err {
            (Color::rgb(64, 20, 26), ACCENT_RED)
        } else {
            (Color::rgb(16, 52, 40), ACCENT_GREEN)
        };
        canvas.fill_rounded_rect(x, y, w, 48, 12, bg);
        canvas.stroke_rounded_rect(x, y, w, 48, 12, 1, fg);
        canvas.draw_text_ellipsis(FontKind::Mono, x + 20, y + 10, w - 40, msg, fg);
    }

    fn draw_pill(canvas: &mut Canvas, x: usize, y: usize, label: &str, color: Color) -> usize {
        let tw = canvas.measure_text(FontKind::Mono, label);
        let w = tw + 28;
        canvas.fill_rounded_rect(x, y, w, 38, 10, BG_CODE);
        canvas.stroke_rounded_rect(x, y, w, 38, 10, 1, color);
        canvas.draw_text(FontKind::Mono, x + 14, y + 5, label, color);
        x + w
    }

    fn draw_kv(canvas: &mut Canvas, x: usize, y: usize, max_w: usize, key: &str, val: &str) -> usize {
        let key_w = 170usize;
        canvas.draw_text(FontKind::Mono, x, y, key, TEXT_MUTED);
        canvas.draw_text_ellipsis(
            FontKind::Mono,
            x + key_w,
            y,
            max_w.saturating_sub(key_w),
            val,
            TEXT_PRIMARY,
        );
        y + FontKind::Mono.line_height() + 12
    }
}
