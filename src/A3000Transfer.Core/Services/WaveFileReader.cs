using System.Text;
using A3000Transfer.Core.Models;

namespace A3000Transfer.Core.Services;

public sealed class WaveFileReader : IWaveReader
{
    public WaveSample Read(string path)
    {
        using var stream = File.OpenRead(path);
        using var reader = new BinaryReader(stream, Encoding.ASCII, leaveOpen: false);

        var riff = new string(reader.ReadChars(4));
        if (riff != "RIFF") throw new InvalidDataException("Fichier WAV invalide: en-tête RIFF manquant.");

        _ = reader.ReadInt32();

        var wave = new string(reader.ReadChars(4));
        if (wave != "WAVE") throw new InvalidDataException("Fichier WAV invalide: signature WAVE manquante.");

        short audioFormat = 0;
        short channels = 0;
        int sampleRate = 0;
        short bitsPerSample = 0;
        byte[]? pcmData = null;

        while (stream.Position < stream.Length)
        {
            var chunkId = new string(reader.ReadChars(4));
            var chunkSize = reader.ReadInt32();

            switch (chunkId)
            {
                case "fmt ":
                    audioFormat = reader.ReadInt16();
                    channels = reader.ReadInt16();
                    sampleRate = reader.ReadInt32();
                    _ = reader.ReadInt32();
                    _ = reader.ReadInt16();
                    bitsPerSample = reader.ReadInt16();

                    if (chunkSize > 16)
                    {
                        reader.ReadBytes(chunkSize - 16);
                    }
                    break;

                case "data":
                    pcmData = reader.ReadBytes(chunkSize);
                    break;

                default:
                    reader.ReadBytes(chunkSize);
                    break;
            }
        }

        if (audioFormat != 1) throw new NotSupportedException("Seuls les WAV PCM non compressés sont supportés dans ce MVP.");
        if (pcmData is null) throw new InvalidDataException("Chunk data introuvable dans le WAV.");
        if (channels <= 0 || sampleRate <= 0 || bitsPerSample <= 0) throw new InvalidDataException("Métadonnées WAV invalides.");

        var bytesPerSample = (bitsPerSample / 8) * channels;
        var sampleCount = bytesPerSample > 0 ? pcmData.Length / bytesPerSample : 0;

        return new WaveSample(path, channels, sampleRate, bitsPerSample, pcmData, sampleCount);
    }
}
