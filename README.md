# gaokunBoot

A custom, tablet-first graphical multi-OS UEFI boot manager written in Rust (`aarch64-unknown-uefi`) for the **Huawei MateBook E Go (Snapdragon 8cx Gen 3 / `SC8280XP` / `GK-W7X` / `gaokun3`)**.

> **Disclaimer:** Running custom UEFI bootloaders and third-party operating systems on Windows-on-ARM hardware is experimental. Always back up your EFI System Partition (ESP) and BitLocker recovery keys before modifying boot order or ESP files. Testing via USB `.img` / `.iso` does not touch your internal NVMe drive.

---

## Features

- **Native `2560×1600` HiDPI UI via `GOP.Blt`**: Built specifically for `gaokun3`'s rotated landscape UEFI GOP (`FrameBufferBase == 0x0`), with pre-baked anti-aliased OFL-1.1 fonts (`Noto Sans SC` + `Cascadia Mono`), custom vector OS badges, and partial-rect dirty updates for countdowns and cursor movement.
- **Clean Sidebar + Detail Layout**:
  - **Top Bar**: Custom `gaokunBoot` vector logo + live SMBIOS device info (`HUAWEI GK-W7X`), BIOS version, `CurrentEL`, Touchscreen status, build version, and partial-rect auto-boot countdown badge.
  - **Left Sidebar**: Clean `Main Boot Menu` (OS cards only) + bottom-pinned `Settings` button.
  - **Right Pane**:
    - **`OS specific info & Settings`**: Per-OS controls (**Boot Method** live toggle between Chainload & Direct EFI Stub, Android **Slot A / Slot B** switcher, Linux/EFI-stub **EL1 (QHEE) / EL2 (KVM)** switch with automatic `slbounce.efi` / `qebspil.efi` and `*-el2.dtb` detection), target paths & cmdline preview, and large touch-friendly `Boot` / `Set as Default` buttons.
    - **`GaokunBoot Settings`**: Auto-Boot Timeout (`Off / 3s / 5s / 10s`), Default Boot Target (any OS or `Remember Last Booted`), Menu Entry Grouping (`Grouped` clean 1-card-per-OS view vs `Raw` all entries), 5-level sub-pixel Touchpad Pointer Speed (`0.35x`–`2.00x`), plus `UEFI BIOS Setup`, `Reboot`, and `Power Off`.
- **Full 3-Mode Input**:
  - **Native Touchscreen**: Automatically wakes Huawei's `I2cTouchPanel.efi` via its `TouchDeviceInit` protocol (`8d59d323-c654-4ae7-7b15-f25904992a42`), with 5ms polling, fast-tap capture, and 180ms debounce.
  - **Tablet Hardware Buttons**: `Volume+` / `Volume-` + `Power` (`0x0102`) for 100% keyboard-less navigation and settings control.
  - **Magnetic Keyboard & Touchpad**: Direct hardware `SimplePointer` (`0x12d1:0x10b8`) with sub-pixel speed scaling + keyboard shortcuts (`Arrows`, `Enter`, `1-9`, `S` Slot, `E` EL1/2, `V` Speed, `D` Default, `T` Timer, `F2` BIOS, `R` Reboot, `P` Power Off, `ESC` Cancel/Back).
- **Zero-Config Multi-OS Boot Engine**:
  - Auto-discovers **Windows 11 ARM64** (`\EFI\Microsoft\Boot\bootmgfw.efi`), **WinRE**, and all standard BLS **`\loader\entries\*.conf`** files (automatically pairing Android A/B slots such as `gk3boot` and `crDroid`), plus optional custom entries in `\EFI\gaokunBoot\entries\*.conf`.
  - **UEFI Chainload**: Constructs real partition `DevicePath` + UTF-16LE `LoadOptions`.
  - **ARM64 Linux EFI Stub**: Validates FDT magic (`0xD00DFEED`), installs DTB into `ACPI_RECLAIM` via `EFI_DTB_TABLE_GUID`, serves initrd via `LINUX_EFI_INITRD_MEDIA_GUID` (`EFI_LOAD_FILE2_PROTOCOL`), and passes kernel cmdline.
  - **UEFI NVRAM Persistence**: All settings save automatically to UEFI NVRAM (`GaokunBootCfg`).

