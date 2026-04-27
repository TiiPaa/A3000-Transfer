using System.Runtime.InteropServices;

namespace A3000Transfer.Windows.Interop;

[StructLayout(LayoutKind.Sequential)]
internal struct ScsiAdapterBusInfoHeader
{
    public byte NumberOfBuses;

    [MarshalAs(UnmanagedType.ByValArray, SizeConst = 3)]
    public byte[] Reserved;
}

[StructLayout(LayoutKind.Sequential)]
internal struct ScsiBusData
{
    public byte NumberOfLogicalUnits;
    public byte InitiatorBusId;
    public uint InquiryDataOffset;
}

[StructLayout(LayoutKind.Sequential)]
internal struct ScsiInquiryDataHeader
{
    public byte PathId;
    public byte TargetId;
    public byte Lun;
    public byte DeviceClaimed;
    public uint InquiryDataLength;
    public uint NextInquiryDataOffset;
}
