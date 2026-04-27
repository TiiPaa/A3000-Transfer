using A3000Transfer.Core.Models;
using A3000Transfer.Core.Services;
using A3000Transfer.Windows.Interop;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Text;

namespace A3000Transfer.Windows.Scsi;

public sealed class SptiScsiTransport : IScsiTransport
{
    private const int MaxHostAdaptersToProbe = 16;
    private const int InitialOutputBufferSize = 16 * 1024;
    private const int MaximumOutputBufferSize = 256 * 1024;

    public IReadOnlyList<ScsiTargetInfo> ScanTargets()
    {
        var results = new List<ScsiTargetInfo>();
        var accessDenied = false;
        var adapterSeen = false;

        for (var hostAdapter = 0; hostAdapter < MaxHostAdaptersToProbe; hostAdapter++)
        {
            var devicePath = $"\\\\.\\Scsi{hostAdapter}:";
            using var handle = SptiNative.CreateFile(
                devicePath,
                SptiNative.GenericRead | SptiNative.GenericWrite,
                SptiNative.FileShareRead | SptiNative.FileShareWrite,
                IntPtr.Zero,
                SptiNative.OpenExisting,
                0,
                IntPtr.Zero);

            if (handle.IsInvalid)
            {
                var error = Marshal.GetLastWin32Error();
                if (error == 5)
                {
                    accessDenied = true;
                }
                continue;
            }

            adapterSeen = true;

            var inquiryBuffer = QueryInquiryData(handle, devicePath);
            results.AddRange(ParseInquiryData(hostAdapter, inquiryBuffer));
        }

        if (!adapterSeen && accessDenied)
        {
            throw new InvalidOperationException("Accès refusé aux adaptateurs SCSI. Lance l’app en administrateur.");
        }

        return results;
    }

    public Task<bool> SendSampleAsync(ScsiTargetInfo target, WaveSample sample, CancellationToken cancellationToken = default)
    {
        cancellationToken.ThrowIfCancellationRequested();
        _ = target;
        _ = sample;
        return Task.FromResult(false);
    }

    private static byte[] QueryInquiryData(Microsoft.Win32.SafeHandles.SafeFileHandle handle, string devicePath)
    {
        var bufferSize = InitialOutputBufferSize;

        while (bufferSize <= MaximumOutputBufferSize)
        {
            var bufferPtr = Marshal.AllocHGlobal(bufferSize);
            try
            {
                if (SptiNative.DeviceIoControl(
                        handle,
                        SptiNative.IoctlScsiGetInquiryData,
                        IntPtr.Zero,
                        0,
                        bufferPtr,
                        (uint)bufferSize,
                        out var bytesReturned,
                        IntPtr.Zero))
                {
                    var managed = new byte[bytesReturned];
                    Marshal.Copy(bufferPtr, managed, 0, (int)bytesReturned);
                    return managed;
                }

                var error = Marshal.GetLastWin32Error();
                if (error is 122 or 234)
                {
                    bufferSize *= 2;
                    continue;
                }

                throw new InvalidOperationException($"Échec de IOCTL_SCSI_GET_INQUIRY_DATA sur {devicePath}: {new Win32Exception(error).Message} (code {error}).");
            }
            finally
            {
                Marshal.FreeHGlobal(bufferPtr);
            }
        }

        throw new InvalidOperationException($"Le buffer d’inquiry SCSI requis dépasse {MaximumOutputBufferSize} octets sur {devicePath}.");
    }

