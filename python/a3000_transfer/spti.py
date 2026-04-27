from __future__ import annotations

import ctypes
from ctypes import wintypes
import os
import struct
from typing import Iterable

from .models import ScsiTargetInfo

GENERIC_READ = 0x80000000
GENERIC_WRITE = 0x40000000
FILE_SHARE_READ = 0x00000001
FILE_SHARE_WRITE = 0x00000002
OPEN_EXISTING = 3
IOCTL_SCSI_GET_INQUIRY_DATA = 0x0004100C

ERROR_FILE_NOT_FOUND = 2
ERROR_PATH_NOT_FOUND = 3
ERROR_ACCESS_DENIED = 5
ERROR_INSUFFICIENT_BUFFER = 122
ERROR_MORE_DATA = 234
ERROR_INVALID_NAME = 123

_HEADER_FORMAT = "<B3x"
_BUS_DATA_FORMAT = "<BB2xI"
_INQUIRY_HEADER_FORMAT = "<BBBBII"
_HEADER_SIZE = struct.calcsize(_HEADER_FORMAT)
_BUS_DATA_SIZE = struct.calcsize(_BUS_DATA_FORMAT)
_INQUIRY_HEADER_SIZE = struct.calcsize(_INQUIRY_HEADER_FORMAT)
_INITIAL_BUFFER_SIZE = 16 * 1024
_MAXIMUM_BUFFER_SIZE = 256 * 1024


def scan_scsi_targets(max_adapters: int = 16) -> list[ScsiTargetInfo]:
    if os.name != "nt":
        raise RuntimeError("Le scan SPTI ne fonctionne que sous Windows.")

    kernel32 = _load_kernel32()
    targets: list[ScsiTargetInfo] = []
    access_denied = False
    adapter_seen = False

    for host_adapter in range(max_adapters):
        device_path = rf"\\.\Scsi{host_adapter}:"
        handle = kernel32.CreateFileW(
            ctypes.c_wchar_p(device_path),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            0,
            None,
        )

        if handle == wintypes.HANDLE(-1).value:
            error = ctypes.get_last_error()
            if error == ERROR_ACCESS_DENIED:
                access_denied = True
            elif error not in (ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, ERROR_INVALID_NAME):
                raise OSError(error, f"Impossible d’ouvrir {device_path}: {_format_winerror(error)}")
            continue

        adapter_seen = True
        try:
            raw = _query_inquiry_data(kernel32, handle, device_path)
            targets.extend(_parse_inquiry_buffer(host_adapter, raw))
        finally:
            kernel32.CloseHandle(handle)

    if not adapter_seen and access_denied:
        raise PermissionError("Accès refusé aux adaptateurs SCSI. Lance le terminal en administrateur.")

    return targets


def _load_kernel32():
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)

    kernel32.CreateFileW.argtypes = [
        wintypes.LPCWSTR,
        wintypes.DWORD,
        wintypes.DWORD,
        wintypes.LPVOID,
        wintypes.DWORD,
        wintypes.DWORD,
        wintypes.HANDLE,
    ]
    kernel32.CreateFileW.restype = wintypes.HANDLE

    kernel32.DeviceIoControl.argtypes = [
        wintypes.HANDLE,
        wintypes.DWORD,
        wintypes.LPVOID,
        wintypes.DWORD,
        wintypes.LPVOID,
        wintypes.DWORD,
        ctypes.POINTER(wintypes.DWORD),
        wintypes.LPVOID,
    ]
    kernel32.DeviceIoControl.restype = wintypes.BOOL

    kernel32.CloseHandle.argtypes = [wintypes.HANDLE]
    kernel32.CloseHandle.restype = wintypes.BOOL

    return kernel32


