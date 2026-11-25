#![no_main]
#![no_std]

extern crate alloc;

mod config;
mod discover;
mod efi_extra;
mod font_data;
mod gfx;
mod input;
mod launcher;
mod ui;

use alloc::format;
use alloc::string::String;

use uefi::boot;
use uefi::prelude::*;
use uefi::system;

use config::BootConfig;
use discover::{discover_entries, BootEntry, El2Drivers, OsKind};
use gfx::Canvas;
use input::{InputEvent, InputManager};
use ui::{ClickAction, FocusPane, SettingFocus, SidebarSel, UiState};

fn resolve_initial_entry(entries: &[BootEntry], cfg: &BootConfig) -> usize {
    if entries.is_empty() {
        return 0;
    }
    if cfg.default_id == "@last" && !cfg.last_booted_id.is_empty() {
        if let Some(pos) = entries.iter().position(|e| e.id == cfg.last_booted_id) {
            return pos;
        }
    }
    if !cfg.default_id.is_empty() && cfg.default_id != "@last" {
        if let Some(pos) = entries.iter().position(|e| e.id == cfg.default_id) {
            return pos;
        }
    }
    0
}

fn cycle_default_entry(entries: &[BootEntry], cfg: &mut BootConfig) {
    if entries.is_empty() {
        return;
    }
    if cfg.default_id == "@last" {
        cfg.default_id = entries[0].id.clone();
        return;
    }
    if let Some(pos) = entries.iter().position(|e| e.id == cfg.default_id) {
        if pos + 1 < entries.len() {
            cfg.default_id = entries[pos + 1].id.clone();
        } else {
            cfg.default_id = String::from("@last");
        }
    } else if entries.len() > 1 {
        cfg.default_id = entries[1].id.clone();
    } else {
        cfg.default_id = String::from("@last");
    }
}

fn try_boot_entry(
    entry: &BootEntry,
    cfg: &mut BootConfig,
    el2_drivers: &El2Drivers,
    ui: &mut UiState,
    canvas: &mut Canvas,
) {
    if cfg.last_booted_id != entry.id {
        cfg.last_booted_id = entry.id.clone();
        let _ = cfg.save();
    }

    let mut detail = format!("{}  |  {}", entry.mode_label(cfg), entry.volume_label);
    if entry.active_supports_ab(cfg) {
        detail = format!("{}  |  {}", detail, cfg.android_slot.label());
    }
    if entry.active_supports_el2(cfg) && cfg.linux_el2 {
        detail = format!("{}  |  EL2 (slbounce)", detail);
    }

    ui.render_launching(canvas, &entry.title, &detail);

    system::with_stdout(|stdout| {
        let _ = stdout.enable_cursor(false);
    });

    match launcher::launch_entry(entry, cfg, el2_drivers) {
        Ok(()) => {
            ui.toast = Some((format!("Returned cleanly from {}", entry.title), false));
        }
        Err(err) => {
            ui.toast = Some((format!("Boot failed: {}", err), true));
        }
    }
}

fn save_cfg_with_toast(cfg: &BootConfig, label: &str, ui: &mut UiState) {
    match cfg.save() {
        Ok(()) => {
            ui.toast = Some((format!("Saved {} to UEFI NVRAM", label), false));
        }
        Err(e) => {
            ui.toast = Some((format!("Updated {} (not persisted: {})", label, e), true));
        }
    }
}

