use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;

use uefi::boot::{self, OpenProtocolAttributes, OpenProtocolParams, SearchType};
use uefi::proto::console::gop::{BltOp, BltPixel, GraphicsOutput};
use uefi::proto::console::pointer::Pointer;
use uefi::proto::console::text::{Input, Key, ScanCode};
use uefi::proto::device_path::DevicePath;
use uefi::{Handle, Identify};

use crate::efi_extra::{check_raw_event, AbsolutePointer, SimpleTextInputEx};

fn has_device_path(handle: Handle) -> bool {
    unsafe {
        boot::open_protocol::<DevicePath>(
            OpenProtocolParams {
                handle,
                agent: boot::image_handle(),
                controller: None,
            },
            OpenProtocolAttributes::GetProtocol,
        )
        .is_ok()
    }
}

/// Sort handles so concrete hardware handles (with DevicePath) are polled
/// BEFORE ConSplitter aggregate handles (without DevicePath).
fn sort_hardware_first(mut handles: Vec<Handle>) -> Vec<(usize, Handle, bool)> {
    let mut tagged: Vec<(usize, Handle, bool)> = handles
        .drain(..)
        .enumerate()
        .map(|(orig_idx, h)| (orig_idx, h, has_device_path(h)))
        .collect();
    tagged.sort_by_key(|&(_, _, has_dp)| !has_dp);
    tagged
}

fn blt_rect(x: usize, y: usize, w: usize, h: usize, color: BltPixel) -> bool {
    let Ok(handle) = boot::get_handle_for_protocol::<GraphicsOutput>() else {
        return false;
    };
    let Ok(mut gop) = (unsafe {
        boot::open_protocol::<GraphicsOutput>(
            OpenProtocolParams {
                handle,
                agent: boot::image_handle(),
                controller: None,
            },
            OpenProtocolAttributes::GetProtocol,
        )
    }) else {
        return false;
    };

    let (scr_w, scr_h) = gop.current_mode_info().resolution();
    if x >= scr_w || y >= scr_h {
        return false;
    }
    let rw = w.min(scr_w - x);
    let rh = h.min(scr_h - y);
    if rw == 0 || rh == 0 {
        return false;
    }

    gop.blt(BltOp::VideoFill {
        color,
        dest: (x, y),
        dims: (rw, rh),
    })
    .is_ok()
}

/// Draw 4 colored corner squares using GOP BltOp::VideoFill (safe even when FrameBufferBase == 0x0!).
///   RED    = (0, 0)       Top-Left
///   GREEN  = (W-sz, 0)    Top-Right
///   BLUE   = (0, H-sz)    Bottom-Left
///   YELLOW = (W-sz, H-sz) Bottom-Right
pub fn draw_orientation_markers(log: &mut String) -> (usize, usize) {
    let Ok(handle) = boot::get_handle_for_protocol::<GraphicsOutput>() else {
        let _ = writeln!(log, "  [GOP Marker] No GOP handle found");
        return (0, 0);
    };
    let Ok(mut gop) = (unsafe {
        boot::open_protocol::<GraphicsOutput>(
            OpenProtocolParams {
                handle,
                agent: boot::image_handle(),
                controller: None,
            },
            OpenProtocolAttributes::GetProtocol,
        )
    }) else {
        let _ = writeln!(log, "  [GOP Marker] Failed to open GOP");
        return (0, 0);
    };

    let (w, h) = gop.current_mode_info().resolution();
    let sz = (w.min(h) / 16).clamp(48, 120);

    let r1 = gop.blt(BltOp::VideoFill {
        color: BltPixel::new(255, 40, 40),
        dest: (0, 0),
        dims: (sz, sz),
    });
    let r2 = gop.blt(BltOp::VideoFill {
        color: BltPixel::new(40, 230, 60),
        dest: (w.saturating_sub(sz), 0),
        dims: (sz, sz),
    });
    let r3 = gop.blt(BltOp::VideoFill {
        color: BltPixel::new(40, 120, 255),
        dest: (0, h.saturating_sub(sz)),
        dims: (sz, sz),
    });
    let r4 = gop.blt(BltOp::VideoFill {
        color: BltPixel::new(250, 220, 40),
        dest: (w.saturating_sub(sz), h.saturating_sub(sz)),
        dims: (sz, sz),
    });

    let _ = writeln!(
        log,
        "  [GOP Marker via Blt::VideoFill] {}x{} squares on {}x{}: TL={:?} TR={:?} BL={:?} BR={:?}",
        sz,
        sz,
        w,
        h,
        r1.map(|_| ()),
        r2.map(|_| ()),
        r3.map(|_| ()),
        r4.map(|_| ()),
    );
    (w, h)
}

