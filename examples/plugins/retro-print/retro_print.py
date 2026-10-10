#!/usr/bin/env python3
"""Retro Print: an example Photo·AIrt plug-in (contract version 1).

It uses only the Python standard library, so it runs anywhere Python 3 is
installed. Its name, description and settings live in plugin.toml; this
script only renders:

    retro_print.py render <request.json>    -> read input.png, write output.png

The request carries every setting declared in plugin.toml, already checked
against its range, options and default.

The effect posterises each colour channel and maps it through an ink tint,
using byte lookup tables so it stays fast even in pure Python. It is
deterministic and resolution independent by nature, so it ignores `seed` and
`scale`; a plug-in with randomness or size-like settings must use them.
"""

import json
import struct
import sys
import zlib

# Ink: (dark RGB, light RGB) that each channel's tones are mapped between.
INKS = {
    "natural": ((0, 0, 0), (255, 255, 255)),
    "sepia": ((44, 28, 14), (255, 241, 205)),
    "cyanotype": ((10, 32, 74), (222, 236, 250)),
    "riso-pink": ((70, 18, 62), (255, 232, 238)),
}


def say(**message):
    """Print one JSON Lines message for the host."""
    print(json.dumps(message), flush=True)


# ---------------------------------------------------------------- PNG I/O


def read_png(path):
    """Decode an 8-bit RGB (or RGBA) PNG into (width, height, rows of RGB bytes)."""
    with open(path, "rb") as f:
        data = f.read()
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise ValueError("input is not a PNG")
    pos, idat, width, height, ctype = 8, [], 0, 0, 2
    while pos < len(data):
        length, kind = struct.unpack(">I4s", data[pos : pos + 8])
        body = data[pos + 8 : pos + 8 + length]
        pos += 12 + length
        if kind == b"IHDR":
            width, height, depth, ctype, _, _, interlace = struct.unpack(">IIBBBBB", body)
            if depth != 8 or ctype not in (2, 6) or interlace:
                raise ValueError("expected a non-interlaced 8-bit RGB PNG")
        elif kind == b"IDAT":
            idat.append(body)
        elif kind == b"IEND":
            break
    bpp = 3 if ctype == 2 else 4
    stride = width * bpp
    raw = zlib.decompress(b"".join(idat))
    rows, prev = [], bytearray(stride)
    for y in range(height):
        start = y * (stride + 1)
        ftype, line = raw[start], bytearray(raw[start + 1 : start + 1 + stride])
        if ftype == 1:  # Sub
            for i in range(bpp, stride):
                line[i] = (line[i] + line[i - bpp]) & 255
        elif ftype == 2:  # Up
            for i in range(stride):
                line[i] = (line[i] + prev[i]) & 255
        elif ftype == 3:  # Average
            for i in range(stride):
                left = line[i - bpp] if i >= bpp else 0
                line[i] = (line[i] + ((left + prev[i]) >> 1)) & 255
        elif ftype == 4:  # Paeth
            for i in range(stride):
                a = line[i - bpp] if i >= bpp else 0
                b, c = prev[i], (prev[i - bpp] if i >= bpp else 0)
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                line[i] = (line[i] + (a if pa <= pb and pa <= pc else b if pb <= pc else c)) & 255
        prev = line
        if bpp == 4:  # drop alpha
            rgb = bytearray(width * 3)
            rgb[0::3], rgb[1::3], rgb[2::3] = line[0::4], line[1::4], line[2::4]
            line = rgb
        rows.append(bytes(line))
    return width, height, rows


def write_png(path, width, height, rows):
    """Encode rows of RGB bytes as an 8-bit RGB PNG."""

    def chunk(kind, body):
        return struct.pack(">I", len(body)) + kind + body + struct.pack(">I", zlib.crc32(kind + body) & 0xFFFFFFFF)

    raw = b"".join(b"\x00" + row for row in rows)
    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(raw, 6))
    png += chunk(b"IEND", b"")
    with open(path, "wb") as f:
        f.write(png)


# ---------------------------------------------------------------- effect


def tables(levels, invert, ink):
    """One 256-byte lookup table per channel."""
    dark, light = INKS.get(ink, INKS["natural"])
    step = 255 / (levels - 1)
    out = []
    for ch in range(3):
        t = bytearray(256)
        for v in range(256):
            q = round(round(v / step) * step)
            if invert:
                q = 255 - q
            t[v] = round(dark[ch] + (light[ch] - dark[ch]) * q / 255)
        out.append(bytes(t))
    return out


def render(request_path):
    with open(request_path, encoding="utf-8") as f:
        req = json.load(f)
    if req.get("protocol") != 1:
        raise ValueError(f"unsupported contract version {req.get('protocol')}")
    params = req["params"]
    levels = int(round(params["levels"]))
    tr, tg, tb = tables(levels, bool(params["invert"]), params["ink"])

    width, height, rows = read_png(req["input"])
    out, report_every = [], max(1, height // 10)
    for y, row in enumerate(rows):
        line = bytearray(row)
        line[0::3] = row[0::3].translate(tr)
        line[1::3] = row[1::3].translate(tg)
        line[2::3] = row[2::3].translate(tb)
        out.append(bytes(line))
        if y % report_every == 0:
            say(progress=round(y / height, 3))
    write_png(req["output"], width, height, out)
    say(progress=1.0)


def main(argv):
    try:
        if len(argv) == 3 and argv[1] == "render":
            render(argv[2])
        else:
            raise ValueError("usage: retro_print.py render <request.json>")
    except Exception as exc:  # report every failure to the host
        say(error=str(exc))
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
