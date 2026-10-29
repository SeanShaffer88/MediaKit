//! MediaKit — media probing core.
//!
//! Reads just enough of a media file's own headers to report its
//! container, duration, overall bitrate, and streams (codec,
//! dimensions, sample rate, channels, language) — parsed in pure
//! Rust, no external tools, no decoding. The C ABI lives in `ffi`;
//! the per-format parsers live in `parsers`.
//!
//! Two rules are load-bearing:
//! - A field a parser did not actually read is `null` in the
//!   result, never guessed, defaulted, or copied from a sibling
//!   field. Hosts can trust "present" to mean "was in the file".
//! - IO is bounded: parsers read headers and small windows
//!   through [`Source::read_at`], never the whole file (the one
//!   exception is [`probe_bytes`], where the caller already holds
//!   the bytes in memory). Truncated or corrupt input produces a
//!   clean [`ERR_PARSE`] error, never a panic.

pub mod ffi;
pub mod json;
pub mod parsers;
pub mod platform;

pub const VERSION: &str = "0.2.0";
pub const VERSION_MAJOR: u32 = 0;
pub const VERSION_MINOR: u32 = 2;
pub const VERSION_PATCH: u32 = 0;

// Error codes (mirror include/mediakit.h).
pub const ME_OK: i32 = 0;
pub const ERR_INVALID_ARG: i32 = 1;
pub const ERR_NOT_SUPPORTED: i32 = 2;
pub const ERR_NOT_FOUND: i32 = 3;
pub const ERR_ACCESS_DENIED: i32 = 4;
pub const ERR_PLATFORM: i32 = 5;
pub const ERR_INTERNAL: i32 = 6;
pub const ERR_PARSE: i32 = 7;

/// Every fallible core operation returns this: an error code from
/// the header set plus a human-readable message (which becomes
/// `me_last_error()` at the FFI boundary).
pub type MeResult<T> = Result<T, (i32, String)>;

pub fn parse_err(msg: impl Into<String>) -> (i32, String) {
    (ERR_PARSE, msg.into())
}

/// One stream inside a container. Every field is `None` unless
/// the parser read that exact value out of the file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StreamInfo {
    /// "video" | "audio" | "subtitle" | "other".
    pub kind: String,
    /// Codec identifier as the container names it (MP4 sample
    /// entry type, Matroska CodecID, Vorbis/Opus magic, WAV/AVI
    /// format mapping, "mp3", "flac").
    pub codec: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// Samples per second. Integral for every v0.1 format except
    /// Matroska, whose sampling frequency is a float.
    pub sample_rate: Option<f64>,
    pub channels: Option<u32>,
    /// Language as stored (ISO 639 code from MP4 mdhd, string
    /// from Matroska/elng), untranslated.
    pub language: Option<String>,
    /// Video frame rate as an exact fraction (numerator,
    /// denominator), reduced. From MP4 `stts` sample timing and the
    /// track timescale (an average when the timing varies), Matroska
    /// `DefaultDuration`, or AVI `strh` rate/scale.
    pub frame_rate: Option<(u64, u64)>,
    /// Interlacing as stored: "progressive", "tt", "bb", "tb" or
    /// "bt" (first letter: field coded first; second: field
    /// displayed first). From the QuickTime `fiel` atom or Matroska
    /// `FlagInterlaced`/`FieldOrder`.
    pub field_order: Option<String>,
}

/// The probe result for one file.
#[derive(Clone, Debug, PartialEq)]
pub struct ProbeResult {
    /// "wav" | "mp3" | "flac" | "ogg" | "mp4" | "mov" |
    /// "matroska" | "webm" | "avi".
    pub container: String,
    /// Milliseconds, or `None` when the container does not carry
    /// a usable duration (or it cannot be derived exactly).
    pub duration_ms: Option<u64>,
    /// True when `duration_ms` was derived from size and bitrate
    /// (MP3 without a Xing/Info header) rather than read or
    /// counted exactly.
    pub duration_is_estimate: bool,
    /// Overall bits per second, or `None` when no duration is
    /// known to divide by.
    pub bitrate_bps: Option<u64>,
    pub streams: Vec<StreamInfo>,
}

