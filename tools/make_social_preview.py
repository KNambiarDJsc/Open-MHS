"""Draw docs/assets/social-preview.png (1280x640), the image GitHub shows when the
repository is shared. Plain text on a dark background; no logo, no claims beyond the README's.

Usage: python tools/make_social_preview.py        (needs: pip install pillow)
The image is a repository asset: upload it by hand in Settings -> Social preview.
"""
import pathlib
import sys

from PIL import Image, ImageDraw, ImageFont

ROOT = pathlib.Path(__file__).resolve().parent.parent
OUT = ROOT / "docs" / "assets" / "social-preview.png"
W, H = 1280, 640

CANDIDATES = {
    "bold": ["C:/Windows/Fonts/segoeuib.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf", "/System/Library/Fonts/Helvetica.ttc"],
    "regular": ["C:/Windows/Fonts/segoeui.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", "/System/Library/Fonts/Helvetica.ttc"],
    "mono": ["C:/Windows/Fonts/consola.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf", "/System/Library/Fonts/Menlo.ttc"],
}


def font(kind, size):
    for path in CANDIDATES[kind]:
        if pathlib.Path(path).exists():
            return ImageFont.truetype(path, size)
    sys.exit(f"no {kind} font found; edit CANDIDATES")


img = Image.new("RGB", (W, H), "#0d1117")
d = ImageDraw.Draw(img)
d.rectangle([80, 150, 86, 150 + 230], fill="#58a6ff")
d.text((120, 140), "OpenMHS", font=font("bold", 120), fill="#f0f6fc")
d.text((124, 290), "Connect AI agents to programmable hardware.", font=font("regular", 40), fill="#c9d1d9")
d.text((124, 360), "Deny by default. Verified by readback. Audited.", font=font("regular", 30), fill="#8b949e")
d.text((124, 540), "open source  ·  Apache-2.0  ·  pre-alpha  ·  independent, not affiliated with Anthropic", font=font("mono", 22), fill="#8b949e")
img.save(OUT, optimize=True)
print(f"wrote {OUT} ({OUT.stat().st_size} bytes)")
