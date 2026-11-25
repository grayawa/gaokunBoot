#!/usr/bin/env python3
"""
Generate a crisp, anti-aliased 4-bit alpha HiDPI font atlas (boot/src/font_data.rs)
using Windows GDI (zero external dependencies) for Huawei MateBook E Go 2560x1600.

Uses 100% open-source SIL OFL 1.1 fonts (safe for public open-source redistribution):
  - FONT_LG / FONT_UI : Noto Sans SC (SIL OFL 1.1) for crisp modern titles, cards, buttons
  - FONT_MONO         : Cascadia Mono (SIL OFL 1.1) for paths, cmdline options, metadata
"""

from __future__ import annotations

import ctypes
from ctypes import wintypes
from pathlib import Path

gdi32 = ctypes.windll.gdi32
user32 = ctypes.windll.user32

BI_RGB = 0
DIB_RGB_COLORS = 0
ANTIALIASED_QUALITY = 4
CLEARTYPE_QUALITY = 5
OUT_TT_PRECIS = 4
CLIP_DEFAULT_PRECIS = 0
DEFAULT_CHARSET = 1
FW_REGULAR = 400
FW_SEMIBOLD = 600
TRANSPARENT = 1


class BITMAPINFOHEADER(ctypes.Structure):
    _fields_ = [
        ("biSize", wintypes.DWORD),
        ("biWidth", wintypes.LONG),
        ("biHeight", wintypes.LONG),
        ("biPlanes", wintypes.WORD),
        ("biBitCount", wintypes.WORD),
        ("biCompression", wintypes.DWORD),
        ("biSizeImage", wintypes.DWORD),
        ("biXPelsPerMeter", wintypes.LONG),
        ("biYPelsPerMeter", wintypes.LONG),
        ("biClrUsed", wintypes.DWORD),
        ("biClrImportant", wintypes.DWORD),
    ]


class BITMAPINFO(ctypes.Structure):
    _fields_ = [("bmiHeader", BITMAPINFOHEADER), ("bmiColors", wintypes.DWORD * 3)]


class SIZE(ctypes.Structure):
    _fields_ = [("cx", wintypes.LONG), ("cy", wintypes.LONG)]


class TEXTMETRICW(ctypes.Structure):
    _fields_ = [
        ("tmHeight", wintypes.LONG),
        ("tmAscent", wintypes.LONG),
        ("tmDescent", wintypes.LONG),
        ("tmInternalLeading", wintypes.LONG),
        ("tmExternalLeading", wintypes.LONG),
        ("tmAveCharWidth", wintypes.LONG),
        ("tmMaxCharWidth", wintypes.LONG),
        ("tmWeight", wintypes.LONG),
        ("tmOverhang", wintypes.LONG),
        ("tmDigitizedAspectX", wintypes.LONG),
        ("tmDigitizedAspectY", wintypes.LONG),
        ("tmFirstChar", wintypes.WCHAR),
        ("tmLastChar", wintypes.WCHAR),
        ("tmDefaultChar", wintypes.WCHAR),
        ("tmBreakChar", wintypes.WCHAR),
        ("tmItalic", wintypes.BYTE),
        ("tmUnderlined", wintypes.BYTE),
        ("tmStruckOut", wintypes.BYTE),
        ("tmPitchAndFamily", wintypes.BYTE),
        ("tmCharSet", wintypes.BYTE),
    ]


