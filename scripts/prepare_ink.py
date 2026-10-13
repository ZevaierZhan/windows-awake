"""Rebuild the embedded mask pack from the credited source, using Python + ffmpeg.

Usage: python prepare_ink.py path/to/source.mp4 path/to/ffmpeg.exe
Only used for asset development, never at build time or runtime.
"""
from pathlib import Path
import subprocess
import sys
import struct
import zlib

width, height, fps = 540, 960, 24
root = Path(__file__).resolve().parents[1]
process = subprocess.Popen([sys.argv[2], '-loglevel', 'error', '-i', sys.argv[1],
    '-vf', f'fps={fps},scale={width}:{height}', '-pix_fmt', 'gray',
    '-f', 'rawvideo', '-'], stdout=subprocess.PIPE)
# Pale water is excluded. Preserve fine density variation and soften the boundary.
lookup = bytes(round(255 * (p*p*(3-2*p))) for value in range(256)
    for p in [max(0, min(1, (155-value)/110))])
shade = bytes(0 if value >= 155 else (min(value, 96) // 8) * 8 for value in range(256))
chunks = []
while True:
    frame = process.stdout.read(width * height)
    if not frame:
        break
    if len(frame) != width * height:
        raise RuntimeError('Truncated decoded frame')
    pixels = bytearray(2 * len(frame))
    pixels[0::2] = frame.translate(lookup)
    pixels[1::2] = frame.translate(shade)
    chunks.append(zlib.compress(pixels, level=9))
if process.wait() != 0 or not chunks:
    raise RuntimeError('ffmpeg failed')
offsets = [0]
for chunk in chunks:
    offsets.append(offsets[-1] + len(chunk))
output = root / 'assets/ink/ink.frames'
output.write_bytes(struct.pack('<4sIIII', b'INK2', width, height, len(chunks), fps)
    + struct.pack('<' + 'I'*len(offsets), *offsets) + b''.join(chunks))
print(f'{len(chunks)} frames, {width} x {height}, {output.stat().st_size} bytes')
