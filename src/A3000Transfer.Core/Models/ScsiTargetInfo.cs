namespace A3000Transfer.Core.Models;

public sealed record ScsiTargetInfo(
    int HostAdapter,
    int TargetId,
    int Lun,
    string Vendor,
    string Product,
    string Revision
)
{
    public string DisplayName => $"HA{HostAdapter} ID{TargetId} LUN{Lun} {Vendor} {Product} {Revision}".Trim();
}
