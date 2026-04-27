using A3000Transfer.Core.Services;
using A3000Transfer.Windows.Commands;
using A3000Transfer.Windows.Scsi;

var cli = CommandLine.Parse(args);
var service = new SampleTransferService(new WaveFileReader(), new SptiScsiTransport());

try
{
    switch (cli.Command)
    {
        case "scan":
        {
            var targets = service.ScanTargets();
            if (targets.Count == 0)
            {
                Console.WriteLine("Aucune cible SCSI détectée.");
                return;
            }

            foreach (var target in targets)
            {
                Console.WriteLine(target.DisplayName);
            }
            break;
        }

        case "send":
        {
            if (string.IsNullOrWhiteSpace(cli.WavePath) || cli.TargetId is null || cli.HostAdapter is null)
            {
                Console.WriteLine("Usage: A3000Transfer.Windows send --wave <fichier.wav> --ha <hostAdapter> --target <id>");
                return;
            }

            var target = service
                .ScanTargets()
                .FirstOrDefault(t => t.HostAdapter == cli.HostAdapter && t.TargetId == cli.TargetId);

            if (target is null)
            {
                Console.WriteLine("Cible SCSI introuvable.");
                return;
            }

            var result = await service.SendWaveAsync(cli.WavePath, target);
            Console.WriteLine(result ? "Transfert réussi." : "Transfert non exécuté ou non finalisé dans ce MVP.");
            break;
        }

        default:
            Console.WriteLine("A3000 Transfer MVP");
            Console.WriteLine("  scan");
            Console.WriteLine("  send --wave <fichier.wav> --ha <hostAdapter> --target <id>");
            break;
    }
}
catch (Exception ex)
{
    Console.Error.WriteLine($"Erreur: {ex.Message}");
}
