namespace A3000Transfer.Core.Models;

public sealed record WaveSample(
    string SourcePath,
    int Channels,
    int SampleRate,
    int BitsPerSample,
    byte[] PcmData,
    int SampleCount
);
