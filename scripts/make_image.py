#!/usr/bin/env python3
"""
Build bootable ARM64 UEFI images for Huawei MateBook E Go (gaokun3):
  1. dist/gaokun-probe.img  - 40 MiB GPT disk with writable FAT32 EFI System Partition
                              (Recommended for phone USB Mass Storage R/W mode so
                               gaokun-probe.txt is saved directly into the image!)
  2. dist/gaokun-probe.iso  - Compact El Torito UEFI bootable ISO9660 image
                              (Fallback for phone CD-ROM / read-only ISO gadget mode)

Zero external dependencies; pure Python standard library.
"""

from __future__ import annotations

import math
import os
import struct
import sys
import uuid
import zlib
from pathlib import Path

SECTOR_SIZE = 512
ISO_SECTOR_SIZE = 2048

# EFI System Partition Type GUID: C12A7328-F81F-11D2-BA4B-00A0C93EC93B
ESP_TYPE_GUID = uuid.UUID("C12A7328-F81F-11D2-BA4B-00A0C93EC93B").bytes_le
DISK_GUID = uuid.UUID("6A0C4B10-8280-4A30-9E01-67616F6B756E").bytes_le
PART_GUID = uuid.UUID("6A0C4B10-8280-4A30-9E02-67616F6B756E").bytes_le


def fat_dir_entry(
    name_83: bytes, attr: int, first_cluster: int, file_size: int
) -> bytes:
    assert len(name_83) == 11
    # Fixed timestamp: 2026-11-25 09:30:00
    fat_date = ((2026 - 1980) << 9) | (11 << 5) | 25
    fat_time = (9 << 11) | (30 << 5) | 0
    return struct.pack(
        "<11sBBBHHHHHHHL",
        name_83,
        attr,
        0,  # NTRes
        0,  # CrtTimeTenth
        fat_time,
        fat_date,
        fat_date,
        (first_cluster >> 16) & 0xFFFF,
        fat_time,
        fat_date,
        first_cluster & 0xFFFF,
        file_size,
    )


