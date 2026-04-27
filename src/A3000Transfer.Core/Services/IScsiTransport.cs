using A3000Transfer.Core.Models;

namespace A3000Transfer.Core.Services;

public interface IScsiTransport
{
    IReadOnlyList<ScsiTargetInfo> ScanTargets();
    Task<bool> SendSampleAsync(ScsiTargetInfo target, WaveSample sample, CancellationToken cancellationToken = default);
}
