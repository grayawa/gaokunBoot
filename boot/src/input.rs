use alloc::vec;
use alloc::vec::Vec;

use uefi::boot::{self, OpenProtocolAttributes, OpenProtocolParams, SearchType};
use uefi::proto::console::pointer::Pointer;
use uefi::proto::console::text::{Input, Key, ScanCode};
use uefi::proto::device_path::DevicePath;
use uefi::{Handle, Identify};

use crate::efi_extra::{
    check_raw_event, init_huawei_touchscreen, AbsolutePointer, SimpleTextInputEx, SCAN_GAOKUN_POWER,
};

#[derive(Clone, Copy, Debug)]
pub enum InputEvent {
    MoveUp,
    MoveDown,
    MoveLeft,
    MoveRight,
    Confirm,
    Cancel,
    SetDefault,
    CycleTimeout,
    ToggleAndroidSlot,
    ToggleLinuxEl2,
    CyclePtrSpeed,
    BiosSetup,
    Reboot,
    PowerOff,
    QuickIndex(usize),
    PointerMove,
    /// Pointer click from touchpad/mouse (`from_touch = false`) or touchscreen (`from_touch = true`).
    PointerClick {
        x: i32,
        y: i32,
        from_touch: bool,
    },
}

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

fn sort_hw_first(handles: Vec<Handle>) -> Vec<Handle> {
    let mut tagged: Vec<(Handle, bool)> = handles
        .into_iter()
        .map(|h| (h, has_device_path(h)))
        .collect();
    tagged.sort_by_key(|&(_, has_dp)| !has_dp);
    tagged.into_iter().map(|(h, _)| h).collect()
}

pub struct InputManager {
    scr_w: i32,
    scr_h: i32,
    text_ex_handles: Vec<Handle>,
    text_handles: Vec<Handle>,
    rel_ptr_handles: Vec<Handle>,
    abs_ptr_handles: Vec<(Handle, u64, u64, u64, u64)>,
    /// Per-handle last raw (x, y, buttons) sample
    last_abs: Vec<Option<(u64, u64, u32)>>,
    /// Timestamp (ms) of last accepted touchscreen tap for debounce
    last_touch_tap_ms: u64,
    pub cursor_visible: bool,
    pub cursor_x: i32,
    pub cursor_y: i32,
    pub touch_ready: bool,
    rem_x: i32,
    rem_y: i32,
    scroll_accum: i32,
    prev_left_btn: bool,
    prev_right_btn: bool,
}

impl InputManager {
    pub fn init(scr_w: usize, scr_h: usize) -> Self {
        let touch_ready = init_huawei_touchscreen().unwrap_or(false);

        let text_ex_handles = sort_hw_first(
            boot::locate_handle_buffer(SearchType::ByProtocol(&SimpleTextInputEx::GUID))
                .map(|b| b.to_vec())
                .unwrap_or_default(),
        );
        let text_handles = sort_hw_first(
            boot::locate_handle_buffer(SearchType::ByProtocol(&Input::GUID))
                .map(|b| b.to_vec())
                .unwrap_or_default(),
        );
        // Only poll concrete hardware SimplePointer handles (skip ConSplitter which explodes delta * 65536)
        let rel_ptr_handles: Vec<Handle> =
            boot::locate_handle_buffer(SearchType::ByProtocol(&Pointer::GUID))
                .map(|b| {
                    let hw: Vec<Handle> = b
                        .iter()
                        .copied()
                        .filter(|&h| has_device_path(h))
                        .collect();
                    if hw.is_empty() {
                        b.to_vec()
                    } else {
                        hw
                    }
                })
                .unwrap_or_default();

        let abs_raw: Vec<Handle> =
            boot::locate_handle_buffer(SearchType::ByProtocol(&AbsolutePointer::GUID))
                .map(|b| {
                    b.iter()
                        .copied()
                        .filter(|&h| has_device_path(h))
                        .collect()
                })
                .unwrap_or_default();

        let abs_ptr_handles: Vec<(Handle, u64, u64, u64, u64)> = abs_raw
            .into_iter()
            .filter_map(|handle| {
                let mut ap = unsafe {
                    boot::open_protocol::<AbsolutePointer>(
                        OpenProtocolParams {
                            handle,
                            agent: boot::image_handle(),
                            controller: None,
                        },
                        OpenProtocolAttributes::GetProtocol,
                    )
                }
                .ok()?;
                let _ = ap.reset_device(false);
                let m = ap.mode()?;
                if m.absolute_max_x > m.absolute_min_x && m.absolute_max_y > m.absolute_min_y {
                    Some((
                        handle,
                        m.absolute_min_x,
                        m.absolute_max_x,
                        m.absolute_min_y,
                        m.absolute_max_y,
                    ))
                } else {
                    None
                }
            })
            .collect();

        let last_abs = vec![None; abs_ptr_handles.len()];
        let has_abs = !abs_ptr_handles.is_empty();

        Self {
            scr_w: scr_w as i32,
            scr_h: scr_h as i32,
            text_ex_handles,
            text_handles,
            rel_ptr_handles,
            abs_ptr_handles,
            last_abs,
            last_touch_tap_ms: 0,
            cursor_visible: false,
            cursor_x: (scr_w / 2) as i32,
            cursor_y: (scr_h / 2) as i32,
            touch_ready: touch_ready && has_abs,
            rem_x: 0,
            rem_y: 0,
            scroll_accum: 0,
            prev_left_btn: false,
            prev_right_btn: false,
        }
    }