/// Interactive input capture across ALL UEFI input handles (hardware handles first, then ConSplitter).
/// Runs until 15s idle (max 45s total), or pressing ESC twice.
pub fn run_live_input_capture(report: &mut String, screen_print: &mut dyn FnMut(&str)) {
    let _ = writeln!(report, "\n=== 6. LIVE INPUT CAPTURE ===");
    screen_print(
        "\r\n[LIVE INPUT TEST - 15s idle timeout, max 45s, press ESC twice to finish]\r\n\
         Try now: Volume+ / Volume- / Power / Touchscreen taps / Magnetic Keyboard / Touchpad\r\n\
         Corner markers (Blt): RED=(0,0)  GREEN=(W,0)  BLUE=(0,H)  YELLOW=(W,H)\r\n\
         Touch taps will also draw a CYAN dot mapped to screen coords.\r\n",
    );

    let (scr_w, scr_h) = draw_orientation_markers(report);

    let text_handles = sort_hardware_first(
        boot::locate_handle_buffer(SearchType::ByProtocol(&Input::GUID))
            .map(|b| b.to_vec())
            .unwrap_or_default(),
    );
    let text_ex_handles = sort_hardware_first(
        boot::locate_handle_buffer(SearchType::ByProtocol(&SimpleTextInputEx::GUID))
            .map(|b| b.to_vec())
            .unwrap_or_default(),
    );
    let ptr_handles = sort_hardware_first(
        boot::locate_handle_buffer(SearchType::ByProtocol(&Pointer::GUID))
            .map(|b| b.to_vec())
            .unwrap_or_default(),
    );
    let abs_handles = sort_hardware_first(
        boot::locate_handle_buffer(SearchType::ByProtocol(&AbsolutePointer::GUID))
            .map(|b| b.to_vec())
            .unwrap_or_default(),
    );

    // Cache AbsolutePointer axis ranges for mapping touch dots onto screen
    let abs_ranges: Vec<(u64, u64, u64, u64)> = abs_handles
        .iter()
        .map(|&(_, handle, _)| {
            if let Ok(ap) = unsafe {
                boot::open_protocol::<AbsolutePointer>(
                    OpenProtocolParams {
                        handle,
                        agent: boot::image_handle(),
                        controller: None,
                    },
                    OpenProtocolAttributes::GetProtocol,
                )
            } {
                if let Some(m) = ap.mode() {
                    return (
                        m.absolute_min_x,
                        m.absolute_max_x,
                        m.absolute_min_y,
                        m.absolute_max_y,
                    );
                }
            }
            (0, 0, 0, 0)
        })
        .collect();

    let mut elapsed_ms: u64 = 0;
    let mut idle_ms: u64 = 0;
    let mut esc_count: u32 = 0;
    let mut event_count: u32 = 0;
    let max_events: u32 = 400;

    // Reset every AbsolutePointer handle once before capture (some touch DXEs wake up only after Reset!)
    for &(orig_i, handle, has_dp) in &abs_handles {
        let tag = if has_dp { "HW" } else { "SPL" };
        if let Ok(mut ap) = unsafe {
            boot::open_protocol::<AbsolutePointer>(
                OpenProtocolParams {
                    handle,
                    agent: boot::image_handle(),
                    controller: None,
                },
                OpenProtocolAttributes::GetProtocol,
            )
        } {
            let rst = ap.reset_device(false);
            let ev_ptr = ap.wait_for_input as usize;
            let chk = check_raw_event(ap.wait_for_input);
            let msg = alloc::format!(
                "  [AbsPtr Init {}|{}] Reset(false)={:?}, WaitForInput=0x{:X}, initial CheckEvent={:?}\r\n",
                orig_i, tag, rst, ev_ptr, chk
            );
            report.push_str(&msg);
            screen_print(&msg);
        }
    }

    // Track previous AbsPtr state per handle so idle firmware repeats don't flood the log
    let mut last_abs: Vec<Option<(u64, u64, u64, u32)>> = alloc::vec![None; abs_handles.len()];
    let mut abs_ev_hits: Vec<u32> = alloc::vec![0; abs_handles.len()];

    let step_ms: u64 = 25;
    let idle_limit_ms: u64 = 15_000;
    let max_total_ms: u64 = 45_000;

    while idle_ms < idle_limit_ms && elapsed_ms < max_total_ms && event_count < max_events {
        let mut got_event = false;

        // 1. Poll SimpleTextInputEx handles (hardware handles first!)
        for &(orig_i, handle, has_dp) in &text_ex_handles {
            if let Ok(mut proto) = unsafe {
                boot::open_protocol::<SimpleTextInputEx>(
                    OpenProtocolParams {
                        handle,
                        agent: boot::image_handle(),
                        controller: None,
                    },
                    OpenProtocolAttributes::GetProtocol,
                )
            } {
                while let Ok(Some(kd)) = proto.read_key_stroke_ex() {
                    got_event = true;
                    event_count += 1;
                    let tag = if has_dp { "HW" } else { "SPL" };
                    let line = alloc::format!(
                        "  [{:>5}ms] TextEx[{}|{}] scan=0x{:04X} uni=0x{:04X} shift=0x{:08X} toggle=0x{:02X}\r\n",
                        elapsed_ms,
                        orig_i,
                        tag,
                        kd.key.scan_code,
                        kd.key.unicode_char,
                        kd.key_state.key_shift_state,
                        kd.key_state.key_toggle_state,
                    );
                    report.push_str(&line);
                    screen_print(&line);

                    if kd.key.scan_code == ScanCode::ESCAPE.0 {
                        esc_count += 1;
                    }
                    if event_count >= max_events {
                        break;
                    }
                }
            }
        }

        // 2. Poll standard SimpleTextInput handles
        for &(orig_i, handle, has_dp) in &text_handles {
            if let Ok(mut proto) = unsafe {
                boot::open_protocol::<Input>(
                    OpenProtocolParams {
                        handle,
                        agent: boot::image_handle(),
                        controller: None,
                    },
                    OpenProtocolAttributes::GetProtocol,
                )
            } {
                while let Ok(Some(key)) = proto.read_key() {
                    got_event = true;
                    event_count += 1;
                    let tag = if has_dp { "HW" } else { "SPL" };
                    let line = match key {
                        Key::Printable(ch) => alloc::format!(
                            "  [{:>5}ms] TextIn[{}|{}] printable='{}' (0x{:04X})\r\n",
                            elapsed_ms,
                            orig_i,
                            tag,
                            char::from(ch),
                            u16::from(ch),
                        ),
                        Key::Special(sc) => {
                            if sc == ScanCode::ESCAPE {
                                esc_count += 1;
                            }
                            alloc::format!(
                                "  [{:>5}ms] TextIn[{}|{}] special={:?} (0x{:04X})\r\n",
                                elapsed_ms,
                                orig_i,
                                tag,
                                sc,
                                sc.0,
                            )
                        }
                    };
                    report.push_str(&line);
                    screen_print(&line);
                    if event_count >= max_events {
                        break;
                    }
                }
            }
        }

        // 3. Poll AbsolutePointer handles (Touchscreen) - call CheckEvent(WaitForInput) FIRST!
        for (slot, &(orig_i, handle, has_dp)) in abs_handles.iter().enumerate() {
            if let Ok(mut proto) = unsafe {
                boot::open_protocol::<AbsolutePointer>(
                    OpenProtocolParams {
                        handle,
                        agent: boot::image_handle(),
                        controller: None,
                    },
                    OpenProtocolAttributes::GetProtocol,
                )
            } {
                let ev_status = check_raw_event(proto.wait_for_input);
                if ev_status == uefi_raw::Status::SUCCESS {
                    abs_ev_hits[slot] = abs_ev_hits[slot].saturating_add(1);
                }

                if let Ok(Some(st)) = proto.read_state() {
                    let cur = (st.current_x, st.current_y, st.current_z, st.active_buttons);
                    let changed = last_abs[slot] != Some(cur);
                    let is_first = last_abs[slot].is_none();
                    last_abs[slot] = Some(cur);

                    // Log if state changed OR if buttons != 0 (skip repeated identical idle samples)
                    if !changed && st.active_buttons == 0 {
                        continue;
                    }
                    if is_first && st.active_buttons == 0 {
                        continue;
                    }

                    got_event = true;
                    event_count += 1;
                    let tag = if has_dp { "HW" } else { "SPL" };
                    let line = alloc::format!(
                        "  [{:>5}ms] AbsPtr[{}|{}] ev={:?} pos=({}, {}, {}) buttons=0x{:X}\r\n",
                        elapsed_ms,
                        orig_i,
                        tag,
                        ev_status,
                        st.current_x,
                        st.current_y,
                        st.current_z,
                        st.active_buttons,
                    );
                    report.push_str(&line);
                    screen_print(&line);

                    // Draw a 28x28 CYAN dot mapped linearly onto screen on real touch activity
                    let (min_x, max_x, min_y, max_y) = abs_ranges[slot];
                    if scr_w > 0 && scr_h > 0 && max_x > min_x && max_y > min_y {
                        let px = ((st.current_x.saturating_sub(min_x)) * (scr_w as u64)
                            / (max_x - min_x)) as usize;
                        let py = ((st.current_y.saturating_sub(min_y)) * (scr_h as u64)
                            / (max_y - min_y)) as usize;
                        let _ = blt_rect(
                            px.saturating_sub(14),
                            py.saturating_sub(14),
                            28,
                            28,
                            BltPixel::new(0, 240, 255),
                        );
                    }
                }
            }
        }

        // 4. Poll SimplePointer handles (Trackpad / Mouse) - skip zero-movement idle samples
        for &(orig_i, handle, has_dp) in &ptr_handles {
            if let Ok(mut proto) = unsafe {
                boot::open_protocol::<Pointer>(
                    OpenProtocolParams {
                        handle,
                        agent: boot::image_handle(),
                        controller: None,
                    },
                    OpenProtocolAttributes::GetProtocol,
                )
            } {
                if let Ok(Some(st)) = proto.read_state() {
                    if st.relative_movement == [0, 0, 0] && !st.button[0] && !st.button[1] {
                        continue;
                    }
                    got_event = true;
                    event_count += 1;
                    let tag = if has_dp { "HW" } else { "SPL" };
                    let line = alloc::format!(
                        "  [{:>5}ms] RelPtr[{}|{}] delta=({}, {}, {}) btn=({},{})\r\n",
                        elapsed_ms,
                        orig_i,
                        tag,
                        st.relative_movement[0],
                        st.relative_movement[1],
                        st.relative_movement[2],
                        st.button[0],
                        st.button[1],
                    );
                    report.push_str(&line);
                    screen_print(&line);
                }
            }
        }

        if esc_count >= 2 {
            let msg = "  [Capture ended early by double-ESC]\r\n";
            report.push_str(msg);
            screen_print(msg);
            break;
        }

        if got_event {
            idle_ms = 0;
        } else {
            idle_ms += step_ms;
        }

        boot::stall((step_ms * 1000) as usize);
        elapsed_ms += step_ms;

        if !got_event && idle_ms.is_multiple_of(3000) && idle_ms < idle_limit_ms {
            let rem = (idle_limit_ms - idle_ms) / 1000;
            let msg = alloc::format!("  ... waiting for input ({}s left)\r\n", rem);
            screen_print(&msg);
        }
    }

    let _ = writeln!(
        report,
        "  AbsPtr WaitForInput SUCCESS counts: {:?}",
        abs_ev_hits
    );
    let _ = writeln!(
        report,
        "  Total captured events: {} (elapsed {}ms)",
        event_count, elapsed_ms
    );
}
