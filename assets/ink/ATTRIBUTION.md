# Ink animation

Author: MART PRODUCTION.
Source: https://www.pexels.com/video/black-paint-dropping-on-water-7565637/
License: Pexels License, https://www.pexels.com/license/
Retrieved: 2026-10-13.

Used as an animation element in WindowsAwake, not as a stock footage or wallpaper distribution service. Author does not endorse this application. This asset is under the Pexels License, not a Rust source-code license.

Modifications: downsampled the original 1080 × 1920 silent video to 540 × 960 alpha/density masks at 24 fps, removed the pale background, converted to independently compressed two-channel frames. The renderer tints the ink deep teal, rotates its flow towards the screen centre from four corners, interpolates adjacent frames and gradually increases opacity. Fullscreen output follows each monitor's native resolution; texture detail is not claimed to be native 4K.

`ink.frames` has a 20-byte little-endian header: magic `INK2`, width, height, frame count and source fps (each u32). A table of frame-count + 1 u32 offsets is followed by zlib-compressed interleaved alpha/density planes. Each decoded frame must have exactly width × height × 2 bytes. See `scripts/prepare_ink.py` for reproducible conversion; building the EXE uses the checked-in pack and needs no Python, ffmpeg, network or separate asset files.
