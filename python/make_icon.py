"""Génère assets/icon.ico — waveform 8-bit master 16×16, NEAREST upscale.

Un seul master à résolution native 16×16 ; toutes les autres tailles sont
obtenues par scaling NEAREST (pas d'interpolation, pas de lissage).
À 256px chaque pixel source devient un bloc 16×16 → look chiptune chunky.
"""
from __future__ import annotations

from pathlib import Path

from PIL import Image, ImageDraw


WAVE_GREEN = (180, 255, 90, 255)


# 32 hauteurs pour 32 barres (résolution doublée vs 16)
# Valeurs entre 1 et 31 (h_max = 31 px au-dessus + 31 en-dessous de cy=32)
HEIGHTS = [
    8, 19, 13, 26, 11, 23, 30, 15, 22, 9, 28, 17, 13, 25, 31, 18,
    14, 27, 10, 22, 16, 29, 12, 20, 26, 9, 17, 30, 14, 24, 19, 11,
]


def make_master() -> Image.Image:
    """Master 64×64 pixel-perfect, fond transparent."""
    img = Image.new("RGBA", (64, 64), (0, 0, 0, 0))
    draw = ImageDraw.Draw(img)
    cy = 32
    # 32 barres × 2 px de large = 64 px → remplit toute la largeur
    bar_w = 2
    for i, h in enumerate(HEIGHTS):
        x = i * bar_w
        draw.rectangle(
            [x, cy - h, x + bar_w - 1, cy + h - 1],
            fill=WAVE_GREEN,
        )
    return img


def make_icon(size: int) -> Image.Image:
    """Master 64×64 NEAREST-scalé à la taille demandée."""
    master = make_master()
    if size == 64:
        return master
    return master.resize((size, size), Image.NEAREST)


def main() -> int:
    sizes = [16, 20, 24, 32, 40, 48, 56, 64, 96, 128, 256]
    images = [make_icon(s) for s in sizes]

    out_dir = Path(__file__).parent / "assets"
    out_dir.mkdir(exist_ok=True)
    images[-1].save(out_dir / "icon.png")
    images[-1].save(
        out_dir / "icon.ico",
        format="ICO",
        sizes=[(s, s) for s in sizes],
        append_images=images[:-1],
    )
    print(f"Icon written: {out_dir / 'icon.ico'}")
    print(f"Preview PNG : {out_dir / 'icon.png'}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