def build_fat32_partition(
    part_sectors: int, hidden_sectors: int, efi_bytes: bytes, readme_bytes: bytes
) -> bytearray:
    """Create a strictly spec-compliant FAT32 filesystem image."""
    reserved_sectors = 32
    num_fats = 2
    sectors_per_cluster = 1
    cluster_bytes = sectors_per_cluster * SECTOR_SIZE

    # Fixed-point calculation of FAT32 table size
    fat_sectors = 1
    for _ in range(16):
        data_sectors = part_sectors - reserved_sectors - num_fats * fat_sectors
        total_clusters = data_sectors // sectors_per_cluster
        needed = math.ceil((total_clusters + 2) * 4 / SECTOR_SIZE)
        if needed == fat_sectors:
            break
        fat_sectors = needed

    data_sectors = part_sectors - reserved_sectors - num_fats * fat_sectors
    total_clusters = data_sectors // sectors_per_cluster
    if total_clusters < 65525:
        raise ValueError(
            f"FAT32 requires >= 65525 clusters, got {total_clusters}"
        )

    part = bytearray(part_sectors * SECTOR_SIZE)

    # Cluster allocation:
    #   2: root dir
    #   3: /EFI
    #   4: /EFI/BOOT
    #   5..: /EFI/BOOT/BOOTAA64.EFI
    #   next..: /README.TXT
    efi_clusters = max(1, math.ceil(len(efi_bytes) / cluster_bytes))
    readme_clusters = max(1, math.ceil(len(readme_bytes) / cluster_bytes))

    efi_start = 5
    readme_start = efi_start + efi_clusters
    next_free_cluster = readme_start + readme_clusters
    used_clusters = next_free_cluster - 2
    free_clusters = total_clusters - used_clusters

    # 1. Boot sector (VBR)
    vbr = bytearray(SECTOR_SIZE)
    vbr[0:3] = b"\xEB\x58\x90"
    vbr[3:11] = b"MSDOS5.0"
    struct.pack_into("<H", vbr, 11, SECTOR_SIZE)  # BPB_BytsPerSec
    vbr[13] = sectors_per_cluster  # BPB_SecPerClus
    struct.pack_into("<H", vbr, 14, reserved_sectors)  # BPB_RsvdSecCnt
    vbr[16] = num_fats  # BPB_NumFATs
    struct.pack_into("<H", vbr, 17, 0)  # BPB_RootEntCnt
    struct.pack_into("<H", vbr, 19, 0)  # BPB_TotSec16
    vbr[21] = 0xF8  # BPB_Media
    struct.pack_into("<H", vbr, 22, 0)  # BPB_FATSz16
    struct.pack_into("<H", vbr, 24, 63)  # BPB_SecPerTrk
    struct.pack_into("<H", vbr, 26, 255)  # BPB_NumHeads
    struct.pack_into("<I", vbr, 28, hidden_sectors)  # BPB_HiddSec
    struct.pack_into("<I", vbr, 32, part_sectors)  # BPB_TotSec32
    struct.pack_into("<I", vbr, 36, fat_sectors)  # BPB_FATSz32
    struct.pack_into("<H", vbr, 40, 0)  # BPB_ExtFlags
    struct.pack_into("<H", vbr, 42, 0)  # BPB_FSVer
    struct.pack_into("<I", vbr, 44, 2)  # BPB_RootClus
    struct.pack_into("<H", vbr, 48, 1)  # BPB_FSInfo
    struct.pack_into("<H", vbr, 50, 6)  # BPB_BkBootSec
    vbr[64] = 0x80  # BS_DrvNum
    vbr[66] = 0x29  # BS_BootSig
    struct.pack_into("<I", vbr, 67, 0x20261125)  # BS_VolID
    vbr[71:82] = b"GAOKUNPROBE"
    vbr[82:90] = b"FAT32   "
    vbr[510:512] = b"\x55\xAA"

    # 2. FSInfo sector
    fsinfo = bytearray(SECTOR_SIZE)
    struct.pack_into("<I", fsinfo, 0, 0x41615252)
    struct.pack_into("<I", fsinfo, 484, 0x61417272)
    struct.pack_into("<I", fsinfo, 488, free_clusters)
    struct.pack_into("<I", fsinfo, 492, next_free_cluster)
    struct.pack_into("<I", fsinfo, 508, 0xAA550000)

    # Write primary and backup VBR / FSInfo
    part[0:512] = vbr
    part[512:1024] = fsinfo
    part[6 * 512 : 7 * 512] = vbr
    part[7 * 512 : 8 * 512] = fsinfo

    # 3. Build FAT table
    fat = bytearray(fat_sectors * SECTOR_SIZE)

    def set_fat(cluster: int, value: int) -> None:
        struct.pack_into("<I", fat, cluster * 4, value & 0x0FFFFFFF)

    set_fat(0, 0x0FFFFFF8)
    set_fat(1, 0x0FFFFFFF)
    set_fat(2, 0x0FFFFFFF)  # Root
    set_fat(3, 0x0FFFFFFF)  # /EFI
    set_fat(4, 0x0FFFFFFF)  # /EFI/BOOT

    for i in range(efi_clusters):
        c = efi_start + i
        set_fat(c, (c + 1) if i + 1 < efi_clusters else 0x0FFFFFFF)

    for i in range(readme_clusters):
        c = readme_start + i
        set_fat(c, (c + 1) if i + 1 < readme_clusters else 0x0FFFFFFF)

    fat1_off = reserved_sectors * SECTOR_SIZE
    fat2_off = (reserved_sectors + fat_sectors) * SECTOR_SIZE
    part[fat1_off : fat1_off + len(fat)] = fat
    part[fat2_off : fat2_off + len(fat)] = fat

    data_off = (reserved_sectors + num_fats * fat_sectors) * SECTOR_SIZE

    def write_cluster(cluster: int, data: bytes) -> None:
        off = data_off + (cluster - 2) * cluster_bytes
        part[off : off + len(data)] = data

    # 4. Root directory (cluster 2)
    root_dir = bytearray()
    root_dir += fat_dir_entry(b"GAOKUNPROBE", 0x08, 0, 0)  # Volume label
    root_dir += fat_dir_entry(b"EFI        ", 0x10, 3, 0)
    root_dir += fat_dir_entry(b"README  TXT", 0x20, readme_start, len(readme_bytes))
    write_cluster(2, root_dir)

    # 5. /EFI directory (cluster 3) - '..' pointing to root MUST be cluster 0 per FAT32 spec
    efi_dir = bytearray()
    efi_dir += fat_dir_entry(b".          ", 0x10, 3, 0)
    efi_dir += fat_dir_entry(b"..         ", 0x10, 0, 0)
    efi_dir += fat_dir_entry(b"BOOT       ", 0x10, 4, 0)
    write_cluster(3, efi_dir)

    # 6. /EFI/BOOT directory (cluster 4)
    boot_dir = bytearray()
    boot_dir += fat_dir_entry(b".          ", 0x10, 4, 0)
    boot_dir += fat_dir_entry(b"..         ", 0x10, 3, 0)
    boot_dir += fat_dir_entry(b"BOOTAA64EFI", 0x20, efi_start, len(efi_bytes))
    write_cluster(4, boot_dir)

    # 7. File payloads
    write_cluster(efi_start, efi_bytes)
    write_cluster(readme_start, readme_bytes)

    return part


