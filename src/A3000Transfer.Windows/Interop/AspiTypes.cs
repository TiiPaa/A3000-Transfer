using System.Runtime.InteropServices;

namespace A3000Transfer.Windows.Interop;

internal static class AspiCommandCode
{
    public const byte HaInquiry = 0x00;
    public const byte GetDeviceType = 0x01;
    public const byte ExecuteScsiCommand = 0x02;
}

internal static class AspiStatus
{
    public const byte Pending = 0x00;
    public const byte Completed = 0x01;
    public const byte Error = 0x04;
    public const byte InvalidHostAdapter = 0x81;
    public const byte NoDevice = 0x82;
    public const byte NoAspi = 0xE3;
    public const byte FailedInit = 0xE4;
    public const byte NoAdapters = 0xE8;
}

internal static class AspiFlags
{
    public const byte DirectionIn = 0x08;
    public const byte DirectionOut = 0x10;
}

internal static class ScsiOpCode
{
    public const byte TestUnitReady = 0x00;
    public const byte Inquiry = 0x12;
}

internal static class ScsiPeripheralDeviceType
{
    public const byte DirectAccess = 0x00;
    public const byte Sequential = 0x01;
    public const byte Printer = 0x02;
    public const byte Processor = 0x03;
    public const byte WriteOnce = 0x04;
    public const byte CdRom = 0x05;
    public const byte Scanner = 0x06;
    public const byte Optical = 0x07;
    public const byte MediumChanger = 0x08;
    public const byte Communications = 0x09;
    public const byte Unknown = 0x1F;
}

internal static class ScsiHostStatus
{
    public const byte Ok = 0x00;
}

[StructLayout(LayoutKind.Sequential, Pack = 1)]
internal struct SrbHaInquiry
{
    public byte SRB_Cmd;
    public byte SRB_Status;
    public byte SRB_HaId;
    public byte SRB_Flags;
    public uint SRB_Hdr_Rsvd;
    public byte HA_Count;
    public byte HA_SCSI_ID;

    [MarshalAs(UnmanagedType.ByValArray, SizeConst = 16)]
    public byte[] HA_ManagerId;

    [MarshalAs(UnmanagedType.ByValArray, SizeConst = 16)]
    public byte[] HA_Identifier;

    [MarshalAs(UnmanagedType.ByValArray, SizeConst = 16)]
    public byte[] HA_Unique;

    public ushort HA_Rsvd1;
}

[StructLayout(LayoutKind.Sequential, Pack = 1)]
internal struct SrbGdevBlock
{
    public byte SRB_Cmd;
    public byte SRB_Status;
    public byte SRB_HaId;
    public byte SRB_Flags;
    public uint SRB_Hdr_Rsvd;
    public byte SRB_Target;
    public byte SRB_Lun;
    public byte SRB_DeviceType;
    public byte SRB_Rsvd1;
}

[StructLayout(LayoutKind.Sequential, Pack = 1)]
internal struct SrbExecScsiCmd
{
    public byte SRB_Cmd;
    public byte SRB_Status;
    public byte SRB_HaId;
    public byte SRB_Flags;
    public uint SRB_Hdr_Rsvd;
    public byte SRB_Target;
    public byte SRB_Lun;
    public ushort SRB_Rsvd1;
    public uint SRB_BufLen;
    public IntPtr SRB_BufPointer;
    public byte SRB_SenseLen;
    public byte SRB_CDBLen;
    public byte SRB_HaStat;
    public byte SRB_TargStat;
    public IntPtr SRB_PostProc;

    [MarshalAs(UnmanagedType.ByValArray, SizeConst = 20)]
    public byte[] SRB_Rsvd2;

    [MarshalAs(UnmanagedType.ByValArray, SizeConst = 16)]
    public byte[] CDBByte;

    [MarshalAs(UnmanagedType.ByValArray, SizeConst = 16)]
    public byte[] SenseArea;
}
