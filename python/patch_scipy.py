"""Post-build patch : workaround pour bug scipy + PyInstaller.

`scipy/stats/_distn_infrastructure.py` plante au load dans le bundle frozen :
`NameError: name 'obj' is not defined`. Ça vient de `dir()` qui se comporte
différemment dans une list-comp quand le module est chargé par le loader
PyInstaller. Le fix : wrap le `del obj` final dans try/except.

Comme le .pyc bundlé est hash-based (check-source unset), Python ne recompile
pas depuis le .py. Donc on doit patcher le .py ET regénérer le .pyc.
"""
from __future__ import annotations

import py_compile
import sys
from pathlib import Path

DIST = Path(__file__).parent / "dist" / "A3000Transfer" / "_internal"
TARGET = DIST / "scipy" / "stats" / "_distn_infrastructure.py"

OLD = """for obj in [s for s in dir() if s.startswith('_doc_')]:
    exec('del ' + obj)
del obj
"""

NEW = """for obj in [s for s in dir() if s.startswith('_doc_')]:
    exec('del ' + obj)
try:
    del obj
except NameError:
    pass
"""


def main() -> int:
    if not TARGET.exists():
        print(f"[patch_scipy] NOT FOUND: {TARGET}", file=sys.stderr)
        return 1
    src = TARGET.read_text(encoding="utf-8")
    if OLD in src:
        src = src.replace(OLD, NEW)
        TARGET.write_text(src, encoding="utf-8")
        print(f"[patch_scipy] Patched {TARGET}")
    else:
        print(f"[patch_scipy] Pattern not found, skipping (scipy version may have changed)")

    pyc = TARGET.with_suffix(".pyc")
    py_compile.compile(str(TARGET), cfile=str(pyc), doraise=True)
    print(f"[patch_scipy] Recompiled {pyc}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