def build_gpt_img(efi_bytes: bytes, readme_bytes: bytes, out_path: Path) -> None:
    """Create a 40 MiB GPT disk image with a single FAT32 EFI System Partition."""
    total_bytes = 40 * 1024 * 1024
    total_sectors = total_bytes // SECTOR_SIZE  # 81920
    part_start_lba = 2048  # 1 MiB aligned
    part_end_lba = total_sectors - 34  # Last usable LBA
    part_sectors = part_end_lba - part_start_lba + 1

    img = bytearray(total_bytes)

    # 1. Protective MBR (LBA 0)
    mbr = bytearray(SECTOR_SIZE)
    struct.pack_into(
        "<BBBBBBBBII",
        mbr,
        446,
        0x00,  # Boot indicator
        0x00,
        0x02,
        0x00,  # Start CHS
        0xEE,  # GPT Protective MBR type
        0xFF,
        0xFF,
        0xFF,  # End CHS
        1,  # Start LBA
        total_sectors - 1,  # Size in sectors
    )
    mbr[510:512] = b"\x55\xAA"
    img[0:512] = mbr

    # 2. GPT Partition Entry Array (128 entries * 128 bytes = 16384 bytes = 32 sectors)
    entries = bytearray(128 * 128)
    part_name = "EFI System Partition".encode("utf-16-le")
    entry0 = bytearray(128)
    entry0[0:16] = ESP_TYPE_GUID
    entry0[16:32] = PART_GUID
    struct.pack_into("<QQQ", entry0, 32, part_start_lba, part_end_lba, 0)
    entry0[56 : 56 + len(part_name)] = part_name
    entries[0:128] = entry0
    entries_crc = zlib.crc32(entries) & 0xFFFFFFFF

    def make_gpt_header(my_lba: int, alt_lba: int, entries_lba: int) -> bytes:
        hdr = bytearray(SECTOR_SIZE)
        struct.pack_into(
            "<8sIIIIQQQQ16sQIII",
            hdr,
            0,
            b"EFI PART",
            0x00010000,  # Revision 1.0
            92,  # HeaderSize
            0,  # HeaderCRC32 placeholder
            0,  # Reserved
            my_lba,
            alt_lba,
            34,  # FirstUsableLBA
            total_sectors - 34,  # LastUsableLBA
            DISK_GUID,
            entries_lba,
            128,  # Num entries
            128,  # Entry size
            entries_crc,
        )
        hdr_crc = zlib.crc32(hdr[:92]) & 0xFFFFFFFF
        struct.pack_into("<I", hdr, 16, hdr_crc)
        return bytes(hdr)

    primary_hdr = make_gpt_header(1, total_sectors - 1, 2)
    backup_hdr = make_gpt_header(total_sectors - 1, 1, total_sectors - 33)

    img[1 * SECTOR_SIZE : 2 * SECTOR_SIZE] = primary_hdr
    img[2 * SECTOR_SIZE : 34 * SECTOR_SIZE] = entries
    img[(total_sectors - 33) * SECTOR_SIZE : (total_sectors - 1) * SECTOR_SIZE] = entries
    img[(total_sectors - 1) * SECTOR_SIZE : total_sectors * SECTOR_SIZE] = backup_hdr

    # 3. FAT32 Partition
    fat32 = build_fat32_partition(
        part_sectors, part_start_lba, efi_bytes, readme_bytes
    )
    start_off = part_start_lba * SECTOR_SIZE
    img[start_off : start_off + len(fat32)] = fat32

    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_bytes(img)


