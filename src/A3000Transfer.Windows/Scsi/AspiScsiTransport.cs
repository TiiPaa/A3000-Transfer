using A3000Transfer.Core.Models;
using A3000Transfer.Core.Services;
using A3000Transfer.Windows.Interop;
using System.Runtime.InteropServices;
using System.Text;

namespace A3000Transfer.Windows.Scsi;

public sealed class AspiScsiTransport : IScsiTransport
{
    private const int MaxTargetsPerAdapter = 16;
    private const int InquiryResponseLength = 36;
    private const byte SenseLength = 14;

    public IReadOnlyList<ScsiTargetInfo> ScanTargets()
    {
        var (status, hostAdapterCount) = GetSupportInfo();
        if (status == AspiStatus.NoAdapters || hostAdapterCount == 0)
        {
            return Array.Empty<ScsiTargetInfo>();
        }

        if (status != AspiStatus.Completed)
        {
            throw new InvalidOperationException($"ASPI indisponible ou mal initialisé (status 0x{status:X2}). Vérifie WNASPI32.DLL et la pile SCSI Windows.");
        }

        var results = new List<ScsiTargetInfo>();

        for (var hostAdapter = 0; hostAdapter < hostAdapterCount; hostAdapter++)
        {
            var adapterInfo = QueryHostAdapter((byte)hostAdapter);

            for (byte targetId = 0; targetId < MaxTargetsPerAdapter; targetId++)
            {
                if (targetId == adapterInfo.HA_SCSI_ID)
                {
                    continue;
                }

                const byte lun = 0;
                var deviceType = TryGetDeviceType((byte)hostAdapter, targetId, lun, out var devicePresent);
                if (!devicePresent)
                {
                    continue;
                }

                var inquiry = TryStandardInquiry((byte)hostAdapter, targetId, lun);

                results.Add(new ScsiTargetInfo(
                    hostAdapter,
                    targetId,
                    lun,
                    inquiry?.Vendor ?? "Unknown",
                    inquiry?.Product ?? MapDeviceType(deviceType),
                    inquiry?.Revision ?? string.Empty));
            }
        }

        return results;
    }

    public Task<bool> SendSampleAsync(ScsiTargetInfo target, WaveSample sample, CancellationToken cancellationToken = default)
    {
        cancellationToken.ThrowIfCancellationRequested();

        // TODO:
        // 1. effectuer une séquence d’identification INQUIRY
        // 2. négocier ou vérifier la compatibilité SMDI
        // 3. pousser le PCM au format attendu par le sampler
        // 4. confirmer l’écriture côté device
        _ = target;
        _ = sample;
        return Task.FromResult(false);
    }

    private static (byte Status, int HostAdapterCount) GetSupportInfo()
    {
        try
        {
            var supportInfo = AspiNative.GetASPI32SupportInfo();
            var status = (byte)((supportInfo >> 8) & 0xFF);
            var hostAdapterCount = (byte)(supportInfo & 0xFF);
            return (status, hostAdapterCount);
        }
        catch (DllNotFoundException ex)
        {
            throw new InvalidOperationException("WNASPI32.DLL introuvable. Il faut une couche ASPI accessible par l’application.", ex);
        }
        catch (EntryPointNotFoundException ex)
        {
            throw new InvalidOperationException("WNASPI32.DLL est présente mais les fonctions ASPI attendues sont introuvables.", ex);
        }
    }

    private static SrbHaInquiry QueryHostAdapter(byte hostAdapter)
    {
        var srb = new SrbHaInquiry
        {
            SRB_Cmd = AspiCommandCode.HaInquiry,
            SRB_HaId = hostAdapter,
            HA_ManagerId = new byte[16],
            HA_Identifier = new byte[16],
            HA_Unique = new byte[16]
        };

        SendCommand(ref srb);

        if (srb.SRB_Status != AspiStatus.Completed)
        {
            throw new InvalidOperationException($"Échec de l’interrogation de l’host adapter {hostAdapter} (status 0x{srb.SRB_Status:X2}).");
        }

        return srb;
    }

    private static byte TryGetDeviceType(byte hostAdapter, byte targetId, byte lun, out bool devicePresent)
    {
        var srb = new SrbGdevBlock
        {
            SRB_Cmd = AspiCommandCode.GetDeviceType,
            SRB_HaId = hostAdapter,
            SRB_Target = targetId,
            SRB_Lun = lun
        };

        SendCommand(ref srb);

        if (srb.SRB_Status == AspiStatus.NoDevice)
        {
            devicePresent = false;
            return ScsiPeripheralDeviceType.Unknown;
        }

        if (srb.SRB_Status != AspiStatus.Completed)
        {
            devicePresent = false;
            return ScsiPeripheralDeviceType.Unknown;
        }

        devicePresent = srb.SRB_DeviceType != ScsiPeripheralDeviceType.Unknown;
        return srb.SRB_DeviceType;
    }