impl ProbeResult {
    pub fn to_value(&self) -> json::Value {
        let streams: Vec<json::Value> = self.streams.iter().map(StreamInfo::to_value).collect();
        json::obj(vec![
            ("container", json::s(&self.container)),
            (
                "duration_ms",
                self.duration_ms.map(json::n).unwrap_or(json::Value::Null),
            ),
            ("duration_is_estimate", json::b(self.duration_is_estimate)),
            (
                "bitrate_bps",
                self.bitrate_bps.map(json::n).unwrap_or(json::Value::Null),
            ),
            ("streams", json::Value::Array(streams)),
        ])
    }

    pub fn to_json(&self) -> String {
        self.to_value().to_json()
    }
}

impl StreamInfo {
    pub fn to_value(&self) -> json::Value {
        let opt_s = |v: &Option<String>| match v {
            Some(x) => json::s(x),
            None => json::Value::Null,
        };
        let opt_u = |v: &Option<u32>| match v {
            Some(x) => json::n(u64::from(*x)),
            None => json::Value::Null,
        };
        let rate = match self.sample_rate {
            Some(r) if r.fract() == 0.0 && (0.0..=9.0e15).contains(&r) => json::n(r as u64),
            Some(r) => json::Value::F64(r),
            None => json::Value::Null,
        };
        json::obj(vec![
            ("kind", json::s(&self.kind)),
            ("codec", opt_s(&self.codec)),
            ("width", opt_u(&self.width)),
            ("height", opt_u(&self.height)),
            ("sample_rate", rate),
            ("channels", opt_u(&self.channels)),
            ("language", opt_s(&self.language)),
            (
                "frame_rate",
                match self.frame_rate {
                    Some((num, den)) => json::s(&format!("{num}/{den}")),
                    None => json::Value::Null,
                },
            ),
            ("field_order", opt_s(&self.field_order)),
        ])
    }
}

/// Random-access byte source. Parsers read headers and bounded
/// windows through this trait so a multi-gigabyte file costs a
/// few kilobytes of IO. `read_at` returns the number of bytes
/// actually read (short at end of input); it is an error only
/// for real IO failures.
pub trait Source {
    fn len(&self) -> u64;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> MeResult<usize>;
}

/// A caller-supplied memory buffer (`me_probe_bytes`).
pub struct BytesSource<'a> {
    data: &'a [u8],
}

impl Source for BytesSource<'_> {
    fn len(&self) -> u64 {
        self.data.len() as u64
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> MeResult<usize> {
        let start = match usize::try_from(offset) {
            Ok(s) if s <= self.data.len() => s,
            _ => return Ok(0),
        };
        let n = buf.len().min(self.data.len() - start);
        buf[..n].copy_from_slice(&self.data[start..start + n]);
        Ok(n)
    }
}

/// A file on disk (`me_probe`). Reads go through the platform
/// layer's positioned reads, so the file is never slurped.
pub struct FileSource {
    file: std::fs::File,
    len: u64,
}

impl Source for FileSource {
    fn len(&self) -> u64 {
        self.len
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> MeResult<usize> {
        platform::file_read_at(&self.file, offset, buf)
            .map_err(|e| (ERR_PLATFORM, format!("read failed at offset {offset}: {e}")))
    }
}

/// Reading helper shared by all parsers: `exact` is the only way
/// parsers consume input, and it turns "file ended early" into
/// a clean parse error instead of a panic or a zero-filled read.
pub struct Reader<'a> {
    pub src: &'a dyn Source,
}

impl<'a> Reader<'a> {
    pub fn new(src: &'a dyn Source) -> Self {
        Reader { src }
    }

