# MediaKit — Parity (spec vs implementation)

Evidence: the Rust test suite in `src/tests.rs`, `src/ffi.rs`,
`src/json.rs`, and `src/lib.rs` (36 tests, all passing at the
v0.1.0 gate). "Verified" means a named test exercises the
behaviour against a byte-built fixture whose expected values
are computed independently of the parser.

| Spec feature | Status | Evidence |
|---|---|---|
| Result JSON: container, duration_ms (or null), duration_is_estimate, bitrate_bps (or null), streams[] with kind/codec/width/height/sample_rate/channels/language (null when unread) | Verified | `result_json_shape`, plus per-format assertions in every format test below |
| Detection by magic bytes, not extension | Verified | every fixture is fed through `probe_bytes` with no name at all; `riff_but_not_wave_or_avi_is_unsupported` |
| Unknown container → ERR_NOT_SUPPORTED with first-bytes hex in the message | Verified | `unknown_container_reports_first_bytes_hex`, `ffi_probe_unknown_bytes_reports_first_bytes` |
| WAV: exact duration from byte rate, codec from format tag incl. float; missing fmt → ERR_PARSE | Verified | `wav_pcm_exact_duration_and_stream`, `wav_float_tag_and_missing_fmt` |
| WAV extensible (0xFFFE) SubFormat GUID tag resolution | Implemented, not directly verified | Code path in `wav::probe`; no fixture exercises a 40-byte fmt chunk yet — stated honestly rather than claimed |
| MP3: ID3v2 skipped via synchsafe size | Verified | `mp3_id3_skip_and_size_estimate` |
| MP3: Xing frame count → exact duration, estimate flag false | Verified | `mp3_xing_header_gives_exact_duration` |
| MP3: no Xing → size-at-first-frame-bitrate estimate, flag true | Verified | `mp3_id3_skip_and_size_estimate` (260 ms from 4170 bytes at 128 kbps) |
| MP3: candidate headers validated against the next frame's sync | Verified by construction of fixtures + `mp3_no_frames_is_a_clean_error` | A tag-only file with no frames returns ERR_PARSE instead of a bogus detection |
| FLAC: STREAMINFO rate/channels/total samples → exact duration; first block not STREAMINFO → ERR_PARSE | Verified | `flac_streaminfo_exact_duration`, `flac_first_block_must_be_streaminfo` |
| Ogg Vorbis: id header rate/channels; duration from last page granule | Verified | `ogg_vorbis_stream_and_granule_duration` |
| Ogg Opus: OpusHead channels; duration from last granule at 48 kHz minus preskip | Verified | `ogg_opus_duration_at_48k` (preskip 0 fixture; nonzero-preskip arithmetic is the same subtraction, not separately fixtured) |
| Ogg unknown codec: stream kind `other`, codec null, duration null | Verified | `ogg_unknown_codec_is_honest_other` |
| MP4: mvhd v0 duration; tkhd 16.16 dimensions; hdlr kinds; stsd codec; mdhd packed language | Verified | `mp4_video_track_full_extraction` |
| MP4: no mvhd → longest track mdhd duration; audio sample entry channels/rate | Verified | `mp4_audio_track_and_duration_fallback` |
| MP4: QuickTime major brand → container `mov` | Verified | `mp4_quicktime_brand_reports_mov` |
| MP4: mvhd v1 (64-bit duration), elng language, 64-bit box sizes | Partial | 64-bit box sizes are walked (`mp4_oversized_box_size_is_a_clean_error` exercises the hostile case); mvhd v1 and elng are implemented but have no fixture yet |
| Matroska/WebM: DocType → container; Info TimecodeScale × Duration (float) → ms | Verified | `matroska_webm_full_extraction` (750 units × 2,000,000 scale = 1500 ms), `matroska_doctype_and_missing_duration` |
| Matroska: tracks with type/codec/language, video pixel dims, audio float rate/channels | Verified | `matroska_webm_full_extraction` |
| Matroska: unknown-size Segment | Implemented, not directly verified | Handled in `matroska::probe`; the fixture uses a sized segment. Unknown-size non-segment elements stop the scan (documented in code) rather than guessing |
| AVI: strh streams + durations, strf dimensions and audio format, avih fallback | Partial | `avi_video_stream_and_duration` (strh path), `avi_audio_stream_from_strf`; the avih-only fallback (no strh durations) is implemented but not fixtured |
| Bounded IO: scan/page/box/element budgets and a 16 MiB single-read cap | Verified structurally | Budgets are constants in the parsers; the u64::MAX-size MP4 test proves a corrupt length cannot drive an unbounded read or walk |
| Truncated/corrupt input never panics | Verified | `mp4_truncation_prefixes_never_panic`, `matroska_truncation_prefixes_never_panic`, `flac_truncation_prefixes_never_panic` (every prefix length) |
| FFI: me_probe / me_probe_bytes / me_free_string / me_last_error(_code), caller-owned strings, null-arg rejection, missing file → ERR_NOT_FOUND | Verified | `ffi_probe_bytes_round_trip`, `ffi_probe_rejects_null_and_bad_args`, `ffi_probe_missing_file_is_not_found` |
| JSON engine: duplicate keys in one object are a parse error | Verified | json module tests (`rejects_garbage`) |
| Header documents every export | Done | `include/mediakit.h` (schema, error contract, ownership, bounded-IO note) |
| Poster frames / thumbnails | Not implemented, by design | Roadmap-only; nothing in v0.1 renders or fakes a frame (see ROADMAP "Explicitly not planned") |
