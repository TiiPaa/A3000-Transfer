r"""
Pass-through SCSI via IOCTL_SCSI_PASS_THROUGH_DIRECT.
Permet d'envoyer un CDB arbitraire à une cible (PathId/TargetId/Lun) attachée
à un host adapter ouvert via \\.\ScsiN:.
"""
from __future__ import annotations

import ctypes
import time
from ctypes import wintypes
from dataclasses import dataclass

GENERIC_READ = 0x80000000
GENERIC_WRITE = 0x40000000
FILE_SHARE_READ = 0x00000001
FILE_SHARE_WRITE = 0x00000002
OPEN_EXISTING = 3
INVALID_HANDLE_VALUE = wintypes.HANDLE(-1).value

IOCTL_SCSI_PASS_THROUGH_DIRECT = 0x0004D014

SCSI_IOCTL_DATA_OUT = 0
SCSI_IOCTL_DATA_IN = 1
SCSI_IOCTL_DATA_UNSPECIFIED = 2

SENSE_BUFFER_LENGTH = 32
DEFAULT_TIMEOUT_SECONDS = 30
DATA_BUFFER_ALIGNMENT = 512  # storport bounce les buffers non-alignés (DMA)

ERROR_IO_DEVICE = 1117  # transitoire : slave busy, retry après délai


def _align_up(value: int, alignment: int) -> int:
    if alignment <= 1:
        return value
    return (value + alignment - 1) & ~(alignment - 1)


def _aligned_buffer(size: int, alignment: int = DATA_BUFFER_ALIGNMENT):
    """Alloue un buffer plus grand et retourne (raw, aligned_address).

    Storport peut bouncer un buffer non-aligné (copie kernel intermédiaire) ce qui
    change le pattern d'I/O bus et peut casser les transferts SMDI multi-paquets
    sur firmware A3000 v0200. Garder la référence à `raw` pour que la mémoire
    reste allouée.
    """
    raw = ctypes.create_string_buffer(size + alignment)
    base = ctypes.addressof(raw)
    aligned = _align_up(base, alignment)
    return raw, aligned


class SCSI_PASS_THROUGH_DIRECT(ctypes.Structure):
    _fields_ = [
        ("Length", ctypes.c_ushort),
        ("ScsiStatus", ctypes.c_ubyte),
        ("PathId", ctypes.c_ubyte),
        ("TargetId", ctypes.c_ubyte),
        ("Lun", ctypes.c_ubyte),
        ("CdbLength", ctypes.c_ubyte),
        ("SenseInfoLength", ctypes.c_ubyte),
        ("DataIn", ctypes.c_ubyte),
        ("DataTransferLength", ctypes.c_ulong),
        ("TimeOutValue", ctypes.c_ulong),
        ("DataBuffer", ctypes.c_void_p),
        ("SenseInfoOffset", ctypes.c_ulong),
        ("Cdb", ctypes.c_ubyte * 16),
    ]


class _SptdWithSense(ctypes.Structure):
    _fields_ = [
        ("spt", SCSI_PASS_THROUGH_DIRECT),
        ("Sense", ctypes.c_ubyte * SENSE_BUFFER_LENGTH),
    ]


@dataclass(slots=True)
class PassThroughResult:
    scsi_status: int
    data: bytes
    sense: bytes
    transferred: int


def _load_kernel32():
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
    k.CloseHandle.restype = wintypes.BOOL
    k.CloseHandle.argtypes = [wintypes.HANDLE]
    return k


def open_adapter(host_adapter: int) -> int:
    k = _load_kernel32()
    path = rf"\\.\Scsi{host_adapter}:"
    handle = k.CreateFileW(
        path,
        GENERIC_READ | GENERIC_WRITE,
        FILE_SHARE_READ | FILE_SHARE_WRITE,
        None,
        OPEN_EXISTING,
        0,
        None,
    )
    if handle == INVALID_HANDLE_VALUE or not handle:
        err = ctypes.get_last_error()
        if err == 5:
            raise PermissionError(f"Accès refusé sur {path}. Lance le terminal en administrateur.")
        raise OSError(err, f"Impossible d'ouvrir {path}: {ctypes.FormatError(err)}")
    return handle