    pub fn len(&self) -> u64 {
        self.src.len()
    }

    pub fn is_empty(&self) -> bool {
        self.src.is_empty()
    }

    /// Read exactly `n` bytes at `offset` or fail with
    /// ERR_PARSE. `n` is capped so a corrupt length field can
    /// never drive an unbounded allocation.
    pub fn exact(&self, offset: u64, n: usize) -> MeResult<Vec<u8>> {
        const MAX_READ: usize = 16 * 1024 * 1024;
        if n > MAX_READ {
            return Err(parse_err(format!(
                "read of {n} bytes at offset {offset} exceeds the probe limit"
            )));
        }
        let mut buf = vec![0u8; n];
        let got = self.src.read_at(offset, &mut buf)?;
        if got != n {
            return Err(parse_err(format!(
                "unexpected end of input at offset {offset}: wanted {n} bytes, got {got}"
            )));
        }
        Ok(buf)
    }

    pub fn u8(&self, off: u64) -> MeResult<u8> {
        Ok(self.exact(off, 1)?[0])
    }

    pub fn u16le(&self, off: u64) -> MeResult<u16> {
        let b = self.exact(off, 2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn u16be(&self, off: u64) -> MeResult<u16> {
        let b = self.exact(off, 2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    pub fn u32le(&self, off: u64) -> MeResult<u32> {
        let b = self.exact(off, 4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn u32be(&self, off: u64) -> MeResult<u32> {
        let b = self.exact(off, 4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn u64le(&self, off: u64) -> MeResult<u64> {
        let b = self.exact(off, 8)?;
        Ok(u64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    pub fn u64be(&self, off: u64) -> MeResult<u64> {
        let b = self.exact(off, 8)?;
        Ok(u64::from_be_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    pub fn tag(&self, off: u64) -> MeResult<[u8; 4]> {
        let b = self.exact(off, 4)?;
        Ok([b[0], b[1], b[2], b[3]])
    }
}

/// Lowercase hex of the first up-to-`max` bytes — used in
/// "not supported" messages so a host can see what it handed us.
pub fn first_bytes_hex(data: &[u8], max: usize) -> String {
    let mut out = String::new();
    for b in data.iter().take(max) {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// Fill in the overall bitrate when a parser left it unset and
/// a duration is known: total bits / total seconds. A parser
/// that knows a more precise figure (WAV byte rate, MP3 frame
/// bitrate) sets it first and is not overwritten.
pub fn finalize(mut result: ProbeResult, total_len: u64) -> ProbeResult {
    if result.bitrate_bps.is_none() && matches!(result.duration_ms, Some(ms) if ms > 0) {
        let ms = result.duration_ms.unwrap_or(1);
        result.bitrate_bps = Some(total_len.saturating_mul(8000) / ms);
    }
    result
}

/// Probe a byte source: detect the container by magic bytes
/// and dispatch to its parser.
pub fn probe_source(src: &dyn Source) -> MeResult<ProbeResult> {
    let r = Reader::new(src);
    let head_len = src.len().min(64) as usize;
    let head = if head_len > 0 {
        // A short head read is not fatal here: detection simply
        // sees fewer bytes (empty input lands in the same place).
        let mut buf = vec![0u8; head_len];
        let got = src.read_at(0, &mut buf)?;
        buf.truncate(got);
        buf
    } else {
        Vec::new()
    };
    parsers::detect_and_probe(&r, &head)
}

/// Probe an in-memory buffer and return the result JSON.
pub fn probe_bytes(data: &[u8]) -> MeResult<String> {
    if data.is_empty() {
        return Err((ERR_INVALID_ARG, "input buffer is empty".to_string()));
    }
    let src = BytesSource { data };
    let result = probe_source(&src)?;
    Ok(finalize(result, src.len()).to_json())
}

/// Probe a file by path and return the result JSON. Only
/// headers and bounded windows are read, however large the
/// file is.
pub fn probe_path(path: &str) -> MeResult<String> {
    if path.is_empty() {
        return Err((ERR_INVALID_ARG, "path must not be empty".to_string()));
    }
    let file = std::fs::File::open(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => (ERR_NOT_FOUND, format!("no such file: \"{path}\"")),
        std::io::ErrorKind::PermissionDenied => {
            (ERR_ACCESS_DENIED, format!("access denied: \"{path}\""))
        }
        _ => (ERR_PLATFORM, format!("cannot open \"{path}\": {e}")),
    })?;
    let len = file
        .metadata()
        .map_err(|e| (ERR_PLATFORM, format!("cannot stat \"{path}\": {e}")))?
        .len();
    let src = FileSource { file, len };
    let result = probe_source(&src)?;
    Ok(finalize(result, len).to_json())
}

/// Escape a string for inclusion inside JSON double quotes.
pub fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn escapes_json() {
        assert_eq!(json_escape("a\"b\\c\nd"), "a\\\"b\\\\c\\nd");
    }

    #[test]
    fn finalize_computes_overall_bitrate_only_when_missing() {
        let mut res = ProbeResult {
            container: "x".into(),
            duration_ms: Some(2000),
            duration_is_estimate: false,
            bitrate_bps: None,
            streams: vec![],
        };
        res = finalize(res, 100_000);
        assert_eq!(res.bitrate_bps, Some(400_000));
        let mut res2 = ProbeResult {
            container: "x".into(),
            duration_ms: Some(2000),
            duration_is_estimate: false,
            bitrate_bps: Some(7),
            streams: vec![],
        };
        res2 = finalize(res2, 100_000);
        assert_eq!(res2.bitrate_bps, Some(7));
        let res3 = ProbeResult {
            container: "x".into(),
            duration_ms: None,
            duration_is_estimate: false,
            bitrate_bps: None,
            streams: vec![],
        };
        assert_eq!(finalize(res3, 100_000).bitrate_bps, None);
    }

    #[test]
    fn result_json_shape() {
        let res = ProbeResult {
            container: "wav".into(),
            duration_ms: Some(1000),
            duration_is_estimate: false,
            bitrate_bps: Some(128_000),
            streams: vec![StreamInfo {
                kind: "audio".into(),
                codec: Some("pcm".into()),
                width: None,
                height: None,
                sample_rate: Some(8000.0),
                channels: Some(1),
                language: None,
                frame_rate: None,
                field_order: None,
            }],
        };
        let v = json::parse(&res.to_json()).unwrap();
        assert_eq!(v.get("container").unwrap().as_str(), Some("wav"));
        assert_eq!(v.get("duration_ms").unwrap().as_i64(), Some(1000));
        assert_eq!(
            v.get("duration_is_estimate").unwrap().as_bool(),
            Some(false)
        );
        let st = &v.get("streams").unwrap().as_array().unwrap()[0];
        assert_eq!(st.get("codec").unwrap().as_str(), Some("pcm"));
        assert_eq!(st.get("sample_rate").unwrap().as_i64(), Some(8000));
        assert!(st.get("width").unwrap() == &json::Value::Null);
        assert!(st.get("language").unwrap() == &json::Value::Null);
        assert!(st.get("frame_rate").unwrap() == &json::Value::Null);
        assert!(st.get("field_order").unwrap() == &json::Value::Null);

        let mut video = res.clone();
        video.streams[0].kind = "video".into();
        video.streams[0].frame_rate = Some((30000, 1001));
        video.streams[0].field_order = Some("tt".into());
        let v = json::parse(&video.to_json()).unwrap();
        let st = &v.get("streams").unwrap().as_array().unwrap()[0];
        assert_eq!(st.get("frame_rate").unwrap().as_str(), Some("30000/1001"));
        assert_eq!(st.get("field_order").unwrap().as_str(), Some("tt"));
    }
}
