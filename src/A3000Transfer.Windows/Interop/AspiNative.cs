using System.Runtime.InteropServices;

namespace A3000Transfer.Windows.Interop;

internal static class AspiNative
{
    private const string LibraryName = "WNASPI32.DLL";

    [DllImport(LibraryName, CharSet = CharSet.Ansi, ExactSpelling = true)]
    public static extern uint GetASPI32SupportInfo();

    [DllImport(LibraryName, CharSet = CharSet.Ansi, ExactSpelling = true)]
    public static extern uint SendASPI32Command(IntPtr srb);
}