def close_handle(handle: int) -> None:
    _load_kernel32().CloseHandle(handle)


def send_cdb(
    handle: int,
    *,
    path_id: int,
    target_id: int,
    lun: int,
    cdb: bytes,
    data_in_length: int = 0,
    data_out: bytes = b"",
    timeout_seconds: int = DEFAULT_TIMEOUT_SECONDS,
) -> PassThroughResult:
    """Envoie un CDB SCSI via IOCTL_SCSI_PASS_THROUGH_DIRECT.

    Le data buffer est aligné sur 512 octets (DMA-friendly, évite le bounce
    storport). Sur ERROR_IO_DEVICE (slave busy), retry jusqu'à 5 fois avec
    délai progressif (200ms à 1s).
    """
    if data_in_length and data_out:
        raise ValueError("send_cdb: pas de CDB bidirectionnel — choisis data_in_length OU data_out.")
    if not 1 <= len(cdb) <= 16:
        raise ValueError(f"CDB doit faire 1..16 octets, reçu {len(cdb)}.")

    raw_buffer = None
    data_address = 0
    if data_in_length:
        direction = SCSI_IOCTL_DATA_IN
        raw_buffer, data_address = _aligned_buffer(data_in_length)
        transfer_length = data_in_length
    elif data_out:
        direction = SCSI_IOCTL_DATA_OUT
        raw_buffer, data_address = _aligned_buffer(len(data_out))
        ctypes.memmove(data_address, bytes(data_out), len(data_out))
        transfer_length = len(data_out)
    else:
        direction = SCSI_IOCTL_DATA_UNSPECIFIED
        transfer_length = 0

    sptd = _SptdWithSense()
    sptd.spt.Length = ctypes.sizeof(SCSI_PASS_THROUGH_DIRECT)
    sptd.spt.PathId = path_id
    sptd.spt.TargetId = target_id
    sptd.spt.Lun = lun
    sptd.spt.CdbLength = len(cdb)
    sptd.spt.SenseInfoLength = SENSE_BUFFER_LENGTH
    sptd.spt.DataIn = direction
    sptd.spt.DataTransferLength = transfer_length
    sptd.spt.TimeOutValue = timeout_seconds
    sptd.spt.DataBuffer = data_address if raw_buffer is not None else 0
    sptd.spt.SenseInfoOffset = ctypes.sizeof(SCSI_PASS_THROUGH_DIRECT)

    cdb_padded = bytes(cdb).ljust(16, b"\x00")
    ctypes.memmove(sptd.spt.Cdb, cdb_padded, 16)

    k = _load_kernel32()
    returned = wintypes.DWORD(0)
    err = 0
    ok = False
    for io_attempt in range(5):
        ok = k.DeviceIoControl(
            handle,
            IOCTL_SCSI_PASS_THROUGH_DIRECT,
            ctypes.byref(sptd),
            ctypes.sizeof(sptd),
            ctypes.byref(sptd),
            ctypes.sizeof(sptd),
            ctypes.byref(returned),
            None,
        )
        if ok:
            break
        err = ctypes.get_last_error()
        if err != ERROR_IO_DEVICE:
            break
        time.sleep(0.2 * (io_attempt + 1))

    if not ok:
        raise OSError(err, f"IOCTL_SCSI_PASS_THROUGH_DIRECT a échoué: {ctypes.FormatError(err)}")

    actual = sptd.spt.DataTransferLength
    if direction == SCSI_IOCTL_DATA_IN and raw_buffer is not None:
        data = ctypes.string_at(data_address, min(actual, transfer_length))
    else:
        data = b""

    return PassThroughResult(
        scsi_status=sptd.spt.ScsiStatus,
        data=data,
        sense=bytes(sptd.Sense),
        transferred=actual,
    )
