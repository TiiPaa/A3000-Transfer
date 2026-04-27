using A3000Transfer.Core.Models;

namespace A3000Transfer.Core.Services;

public interface IWaveReader
{
    WaveSample Read(string path);
}
