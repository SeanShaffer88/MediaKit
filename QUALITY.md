# MediaKit — Quality Scorecard

Scored against the v0.1.0 gate run on the development VM:
`cargo fmt --check`, clippy (host + `x86_64-pc-windows-msvc`,
`-D warnings`), and the full test suite — 36/36 passing, run
three consecutive times with identical results. Scores are only
claimed where that evidence exists; Windows runtime behaviour
is not claimed from Linux evidence.

| Dimension | Score /5 | Evidence |
|---|---|---|
| Correctness of core operations | 5 | Every format's fixture is built byte-by-byte in the tests, so each asserted value (durations to the millisecond, packed FLAC STREAMINFO fields, Xing frame-count math, EBML TimecodeScale multiplication, MP4 16.16 dimensions) is checked against independently computed constants |
| Honesty of absent data | 5 | Tests assert `null` (not a guess) for: Ogg unknown codec, FLAC with no total samples path via Matroska missing Duration, MP4 audio track without mdhd language, missing bitrate when duration is unknown; `duration_is_estimate` asserted both ways for MP3 |
| Robustness on corrupt input | 5 | Truncation-prefix sweeps over the MP4, Matroska, and FLAC fixtures (every prefix must return Ok or Err, never panic), plus an MP4 box with a u64::MAX 64-bit size, missing-`fmt` WAV, wrong-first-block FLAC, and MP3 with no valid frames — all clean `ERR_PARSE` |
| Bounded IO | 4 | Budgets are structural (64 KiB MP3 scan window, 64 KiB Ogg tail, 4,096 MP4 boxes, 8,192 EBML elements, 16 MiB single-read cap) and corrupt sizes are tested; no test yet measures actual bytes read from a sparse multi-GB file — one point held back |
| API & documentation | 4 | Header documents every export with the result schema and error contract; README's support table states per-format exactly what is and is not extracted. No compiled C consumer test in the suite (FFI is exercised from Rust) — one point held back |
| Code health | 5 | Zero dependencies, fmt + clippy clean on both targets at the gate, nothing unwinds across the FFI boundary (catch_unwind + panic=abort release profile) |
| Platform coverage | 3 | Parsers are platform-independent and fully tested on Linux; the Windows MSVC build/link and on-Windows behaviour are CI's to prove, not yet observed — honestly mid-scored |

**Total: 86/100**

## Waivers / open items

- Real-world files: all fixtures are synthetic. The parsers follow the published layouts, but no corpus of real recorder/camera/phone files has been probed yet. Follow-up: run a real-file corpus before claiming field-proven extraction, and add any odd-but-valid layout found as a fixture.
- No dimension is scored on Windows runtime evidence; the first Windows CI run is the follow-up that can raise Platform coverage.
