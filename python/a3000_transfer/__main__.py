import sys


def _route() -> int:
    if "--worker" in sys.argv:
        sys.argv.remove("--worker")
        from a3000_transfer._worker import main as worker_main
        return worker_main()
    # Sans sous-commande (double-clic sur le .exe), on lance la GUI par défaut
    if len(sys.argv) <= 1:
        sys.argv.append("gui")
    from a3000_transfer.cli import main as cli_main
    return cli_main()


raise SystemExit(_route())
