# MediaKit — agent notes

- The no-guessing rule is load-bearing: a field the parser did not read out of the file is `null`, never guessed, defaulted from a sibling field, or filled from the file name/extension. If you add an extraction, it must trace to specific bytes; if the bytes are ambiguous (e.g. versioned MP4 audio sample entries), report `null` rather than a maybe.
- `duration_is_estimate` honesty: only MP3 without a Xing/Info header may set it. Everything else either read or counted exact values or reports `null`. Do not "improve" an MP3 estimate by scanning the whole file — bounded IO is a design rule, not laziness (see below).
- Bounded IO is load-bearing: parsers read through `Reader::exact` only, every loop over chunks/boxes/pages/elements has an explicit budget, and `Reader::exact` caps a single read at 16 MiB. A corrupt length field must always end in a clean `ERR_PARSE`, never a huge allocation or an endless walk. The truncation-prefix tests (MP4, Matroska, FLAC) exist to prove this — keep them when touching parsers.
- Poster frames are ROADMAP ONLY. Never add code that renders a frame, an icon, or a placeholder image and reports it as a thumbnail or still. v0.1 probes metadata; it does not decode media.
- Codec strings stay as the container names them (MP4 sample-entry type, Matroska CodecID verbatim, fourcc for AVI). Do not add a translation table to friendly names — hosts that want one can map the stable stored value; two sources of truth would drift.
- Detection is by magic bytes only. Extension-based detection is never a fallback: a renamed file must probe as what it is.
- MP3 frame scanning validates a candidate header against the following frame's sync before accepting it; removing that check makes random 0xFF bytes inside tags "detect" as MP3.
- The Ogg final-granule shortcut assumes the last page belongs to the first stream; that is documented in the README. Multi-stream/chained Ogg support means tracking serials, not dropping the assumption silently.
- The JSON engine (src/json.rs) is the family design — duplicate keys in one object are a PARSE ERROR by design; do not "fix" that into last-wins.
- Zero dependencies; gate: WinDevKit tools/gate.sh (fmt + clippy both targets + full tests). Check/clippy do not link — CI's real Windows build is the link gate.