    fn map_key(scan: u16, uni: u16) -> Option<InputEvent> {
        match scan {
            0x0001 => return Some(InputEvent::MoveUp),          // UP / Volume+
            0x0002 => return Some(InputEvent::MoveDown),        // DOWN / Volume-
            0x0004 => return Some(InputEvent::MoveLeft),        // LEFT
            0x0003 => return Some(InputEvent::MoveRight),       // RIGHT
            SCAN_GAOKUN_POWER => return Some(InputEvent::Confirm), // Hardware Power button
            0x0017 => return Some(InputEvent::Cancel),          // ESC
            0x000C => return Some(InputEvent::BiosSetup),       // F2
            _ => {}
        }
        match uni {
            0x000D | 0x000A | 0x0020 => Some(InputEvent::Confirm), // Enter / Space
            0x001B => Some(InputEvent::Cancel),
            0x006B | 0x004B | 0x0077 | 0x0057 => Some(InputEvent::MoveUp), // k / w
            0x006A | 0x004A => Some(InputEvent::MoveDown),                 // j
            0x0073 | 0x0053 => Some(InputEvent::ToggleAndroidSlot),        // s / S
            0x0065 | 0x0045 => Some(InputEvent::ToggleLinuxEl2),           // e / E
            0x0076 | 0x0056 => Some(InputEvent::CyclePtrSpeed),            // v / V
            0x0064 | 0x0044 => Some(InputEvent::SetDefault),               // d / D
            0x0074 | 0x0054 => Some(InputEvent::CycleTimeout),             // t / T
            0x0072 | 0x0052 => Some(InputEvent::Reboot),                   // r / R
            0x0070 | 0x0050 => Some(InputEvent::PowerOff),                 // p / P
            0x0031..=0x0039 => Some(InputEvent::QuickIndex((uni - 0x0031) as usize)),
            _ => None,
        }
    }

