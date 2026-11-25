use alloc::string::String;
use core::fmt::Write;

/// Minimal read-only SMBIOS 3.0 (64-bit entry point) parser for Types 0, 1, 2, 3.
pub fn dump_smbios3(addr: usize, out: &mut String) {
    if addr == 0 {
        let _ = writeln!(out, "  SMBIOS3 address is null");
        return;
    }

    // SMBIOS 3.0 Entry Point is 24 bytes starting with "_SM3_"
    let ep = unsafe { core::slice::from_raw_parts(addr as *const u8, 24) };
    if &ep[0..5] != b"_SM3_" {
        let _ = writeln!(
            out,
            "  SMBIOS3 signature mismatch: {:02X?}",
            &ep[0..5]
        );
        return;
    }

    let major = ep[7];
    let minor = ep[8];
    let docrev = ep[9];
    let max_size = u32::from_le_bytes([ep[12], ep[13], ep[14], ep[15]]) as usize;
    let table_addr = u64::from_le_bytes([
        ep[16], ep[17], ep[18], ep[19], ep[20], ep[21], ep[22], ep[23],
    ]) as usize;

    let _ = writeln!(
        out,
        "  SMBIOS 3.{}.{} (docrev {}), table_addr=0x{:X}, max_size={}",
        major, minor, docrev, table_addr, max_size
    );

    if table_addr == 0 || !(4..=1024 * 1024).contains(&max_size) {
        let _ = writeln!(out, "  SMBIOS3 table bounds look invalid, skipping parse");
        return;
    }

    parse_smbios_structures(table_addr, max_size, out);
}

/// Minimal read-only SMBIOS 2.x (32-bit entry point, "_SM_") parser.
pub fn dump_smbios2(addr: usize, out: &mut String) {
    if addr == 0 {
        let _ = writeln!(out, "  SMBIOS2 address is null");
        return;
    }

    let ep = unsafe { core::slice::from_raw_parts(addr as *const u8, 31) };
    if &ep[0..4] != b"_SM_" {
        let _ = writeln!(out, "  SMBIOS2 signature mismatch: {:02X?}", &ep[0..4]);
        return;
    }

    let major = ep[6];
    let minor = ep[7];
    let table_len = u16::from_le_bytes([ep[22], ep[23]]) as usize;
    let table_addr = u32::from_le_bytes([ep[24], ep[25], ep[26], ep[27]]) as usize;

    let _ = writeln!(
        out,
        "  SMBIOS 2.x ({}.{}), table_addr=0x{:X}, table_len={}",
        major, minor, table_addr, table_len
    );

    if table_addr == 0 || !(4..=1024 * 1024).contains(&table_len) {
        let _ = writeln!(out, "  SMBIOS2 table bounds look invalid, skipping parse");
        return;
    }

    parse_smbios_structures(table_addr, table_len, out);
}

fn parse_smbios_structures(table_addr: usize, max_size: usize, out: &mut String) {
    let data = unsafe { core::slice::from_raw_parts(table_addr as *const u8, max_size) };
    let mut off = 0usize;

    while off + 4 <= data.len() {
        let ty = data[off];
        let len = data[off + 1] as usize;
        if len < 4 || off + len > data.len() {
            break;
        }
        let formatted = &data[off..off + len];

        // Find end of string-set (double NUL)
        let mut s_off = off + len;
        let strings_start = s_off;
        while s_off + 1 < data.len() {
            if data[s_off] == 0 && data[s_off + 1] == 0 {
                s_off += 2;
                break;
            }
            s_off += 1;
        }
        let strings_end = s_off.min(data.len());
        let string_area = &data[strings_start..strings_end];

        match ty {
            0 => {
                // BIOS Information
                let _ = writeln!(out, "  [Type 0 - BIOS]");
                if len > 0x04 {
                    let _ = writeln!(out, "    Vendor:  {}", get_str(string_area, formatted[0x04]));
                }
                if len > 0x05 {
                    let _ = writeln!(out, "    Version: {}", get_str(string_area, formatted[0x05]));
                }
                if len > 0x08 {
                    let _ = writeln!(out, "    Date:    {}", get_str(string_area, formatted[0x08]));
                }
            }
            1 => {
                // System Information
                let _ = writeln!(out, "  [Type 1 - System]");
                if len > 0x04 {
                    let _ = writeln!(out, "    Manufacturer: {}", get_str(string_area, formatted[0x04]));
                }
                if len > 0x05 {
                    let _ = writeln!(out, "    ProductName:  {}", get_str(string_area, formatted[0x05]));
                }
                if len > 0x06 {
                    let _ = writeln!(out, "    Version:      {}", get_str(string_area, formatted[0x06]));
                }
                if len > 0x19 {
                    let _ = writeln!(out, "    SKUNumber:    {}", get_str(string_area, formatted[0x19]));
                }
                if len > 0x1A {
                    let _ = writeln!(out, "    Family:       {}", get_str(string_area, formatted[0x1A]));
                }
            }
            2 => {
                // Baseboard Information
                let _ = writeln!(out, "  [Type 2 - Baseboard]");
                if len > 0x04 {
                    let _ = writeln!(out, "    Manufacturer: {}", get_str(string_area, formatted[0x04]));
                }
                if len > 0x05 {
                    let _ = writeln!(out, "    Product:      {}", get_str(string_area, formatted[0x05]));
                }
                if len > 0x06 {
                    let _ = writeln!(out, "    Version:      {}", get_str(string_area, formatted[0x06]));
                }
            }
            3 => {
                // Chassis Information
                let _ = writeln!(out, "  [Type 3 - Chassis]");
                if len > 0x04 {
                    let _ = writeln!(out, "    Manufacturer: {}", get_str(string_area, formatted[0x04]));
                }
                if len > 0x05 {
                    let _ = writeln!(out, "    Type:         0x{:02X}", formatted[0x05]);
                }
            }
            127 => break, // End-of-Table
            _ => {}
        }

        off = strings_end;
    }
}

fn get_str(area: &[u8], idx: u8) -> &str {
    if idx == 0 {
        return "<none>";
    }
    let mut cur = 1u8;
    let mut start = 0usize;
    for (i, &b) in area.iter().enumerate() {
        if b == 0 {
            if cur == idx {
                return core::str::from_utf8(&area[start..i]).unwrap_or("<non-utf8>");
            }
            cur = cur.saturating_add(1);
            start = i + 1;
            if start < area.len() && area[start] == 0 {
                break;
            }
        }
    }
    "<not-found>"
}
