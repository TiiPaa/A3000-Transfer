import os
import sys
from pathlib import Path


def _setup_numba_cache() -> None:
    """Redirige le cache JIT de numba vers un dossier user persistant.

    Sans ça, dans un bundle PyInstaller, le cache est tenté à côté des sources
    librosa qui sont read-only → numba refait toute la compile à CHAQUE
    lancement (~20-30 s). Avec un cache stable :
    - 1er lancement : compile lente une fois, écrit le cache
    - tous les suivants : lit le cache, démarrage instantané
    """
    if sys.platform == "win32":
        base = Path(os.environ.get("APPDATA") or Path.home())
    else:
        base = Path.home() / ".cache"
    cache_dir = base / "a3000_transfer" / "numba_cache"
    try:
        cache_dir.mkdir(parents=True, exist_ok=True)
    except OSError:
        return
    os.environ.setdefault("NUMBA_CACHE_DIR", str(cache_dir))


# Doit être appelé AVANT tout import qui touche numba/librosa
_setup_numba_cache()


def _route() -> int:
    if "--worker" in sys.argv:
        sys.argv.remove("--worker")
        from a3000_transfer._worker import main as worker_main
        return worker_main()
    if len(sys.argv) <= 1:
        sys.argv.append("gui")
    from a3000_transfer.cli import main as cli_main
    return cli_main()


raise SystemExit(_route())