def _query_inquiry_data(kernel32, handle, device_path: str) -> bytes:
    buffer_size = _INITIAL_BUFFER_SIZE

    while buffer_size <= _MAXIMUM_BUFFER_SIZE:
        out_buffer = ctypes.create_string_buffer(buffer_size)
        returned = wintypes.DWORD(0)
        ok = kernel32.DeviceIoControl(
            handle,
            IOCTL_SCSI_GET_INQUIRY_DATA,
            None,
            0,
            out_buffer,
            buffer_size,
            ctypes.byref(returned),
            None,
        )
        if ok:
            return out_buffer.raw[: returned.value]

        error = ctypes.get_last_error()
        if error in (ERROR_INSUFFICIENT_BUFFER, ERROR_MORE_DATA):
            buffer_size *= 2
            continue

        raise OSError(error, f"Échec de IOCTL_SCSI_GET_INQUIRY_DATA sur {device_path}: {_format_winerror(error)}")

    raise BufferError(f"Le buffer d’inquiry SCSI requis dépasse {_MAXIMUM_BUFFER_SIZE} octets sur {device_path}.")


def _parse_inquiry_buffer(host_adapter: int, raw: bytes) -> list[ScsiTargetInfo]:
    if len(raw) < _HEADER_SIZE:
        return []

    number_of_buses = struct.unpack_from(_HEADER_FORMAT, raw, 0)[0]
    results: list[ScsiTargetInfo] = []

    for bus_index in range(number_of_buses):
        bus_offset = _HEADER_SIZE + (bus_index * _BUS_DATA_SIZE)
        if bus_offset + _BUS_DATA_SIZE > len(raw):
            break

        _logical_units, _initiator_bus_id, inquiry_offset = struct.unpack_from(_BUS_DATA_FORMAT, raw, bus_offset)
        if inquiry_offset <= 0 or inquiry_offset >= len(raw):
            continue

        for item in _iter_inquiries(raw, inquiry_offset):
            results.append(
                ScsiTargetInfo(
                    host_adapter=host_adapter,
                    path_id=item["path_id"],
                    target_id=item["target_id"],
                    lun=item["lun"],
                    vendor=item["vendor"] or "Unknown",
                    product=item["product"] or _device_type_name(item["device_type"]),
                    revision=item["revision"],
                    device_type=item["device_type"],
                    device_claimed=item["device_claimed"],
                )
            )

    return results


def _iter_inquiries(raw: bytes, start_offset: int) -> Iterable[dict]:
    current_offset = start_offset

    while current_offset > 0 and current_offset + _INQUIRY_HEADER_SIZE <= len(raw):
        path_id, target_id, lun, claimed, inquiry_length, next_offset = struct.unpack_from(
            _INQUIRY_HEADER_FORMAT, raw, current_offset
        )

        data_offset = current_offset + _INQUIRY_HEADER_SIZE
        max_available = max(0, len(raw) - data_offset)
        safe_length = min(inquiry_length, max_available)
        if safe_length <= 0:
            break

        inquiry = raw[data_offset : data_offset + safe_length]
        device_type = inquiry[0] & 0x1F if inquiry else 0x1F

        yield {
            "path_id": path_id,
            "target_id": target_id,
            "lun": lun,
            "device_claimed": bool(claimed),
            "device_type": device_type,
            "vendor": _read_ascii(inquiry, 8, 8),
            "product": _read_ascii(inquiry, 16, 16),
            "revision": _read_ascii(inquiry, 32, 4),
        }

        if next_offset == 0 or next_offset <= current_offset or next_offset >= len(raw):
            break

        current_offset = next_offset


def _read_ascii(data: bytes, offset: int, length: int) -> str:
    if offset >= len(data):
        return ""
    return data[offset : offset + length].decode("ascii", errors="ignore").strip(" \0")


def _device_type_name(device_type: int) -> str:
    mapping = {
        0x00: "Direct-Access",
        0x01: "Sequential",
        0x02: "Printer",
        0x03: "Processor",
        0x04: "WORM",
        0x05: "CD-ROM",
        0x06: "Scanner",
        0x07: "Optical",
        0x08: "Changer",
        0x09: "Communications",
    }
    return mapping.get(device_type, f"Type-0x{device_type:02X}")


def _format_winerror(error: int) -> str:
    try:
        return ctypes.FormatError(error).strip()
    except Exception:
        return f"erreur Windows {error}"