def build_fat16_efi_blob(efi_bytes: bytes) -> bytearray:
    """Create a compact 4 MiB FAT16 EFI System Partition image for El Torito ISO."""
    part_sectors = 8192  # 4 MiB (fits in El Torito 16-bit sector count!)
    sectors_per_cluster = 1
    reserved_sectors = 1
    num_fats = 2
    root_ent_cnt = 512
    root_dir_sectors = (root_ent_cnt * 32) // SECTOR_SIZE  # 32
    fat_sectors = 32  # 32 * 512 / 2 = 8192 entries (>= data clusters)

    data_start_sector = reserved_sectors + num_fats * fat_sectors + root_dir_sectors
    cluster_bytes = sectors_per_cluster * SECTOR_SIZE

    part = bytearray(part_sectors * SECTOR_SIZE)

    # VBR
    vbr = bytearray(SECTOR_SIZE)
    vbr[0:3] = b"\xEB\x3C\x90"
    vbr[3:11] = b"MSDOS5.0"
    struct.pack_into("<H", vbr, 11, SECTOR_SIZE)
    vbr[13] = sectors_per_cluster
    struct.pack_into("<H", vbr, 14, reserved_sectors)
    vbr[16] = num_fats
    struct.pack_into("<H", vbr, 17, root_ent_cnt)
    struct.pack_into("<H", vbr, 19, part_sectors)
    vbr[21] = 0xF8
    struct.pack_into("<H", vbr, 22, fat_sectors)
    struct.pack_into("<H", vbr, 24, 63)
    struct.pack_into("<H", vbr, 26, 255)
    struct.pack_into("<I", vbr, 28, 0)
    struct.pack_into("<I", vbr, 32, 0)
    vbr[36] = 0x80
    vbr[38] = 0x29
    struct.pack_into("<I", vbr, 39, 0x20261125)
    vbr[43:54] = b"GAOKUNPROBE"
    vbr[54:62] = b"FAT16   "
    vbr[510:512] = b"\x55\xAA"
    part[0:512] = vbr

    # Clusters: 2 = /EFI, 3 = /EFI/BOOT, 4.. = BOOTAA64.EFI
    efi_clusters = max(1, math.ceil(len(efi_bytes) / cluster_bytes))
    efi_start = 4

    fat = bytearray(fat_sectors * SECTOR_SIZE)

    def set_fat16(cluster: int, value: int) -> None:
        struct.pack_into("<H", fat, cluster * 2, value & 0xFFFF)

    set_fat16(0, 0xFFF8)
    set_fat16(1, 0xFFFF)
    set_fat16(2, 0xFFFF)  # /EFI
    set_fat16(3, 0xFFFF)  # /EFI/BOOT
    for i in range(efi_clusters):
        c = efi_start + i
        set_fat16(c, (c + 1) if i + 1 < efi_clusters else 0xFFFF)

    fat1_off = reserved_sectors * SECTOR_SIZE
    fat2_off = (reserved_sectors + fat_sectors) * SECTOR_SIZE
    part[fat1_off : fat1_off + len(fat)] = fat
    part[fat2_off : fat2_off + len(fat)] = fat

    # Root directory (fixed region in FAT16)
    root_off = (reserved_sectors + num_fats * fat_sectors) * SECTOR_SIZE
    root_dir = bytearray()
    root_dir += fat_dir_entry(b"GAOKUNPROBE", 0x08, 0, 0)
    root_dir += fat_dir_entry(b"EFI        ", 0x10, 2, 0)
    part[root_off : root_off + len(root_dir)] = root_dir

    data_off = data_start_sector * SECTOR_SIZE

    def write_cluster(cluster: int, data: bytes) -> None:
        off = data_off + (cluster - 2) * cluster_bytes
        part[off : off + len(data)] = data

    efi_dir = bytearray()
    efi_dir += fat_dir_entry(b".          ", 0x10, 2, 0)
    efi_dir += fat_dir_entry(b"..         ", 0x10, 0, 0)
    efi_dir += fat_dir_entry(b"BOOT       ", 0x10, 3, 0)
    write_cluster(2, efi_dir)

    boot_dir = bytearray()
    boot_dir += fat_dir_entry(b".          ", 0x10, 3, 0)
    boot_dir += fat_dir_entry(b"..         ", 0x10, 2, 0)
    boot_dir += fat_dir_entry(b"BOOTAA64EFI", 0x20, efi_start, len(efi_bytes))
    write_cluster(3, boot_dir)

    write_cluster(efi_start, efi_bytes)
    return part


