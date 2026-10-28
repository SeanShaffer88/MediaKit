# MediaKit Roadmap

## v0.1.0 (current)

Pure-Rust probing for WAV, MP3, FLAC, Ogg (Vorbis/Opus), MP4/MOV, Matroska/WebM, and AVI behind the `me_probe` / `me_probe_bytes` C ABI: container, duration (exact, or flagged as an estimate for MP3 without a Xing/Info header), overall bitrate, and per-stream codec, dimensions, sample rate, channels, and language where the container stores them. Bounded IO throughout: headers and small windows only.

## Next, in order

1. **More Ogg codecs**: identify Theora (video kind + dimensions from its identification header) and FLAC-in-Ogg mappings, with the same first-packet approach as Vorbis/Opus.
2. **MP4 depth**: versioned audio sample entries (QuickTime v1/v2 and `enda`-style layouts) for channels/rate, per-track durations reported alongside streams, and chapter/`udta` metadata only where a host need is demonstrated first.
3. **Matroska depth**: `LanguageIETF`, `DefaultDuration`-derived stream durations when `Info` carries none, and tag metadata behind a separate call so the probe result stays lean.
4. **Additional containers**: WMA/ASF and AIFF are the most common gaps; each lands with byte-built synthetic fixtures and truncation tests like the v0.1 formats.
5. **Batch probing**: one call that probes a list of paths and returns one result array, for hosts describing a whole folder without per-file FFI overhead.

## Explicitly not planned

- Decoding media, in any form. MediaKit reads headers; it will never grow a codec.
- Guessed or derived-by-heuristic metadata (e.g. estimating a missing duration from a sibling track). Missing stays `null`.
- A poster-frame/thumbnail renderer inside this DLL. If stills are ever offered, they will be a separate kit with real decode paths, and formats without one will say so — never a placeholder image presented as a poster frame.