gdi32.CreateCompatibleDC.argtypes = [wintypes.HDC]
gdi32.CreateCompatibleDC.restype = wintypes.HDC
gdi32.DeleteDC.argtypes = [wintypes.HDC]
gdi32.SelectObject.argtypes = [wintypes.HDC, wintypes.HGDIOBJ]
gdi32.SelectObject.restype = wintypes.HGDIOBJ
gdi32.DeleteObject.argtypes = [wintypes.HGDIOBJ]
gdi32.CreateDIBSection.argtypes = [
    wintypes.HDC,
    ctypes.POINTER(BITMAPINFO),
    wintypes.UINT,
    ctypes.POINTER(ctypes.c_void_p),
    wintypes.HANDLE,
    wintypes.DWORD,
]
gdi32.CreateDIBSection.restype = wintypes.HBITMAP
gdi32.CreateFontW.argtypes = [
    ctypes.c_int,
    ctypes.c_int,
    ctypes.c_int,
    ctypes.c_int,
    ctypes.c_int,
    wintypes.DWORD,
    wintypes.DWORD,
    wintypes.DWORD,
    wintypes.DWORD,
    wintypes.DWORD,
    wintypes.DWORD,
    wintypes.DWORD,
    wintypes.DWORD,
    wintypes.LPCWSTR,
]
gdi32.CreateFontW.restype = wintypes.HFONT
gdi32.SetTextColor.argtypes = [wintypes.HDC, wintypes.DWORD]
gdi32.SetBkMode.argtypes = [wintypes.HDC, ctypes.c_int]
gdi32.TextOutW.argtypes = [wintypes.HDC, ctypes.c_int, ctypes.c_int, wintypes.LPCWSTR, ctypes.c_int]
gdi32.GetTextExtentPoint32W.argtypes = [
    wintypes.HDC,
    wintypes.LPCWSTR,
    ctypes.c_int,
    ctypes.POINTER(SIZE),
]
gdi32.GetTextMetricsW.argtypes = [wintypes.HDC, ctypes.POINTER(TEXTMETRICW)]


