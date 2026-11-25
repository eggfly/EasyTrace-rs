#!/usr/bin/env python3
"""Package the JLCEDA Pro (EasyEDA Pro) local extension into `dist/easytrace-jlceda.eext`.

Zero external Python dependencies: uses stdlib `zlib`, `struct`, and `zipfile` to generate
`images/logo.png` (if missing) and build `dist/easytrace-jlceda.eext`.
"""
import os
import struct
import zipfile
import zlib

EXT_DIR = os.path.dirname(os.path.abspath(__file__))
REPO_ROOT = os.path.abspath(os.path.join(EXT_DIR, "..", ".."))


def make_logo_png(path: str, w: int = 128, h: int = 128) -> None:
    """Render a crisp PCB trace + via icon as a valid RGBA PNG using pure Python stdlib."""
    os.makedirs(os.path.dirname(path), exist_ok=True)
    rows = []
    for y in range(h):
        row = bytearray([0])  # filter type 0 (None)
        for x in range(w):
            # Rounded dark-emerald PCB background
            dx = min(x, w - 1 - x)
            dy = min(y, h - 1 - y)
            in_corner = dx < 18 and dy < 18 and ((18 - dx) ** 2 + (18 - dy) ** 2 > 18 ** 2)
            if in_corner:
                row.extend((0, 0, 0, 0))
                continue

            r, g, b, a = 18, 46, 38, 255
            # Subtle grid dots
            if x % 16 == 8 and y % 16 == 8:
                r, g, b = 26, 66, 54

            # Octilinear copper trace #1 (gold/amber)
            d1 = min(
                abs(y - 34) if 24 <= x <= 56 else 999,
                abs((x - 56) - (y - 34)) / 1.4142 if 56 <= x <= 84 and 34 <= y <= 62 else 999,
                abs(x - 84) if 62 <= y <= 96 else 999,
            )
            # Octilinear copper trace #2 (cyan/blue)
            d2 = min(
                abs(y - 94) if 28 <= x <= 52 else 999,
                abs((x - 52) + (y - 94)) / 1.4142 if 52 <= x <= 78 and 68 <= y <= 94 else 999,
                abs(y - 68) if 78 <= x <= 104 else 999,
            )
            if d1 <= 4.2:
                r, g, b = 245, 178, 52
            elif d2 <= 4.2:
                r, g, b = 76, 175, 255

            # Via pads & drill holes at endpoints
            for vx, vy in ((26, 34), (84, 96), (28, 94), (104, 68)):
                dist = ((x - vx) ** 2 + (y - vy) ** 2) ** 0.5
                if dist <= 4.2:
                    r, g, b = 18, 22, 26
                elif dist <= 9.2:
                    r, g, b = 236, 201, 105

            row.extend((r, g, b, a))
        rows.append(bytes(row))

    raw = b"".join(rows)

    def chunk(tag: bytes, data: bytes) -> bytes:
        return (
            struct.pack(">I", len(data))
            + tag
            + data
            + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
        )

    ihdr = struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0)
    png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b"")
    with open(path, "wb") as f:
        f.write(png)


def build() -> None:
    logo_path = os.path.join(EXT_DIR, "images", "logo.png")
    if not os.path.exists(logo_path):
        make_logo_png(logo_path)

    targets = [
        os.path.join(REPO_ROOT, "dist", "easytrace-jlceda.eext"),
        os.path.join(EXT_DIR, "dist", "easytrace-jlceda.eext"),
    ]
    files_to_pack = [
        ("extension.json", os.path.join(EXT_DIR, "extension.json")),
        ("dist/index.js", os.path.join(EXT_DIR, "dist", "index.js")),
        ("iframe/index.html", os.path.join(EXT_DIR, "iframe", "index.html")),
        ("images/logo.png", logo_path),
        ("LICENSE", os.path.join(REPO_ROOT, "LICENSE")),
    ]

    for out_eext in targets:
        os.makedirs(os.path.dirname(out_eext), exist_ok=True)
        with zipfile.ZipFile(out_eext, "w", compression=zipfile.ZIP_DEFLATED) as zf:
            for arcname, fullpath in files_to_pack:
                if os.path.exists(fullpath):
                    zf.write(fullpath, arcname)
        print(f"Built JLCEDA Pro extension: {out_eext} ({os.path.getsize(out_eext)} bytes)")


if __name__ == "__main__":
    build()