    private static InquiryIdentity? TryStandardInquiry(byte hostAdapter, byte targetId, byte lun)
    {
        var buffer = new byte[InquiryResponseLength];
        var srb = new SrbExecScsiCmd
        {
            SRB_Cmd = AspiCommandCode.ExecuteScsiCommand,
            SRB_HaId = hostAdapter,
            SRB_Flags = AspiFlags.DirectionIn,
            SRB_Target = targetId,
            SRB_Lun = lun,
            SRB_BufLen = (uint)buffer.Length,
            SRB_SenseLen = SenseLength,
            SRB_CDBLen = 6,
            SRB_Rsvd2 = new byte[20],
            CDBByte = new byte[16],
            SenseArea = new byte[16]
        };

        srb.CDBByte[0] = ScsiOpCode.Inquiry;
        srb.CDBByte[4] = InquiryResponseLength;

        SendCommandWithBuffer(ref srb, buffer);

        if (srb.SRB_Status != AspiStatus.Completed || srb.SRB_HaStat != ScsiHostStatus.Ok || srb.SRB_TargStat != 0)
        {
            return null;
        }

        if (buffer.Length < 36)
        {
            return null;
        }

        return new InquiryIdentity(
            TrimAscii(buffer, 8, 8),
            TrimAscii(buffer, 16, 16),
            TrimAscii(buffer, 32, 4));
    }

    private static string MapDeviceType(byte deviceType) => deviceType switch
    {
        ScsiPeripheralDeviceType.DirectAccess => "Direct-Access",
        ScsiPeripheralDeviceType.Sequential => "Sequential",
        ScsiPeripheralDeviceType.Printer => "Printer",
        ScsiPeripheralDeviceType.Processor => "Processor",
        ScsiPeripheralDeviceType.WriteOnce => "WORM",
        ScsiPeripheralDeviceType.CdRom => "CD-ROM",
        ScsiPeripheralDeviceType.Scanner => "Scanner",
        ScsiPeripheralDeviceType.Optical => "Optical",
        ScsiPeripheralDeviceType.MediumChanger => "Changer",
        ScsiPeripheralDeviceType.Communications => "Communications",
        _ => $"Type-0x{deviceType:X2}"
    };

    private static string TrimAscii(byte[] buffer, int offset, int length)
    {
        return Encoding.ASCII.GetString(buffer, offset, length).Trim(' ', '\0');
    }

    private static void SendCommand<T>(ref T srb) where T : struct
    {
        var size = Marshal.SizeOf<T>();
        var ptr = Marshal.AllocHGlobal(size);

        try
        {
            Marshal.StructureToPtr(srb, ptr, fDeleteOld: false);
            _ = AspiNative.SendASPI32Command(ptr);
            srb = Marshal.PtrToStructure<T>(ptr);
        }
        finally
        {
            Marshal.FreeHGlobal(ptr);
        }
    }

    private static void SendCommandWithBuffer(ref SrbExecScsiCmd srb, byte[] buffer)
    {
        var srbPtr = Marshal.AllocHGlobal(Marshal.SizeOf<SrbExecScsiCmd>());
        var bufferPtr = Marshal.AllocHGlobal(buffer.Length);

        try
        {
            Marshal.Copy(buffer, 0, bufferPtr, buffer.Length);
            srb.SRB_BufPointer = bufferPtr;

            Marshal.StructureToPtr(srb, srbPtr, fDeleteOld: false);
            _ = AspiNative.SendASPI32Command(srbPtr);
            srb = Marshal.PtrToStructure<SrbExecScsiCmd>(srbPtr);

            if (srb.SRB_BufLen > 0)
            {
                var bytesToCopy = (int)Math.Min((uint)buffer.Length, srb.SRB_BufLen);
                Marshal.Copy(bufferPtr, buffer, 0, bytesToCopy);
            }
        }
        finally
        {
            Marshal.FreeHGlobal(bufferPtr);
            Marshal.FreeHGlobal(srbPtr);
        }
    }

    private sealed record InquiryIdentity(string Vendor, string Product, string Revision);
}
