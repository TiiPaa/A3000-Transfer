using A3000Transfer.Core.Models;

namespace A3000Transfer.Core.Services;

public sealed class SampleTransferService
{
    private readonly IWaveReader _waveReader;
    private readonly IScsiTransport _scsiTransport;

    public SampleTransferService(IWaveReader waveReader, IScsiTransport scsiTransport)
    {
        _waveReader = waveReader;
        _scsiTransport = scsiTransport;
    }

    public IReadOnlyList<ScsiTargetInfo> ScanTargets() => _scsiTransport.ScanTargets();

    public async Task<bool> SendWaveAsync(string wavePath, ScsiTargetInfo target, CancellationToken cancellationToken = default)
    {
        var sample = _waveReader.Read(wavePath);
        ValidateForMvp(sample);
        return await _scsiTransport.SendSampleAsync(target, sample, cancellationToken);
    }

    private static void ValidateForMvp(WaveSample sample)
    {
        if (sample.BitsPerSample != 16)
        {
            throw new NotSupportedException("MVP: seuls les WAV 16 bits PCM sont acceptés pour l’instant.");
        }

        if (sample.Channels is < 1 or > 2)
        {
            throw new NotSupportedException("MVP: seuls les WAV mono ou stéréo sont acceptés.");
        }

        if (sample.SampleCount <= 0)
        {
            throw new InvalidDataException("Le fichier WAV ne contient pas d’échantillons exploitables.");
        }
    }
}