def both16(v: int) -> bytes:
    return struct.pack("<HH", v, ((v & 0xFF) << 8) | ((v >> 8) & 0xFF))


def both32(v: int) -> bytes:
    return struct.pack("<I", v) + struct.pack(">I", v)


def iso_dir_record(
    name: bytes, extent_lba: int, data_len: int, is_dir: bool
) -> bytes:
    flags = 0x02 if is_dir else 0x00
    rec_len = 33 + len(name)
    if rec_len % 2 == 1:
        rec_len += 1
    rec = bytearray(rec_len)
    rec[0] = rec_len
    rec[1] = 0
    rec[2:10] = both32(extent_lba)
    rec[10:18] = both32(data_len)
    # Date: 2026-11-25 09:30:00 UTC
    rec[18:25] = bytes([126, 11, 25, 9, 30, 0, 0])
    rec[25] = flags
    rec[26] = 0
    rec[27] = 0
    rec[28:32] = both16(1)  # Volume sequence number
    rec[32] = len(name)
    rec[33 : 33 + len(name)] = name
    return bytes(rec)


def build_uefi_iso(efi_bytes: bytes, out_path: Path) -> None:
    """Create an El Torito UEFI-bootable ISO9660 image."""
    efi_img = build_fat16_efi_blob(efi_bytes)
    efi_iso_sectors = math.ceil(len(efi_img) / ISO_SECTOR_SIZE)
    efi_512_sectors = len(efi_img) // SECTOR_SIZE

    # ISO Layout (2048-byte sectors):
    #   0..15: System area (zeros)
    #   16:    Primary Volume Descriptor (PVD)
    #   17:    El Torito Boot Record Volume Descriptor
    #   18:    Volume Descriptor Set Terminator
    #   19:    El Torito Boot Catalog
    #   20:    Path Table (L-type)
    #   21:    Path Table (M-type)
    #   22:    Root Directory
    #   23..:  EFI FAT16 Boot Image (also exposed as /EFIBOOT.IMG)
    catalog_lba = 19
    l_path_lba = 20
    m_path_lba = 21
    root_lba = 22
    efi_img_lba = 23
    total_iso_sectors = efi_img_lba + efi_iso_sectors

    iso = bytearray(total_iso_sectors * ISO_SECTOR_SIZE)

    # Root directory contents
    root_dir = bytearray()
    root_dir += iso_dir_record(b"\x00", root_lba, ISO_SECTOR_SIZE, True)
    root_dir += iso_dir_record(b"\x01", root_lba, ISO_SECTOR_SIZE, True)
    root_dir += iso_dir_record(b"EFIBOOT.IMG;1", efi_img_lba, len(efi_img), False)
    iso[root_lba * ISO_SECTOR_SIZE : root_lba * ISO_SECTOR_SIZE + len(root_dir)] = (
        root_dir
    )

    # Path tables (10 bytes each for root)
    l_pt = struct.pack("<BBIH", 1, 0, root_lba, 1) + b"\x00\x00"
    m_pt = struct.pack(">BBIH", 1, 0, root_lba, 1) + b"\x00\x00"
    iso[l_path_lba * ISO_SECTOR_SIZE : l_path_lba * ISO_SECTOR_SIZE + len(l_pt)] = l_pt
    iso[m_path_lba * ISO_SECTOR_SIZE : m_path_lba * ISO_SECTOR_SIZE + len(m_pt)] = m_pt

    # Sector 16: Primary Volume Descriptor
    pvd = bytearray(ISO_SECTOR_SIZE)
    pvd[0] = 0x01
    pvd[1:6] = b"CD001"
    pvd[6] = 0x01
    pvd[8:40] = b"GAOKUNBOOT".ljust(32, b" ")
    pvd[40:72] = b"GAOKUN_PROBE".ljust(32, b" ")
    pvd[80:88] = both32(total_iso_sectors)
    pvd[120:124] = both16(1)
    pvd[124:128] = both16(1)
    pvd[128:132] = both16(ISO_SECTOR_SIZE)
    pvd[132:140] = both32(len(l_pt))
    struct.pack_into("<I", pvd, 140, l_path_lba)
    struct.pack_into(">I", pvd, 148, m_path_lba)
    pvd[156:190] = iso_dir_record(b"\x00", root_lba, ISO_SECTOR_SIZE, True)
    pvd[190:318] = b" " * 128
    pvd[318:446] = b"GAOKUNBOOT".ljust(128, b" ")
    pvd[446:574] = b"GAOKUNBOOT".ljust(128, b" ")
    pvd[574:702] = b"GAOKUNBOOT".ljust(128, b" ")
    pvd[702:813] = b" " * 111
    ts = b"2026112509300000\x00"
    pvd[813:830] = ts
    pvd[830:847] = ts
    pvd[847:864] = b"0000000000000000\x00"
    pvd[864:881] = ts
    pvd[881] = 0x01
    iso[16 * ISO_SECTOR_SIZE : 17 * ISO_SECTOR_SIZE] = pvd

    # Sector 17: El Torito Boot Record Volume Descriptor
    br = bytearray(ISO_SECTOR_SIZE)
    br[0] = 0x00
    br[1:6] = b"CD001"
    br[6] = 0x01
    br[7:39] = b"EL TORITO SPECIFICATION".ljust(32, b"\x00")
    struct.pack_into("<I", br, 71, catalog_lba)
    iso[17 * ISO_SECTOR_SIZE : 18 * ISO_SECTOR_SIZE] = br

    # Sector 18: Volume Descriptor Set Terminator
    term = bytearray(ISO_SECTOR_SIZE)
    term[0] = 0xFF
    term[1:6] = b"CD001"
    term[6] = 0x01
    iso[18 * ISO_SECTOR_SIZE : 19 * ISO_SECTOR_SIZE] = term

    # Sector 19: El Torito Boot Catalog (UEFI Platform ID = 0xEF)
    cat = bytearray(ISO_SECTOR_SIZE)
    # Validation Entry (32 bytes)
    val = bytearray(32)
    val[0] = 0x01  # Header ID
    val[1] = 0xEF  # Platform ID: UEFI
    val[4:28] = b"GAOKUNBOOT".ljust(24, b"\x00")
    val[30] = 0x55
    val[31] = 0xAA
    words = struct.unpack("<16H", val)
    checksum = (-sum(words)) & 0xFFFF
    struct.pack_into("<H", val, 28, checksum)
    assert sum(struct.unpack("<16H", val)) & 0xFFFF == 0
    cat[0:32] = val

    # Initial / Default Entry (32 bytes)
    ent = bytearray(32)
    ent[0] = 0x88  # Bootable
    ent[1] = 0x00  # No emulation
    struct.pack_into("<H", ent, 2, 0)  # Load segment
    ent[4] = 0x00  # System type
    struct.pack_into("<H", ent, 6, efi_512_sectors)  # 512-byte sector count
    struct.pack_into("<I", ent, 8, efi_img_lba)  # Load RBA (2048-byte LBA)
    cat[32:64] = ent

    iso[catalog_lba * ISO_SECTOR_SIZE : (catalog_lba + 1) * ISO_SECTOR_SIZE] = cat

    # Payload: FAT16 EFI image
    off = efi_img_lba * ISO_SECTOR_SIZE
    iso[off : off + len(efi_img)] = efi_img

    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_bytes(iso)