---

## Building from Source

### Prerequisites
- Rust toolchain (`stable`) with target `aarch64-unknown-uefi`:
  ```bash
  rustup target add aarch64-unknown-uefi
  ```
- Python 3.10+ (standard library only, used by `scripts/make_image.py`)

### Build & Package Bootable Images

```bash
cargo build --release -p gaokun-boot
python scripts/make_image.py boot
```

Artifacts are written to `dist/`:
- `dist/gaokun-boot.efi` — Standalone AArch64 UEFI boot manager binary
- `dist/gaokun-boot.img` — 40 MiB GPT + FAT32 USB disk image (great for USB sticks or Android USB Mass Storage gadget apps)
- `dist/gaokun-boot.iso` — Compact El Torito UEFI ISO9660 image (CD-ROM fallback)

### Optional: Hardware Diagnostic Probe (`gaokun-probe`)

```bash
cargo build --release -p gaokun-probe
python scripts/make_image.py probe
```

---

## Usage

### 1. Safe Non-Destructive Test (USB / Phone USB Gadget)
1. Disable **Secure Boot** in Huawei BIOS (`Power + Volume+` or `F2`).
2. Mount `dist/gaokun-boot.img` (or write to a FAT32 USB drive as `\EFI\BOOT\BOOTAA64.EFI`).
3. Boot via `F12` Boot Menu — `gaokunBoot` automatically scans your internal NVMe ESP and boots your installed OSes without modifying your SSD.

### 2. Optional EL2 (KVM) Support
To use the built-in **EL2 (KVM)** toggle on Linux / EFI-stub entries:
- Place `slbounce.efi` (and optionally `qebspil.efi` + `tcblaunch.exe` as required by `slbounce`) in `\EFI\gaokunBoot\drivers\` on your ESP.
- If a sibling `*-el2.dtb` exists next to an entry's `devicetree` (or `el2-devicetree` is specified in the `.conf`), `gaokunBoot` automatically selects it when EL2 is enabled.

### 3. Optional Internal ESP Installation
Copy `dist/gaokun-boot.efi` to `\EFI\gaokunBoot\gaokun-boot.efi` on your NVMe ESP and register a UEFI boot entry (e.g. with `efibootmgr` on Linux or `bcdedit` on Windows).

---

## Project Layout

```text
gaokunBoot/
├── boot/                  # Main graphical UEFI boot manager (gaokun-boot)
│   └── src/
│       ├── main.rs        # 5ms event loop, dirty-rect countdown, pane focus
│       ├── ui.rs          # Sidebar + Detail UI (Main Boot Menu / OS Pane / Settings Pane)
│       ├── gfx.rs         # GOP.Blt offscreen renderer, anti-aliased shapes & vector OS icons
│       ├── font_data.rs   # Pre-baked OFL-1.1 HiDPI font atlas (Noto Sans SC + Cascadia Mono)
│       ├── input.rs       # Unified Touchscreen + Vol/Power + Keyboard + Touchpad input engine
│       ├── discover.rs    # Windows + BLS \loader\entries\*.conf + Android A/B pairing + EL2 drivers
│       ├── launcher.rs    # UEFI Chainload & ARM64 Linux EFI Stub (DTB + LoadFile2 initrd)
│       ├── config.rs      # UEFI NVRAM configuration persistence
│       └── efi_extra.rs   # UEFI protocols (TouchDeviceInit, AbsolutePointer, LoadFile2, SMBIOS)
├── probe/                 # Phase 0 UEFI hardware diagnostic probe (gaokun-probe)
└── scripts/
    ├── make_image.py      # Zero-dependency GPT+FAT32 .img and El Torito .iso builder
    └── gen_font.py        # Windows GDI font atlas generator (OFL-1.1 fonts)
```

---

## License

This project is licensed under the **MIT License** — see [`LICENSE`](LICENSE).
Third-party font and crate notices are documented in [`THIRD_PARTY_LICENSES.md`](THIRD_PARTY_LICENSES.md).
