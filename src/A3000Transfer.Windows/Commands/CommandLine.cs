namespace A3000Transfer.Windows.Commands;

public sealed record CommandLine(
    string Command,
    string? WavePath,
    int? TargetId,
    int? HostAdapter
)
{
    public static CommandLine Parse(string[] args)
    {
        if (args.Length == 0)
        {
            return new CommandLine("help", null, null, null);
        }

        var command = args[0].ToLowerInvariant();
        string? wavePath = null;
        int? targetId = null;
        int? hostAdapter = null;

        for (var i = 1; i < args.Length; i++)
        {
            switch (args[i])
            {
                case "--wave" when i + 1 < args.Length:
                    wavePath = args[++i];
                    break;
                case "--target" when i + 1 < args.Length && int.TryParse(args[i + 1], out var parsedTarget):
                    targetId = parsedTarget;
                    i++;
                    break;
                case "--ha" when i + 1 < args.Length && int.TryParse(args[i + 1], out var parsedHostAdapter):
                    hostAdapter = parsedHostAdapter;
                    i++;
                    break;
            }
        }

        return new CommandLine(command, wavePath, targetId, hostAdapter);
    }
}
