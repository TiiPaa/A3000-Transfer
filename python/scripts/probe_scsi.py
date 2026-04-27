r"""
Diagnostic: sonde chaque \\.\ScsiN: et affiche si l'ouverture marche,
puis affiche le nombre de bus retournés par IOCTL_SCSI_GET_INQUIRY_DATA.
A lancer depuis un terminal Administrateur.
"""
from __future__ import annotations

import ctypes
from ctypes import wintypes

GENERIC_READ = 0x80000000
GENERIC_WRITE = 0x40000000
FILE_SHARE_READ = 0x00000001
FILE_SHARE_WRITE = 0x00000002
OPEN_EXISTING = 3
IOCTL_SCSI_GET_INQUIRY_DATA = 0x0004100C

k = ctypes.WinDLL("kernel32", use_last_error=True)
k.CreateFileW.restype = wintypes.HANDLE
k.CreateFileW.argtypes = [
    wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD, wintypes.LPVOID,
    wintypes.DWORD, wintypes.DWORD, wintypes.HANDLE,
]
k.DeviceIoControl.restype = wintypes.BOOL
k.DeviceIoControl.argtypes = [
    wintypes.HANDLE, wintypes.DWORD, wintypes.LPVOID, wintypes.DWORD,
    wintypes.LPVOID, wintypes.DWORD, ctypes.POINTER(wintypes.DWORD), wintypes.LPVOID,
]
k.CloseHandle.argtypes = [wintypes.HANDLE]
k.CloseHandle.restype = wintypes.BOOL

INVALID = wintypes.HANDLE(-1).value


def fmt_err(code: int) -> str:
    try:
        return ctypes.FormatError(code).strip()
    except Exception:
        return f"erreur {code}"


for i in range(16):
    path = rf"\\.\Scsi{i}:"
    h = k.CreateFileW(
        path,
        GENERIC_READ | GENERIC_WRITE,
        FILE_SHARE_READ | FILE_SHARE_WRITE,
        None,
        OPEN_EXISTING,
        0,
        None,
    )
    if h == INVALID or h == 0:
        err = ctypes.get_last_error()
        print(f"{path:14s} FAIL  err={err:<5d} ({fmt_err(err)})")
        continue

    buf = ctypes.create_string_buffer(64 * 1024)
    returned = wintypes.DWORD(0)
    ok = k.DeviceIoControl(
        h,
        IOCTL_SCSI_GET_INQUIRY_DATA,
        None,
        0,
        buf,
        len(buf),
        ctypes.byref(returned),
        None,
    )
    if ok:
        nb = buf.raw[0] if returned.value > 0 else 0
        print(f"{path:14s} OPEN  ioctl=OK    bytes={returned.value} buses={nb}")
    else:
        err = ctypes.get_last_error()
        print(f"{path:14s} OPEN  ioctl=FAIL err={err} ({fmt_err(err)})")

    k.CloseHandle(h)
