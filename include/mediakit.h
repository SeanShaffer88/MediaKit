/* MediaKit.dll — C ABI. All strings UTF-8, NUL-terminated.
 * char* results are caller-owned: free with me_free_string().
 * On failure they return NULL; see me_last_error(). */
#ifndef MEDIAKIT_H
#define MEDIAKIT_H
#include <stdint.h>
#include <stddef.h>
#ifdef __cplusplus
extern "C" {
#endif
#if defined(_WIN32) && defined(ME_BUILD)
#define ME_API __declspec(dllexport)
#elif defined(_WIN32)
#define ME_API __declspec(dllimport)
#else
#define ME_API
#endif
#define ME_OK 0
#define ME_ERR_INVALID_ARG 1
#define ME_ERR_NOT_SUPPORTED 2
#define ME_ERR_NOT_FOUND 3
#define ME_ERR_ACCESS_DENIED 4
#define ME_ERR_PLATFORM 5
#define ME_ERR_INTERNAL 6
#define ME_ERR_PARSE 7

/* Library version string, e.g. "0.2.0". Static storage: do NOT
 * free the returned pointer. */
ME_API const char *me_version(void);

/* Message for the most recent failure on this thread, and its
 * code (one of the ME_ERR_* values above; ME_OK after a
 * successful call). Thread-local; do NOT free. The pointer is
 * valid until the next MediaKit call on the same thread. */
ME_API const char *me_last_error(void);
ME_API int me_last_error_code(void);

/* Free a string returned by me_probe / me_probe_bytes.
 * NULL is accepted and does nothing. Freeing a pointer twice,
 * or a pointer this library did not return, is a caller bug. */
ME_API void me_free_string(char *p);

/* Probe a media file on disk and describe it as JSON:
 *
 *   {"container":"mp4",
 *    "duration_ms":123456  | null,
 *    "duration_is_estimate":false,
 *    "bitrate_bps":1234567 | null,
 *    "streams":[{"kind":"video"|"audio"|"subtitle"|"other",
 *                "codec":"avc1" | null,
 *                "width":1920 | null, "height":1080 | null,
 *                "sample_rate":48000 | null,
 *                "channels":2 | null,
 *                "language":"eng" | null,
 *                "frame_rate":"30000/1001" | null,
 *                "field_order":"progressive"|"tt"|"bb"|"tb"|"bt" | null}]}
 *
 * A field is null when the file does not carry that value;
 * MediaKit never guesses. frame_rate (video streams; since 0.2.0)
 * is an exact reduced fraction "num/den" from MP4/MOV stts timing
 * (an average when timing varies), Matroska DefaultDuration, or AVI
 * strh rate/scale. field_order (video; since 0.2.0) comes from the
 * QuickTime fiel atom or Matroska FlagInterlaced/FieldOrder: first
 * letter is the field coded first, second the field displayed
 * first. Fields stored only inside the coded video (not in the
 * container) are null: MediaKit reads headers, not frames. duration_is_estimate is true only
 * for MP3 files without a Xing/Info header, where duration is
 * derived from file size at the first frame's bitrate.
 *
 * Supported containers (detected by content, not extension):
 * WAV, AVI (RIFF), MP3 (with/without ID3v2), FLAC, Ogg
 * (Vorbis/Opus), MP4/MOV, Matroska/WebM.
 *
 * Only headers and bounded windows are read, however large the
 * file is. Returns NULL + last-error on failure:
 * ME_ERR_INVALID_ARG (null/empty/non-UTF-8 path),
 * ME_ERR_NOT_FOUND, ME_ERR_ACCESS_DENIED, ME_ERR_NOT_SUPPORTED
 * (unrecognized container; the message includes the first bytes
 * as hex), ME_ERR_PARSE (recognized but truncated/corrupt). */
ME_API char *me_probe(const char *path);

/* Probe media bytes already in memory. Same result and error
 * contract as me_probe. data must point to len readable bytes;
 * a NULL data pointer or a len of 0 is ME_ERR_INVALID_ARG. */
ME_API char *me_probe_bytes(const uint8_t *data, size_t len);

#ifdef __cplusplus
}
#endif
#endif