#[entry]
fn main() -> Status {
    system::with_stdout(|stdout| {
        let _ = stdout.enable_cursor(false);
    });

    let Some(mut canvas) = Canvas::init() else {
        return Status::UNSUPPORTED;
    };

    let disc = discover_entries();
    let grouped_entries = disc.grouped_entries;
    let raw_entries = disc.raw_entries;
    let el2_drivers = disc.el2_drivers;

    let mut cfg = BootConfig::load();
    if let Some(text) = disc.esp_config_text {
        if cfg.default_id.is_empty() && cfg.last_booted_id.is_empty() {
            cfg.apply_text(&text);
        }
    }

    let active_entries = |grouped: bool| -> &[BootEntry] {
        if grouped {
            &grouped_entries
        } else {
            &raw_entries
        }
    };

    let initial_idx = resolve_initial_entry(active_entries(cfg.group_entries), &cfg);
    let current_el = efi_extra::read_current_el();
    let (device_name, bios_ver) = efi_extra::read_device_summary();
    let mut input = InputManager::init(canvas.w, canvas.h);
    let mut ui = UiState::new(
        active_entries(cfg.group_entries).len(),
        initial_idx,
        device_name,
        bios_ver,
        current_el,
        input.touch_ready,
        cfg.timeout_secs,
    );

    if active_entries(cfg.group_entries).is_empty() {
        ui.toast = Some((
            String::from("No Windows or BLS entries found on attached ESPs"),
            true,
        ));
    }

    ui.render(&mut canvas, active_entries(cfg.group_entries), &cfg, &el2_drivers);
    canvas.present_full();

    // 5ms main loop tick: keeps Huawei 40ms/25Hz I2cTouchPanel responsive without dropping fast taps!
    let tick_ms: u64 = 5;
    let mut now_ms: u64 = 0;
    let mut bar_accum_ms: u64 = 0;
    let mut last_card_select_ms: u64 = 0;

    loop {
        let events = input.poll(cfg.ptr_speed_num(), now_ms);
        let mut needs_redraw = false;
        let mut cursor_moved = false;

        for ev in events {
            let entries = active_entries(cfg.group_entries);
            match ev {
                InputEvent::PointerMove => {
                    cursor_moved = true;
                }
                _ => {
                    if ui.countdown_ms.is_some() {
                        ui.cancel_countdown();
                        ui.render_countdown_only(&mut canvas, &cfg);
                    }
                }
            }

            match ev {
                InputEvent::MoveUp => {
                    ui.move_up();
                    needs_redraw = true;
                }
                InputEvent::MoveDown => {
                    ui.move_down();
                    needs_redraw = true;
                }
                InputEvent::MoveLeft => {
                    if ui.focus == FocusPane::SettingsPane {
                        ui.focus = FocusPane::Sidebar;
                        if !entries.is_empty() {
                            ui.sidebar_sel =
                                SidebarSel::Entry(ui.last_entry_idx.min(entries.len() - 1));
                        }
                        needs_redraw = true;
                    }
                }
                InputEvent::MoveRight => {
                    if ui.sidebar_sel == SidebarSel::Settings {
                        ui.focus = FocusPane::SettingsPane;
                        needs_redraw = true;
                    }
                }
                InputEvent::Cancel => {
                    if ui.focus == FocusPane::SettingsPane {
                        ui.focus = FocusPane::Sidebar;
                        if !entries.is_empty() {
                            ui.sidebar_sel =
                                SidebarSel::Entry(ui.last_entry_idx.min(entries.len() - 1));
                        }
                    } else {
                        ui.cancel_countdown();
                        ui.toast = Some((String::from("Auto-boot cancelled"), false));
                    }
                    needs_redraw = true;
                }
                InputEvent::CycleTimeout => {
                    cfg.cycle_timeout();
                    let lbl = format!("timeout = {}", cfg.timeout_label());
                    save_cfg_with_toast(&cfg, &lbl, &mut ui);
                    needs_redraw = true;
                }
                InputEvent::ToggleBootMethod => {
                    if let SidebarSel::Entry(idx) = ui.sidebar_sel {
                        if idx < entries.len() && entries[idx].has_both_methods() {
                            let kind = entries[idx].kind;
                            if kind == OsKind::Android {
                                cfg.android_method = cfg.android_method.toggle();
                            } else {
                                cfg.linux_method = cfg.linux_method.toggle();
                            }
                            let lbl = format!(
                                "boot method = {}",
                                entries[idx].active_variant(&cfg).label
                            );
                            save_cfg_with_toast(&cfg, &lbl, &mut ui);
                            needs_redraw = true;
                        }
                    }
                }
                InputEvent::ToggleAndroidSlot => {
                    cfg.android_slot = cfg.android_slot.toggle();
                    let lbl = format!("Android {}", cfg.android_slot.label());
                    save_cfg_with_toast(&cfg, &lbl, &mut ui);
                    needs_redraw = true;
                }
                InputEvent::ToggleLinuxEl2 => {
                    cfg.linux_el2 = !cfg.linux_el2;
                    let lbl = if cfg.linux_el2 {
                        "Kernel mode = EL2 (KVM)"
                    } else {
                        "Kernel mode = EL1 (QHEE)"
                    };
                    save_cfg_with_toast(&cfg, lbl, &mut ui);
                    needs_redraw = true;
                }
                InputEvent::CyclePtrSpeed => {
                    cfg.cycle_ptr_speed();
                    let lbl = format!("pointer speed = {}", cfg.ptr_speed_label());
                    save_cfg_with_toast(&cfg, &lbl, &mut ui);
                    needs_redraw = true;
                }
                InputEvent::SetDefault => {
                    if let SidebarSel::Entry(idx) = ui.sidebar_sel {
                        if idx < entries.len() {
                            cfg.default_id = entries[idx].id.clone();
                            let lbl = format!("default = {}", entries[idx].title);
                            save_cfg_with_toast(&cfg, &lbl, &mut ui);
                            needs_redraw = true;
                        }
                    }
                }
                InputEvent::BiosSetup => {
                    ui.render_launching(&mut canvas, "UEFI Firmware Setup", "Setting OsIndications...");
                    if let Err(e) = launcher::reboot_to_firmware_setup() {
                        ui.toast = Some((e, true));
                        needs_redraw = true;
                    }
                }
                InputEvent::Reboot => {
                    ui.render_launching(&mut canvas, "Reboot", "Performing UEFI cold reset...");
                    launcher::reboot();
                }
                InputEvent::PowerOff => {
                    ui.render_launching(&mut canvas, "Shut Down", "Powering off...");
                    launcher::shutdown();
                }
                InputEvent::QuickIndex(idx) => {
                    if idx < entries.len() {
                        ui.sidebar_sel = SidebarSel::Entry(idx);
                        ui.last_entry_idx = idx;
                        ui.focus = FocusPane::Sidebar;
                        try_boot_entry(&entries[idx], &mut cfg, &el2_drivers, &mut ui, &mut canvas);
                        needs_redraw = true;
                    }
                }
                InputEvent::Confirm => match ui.focus {
                    FocusPane::Sidebar => match ui.sidebar_sel {
                        SidebarSel::Entry(idx) if idx < entries.len() => {
                            try_boot_entry(
                                &entries[idx],
                                &mut cfg,
                                &el2_drivers,
                                &mut ui,
                                &mut canvas,
                            );
                            needs_redraw = true;
                        }
                        SidebarSel::Settings => {
                            ui.focus = FocusPane::SettingsPane;
                            ui.setting_idx = 0;
                            needs_redraw = true;
                        }
                        _ => {}
                    },
                    FocusPane::SettingsPane => match ui.current_setting() {
                        SettingFocus::Timeout => {
                            cfg.cycle_timeout();
                            let lbl = format!("timeout = {}", cfg.timeout_label());
                            save_cfg_with_toast(&cfg, &lbl, &mut ui);
                            needs_redraw = true;
                        }
                        SettingFocus::DefaultEntry => {
                            cycle_default_entry(entries, &mut cfg);
                            save_cfg_with_toast(&cfg, "default boot entry", &mut ui);
                            needs_redraw = true;
                        }
                        SettingFocus::EntryGrouping => {
                            cfg.group_entries = !cfg.group_entries;
                            ui.entry_count = active_entries(cfg.group_entries).len();
                            ui.scroll_top = 0;
                            let lbl = if cfg.group_entries {
                                "menu = Grouped (Clean)"
                            } else {
                                "menu = Raw (All Entries)"
                            };
                            save_cfg_with_toast(&cfg, lbl, &mut ui);
                            needs_redraw = true;
                        }
                        SettingFocus::PtrSpeed => {
                            cfg.cycle_ptr_speed();
                            let lbl = format!("pointer speed = {}", cfg.ptr_speed_label());
                            save_cfg_with_toast(&cfg, &lbl, &mut ui);
                            needs_redraw = true;
                        }
                        SettingFocus::BiosSetup => {
                            ui.render_launching(
                                &mut canvas,
                                "UEFI Firmware Setup",
                                "Setting OsIndications...",
                            );
                            if let Err(e) = launcher::reboot_to_firmware_setup() {
                                ui.toast = Some((e, true));
                                needs_redraw = true;
                            }
                        }
                        SettingFocus::Reboot => {
                            ui.render_launching(&mut canvas, "Reboot", "Performing UEFI cold reset...");
                            launcher::reboot();
                        }
                        SettingFocus::Shutdown => {
                            ui.render_launching(&mut canvas, "Shut Down", "Powering off...");
                            launcher::shutdown();
                        }
                        SettingFocus::BackToMenu => {
                            ui.focus = FocusPane::Sidebar;
                            let cur_entries = active_entries(cfg.group_entries);
                            if !cur_entries.is_empty() {
                                ui.sidebar_sel =
                                    SidebarSel::Entry(ui.last_entry_idx.min(cur_entries.len() - 1));
                            }
                            needs_redraw = true;
                        }
                    },
                },
                InputEvent::PointerMove => {}
                InputEvent::PointerClick { x, y, from_touch } => {
                    if let Some(action) = ui.hit_test(x, y) {
                        match action {
                            ClickAction::SelectEntry(idx) => {
                                let already_sel = ui.sidebar_sel == SidebarSel::Entry(idx);
                                ui.focus = FocusPane::Sidebar;
                                ui.sidebar_sel = SidebarSel::Entry(idx);
                                ui.last_entry_idx = idx;
                                if already_sel
                                    && (!from_touch
                                        || now_ms.saturating_sub(last_card_select_ms) >= 400)
                                    && idx < entries.len()
                                {
                                    try_boot_entry(
                                        &entries[idx],
                                        &mut cfg,
                                        &el2_drivers,
                                        &mut ui,
                                        &mut canvas,
                                    );
                                } else {
                                    last_card_select_ms = now_ms;
                                }
                                needs_redraw = true;
                            }
                            ClickAction::OpenSettings => {
                                ui.sidebar_sel = SidebarSel::Settings;
                                ui.focus = FocusPane::SettingsPane;
                                needs_redraw = true;
                            }
                            ClickAction::BackToMenu => {
                                ui.focus = FocusPane::Sidebar;
                                let cur_entries = active_entries(cfg.group_entries);
                                if !cur_entries.is_empty() {
                                    ui.sidebar_sel = SidebarSel::Entry(
                                        ui.last_entry_idx.min(cur_entries.len() - 1),
                                    );
                                }
                                needs_redraw = true;
                            }
                            ClickAction::BootSelected => {
                                if let SidebarSel::Entry(idx) = ui.sidebar_sel {
                                    if idx < entries.len() {
                                        try_boot_entry(
                                            &entries[idx],
                                            &mut cfg,
                                            &el2_drivers,
                                            &mut ui,
                                            &mut canvas,
                                        );
                                        needs_redraw = true;
                                    }
                                }
                            }
                            ClickAction::SetSelectedDefault => {
                                if let SidebarSel::Entry(idx) = ui.sidebar_sel {
                                    if idx < entries.len() {
                                        cfg.default_id = entries[idx].id.clone();
                                        let lbl = format!("default = {}", entries[idx].title);
                                        save_cfg_with_toast(&cfg, &lbl, &mut ui);
                                        needs_redraw = true;
                                    }
                                }
                            }
                            ClickAction::SetBootMethod(method) => {
                                if let SidebarSel::Entry(idx) = ui.sidebar_sel {
                                    if idx < entries.len() {
                                        if entries[idx].kind == OsKind::Android {
                                            cfg.android_method = method;
                                        } else {
                                            cfg.linux_method = method;
                                        }
                                        let lbl = format!(
                                            "boot method = {}",
                                            entries[idx].active_variant(&cfg).label
                                        );
                                        save_cfg_with_toast(&cfg, &lbl, &mut ui);
                                        needs_redraw = true;
                                    }
                                }
                            }
                            ClickAction::SetAndroidSlot(slot) => {
                                cfg.android_slot = slot;
                                let lbl = format!("Android {}", cfg.android_slot.label());
                                save_cfg_with_toast(&cfg, &lbl, &mut ui);
                                needs_redraw = true;
                            }
                            ClickAction::SetLinuxEl2(enabled) => {
                                cfg.linux_el2 = enabled;
                                let lbl = if cfg.linux_el2 {
                                    "Kernel mode = EL2 (KVM)"
                                } else {
                                    "Kernel mode = EL1 (QHEE)"
                                };
                                save_cfg_with_toast(&cfg, lbl, &mut ui);
                                needs_redraw = true;
                            }
                            ClickAction::SetTimeout(secs) => {
                                cfg.timeout_secs = secs;
                                let lbl = format!("timeout = {}", cfg.timeout_label());
                                save_cfg_with_toast(&cfg, &lbl, &mut ui);
                                needs_redraw = true;
                            }
                            ClickAction::CycleDefault => {
                                cycle_default_entry(entries, &mut cfg);
                                save_cfg_with_toast(&cfg, "default boot entry", &mut ui);
                                needs_redraw = true;
                            }
                            ClickAction::SetGrouping(grouped) => {
                                cfg.group_entries = grouped;
                                ui.entry_count = active_entries(cfg.group_entries).len();
                                ui.scroll_top = 0;
                                let lbl = if cfg.group_entries {
                                    "menu = Grouped (Clean)"
                                } else {
                                    "menu = Raw (All Entries)"
                                };
                                save_cfg_with_toast(&cfg, lbl, &mut ui);
                                needs_redraw = true;
                            }
                            ClickAction::SetPtrSpeed(level) => {
                                cfg.ptr_speed = level.clamp(1, 5);
                                let lbl = format!("pointer speed = {}", cfg.ptr_speed_label());
                                save_cfg_with_toast(&cfg, &lbl, &mut ui);
                                needs_redraw = true;
                            }
                            ClickAction::BiosSetup => {
                                ui.render_launching(
                                    &mut canvas,
                                    "UEFI Firmware Setup",
                                    "Setting OsIndications...",
                                );
                                if let Err(e) = launcher::reboot_to_firmware_setup() {
                                    ui.toast = Some((e, true));
                                    needs_redraw = true;
                                }
                            }
                            ClickAction::Reboot => {
                                ui.render_launching(
                                    &mut canvas,
                                    "Reboot",
                                    "Performing UEFI cold reset...",
                                );
                                launcher::reboot();
                            }
                            ClickAction::Shutdown => {
                                ui.render_launching(&mut canvas, "Shut Down", "Powering off...");
                                launcher::shutdown();
                            }
                        }
                    }
                }
            }
        }

        // Countdown tick: ONLY partial-rect updates (<1ms) so 40ms touch timer is never starved!
        if let Some(rem) = ui.countdown_ms {
            let prev_sec = rem.div_ceil(1000);
            if rem <= tick_ms {
                ui.countdown_ms = None;
                let entries = active_entries(cfg.group_entries);
                if let SidebarSel::Entry(idx) = ui.sidebar_sel {
                    if idx < entries.len() {
                        try_boot_entry(&entries[idx], &mut cfg, &el2_drivers, &mut ui, &mut canvas);
                        needs_redraw = true;
                    }
                }
            } else {
                let next_rem = rem - tick_ms;
                ui.countdown_ms = Some(next_rem);
                bar_accum_ms += tick_ms;
                if !needs_redraw && (next_rem.div_ceil(1000) != prev_sec || bar_accum_ms >= 80) {
                    ui.render_countdown_only(&mut canvas, &cfg);
                    bar_accum_ms = 0;
                }
            }
        }

        if needs_redraw {
            ui.render(
                &mut canvas,
                active_entries(cfg.group_entries),
                &cfg,
                &el2_drivers,
            );
            canvas.present_full();
            if input.cursor_visible {
                canvas.update_cursor(Some((input.cursor_x, input.cursor_y)));
            }
        } else if cursor_moved && input.cursor_visible {
            canvas.update_cursor(Some((input.cursor_x, input.cursor_y)));
        }

        boot::stall((tick_ms * 1000) as usize);
        now_ms = now_ms.saturating_add(tick_ms);
    }
}