def rasterize_font(face: str, pixel_height: int, weight: int):
    hdc = gdi32.CreateCompatibleDC(None)
    assert hdc, "CreateCompatibleDC failed"

    hfont = gdi32.CreateFontW(
        -pixel_height,
        0,
        0,
        0,
        weight,
        0,
        0,
        0,
        DEFAULT_CHARSET,
        OUT_TT_PRECIS,
        CLIP_DEFAULT_PRECIS,
        ANTIALIASED_QUALITY,
        0,
        face,
    )
    assert hfont, f"CreateFontW failed for {face}"
    old_font = gdi32.SelectObject(hdc, hfont)

    tm = TEXTMETRICW()
    gdi32.GetTextMetricsW(hdc, ctypes.byref(tm))
    line_height = int(tm.tmHeight)

    canvas_w = max(64, pixel_height * 2)
    canvas_h = max(64, line_height + 16)

    bmi = BITMAPINFO()
    bmi.bmiHeader.biSize = ctypes.sizeof(BITMAPINFOHEADER)
    bmi.bmiHeader.biWidth = canvas_w
    bmi.bmiHeader.biHeight = -canvas_h  # top-down
    bmi.bmiHeader.biPlanes = 1
    bmi.bmiHeader.biBitCount = 32
    bmi.bmiHeader.biCompression = BI_RGB

    bits_ptr = ctypes.c_void_p()
    hbmp = gdi32.CreateDIBSection(
        hdc, ctypes.byref(bmi), DIB_RGB_COLORS, ctypes.byref(bits_ptr), None, 0
    )
    assert hbmp and bits_ptr.value, "CreateDIBSection failed"
    old_bmp = gdi32.SelectObject(hdc, hbmp)

    gdi32.SetBkMode(hdc, TRANSPARENT)
    gdi32.SetTextColor(hdc, 0x00FFFFFF)

    buf_type = ctypes.c_uint8 * (canvas_w * canvas_h * 4)
    buf = buf_type.from_address(bits_ptr.value)

    glyphs = []  # (advance, x_off, y_off, w, h, data_offset)
    packed_data = bytearray()
    non_empty_count = 0

    for code in range(32, 127):
        ch = chr(code)
        ctypes.memset(bits_ptr.value, 0, canvas_w * canvas_h * 4)

        sz = SIZE()
        gdi32.GetTextExtentPoint32W(hdc, ch, 1, ctypes.byref(sz))
        adv = max(1, int(sz.cx))

        # Draw with 4px X margin to capture slight overhang
        pad_x = 4
        gdi32.TextOutW(hdc, pad_x, 0, ch, 1)
        gdi32.GdiFlush()

        # Find tight bounding box
        min_x, min_y = canvas_w, canvas_h
        max_x, max_y = -1, -1
        for y in range(canvas_h):
            row_off = y * canvas_w * 4
            for x in range(canvas_w):
                # Grayscale value in green channel (white on black)
                v = buf[row_off + x * 4 + 1]
                if v >= 12:
                    if x < min_x:
                        min_x = x
                    if x > max_x:
                        max_x = x
                    if y < min_y:
                        min_y = y
                    if y > max_y:
                        max_y = y

        if max_x < min_x:
            glyphs.append((adv, 0, 0, 0, 0, 0))
            continue

        non_empty_count += 1
        gw = max_x - min_x + 1
        gh = max_y - min_y + 1
        gx_off = min_x - pad_x
        gy_off = min_y
        data_off = len(packed_data)

        # Pack 4-bit alpha (2 pixels per byte, high nibble first)
        nibbles = []
        for y in range(min_y, max_y + 1):
            row_off = y * canvas_w * 4
            for x in range(min_x, max_x + 1):
                v = buf[row_off + x * 4 + 1]
                nibbles.append((v + 8) // 17)  # 0..15
        if len(nibbles) % 2 == 1:
            nibbles.append(0)
        for i in range(0, len(nibbles), 2):
            packed_data.append((nibbles[i] << 4) | nibbles[i + 1])

        glyphs.append((adv, gx_off, gy_off, gw, gh, data_off))

    gdi32.SelectObject(hdc, old_bmp)
    gdi32.SelectObject(hdc, old_font)
    gdi32.DeleteObject(hbmp)
    gdi32.DeleteObject(hfont)
    gdi32.DeleteDC(hdc)

    assert non_empty_count >= 90, f"Font {face} rendered too few glyphs ({non_empty_count})"
    return line_height, glyphs, bytes(packed_data)


def emit_font_rs(name: str, line_height: int, glyphs, data: bytes) -> str:
    lines = []
    lines.append(f"pub const {name}_LINE_HEIGHT: usize = {line_height};")
    lines.append(f"pub static {name}_GLYPHS: [GlyphMetric; 95] = [")
    for idx, (adv, xo, yo, w, h, off) in enumerate(glyphs):
        ch = chr(32 + idx)
        safe = ch.replace("\\", "\\\\").replace("'", "\\'")
        lines.append(
            f"    GlyphMetric {{ advance: {adv}, x_off: {xo}, y_off: {yo}, w: {w}, h: {h}, offset: {off} }}, // '{safe}'"
        )
    lines.append("];")
    lines.append(f"pub static {name}_DATA: [u8; {len(data)}] = [")
    for i in range(0, len(data), 20):
        chunk = ", ".join(f"0x{b:02X}" for b in data[i : i + 20])
        lines.append(f"    {chunk},")
    lines.append("];")
    return "\n".join(lines)


def main() -> int:
    root = Path(__file__).resolve().parent.parent
    out_path = root / "boot" / "src" / "font_data.rs"
    out_path.parent.mkdir(parents=True, exist_ok=True)

    ui_lh, ui_glyphs, ui_data = rasterize_font("Noto Sans SC", 34, FW_SEMIBOLD)
    lg_lh, lg_glyphs, lg_data = rasterize_font("Noto Sans SC", 48, FW_SEMIBOLD)
    mono_lh, mono_glyphs, mono_data = rasterize_font("Cascadia Mono", 22, FW_REGULAR)

    rs = [
        "// Auto-generated by scripts/gen_font.py - DO NOT EDIT BY HAND",
        "// Rasterized from SIL OFL 1.1 open-source fonts: Noto Sans SC & Cascadia Mono",
        "// See THIRD_PARTY_LICENSES.md for font license attribution.",
        "#[derive(Clone, Copy)]",
        "pub struct GlyphMetric {",
        "    pub advance: u8,",
        "    pub x_off: i8,",
        "    pub y_off: i8,",
        "    pub w: u8,",
        "    pub h: u8,",
        "    pub offset: u32,",
        "}",
        "",
        emit_font_rs("FONT_LG", lg_lh, lg_glyphs, lg_data),
        "",
        emit_font_rs("FONT_UI", ui_lh, ui_glyphs, ui_data),
        "",
        emit_font_rs("FONT_MONO", mono_lh, mono_glyphs, mono_data),
        "",
    ]

    out_path.write_text("\n".join(rs), encoding="utf-8")
    total_kb = (len(lg_data) + len(ui_data) + len(mono_data)) / 1024
    print(
        f"[OK] Generated {out_path} (LG={lg_lh}px {len(lg_data)}B, UI={ui_lh}px {len(ui_data)}B, MONO={mono_lh}px {len(mono_data)}B, total={total_kb:.1f} KiB)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