    /// Poll all input devices.
    /// `ptr_speed_num`: fixed-point multiplier over 20.
    /// `now_ms`: monotonic elapsed loop time in ms for touch debouncing.
    pub fn poll(&mut self, ptr_speed_num: i32, now_ms: u64) -> Vec<InputEvent> {
        let mut events = Vec::new();

        // 1. SimpleTextInputEx (Vol+, Vol-, Power 0x0102, magnetic keyboard, USB keyboard)
        for &handle in &self.text_ex_handles {
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
                    if let Some(ev) = Self::map_key(kd.key.scan_code, kd.key.unicode_char) {
                        events.push(ev);
                    }
                }
            }
        }

        // 2. Fallback SimpleTextInput
        for &handle in &self.text_handles {
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
                    let ev = match key {
                        Key::Special(ScanCode::UP) => Some(InputEvent::MoveUp),
                        Key::Special(ScanCode::DOWN) => Some(InputEvent::MoveDown),
                        Key::Special(ScanCode::LEFT) => Some(InputEvent::MoveLeft),
                        Key::Special(ScanCode::RIGHT) => Some(InputEvent::MoveRight),
                        Key::Special(ScanCode::ESCAPE) => Some(InputEvent::Cancel),
                        Key::Special(ScanCode::FUNCTION_2) => Some(InputEvent::BiosSetup),
                        Key::Special(sc) if sc.0 == SCAN_GAOKUN_POWER => Some(InputEvent::Confirm),
                        Key::Printable(ch) => Self::map_key(0, u16::from(ch)),
                        _ => None,
                    };
                    if let Some(e) = ev {
                        events.push(e);
                    }
                }
            }
        }

        // 3. Hardware SimplePointer (Magnetic Touchpad / USB Mouse) with smooth sub-pixel scaling
        let mut moved = false;
        let mut clicked = false;
        let denom = 20;

        for &handle in &self.rel_ptr_handles {
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
                    let dx = st.relative_movement[0];
                    let dy = st.relative_movement[1];
                    let dz = st.relative_movement[2];

                    if dx != 0 || dy != 0 {
                        let sx = dx * ptr_speed_num + self.rem_x;
                        let sy = dy * ptr_speed_num + self.rem_y;
                        let step_x = sx / denom;
                        let step_y = sy / denom;
                        self.rem_x = sx % denom;
                        self.rem_y = sy % denom;

                        if step_x != 0 || step_y != 0 {
                            self.cursor_x = (self.cursor_x + step_x).clamp(0, self.scr_w - 1);
                            self.cursor_y = (self.cursor_y + step_y).clamp(0, self.scr_h - 1);
                            self.cursor_visible = true;
                            moved = true;
                        }
                    }

                    if dz != 0 {
                        self.scroll_accum += dz;
                        if self.scroll_accum >= 10 {
                            events.push(InputEvent::MoveDown);
                            self.scroll_accum = 0;
                        } else if self.scroll_accum <= -10 {
                            events.push(InputEvent::MoveUp);
                            self.scroll_accum = 0;
                        }
                    }

                    let left = st.button[0];
                    if left && !self.prev_left_btn {
                        self.cursor_visible = true;
                        clicked = true;
                    }
                    self.prev_left_btn = left;

                    let right = st.button[1];
                    if right && !self.prev_right_btn {
                        events.push(InputEvent::Cancel);
                    }
                    self.prev_right_btn = right;
                }
            }
        }
        if moved {
            events.push(InputEvent::PointerMove);
        }
        if clicked {
            events.push(InputEvent::PointerClick {
                x: self.cursor_x,
                y: self.cursor_y,
                from_touch: false,
            });
        }

        // 4. Native Touchscreen via AbsolutePointer (tuned for Huawei I2cTouchPanel 25Hz polling!)
        //    - Finger down reports buttons=0x07 (bit 0 = 1), finger up reports buttons=0x04 (bit 0 = 0).
        //    - Because I2cTouchPanel has no FIFO, we catch both normal down edges AND ultra-fast taps
        //      that jumped to a new coordinate already on the release frame (0x04), with 180ms debounce.
        //    - Touch hides the mouse arrow cursor so we avoid unnecessary GOP.Blt overhead during touch!
        for (i, &(handle, min_x, max_x, min_y, max_y)) in self.abs_ptr_handles.iter().enumerate() {
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
                let _ = check_raw_event(proto.wait_for_input);
                if let Ok(Some(st)) = proto.read_state() {
                    let cur = (st.current_x, st.current_y, st.active_buttons);
                    let prev = self.last_abs[i];
                    self.last_abs[i] = Some(cur);

                    let Some((prev_x, prev_y, prev_btn)) = prev else {
                        // Seed initial idle sample (0, 4096, 0x00) without firing a phantom tap
                        continue;
                    };
                    if cur == (prev_x, prev_y, prev_btn) {
                        continue;
                    }

                    let is_down = (st.active_buttons & 0x1) != 0;
                    let was_down = (prev_btn & 0x1) != 0;
                    let dx_raw = st.current_x.abs_diff(prev_x);
                    let dy_raw = st.current_y.abs_diff(prev_y);
                    let moved_significantly = dx_raw > 16 || dy_raw > 16;

                    let normal_press = is_down && !was_down;
                    // Catch ultra-quick tap whose 0x07 frame was overwritten by 0x04 before we polled
                    let fast_release_tap =
                        !is_down && !was_down && st.active_buttons == 0x04 && moved_significantly;

                    if (normal_press || fast_release_tap)
                        && now_ms.saturating_sub(self.last_touch_tap_ms) >= 180
                    {
                        self.last_touch_tap_ms = now_ms;
                        let px = ((st.current_x.saturating_sub(min_x)) * (self.scr_w as u64)
                            / (max_x - min_x)) as i32;
                        let py = ((st.current_y.saturating_sub(min_y)) * (self.scr_h as u64)
                            / (max_y - min_y)) as i32;
                        self.cursor_x = px.clamp(0, self.scr_w - 1);
                        self.cursor_y = py.clamp(0, self.scr_h - 1);
                        self.cursor_visible = false; // Touch hides arrow cursor for snappier Blt!
                        events.push(InputEvent::PointerClick {
                            x: self.cursor_x,
                            y: self.cursor_y,
                            from_touch: true,
                        });
                    }
                }
            }
        }

        events
    }
}
