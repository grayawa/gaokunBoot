#!/usr/bin/env python3
"""Extract GAOKUN-PROBE.TXT and /FV_DUMP/* from a GPT+FAT32 gaokun-probe.img."""

from __future__ import annotations

import struct
import sys
from pathlib import Path


def parse_lfn_name(entries: list[bytes]) -> str:
    # LFN entries arrive in reverse order before the 8.3 entry
    chars: list[int] = []
    for ent in reversed(entries):
        for off in (1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30):
            u = struct.unpack_from("<H", ent, off)[0]
            if u == 0x0000 or u == 0xFFFF:
                break
            chars.append(u)
    return "".join(chr(c) for c in chars)


def parse_83_name(ent: bytes) -> str:
    base = ent[0:8].decode("ascii", errors="replace").rstrip(" ")
    ext = ent[8:11].decode("ascii", errors="replace").rstrip(" ")
    return f"{base}.{ext}" if ext else base


def extract_image(img_path: Path, out_dir: Path) -> None:
    img = img_path.read_bytes()
    part_off = 2048 * 512
    p = img[part_off:]

    bps, spc, rsvd, nfats = struct.unpack_from("<HBHB", p, 11)
    fatsz32 = struct.unpack_from("<I", p, 36)[0]
    root_clus = struct.unpack_from("<I", p, 44)[0]
    fat_off = rsvd * bps
    data_off = (rsvd + nfats * fatsz32) * bps
    clus_sz = spc * bps

    def read_chain(clus: int, size: int | None = None) -> bytes:
        out = bytearray()
        seen = set()
        while 2 <= clus < 0x0FFFFFF8 and clus not in seen:
            seen.add(clus)
            o = data_off + (clus - 2) * clus_sz
            out += p[o : o + clus_sz]
            clus = struct.unpack_from("<I", p, fat_off + clus * 4)[0] & 0x0FFFFFFF
        return bytes(out[:size] if size is not None else out)

    def list_dir(clus: int) -> list[tuple[str, int, int, int]]:
        raw = read_chain(clus)
        res = []
        lfn_buf: list[bytes] = []
        for i in range(0, len(raw), 32):
            ent = raw[i : i + 32]
            if len(ent) < 32 or ent[0] == 0x00:
                break
            if ent[0] == 0xE5:
                lfn_buf.clear()
                continue
            attr = ent[11]
            if attr == 0x0F:
                lfn_buf.append(ent)
                continue
            if attr & 0x08:  # volume label
                lfn_buf.clear()
                continue
            name = parse_lfn_name(lfn_buf) if lfn_buf else parse_83_name(ent)
            lfn_buf.clear()
            hi = struct.unpack_from("<H", ent, 20)[0]
            lo = struct.unpack_from("<H", ent, 26)[0]
            sz = struct.unpack_from("<I", ent, 28)[0]
            res.append((name, attr, (hi << 16) | lo, sz))
        return res

    out_dir.mkdir(parents=True, exist_ok=True)
    root_entries = list_dir(root_clus)
    print("Root entries:", [(n, sz) for n, _, _, sz in root_entries])

    for name, attr, clus, sz in root_entries:
        if name.upper() == "GAOKUN-PROBE.TXT":
            data = read_chain(clus, sz)
            dst = out_dir / "GAOKUN-PROBE.TXT"
            dst.write_bytes(data)
            print(f"[EXTRACTED] {dst} ({len(data)} bytes)")
        elif name.upper() == "FV_DUMP" and (attr & 0x10):
            sub_entries = list_dir(clus)
            for sname, sattr, sclus, ssz in sub_entries:
                if sname in (".", "..") or (sattr & 0x10):
                    continue
                data = read_chain(sclus, ssz)
                dst = out_dir / sname
                dst.write_bytes(data)
                print(f"[EXTRACTED] {dst} ({len(data)} bytes)")


if __name__ == "__main__":
    root = Path(__file__).resolve().parent.parent
    src = Path(sys.argv[1]) if len(sys.argv) > 1 else root / "dist" / "gaokun-probe.img"
    dst = root / "fv_dump"
    extract_image(src, dst)
