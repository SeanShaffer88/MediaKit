# MediaKit

## TL;DR

MediaKit answers "what is this media file?" — how long it plays, what codecs are inside, how big the picture is, how many audio channels — by reading the file's own headers, in pure Rust, with no external tools and no decoding. One DLL call hands back a small JSON description. It is like asking the file to introduce itself instead of playing it to find out.

```c
#include "mediakit.h"
#include <stdio.h>

int main(void) {
    char *info = me_probe("C:/media/song.flac");
    if (!info) { printf("error: %s\n", me_last_error()); return 1; }
    printf("%s\n", info);
    me_free_string(info);
    return 0;
}
```

```json
{"container":"flac","duration_ms":213400,"duration_is_estimate":false,
 "bitrate_bps":903112,"streams":[{"kind":"audio","codec":"flac",
 "width":null,"height":null,"sample_rate":44100,"channels":2,"language":null}]}
```

Every field is either a value read out of the file or `null`. MediaKit never guesses: if a container does not store a value, the field is null, and the table below says exactly which is which.

## How to use it

- `me_probe(path)` — probe a file on disk. Only headers and small bounded windows are read, so a 4 GB movie costs the same few kilobytes of IO as a jingle.
- `me_probe_bytes(data, len)` — probe bytes already in memory.
- Both return a caller-owned JSON string; free it with `me_free_string()`. On failure they return `NULL` and `me_last_error()` / `me_last_error_code()` say why.
- The full contract (fields, error codes, lifetime rules) is documented in `include/mediakit.h`.

Containers are detected by content (magic bytes), never by file extension.

## Format support

What v0.1 extracts per format — anything not listed is reported `null`:

| Format | Container name | Duration | Duration exact? | Codec | Width/Height | Sample rate | Channels | Language |
|---|---|---|---|---|---|---|---|---|
| WAV (RIFF) | `wav` | from `data` chunk size ÷ byte rate | yes | from format tag (`pcm`, `ieee_float`, `mp3`, …; extensible GUID resolved) | — | yes | yes | — |
| MP3 | `mp3` | Xing/Info frame count when present, else size ÷ first-frame bitrate | only with Xing/Info (`duration_is_estimate` says which) | `mp3` | — | yes (first frame) | yes (first frame) | — |
| FLAC | `flac` | total samples ÷ sample rate from STREAMINFO | yes (null when the file stores total samples = 0) | `flac` | — | yes | yes | — |
| Ogg Vorbis | `ogg` | last page's granule position ÷ rate | yes when a final granule is findable, else null | `vorbis` | — | yes | yes | — |
| Ogg Opus | `ogg` | last granule (minus preskip) at 48 kHz | yes when findable, else null | `opus` | — | the OpusHead input rate when nonzero, else null | yes | — |
| Ogg, other codec | `ogg` | null | — | null (stream reported as kind `other`) | — | — | — | — |
| MP4 / MOV | `mp4` / `mov` (QuickTime brand) | `mvhd` timescale/duration; falls back to the longest track | yes | first `stsd` entry type (`avc1`, `mp4a`, …) | from `tkhd` | audio tracks: from the `mp4a`-style sample entry | audio tracks: same source | `mdhd` packed code, or `elng` when present |
| Matroska / WebM | `matroska` / `webm` (from DocType) | `Info` Duration × TimecodeScale | yes (null when the element is absent) | CodecID string, verbatim (`V_VP9`, `A_OPUS`, …) | `PixelWidth`/`PixelHeight` | `SamplingFrequency` (may be fractional) | yes | `Language` element |
| AVI (RIFF) | `avi` | longest stream (`strh` length ÷ rate), else `avih` frames × µs-per-frame | yes | stream handler fourcc (or `strf` compression / WAV tag for audio) | from `strf` BITMAPINFOHEADER | audio streams, from `strf` | audio streams, from `strf` | — |

Bitrate (`bitrate_bps`) is the overall file bitrate — total bits ÷ duration — except where the format states one directly: WAV reports its byte rate × 8 and MP3 reports its first frame's bitrate. It is `null` whenever the duration is unknown.

## Errors

| Code | Meaning |
|---|---|
| `ME_ERR_INVALID_ARG` (1) | null/empty argument, non-UTF-8 path, empty buffer |
| `ME_ERR_NOT_SUPPORTED` (2) | unrecognized container — the message includes the first bytes as hex; also RIFF files that are neither WAVE nor AVI |
| `ME_ERR_NOT_FOUND` (3) | the file does not exist |
| `ME_ERR_ACCESS_DENIED` (4) | the OS refused to open the file |
| `ME_ERR_PLATFORM` (5) | other OS-level open/read failure |
| `ME_ERR_INTERNAL` (6) | an internal fault caught at the FFI boundary |
| `ME_ERR_PARSE` (7) | recognized container, but truncated or corrupt |

## Limits and honesty notes

- Probing reads bounded windows: MP3 scans at most 64 KiB for the first frame, Ogg reads its last 64 KiB for the final granule, MP4 walks at most 4,096 boxes, Matroska at most 8,192 elements, and no single read exceeds 16 MiB. Files that hide their facts outside those windows report `null` rather than reading unboundedly.
- MP4 audio sample-entry details assume the classic (version 0) layout; newer versioned audio entries can report `null` channels/rate rather than wrong ones.
- Ogg page CRCs are not verified, and the final-granule duration assumes the last page in the file belongs to the first stream — true for ordinary single-stream files.
- AVI codec names are the fourccs stored in the file; no attempt is made to translate them into marketing names.
- Poster frames / thumbnails are **not** produced — that is a roadmap item for a companion pipeline, and MediaKit will not fake one from anything else.

## Building

Rust 1.80+, zero dependencies:

```
cargo build --release
cargo test
```

Produces a cdylib (`mediakit.dll` / `.so` / `.dylib`), a static library, and an rlib. The C header is `include/mediakit.h`.

## License

MIT — see `LICENSE`.