    private static IReadOnlyList<ScsiTargetInfo> ParseInquiryData(int hostAdapter, byte[] buffer)
    {
        var results = new List<ScsiTargetInfo>();
        if (buffer.Length < Marshal.SizeOf<ScsiAdapterBusInfoHeader>())
        {
            return results;
        }

        var gcHandle = GCHandle.Alloc(buffer, GCHandleType.Pinned);
        try
        {
            var basePtr = gcHandle.AddrOfPinnedObject();
            var header = Marshal.PtrToStructure<ScsiAdapterBusInfoHeader>(basePtr);
            var busDataOffset = Marshal.SizeOf<ScsiAdapterBusInfoHeader>();
            var busDataSize = Marshal.SizeOf<ScsiBusData>();
            var inquiryHeaderSize = Marshal.SizeOf<ScsiInquiryDataHeader>();

            for (var busIndex = 0; busIndex < header.NumberOfBuses; busIndex++)
            {
                var busPtr = IntPtr.Add(basePtr, busDataOffset + (busIndex * busDataSize));
                if (!HasRoom(buffer, busDataOffset + (busIndex * busDataSize), busDataSize))
                {
                    break;
                }

                var busData = Marshal.PtrToStructure<ScsiBusData>(busPtr);
                if (busData.InquiryDataOffset == 0 || busData.InquiryDataOffset >= buffer.Length)
                {
                    continue;
                }

                var inquiryOffset = (int)busData.InquiryDataOffset;
                while (inquiryOffset > 0 && HasRoom(buffer, inquiryOffset, inquiryHeaderSize))
                {
                    var inquiryPtr = IntPtr.Add(basePtr, inquiryOffset);
                    var inquiryHeader = Marshal.PtrToStructure<ScsiInquiryDataHeader>(inquiryPtr);

                    var inquiryDataOffset = inquiryOffset + inquiryHeaderSize;
                    var maxReadable = Math.Max(0, buffer.Length - inquiryDataOffset);
                    var inquiryDataLength = (int)Math.Min(inquiryHeader.InquiryDataLength, (uint)maxReadable);
                    if (inquiryDataLength <= 0)
                    {
                        break;
                    }

                    var inquiryData = new byte[inquiryDataLength];
                    Marshal.Copy(IntPtr.Add(basePtr, inquiryDataOffset), inquiryData, 0, inquiryDataLength);

                    var deviceType = inquiryData[0] & 0x1F;
                    var vendor = ReadAscii(inquiryData, 8, 8);
                    var product = ReadAscii(inquiryData, 16, 16);
                    var revision = ReadAscii(inquiryData, 32, 4);

                    results.Add(new ScsiTargetInfo(
                        hostAdapter,
                        inquiryHeader.TargetId,
                        inquiryHeader.Lun,
                        string.IsNullOrWhiteSpace(vendor) ? "Unknown" : vendor,
                        string.IsNullOrWhiteSpace(product) ? MapDeviceType(deviceType) : product,
                        revision));

                    if (inquiryHeader.NextInquiryDataOffset == 0 || inquiryHeader.NextInquiryDataOffset <= inquiryOffset)
                    {
                        break;
                    }

                    inquiryOffset = (int)inquiryHeader.NextInquiryDataOffset;
                }
            }
        }
        finally
        {
            gcHandle.Free();
        }

        return results;
    }

    private static bool HasRoom(byte[] buffer, int offset, int size)
    {
        return offset >= 0 && size >= 0 && offset + size <= buffer.Length;
    }

    private static string ReadAscii(byte[] buffer, int offset, int length)
    {
        if (offset < 0 || offset >= buffer.Length)
        {
            return string.Empty;
        }

        var safeLength = Math.Min(length, buffer.Length - offset);
        return Encoding.ASCII.GetString(buffer, offset, safeLength).Trim(' ', '\0');
    }

    private static string MapDeviceType(byte deviceType) => deviceType switch
    {
        0x00 => "Direct-Access",
        0x01 => "Sequential",
        0x02 => "Printer",
        0x03 => "Processor",
        0x04 => "WORM",
        0x05 => "CD-ROM",
        0x06 => "Scanner",
        0x07 => "Optical",
        0x08 => "Changer",
        0x09 => "Communications",
        _ => $"Type-0x{deviceType:X2}"
    };
}
