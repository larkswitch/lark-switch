#!/usr/bin/env python3
"""Generate dmg-background.png from the SVG source (660×400, Tauri default DMG size).

Uses Pillow for a reproducible PNG when no SVG renderer is available.
Run from repo root: python scripts/generate-dmg-background.py
"""

from __future__ import annotations

from pathlib import Path

try:
    from PIL import Image, ImageDraw, ImageFont
except ImportError as exc:  # pragma: no cover
    raise SystemExit(
        "Pillow is required. Install with: pip install pillow"
    ) from exc

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "apps/desktop/src-tauri/icons/dmg-background.png"
SVG = ROOT / "apps/desktop/src-tauri/icons/dmg-background.svg"

W, H = 660, 400
BAND_TOP = 248


def _font(size: int, mono: bool = False) -> ImageFont.FreeTypeFont | ImageFont.ImageFont:
    candidates = (
        [
            "C:/Windows/Fonts/consola.ttf",
            "C:/Windows/Fonts/cour.ttf",
            "/System/Library/Fonts/Menlo.ttc",
            "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
        ]
        if mono
        else [
            "C:/Windows/Fonts/msyh.ttc",
            "C:/Windows/Fonts/msyhbd.ttc",
            "C:/Windows/Fonts/segoeui.ttf",
            "/System/Library/Fonts/PingFang.ttc",
            "/System/Library/Fonts/Helvetica.ttc",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        ]
    )
    for path in candidates:
        if Path(path).exists():
            try:
                return ImageFont.truetype(path, size=size)
            except OSError:
                continue
    return ImageFont.load_default()


def _gradient(img: Image.Image, top: tuple[int, int, int], bottom: tuple[int, int, int]) -> None:
    draw = ImageDraw.Draw(img)
    for y in range(H):
        t = y / (H - 1)
        color = tuple(int(top[i] + (bottom[i] - top[i]) * t) for i in range(3))
        draw.line([(0, y), (W, y)], fill=color)


def _center_text(
    draw: ImageDraw.ImageDraw,
    y: int,
    text: str,
    font: ImageFont.ImageFont,
    fill: tuple[int, int, int],
) -> None:
    bbox = draw.textbbox((0, 0), text, font=font)
    tw = bbox[2] - bbox[0]
    draw.text(((W - tw) // 2, y), text, font=font, fill=fill)


def render() -> Image.Image:
    img = Image.new("RGB", (W, H), "#ffffff")
    _gradient(img, (247, 248, 250), (238, 241, 245))
    draw = ImageDraw.Draw(img)

    # Subtle arrow between icon zones
    arrow_y = 175
    draw.line([(330, arrow_y), (395, arrow_y)], fill=(107, 114, 128), width=3)
    draw.polygon([(382, 165), (395, arrow_y), (382, 185)], fill=(107, 114, 128))

    # Bottom band
    draw.rectangle([(0, BAND_TOP), (W, H)], fill=(232, 237, 243))
    draw.line([(0, BAND_TOP), (W, BAND_TOP)], fill=(200, 208, 218), width=1)

    title_font = _font(15)
    sub_font = _font(11)
    cmd_font = _font(13, mono=True)
    open_font = _font(11, mono=True)

    _center_text(
        draw,
        262,
        "拖进去之后如果提示「已损坏」，打开终端粘贴：",
        title_font,
        (31, 41, 55),
    )
    _center_text(
        draw,
        284,
        "If macOS says the app is damaged, paste in Terminal:",
        sub_font,
        (107, 114, 128),
    )

    cmd = "xattr -dr com.apple.quarantine /Applications/larkswitch.app"
    cmd_box = (24, 312, W - 24, 346)
    draw.rounded_rectangle(cmd_box, radius=6, fill=(255, 255, 255), outline=(200, 208, 218), width=1)
    _center_text(draw, 318, cmd, cmd_font, (17, 24, 39))
    _center_text(draw, 354, "open /Applications/larkswitch.app", open_font, (75, 85, 99))

    return img


def main() -> None:
    if not SVG.exists():
        raise SystemExit(f"Missing SVG source: {SVG}")
    OUT.parent.mkdir(parents=True, exist_ok=True)
    img = render()
    img.save(OUT, format="PNG", optimize=True)
    print(f"Wrote {OUT.relative_to(ROOT)} ({W}x{H})")


if __name__ == "__main__":
    main()