def verify_gpt_img(img_path: Path, expected_efi_len: int) -> None:
    data = img_path.read_bytes()
    assert data[510:512] == b"\x55\xAA", "Invalid MBR signature"
    assert data[512:520] == b"EFI PART", "Invalid primary GPT signature"
    hdr_crc_stored = struct.unpack_from("<I", data, 512 + 16)[0]
    hdr_copy = bytearray(data[512 : 512 + 92])
    struct.pack_into("<I", hdr_copy, 16, 0)
    assert (zlib.crc32(hdr_copy) & 0xFFFFFFFF) == hdr_crc_stored, "GPT header CRC mismatch"

    part_off = 2048 * SECTOR_SIZE
    assert data[part_off + 510 : part_off + 512] == b"\x55\xAA", "Invalid FAT32 VBR signature"
    assert data[part_off + 82 : part_off + 90] == b"FAT32   ", "Not FAT32"
    print(f"[OK] Verified GPT+FAT32 image: {img_path} ({len(data) // (1024*1024)} MiB, EFI={expected_efi_len} bytes)")


def main() -> int:
    root = Path(__file__).resolve().parent.parent
    mode = sys.argv[1] if len(sys.argv) > 1 else "boot"

    if mode == "probe":
        efi_src = root / "target" / "aarch64-unknown-uefi" / "release" / "gaokun-probe.efi"
        base_name = "gaokun-probe"
        readme_text = (
            "gaokun-probe UEFI hardware diagnostic image for Huawei MateBook E Go (gaokun3)\r\n"
            "Boot this image from UEFI Boot Menu (F12) with Secure Boot disabled.\r\n"
            "If mounted Read-Write as USB Mass Storage, the probe will save GAOKUN-PROBE.TXT here.\r\n"
        )
    elif mode == "boot":
        efi_src = root / "target" / "aarch64-unknown-uefi" / "release" / "gaokun-boot.efi"
        base_name = "gaokun-boot"
        readme_text = (
            "gaokunBoot - Graphical Multi-OS UEFI Boot Manager for Huawei MateBook E Go (gaokun3)\r\n"
            "Boot from UEFI Boot Menu (F12) with Secure Boot disabled.\r\n"
            "Auto-discovers Windows, BLS /loader/entries/*.conf (Android/Linux/gk3boot) on NVMe ESP.\r\n"
            "Controls: Vol+/Vol- select, Power/Enter confirm, Touchpad click, D set default, T timeout.\r\n"
        )
    else:
        efi_src = Path(mode)
        base_name = efi_src.stem
        readme_text = f"{base_name} UEFI bootable image\r\n"

    if not efi_src.is_file():
        print(f"ERROR: EFI binary not found: {efi_src}", file=sys.stderr)
        return 1

    efi_bytes = efi_src.read_bytes()
    readme_bytes = readme_text.encode("ascii")

    dist = root / "dist"
    dist.mkdir(parents=True, exist_ok=True)

    (dist / f"{base_name}.efi").write_bytes(efi_bytes)
    (dist / "BOOTAA64.EFI").write_bytes(efi_bytes)

    img_path = dist / f"{base_name}.img"
    iso_path = dist / f"{base_name}.iso"

    build_gpt_img(efi_bytes, readme_bytes, img_path)
    verify_gpt_img(img_path, len(efi_bytes))

    build_uefi_iso(efi_bytes, iso_path)
    print(f"[OK] Built UEFI El Torito ISO: {iso_path} ({iso_path.stat().st_size // 1024} KiB)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
