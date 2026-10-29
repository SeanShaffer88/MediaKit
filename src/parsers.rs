//! Per-format parsers. Each takes a [`Reader`] over a
//! random-access source and returns a [`ProbeResult`].
//!
//! Ground rules for every parser here:
//! - Read through `Reader::exact` only, so truncated input is a
//!   clean [`ERR_PARSE`], never a panic or a silent zero.
//! - Loops over chunks/boxes/pages/elements carry explicit
//!   budgets, so a corrupt length field cannot make a parser
//!   walk forever or read without bound.
//! - Set a field only from bytes actually present in the file.

use crate::{parse_err, ProbeResult, Reader, StreamInfo, ERR_NOT_SUPPORTED};

type PResult<T> = Result<T, (i32, String)>;

/// `num/den` reduced to lowest terms, or `None` when either part is
/// zero or the reduced fraction does not fit in 64 bits.
pub fn reduced_ratio(num: u128, den: u128) -> Option<(u64, u64)> {
    if num == 0 || den == 0 {
        return None;
    }
    let (mut a, mut b) = (num, den);
    while b != 0 {
        (a, b) = (b, a % b);
    }
    Some((u64::try_from(num / a).ok()?, u64::try_from(den / a).ok()?))
}

/// Field order from a QuickTime `fiel` atom (field count, detail)
/// or a Matroska `FieldOrder` value, in the shared spelling: first
/// letter is the field coded (stored) first, second the field
/// displayed first. Undetermined or unknown values are `None`.
pub fn field_order_name(code: u64) -> Option<String> {
    match code {
        0 => Some("progressive"),
        1 => Some("tt"),
        6 => Some("bb"),
        // "Bottom displayed first, top stored first".
        9 => Some("tb"),
        // "Top displayed first, bottom stored first".
        14 => Some("bt"),
        _ => None,
    }
    .map(String::from)
}

fn empty_result(container: &str) -> ProbeResult {
    ProbeResult {
        container: container.to_string(),
        duration_ms: None,
        duration_is_estimate: false,
        bitrate_bps: None,
        streams: Vec::new(),
    }
}

fn audio_stream() -> StreamInfo {
    StreamInfo {
        kind: "audio".to_string(),
        ..Default::default()
    }
}

/// Dispatch on magic bytes. `head` is the first up-to-64 bytes
/// of the input (fewer for a short input).
pub fn detect_and_probe(r: &Reader, head: &[u8]) -> PResult<ProbeResult> {
    let starts = |magic: &[u8]| head.starts_with(magic);
    if starts(b"RIFF") && head.len() >= 12 && &head[8..12] == b"WAVE" {
        return wav::probe(r);
    }
    if starts(b"RIFF") && head.len() >= 12 && &head[8..12] == b"AVI " {
        return avi::probe(r);
    }
    if starts(b"fLaC") {
        return flac::probe(r);
    }
    if starts(b"OggS") {
        return ogg::probe(r);
    }
    if starts(&[0x1A, 0x45, 0xDF, 0xA3]) {
        return matroska::probe(r);
    }
    if head.len() >= 8 && &head[4..8] == b"ftyp" {
        return mp4::probe(r);
    }
    if starts(b"ID3") || mp3::looks_like_mp3(r) {
        return mp3::probe(r);
    }
    if head.is_empty() {
        return Err((
            ERR_NOT_SUPPORTED,
            "empty input: nothing to detect".to_string(),
        ));
    }
    Err((
        ERR_NOT_SUPPORTED,
        format!(
            "unrecognized container (first bytes: {})",
            crate::first_bytes_hex(head, 16)
        ),
    ))
}

/// Codec name for a WAVE format tag (shared by WAV and AVI
/// audio streams). Unknown tags are reported as the bare tag
/// number rather than a guessed name.
pub fn wave_codec_name(tag: u16) -> String {
    match tag {
        0x0001 => "pcm".to_string(),
        0x0003 => "ieee_float".to_string(),
        0x0006 => "alaw".to_string(),
        0x0007 => "mulaw".to_string(),
        0x0050 => "mpeg1_audio".to_string(),
        0x0055 => "mp3".to_string(),
        0x2000 => "ac3".to_string(),
        other => format!("wave_format_0x{other:04x}"),
    }
}

// ----------------------------------------------------------------
// WAV (RIFF/WAVE)
// ----------------------------------------------------------------

pub mod wav {
    use super::*;

    /// One pass over the RIFF chunks collecting `fmt ` and the
    /// `data` chunk size; duration is exact because byte rate
    /// and byte count are both in the file.
    pub fn probe(r: &Reader) -> PResult<ProbeResult> {
        let mut fmt: Option<(u16, u32, u32, u32)> = None; // tag, channels, rate, byte_rate
        let mut data_bytes: Option<u64> = None;
        let mut pos = 12u64;
        let len = r.len();
        for _ in 0..1024 {
            if pos + 8 > len {
                break;
            }
            let id = r.tag(pos)?;
            let size = u64::from(r.u32le(pos + 4)?);
            let body = pos + 8;
            match &id {
                b"fmt " => {
                    if size < 16 {
                        return Err(parse_err("WAV fmt chunk shorter than 16 bytes"));
                    }
                    let tag = r.u16le(body)?;
                    let channels = u32::from(r.u16le(body + 2)?);
                    let rate = r.u32le(body + 4)?;
                    let byte_rate = r.u32le(body + 8)?;
                    // WAVE_FORMAT_EXTENSIBLE: the real tag is the
                    // first two bytes of the SubFormat GUID.
                    let real_tag = if tag == 0xFFFE && size >= 40 {
                        r.u16le(body + 24)?
                    } else {
                        tag
                    };
                    fmt = Some((real_tag, channels, rate, byte_rate));
                }
                b"data" => {
                    // A data chunk may legally claim more than
                    // the file holds in streamed captures; clamp
                    // to what is actually there.
                    let avail = len.saturating_sub(body);
                    data_bytes = Some(size.min(avail));
                    if fmt.is_some() {
                        break;
                    }
                }
                _ => {}
            }
            // Chunks are word-aligned.
            pos = body + size + (size & 1);
            if size == 0 {
                break;
            }
        }
        let (tag, channels, rate, byte_rate) =
            fmt.ok_or_else(|| parse_err("WAV file has no fmt chunk"))?;
        let mut stream = audio_stream();
        stream.codec = Some(super::wave_codec_name(tag));
        if rate > 0 {
            stream.sample_rate = Some(f64::from(rate));
        }
        if channels > 0 {
            stream.channels = Some(channels);
        }
        let mut out = empty_result("wav");
        if let (Some(bytes), true) = (data_bytes, byte_rate > 0) {
            out.duration_ms = Some(bytes.saturating_mul(1000) / u64::from(byte_rate));
        }
        if byte_rate > 0 {
            out.bitrate_bps = Some(u64::from(byte_rate) * 8);
        }
        out.streams.push(stream);
        Ok(out)
    }
}

// ----------------------------------------------------------------
// AVI (RIFF/AVI)
// ----------------------------------------------------------------

pub mod avi {
    use super::*;

    #[derive(Default)]
    struct AviState {
        streams: Vec<StreamInfo>,
        durations_ms: Vec<u64>,
        avih_micro_per_frame: u64,
        avih_total_frames: u64,
    }

    pub fn probe(r: &Reader) -> PResult<ProbeResult> {
        let mut st = AviState::default();
        walk(r, 12, r.len(), 0, &mut st)?;
        let mut out = empty_result("avi");
        out.streams = st.streams;
        let mut duration = st.durations_ms.iter().max().copied();
        if duration.is_none() && st.avih_micro_per_frame > 0 && st.avih_total_frames > 0 {
            duration = Some(st.avih_total_frames * st.avih_micro_per_frame / 1000);
        }
        out.duration_ms = duration;
        Ok(out)
    }

    fn walk(r: &Reader, start: u64, end: u64, depth: u32, st: &mut AviState) -> PResult<()> {
        if depth > 4 {
            return Ok(());
        }
        let mut pos = start;
        for _ in 0..4096 {
            if pos + 8 > end || pos + 8 > r.len() {
                return Ok(());
            }
            let id = r.tag(pos)?;
            let size = u64::from(r.u32le(pos + 4)?);
            let body = pos + 8;
            let body_end = body.saturating_add(size).min(end);
            match &id {
                b"LIST" | b"RIFF" => {
                    if size >= 4 {
                        let ltype = r.tag(body)?;
                        if &ltype == b"hdrl" || &ltype == b"strl" {
                            walk(r, body + 4, body_end, depth + 1, st)?;
                        }
                    }
                }
                b"avih" => {
                    if size >= 40 {
                        st.avih_micro_per_frame = u64::from(r.u32le(body)?);
                        st.avih_total_frames = u64::from(r.u32le(body + 16)?);
                    }
                }
                b"strh" => {
                    if size >= 56 {
                        let fcc = r.tag(body)?;
                        let handler = r.tag(body + 4)?;
                        let scale = u64::from(r.u32le(body + 20)?);
                        let rate = u64::from(r.u32le(body + 24)?);
                        let length = u64::from(r.u32le(body + 32)?);
                        let kind = match &fcc {
                            b"vids" => "video",
                            b"auds" => "audio",
                            b"txts" | b"subs" => "subtitle",
                            _ => "other",
                        }
                        .to_string();
                        let mut s = StreamInfo {
                            kind,
                            ..Default::default()
                        };
                        // Frames per second is rate / scale for a
                        // video stream.
                        if s.kind == "video" {
                            s.frame_rate = reduced_ratio(u128::from(rate), u128::from(scale));
                        }
                        // Codec: the handler fourcc, unless it is
                        // all zeros (then strf carries it).
                        if handler != [0, 0, 0, 0] {
                            s.codec = Some(fourcc_string(&handler));
                        }
                        let stream_ms = length
                            .saturating_mul(scale)
                            .saturating_mul(1000)
                            .checked_div(rate)
                            .unwrap_or(0);
                        st.durations_ms.push(stream_ms);
                        st.streams.push(s);
                    }
                }
                b"strf" => {
                    if let Some(s) = st.streams.last_mut() {
                        if s.kind == "video" && size >= 16 {
                            // BITMAPINFOHEADER.
                            let w = r.u32le(body + 4)? as i32;
                            let h = r.u32le(body + 8)? as i32;
                            if w > 0 {
                                s.width = Some(w as u32);
                            }
                            if h != 0 {
                                s.height = Some(h.unsigned_abs());
                            }
                            if s.codec.is_none() {
                                let comp = r.tag(body + 16)?;
                                if comp != [0, 0, 0, 0] {
                                    s.codec = Some(fourcc_string(&comp));
                                }
                            }
                        } else if s.kind == "audio" && size >= 14 {
                            // WAVEFORMATEX.
                            let tag = r.u16le(body)?;
                            if s.codec.is_none() {
                                s.codec = Some(super::wave_codec_name(tag));
                            }
                            let ch = r.u16le(body + 2)?;
                            if ch > 0 {
                                s.channels = Some(u32::from(ch));
                            }
                            let rate = r.u32le(body + 4)?;
                            if rate > 0 {
                                s.sample_rate = Some(f64::from(rate));
                            }
                        }
                    }
                }
                _ => {}
            }
            pos = body + size + (size & 1);
            if size == 0 && &id != b"LIST" {
                // Zero-size non-list chunk: keep advancing by the
                // header so a corrupt file cannot spin the walk.
                pos = body;
            }
        }
        Ok(())
    }

    fn fourcc_string(bytes: &[u8; 4]) -> String {
        bytes
            .iter()
            .map(|&b| {
                if (0x20..0x7F).contains(&b) {
                    b as char
                } else {
                    '?'
                }
            })
            .collect::<String>()
            .trim_end()
            .to_string()
    }
}

// ----------------------------------------------------------------
// MP3 (MPEG audio, optional ID3v2 prefix)
// ----------------------------------------------------------------

pub mod mp3 {
    use super::*;

    const BITRATES_V1: [[u32; 15]; 3] = [
        [
            0, 32, 64, 96, 128, 160, 192, 224, 256, 288, 320, 352, 384, 416, 448,
        ], // Layer I
        [
            0, 32, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384,
        ], // Layer II
        [
            0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320,
        ], // Layer III
    ];
    const BITRATES_V2: [[u32; 15]; 3] = [
        [
            0, 32, 48, 56, 64, 80, 96, 112, 128, 144, 160, 176, 192, 224, 256,
        ], // Layer I
        [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160], // Layer II
        [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160], // Layer III
    ];

    #[derive(Clone, Copy, Debug)]
    struct FrameHeader {
        version: u8, // 1, 2, or 25 (meaning 2.5)
        bitrate_kbps: u32,
        sample_rate: u32,
        channels: u32,
        frame_len: u64,
        samples_per_frame: u32,
    }

    fn parse_header(b: &[u8; 4]) -> Option<FrameHeader> {
        if b[0] != 0xFF || b[1] & 0xE0 != 0xE0 {
            return None;
        }
        let version = match (b[1] >> 3) & 0x03 {
            0 => 25,
            2 => 2,
            3 => 1,
            _ => return None,
        };
        let layer: u8 = match (b[1] >> 1) & 0x03 {
            1 => 3,
            2 => 2,
            3 => 1,
            _ => return None,
        };
        let br_idx = usize::from(b[2] >> 4);
        if br_idx == 0 || br_idx == 15 {
            return None;
        }
        let sr_idx = usize::from((b[2] >> 2) & 0x03);
        if sr_idx == 3 {
            return None;
        }
        let base_rate: u32 = [44100, 48000, 32000][sr_idx];
        let sample_rate = match version {
            1 => base_rate,
            2 => base_rate / 2,
            _ => base_rate / 4,
        };
        let bitrate_kbps = if version == 1 {
            BITRATES_V1[usize::from(layer - 1)][br_idx]
        } else {
            BITRATES_V2[usize::from(layer - 1)][br_idx]
        };
        if bitrate_kbps == 0 {
            return None;
        }
        let padding = u32::from((b[2] >> 1) & 0x01);
        let channel_mode = (b[3] >> 6) & 0x03;
        let channels = if channel_mode == 3 { 1 } else { 2 };
        let samples_per_frame: u32 = match (layer, version) {
            (1, _) => 384,
            (2, _) => 1152,
            (3, 1) => 1152,
            (3, _) => 576,
            _ => return None,
        };
        let br = u64::from(bitrate_kbps) * 1000;
        let sr = u64::from(sample_rate);
        let frame_len = match layer {
            1 => ((12 * br / sr) + u64::from(padding)) * 4,
            2 => 144 * br / sr + u64::from(padding),
            3 if version == 1 => 144 * br / sr + u64::from(padding),
            3 => 72 * br / sr + u64::from(padding),
            _ => return None,
        };
        if frame_len < 4 {
            return None;
        }
        Some(FrameHeader {
            version,
            bitrate_kbps,
            sample_rate,
            channels,
            frame_len,
            samples_per_frame,
        })
    }

    /// Offset of the audio data (after an ID3v2 tag if present).
    fn audio_start(r: &Reader) -> PResult<u64> {
        if r.len() >= 10 {
            let fixed = r.exact(0, 10)?;
            if &fixed[0..3] == b"ID3" {
                // Synchsafe size: 7 usable bits per byte.
                let size = (u64::from(fixed[6] & 0x7F) << 21)
                    | (u64::from(fixed[7] & 0x7F) << 14)
                    | (u64::from(fixed[8] & 0x7F) << 7)
                    | u64::from(fixed[9] & 0x7F);
                let footer = if fixed[5] & 0x10 != 0 { 10 } else { 0 };
                return Ok(10 + size + footer);
            }
        }
        Ok(0)
    }

    /// End of the audio data (before an ID3v1 tag if present).
    fn audio_end(r: &Reader) -> u64 {
        let len = r.len();
        if len >= 128 {
            if let Ok(tag) = r.exact(len - 128, 3) {
                if &tag == b"TAG" {
                    return len - 128;
                }
            }
        }
        len
    }

    /// Find the first valid frame header at or after `from`,
    /// validating each candidate against the frame that should
    /// follow it (so random 0xFF bytes inside a tag are not
    /// mistaken for audio). Bounded scan.
    fn find_frame(r: &Reader, from: u64, end: u64) -> Option<(u64, FrameHeader)> {
        let scan_limit = from.saturating_add(64 * 1024).min(end);
        let mut pos = from;
        while pos + 4 <= scan_limit {
            let raw = r.exact(pos, 4).ok()?;
            let b = [raw[0], raw[1], raw[2], raw[3]];
            if let Some(h) = parse_header(&b) {
                let next = pos + h.frame_len;
                if next + 4 > end {
                    // Last frame in the data: nothing to validate
                    // against; accept it.
                    return Some((pos, h));
                }
                if let Ok(raw2) = r.exact(next, 4) {
                    let b2 = [raw2[0], raw2[1], raw2[2], raw2[3]];
                    if parse_header(&b2).is_some() {
                        return Some((pos, h));
                    }
                }
            }
            pos += 1;
        }
        None
    }

    /// Detection helper: a sync pattern exists shortly after
    /// the (optional) ID3v2 tag.
    pub fn looks_like_mp3(r: &Reader) -> bool {
        if r.len() < 4 {
            return false;
        }
        let start = audio_start(r).unwrap_or(0);
        find_frame(r, start, audio_end(r)).is_some()
    }

    pub fn probe(r: &Reader) -> PResult<ProbeResult> {
        let start = audio_start(r)?;
        let end = audio_end(r);
        let (frame_off, h) = find_frame(r, start, end)
            .ok_or_else(|| parse_err("no valid MPEG audio frame header found"))?;
        let audio_bytes = end.saturating_sub(start);

        let mut out = empty_result("mp3");
        let mut stream = audio_stream();
        stream.codec = Some("mp3".to_string());
        stream.sample_rate = Some(f64::from(h.sample_rate));
        stream.channels = Some(h.channels);
        out.bitrate_bps = Some(u64::from(h.bitrate_kbps) * 1000);

        // Xing / Info header: sits right after the first frame's
        // side information. When it carries a frame count the
        // duration is exact; otherwise fall back to the
        // size-at-first-frame-bitrate estimate.
        let side_info: u64 = match (h.version, h.channels) {
            (1, 1) => 17,
            (1, _) => 32,
            (_, 1) => 9,
            (_, _) => 17,
        };
        let xing_off = frame_off + 4 + side_info;
        let mut exact_ms: Option<u64> = None;
        if xing_off + 8 <= end {
            if let Ok(tag) = r.exact(xing_off, 4) {
                if &tag == b"Xing" || &tag == b"Info" {
                    let flags = r.u32be(xing_off + 4)?;
                    if flags & 0x01 != 0 && xing_off + 12 <= end {
                        let frames = u64::from(r.u32be(xing_off + 8)?);
                        if frames > 0 {
                            exact_ms = Some(
                                frames * u64::from(h.samples_per_frame) * 1000
                                    / u64::from(h.sample_rate),
                            );
                        }
                    }
                }
            }
        }
        match exact_ms {
            Some(ms) => {
                out.duration_ms = Some(ms);
                out.duration_is_estimate = false;
            }
            None => {
                out.duration_ms = Some(audio_bytes.saturating_mul(8) / u64::from(h.bitrate_kbps));
                out.duration_is_estimate = true;
            }
        }
        out.streams.push(stream);
        Ok(out)
    }
}

// ----------------------------------------------------------------
// FLAC
// ----------------------------------------------------------------

pub mod flac {
    use super::*;

    pub fn probe(r: &Reader) -> PResult<ProbeResult> {
        // First metadata block must be STREAMINFO (type 0), 34
        // bytes: min/max block+frame sizes (16 bytes), then a
        // packed field: sample rate (20 bits), channels-1
        // (3 bits), bits-per-sample-1 (5 bits), total samples
        // (36 bits), then a 16-byte MD5.
        let header = r.exact(4, 4)?;
        let block_type = header[0] & 0x7F;
        let block_len =
            (u32::from(header[1]) << 16) | (u32::from(header[2]) << 8) | u32::from(header[3]);
        if block_type != 0 {
            return Err(parse_err(format!(
                "FLAC first metadata block is type {block_type}, not STREAMINFO"
            )));
        }
        if block_len < 34 {
            return Err(parse_err("FLAC STREAMINFO shorter than 34 bytes"));
        }
        let packed = r.u64be(8 + 16)?;
        let sample_rate = (packed >> 44) as u32;
        let channels = ((packed >> 41) & 0x07) as u32 + 1;
        let total_samples = packed & 0x0F_FFFF_FFFF;
        let mut out = empty_result("flac");
        let mut stream = audio_stream();
        stream.codec = Some("flac".to_string());
        if sample_rate > 0 {
            stream.sample_rate = Some(f64::from(sample_rate));
        }
        stream.channels = Some(channels);
        if sample_rate > 0 && total_samples > 0 {
            out.duration_ms = Some(total_samples * 1000 / u64::from(sample_rate));
        }
        out.streams.push(stream);
        Ok(out)
    }
}

// ----------------------------------------------------------------
// OGG (Vorbis / Opus)
// ----------------------------------------------------------------

pub mod ogg {
    use super::*;

    struct Page {
        body_off: u64,
        segments: Vec<u8>,
    }

    /// Parse the page header at `off`. Returns the page and the
    /// offset just past its body.
    fn read_page(r: &Reader, off: u64) -> PResult<Option<(Page, u64)>> {
        if off + 27 > r.len() {
            return Ok(None);
        }
        let tag = r.tag(off)?;
        if &tag != b"OggS" {
            return Ok(None);
        }
        let nseg = usize::from(r.u8(off + 26)?);
        let seg_bytes = r.exact(off + 27, nseg)?;
        let body_len: u64 = seg_bytes.iter().map(|&s| u64::from(s)).sum();
        let body_off = off + 27 + nseg as u64;
        let next = body_off + body_len;
        Ok(Some((
            Page {
                body_off,
                segments: seg_bytes,
            },
            next,
        )))
    }

    /// The first complete packet in the stream (assembled across
    /// pages if it spans them). Bounded pages and bytes.
    fn first_packet(r: &Reader) -> PResult<Vec<u8>> {
        let mut packet: Vec<u8> = Vec::new();
        let mut off = 0u64;
        for _ in 0..64 {
            let (page, next) = match read_page(r, off)? {
                Some(p) => p,
                None => break,
            };
            let mut seg_pos = page.body_off;
            let mut completes = false;
            for &seg in &page.segments {
                let n = usize::from(seg);
                if packet.len() + n > 1024 * 1024 {
                    return Err(parse_err("OGG first packet exceeds the 1 MiB probe cap"));
                }
                packet.extend_from_slice(&r.exact(seg_pos, n)?);
                seg_pos += seg as u64;
                if seg < 255 {
                    completes = true;
                    break;
                }
            }
            if completes {
                return Ok(packet);
            }
            off = next;
        }
        Err(parse_err("OGG stream has no complete first packet"))
    }

    /// Granule position of the last page in the file, from a
    /// bounded tail window. Pages whose granule is -1 (no packet
    /// ends on them) are skipped walking backwards.
    fn last_granule(r: &Reader) -> Option<u64> {
        let len = r.len();
        let window = len.min(64 * 1024);
        let data = r.exact(len - window, window as usize).ok()?;
        let base = len - window;
        let mut candidates: Vec<u64> = Vec::new();
        let mut i = 0usize;
        while i + 4 <= data.len() {
            if &data[i..i + 4] == b"OggS" {
                candidates.push(base + i as u64);
                i += 4;
            } else {
                i += 1;
            }
        }
        for &off in candidates.iter().rev() {
            if off + 27 <= len {
                if let Ok(granule) = r.u64le(off + 6) {
                    if granule != u64::MAX {
                        return Some(granule);
                    }
                }
            }
        }
        None
    }

    pub fn probe(r: &Reader) -> PResult<ProbeResult> {
        let packet = first_packet(r)?;
        let mut out = empty_result("ogg");
        let mut stream = audio_stream();
        if packet.starts_with(b"\x01vorbis") {
            stream.codec = Some("vorbis".to_string());
            if packet.len() >= 16 {
                // Identification header: byte 11 is channels,
                // bytes 12..16 the sample rate (little endian).
                let channels = u32::from(packet[11]);
                let rate = u32::from_le_bytes([packet[12], packet[13], packet[14], packet[15]]);
                if channels > 0 {
                    stream.channels = Some(channels);
                }
                if rate > 0 {
                    stream.sample_rate = Some(f64::from(rate));
                    if let Some(granule) = last_granule(r) {
                        out.duration_ms = Some(granule * 1000 / u64::from(rate));
                    }
                }
            }
        } else if packet.starts_with(b"OpusHead") {
            stream.codec = Some("opus".to_string());
            let mut preskip: u64 = 0;
            if packet.len() >= 16 {
                let channels = u32::from(packet[9]);
                if channels > 0 {
                    stream.channels = Some(channels);
                }
                preskip = u64::from(u16::from_le_bytes([packet[10], packet[11]]));
                let input_rate =
                    u32::from_le_bytes([packet[12], packet[13], packet[14], packet[15]]);
                if input_rate > 0 {
                    stream.sample_rate = Some(f64::from(input_rate));
                }
            }
            // Opus granule positions count 48 kHz samples and
            // include the preskip.
            if let Some(granule) = last_granule(r) {
                let samples = granule.saturating_sub(preskip);
                out.duration_ms = Some(samples * 1000 / 48000);
            }
        } else if !packet.is_empty() {
            // A real Ogg stream whose codec we do not identify in
            // v0.1: honest "other" stream, no invented codec.
            stream.kind = "other".to_string();
        }
        out.streams.push(stream);
        Ok(out)
    }
}

// ----------------------------------------------------------------
// MP4 / MOV (ISO base media file format)
// ----------------------------------------------------------------

pub mod mp4 {
    use super::*;

    const MAX_BOXES: usize = 4096;
    const MAX_DEPTH: u32 = 10;

    #[derive(Default)]
    struct Track {
        kind: Option<String>,
        codec: Option<String>,
        width: Option<u32>,
        height: Option<u32>,
        sample_rate: Option<f64>,
        channels: Option<u32>,
        language: Option<String>,
        duration_ms: Option<u64>,
        timescale: Option<u64>,
        /// (total samples, total sample time) from `stts`, when the
        /// table fits the read budget.
        sample_timing: Option<(u128, u128)>,
        field_order: Option<String>,
    }

    /// Most `stts` entries read for a frame rate: 8 bytes each, so
    /// 512 KiB at most. Longer tables leave the rate `null`.
    const MAX_STTS_ENTRIES: u64 = 65_536;
    /// Most child boxes walked inside one visual sample entry.
    const MAX_ENTRY_CHILDREN: usize = 32;

    #[derive(Default)]
    struct Ctx {
        container: String,
        boxes: usize,
        movie_duration_ms: Option<u64>,
        tracks: Vec<Track>,
        current: Option<usize>,
    }

    pub fn probe(r: &Reader) -> PResult<ProbeResult> {
        let mut ctx = Ctx {
            container: "mp4".to_string(),
            ..Default::default()
        };
        walk(r, 0, r.len(), 0, &mut ctx)?;
        let mut out = empty_result(&ctx.container);
        out.duration_ms = ctx
            .movie_duration_ms
            .or_else(|| ctx.tracks.iter().filter_map(|t| t.duration_ms).max());
        for t in ctx.tracks {
            let kind = t.kind.unwrap_or_else(|| "other".to_string());
            // Average rate: samples × timescale ÷ total sample time.
            let frame_rate = match (kind.as_str(), t.sample_timing, t.timescale) {
                ("video", Some((samples, time)), Some(scale)) => {
                    reduced_ratio(samples.saturating_mul(u128::from(scale)), time)
                }
                _ => None,
            };
            let field_order = if kind == "video" { t.field_order } else { None };
            out.streams.push(StreamInfo {
                kind,
                codec: t.codec,
                width: t.width,
                height: t.height,
                sample_rate: t.sample_rate,
                channels: t.channels,
                language: t.language,
                frame_rate,
                field_order,
            });
        }
        Ok(out)
    }

    fn read_header(r: &Reader, pos: u64, parent_end: u64) -> PResult<Option<(u64, [u8; 4], u64)>> {
        // Overflow-safe form of "fewer than 8 bytes remain":
        // a corrupt size can push pos near u64::MAX.
        let limit = parent_end.min(r.len());
        if pos > limit || limit - pos < 8 {
            return Ok(None);
        }
        let size32 = r.u32be(pos)?;
        let ty = r.tag(pos + 4)?;
        let (total, header_len) = match size32 {
            1 => {
                let big = r.u64be(pos + 8)?;
                (big, 16u64)
            }
            0 => (parent_end - pos, 8u64),
            s => (u64::from(s), 8u64),
        };
        if total < header_len {
            return Err(parse_err(format!(
                "MP4 box at offset {pos} declares size {total} smaller than its header"
            )));
        }
        Ok(Some((total, ty, header_len)))
    }

    fn track_mut(ctx: &mut Ctx) -> Option<&mut Track> {
        ctx.current.and_then(|i| ctx.tracks.get_mut(i))
    }

    fn walk(r: &Reader, start: u64, end: u64, depth: u32, ctx: &mut Ctx) -> PResult<()> {
        if depth > MAX_DEPTH {
            return Ok(());
        }
        let mut pos = start;
        loop {
            ctx.boxes += 1;
            if ctx.boxes > MAX_BOXES {
                return Err(parse_err("MP4 box count exceeds the probe budget"));
            }
            let (total, ty, header_len) = match read_header(r, pos, end)? {
                Some(h) => h,
                None => return Ok(()),
            };
            let body = pos + header_len;
            let box_end = pos.saturating_add(total).min(end).min(r.len());
            match &ty {
                b"ftyp" | b"styp" => {
                    if total >= 12 {
                        let brand = r.tag(body)?;
                        if &brand == b"qt  " {
                            ctx.container = "mov".to_string();
                        }
                    }
                }
                b"moov" | b"mdia" | b"minf" | b"stbl" => {
                    walk(r, body, box_end, depth + 1, ctx)?;
                }
                b"trak" => {
                    ctx.tracks.push(Track::default());
                    ctx.current = Some(ctx.tracks.len() - 1);
                    walk(r, body, box_end, depth + 1, ctx)?;
                    ctx.current = None;
                }
                b"mvhd" => {
                    let version = r.u8(body)?;
                    let (timescale, duration) = match version {
                        1 => (u64::from(r.u32be(body + 4 + 16)?), r.u64be(body + 4 + 20)?),
                        _ => (
                            u64::from(r.u32be(body + 4 + 8)?),
                            u64::from(r.u32be(body + 4 + 12)?),
                        ),
                    };
                    ctx.movie_duration_ms = duration.saturating_mul(1000).checked_div(timescale);
                }
                b"tkhd" => {
                    // Width/height are the last 8 payload bytes,
                    // 16.16 fixed point, in every tkhd version.
                    if total >= header_len + 8 {
                        let w = r.u32be(box_end - 8)?;
                        let h = r.u32be(box_end - 4)?;
                        if let Some(t) = track_mut(ctx) {
                            if w >> 16 > 0 {
                                t.width = Some(w >> 16);
                            }
                            if h >> 16 > 0 {
                                t.height = Some(h >> 16);
                            }
                        }
                    }
                }
                b"mdhd" => {
                    let version = r.u8(body)?;
                    let (timescale, duration, lang_off) = match version {
                        1 => (
                            u64::from(r.u32be(body + 4 + 16)?),
                            r.u64be(body + 4 + 20)?,
                            body + 4 + 28,
                        ),
                        _ => (
                            u64::from(r.u32be(body + 4 + 8)?),
                            u64::from(r.u32be(body + 4 + 12)?),
                            body + 4 + 16,
                        ),
                    };
                    let packed = r.u16be(lang_off)?;
                    if let Some(t) = track_mut(ctx) {
                        if timescale > 0 {
                            t.timescale = Some(timescale);
                        }
                        t.duration_ms = duration.saturating_mul(1000).checked_div(timescale);
                        // Values below 0x400 are Macintosh language codes
                        // (QuickTime), not packed ISO 639-2 letters; they
                        // are not reported rather than translated.
                        if packed >= 0x400 {
                            let code = decode_packed_language(packed);
                            if code.len() == 3 {
                                t.language = Some(code);
                            }
                        }
                    }
                }
                // QuickTime tracks carry a second hdlr inside minf whose
                // component type is "dhlr" (a data-reference handler,
                // subtype "alis"/"url "); it says nothing about the
                // media kind and must not overwrite the mdia one.
                b"hdlr" if &r.tag(body + 4)? != b"dhlr" => {
                    let handler = r.tag(body + 8)?;
                    if let Some(t) = track_mut(ctx) {
                        t.kind = Some(
                            match &handler {
                                b"vide" => "video",
                                b"soun" => "audio",
                                b"text" | b"sbtl" | b"subt" | b"stpp" => "subtitle",
                                _ => "other",
                            }
                            .to_string(),
                        );
                    }
                }
                b"elng" => {
                    // version/flags, then a NUL-terminated string.
                    let avail = (box_end - body).min(256) as usize;
                    if avail > 4 {
                        let raw = r.exact(body + 4, avail - 4)?;
                        let text: Vec<u8> = raw.iter().take_while(|&&b| b != 0).copied().collect();
                        if let Some(t) = track_mut(ctx) {
                            if !text.is_empty() {
                                t.language = Some(String::from_utf8_lossy(&text).into_owned());
                            }
                        }
                    }
                }
                b"stsd" => {
                    parse_stsd(r, body, box_end, ctx)?;
                }
                // version/flags, entry count, then (sample count,
                // sample delta) pairs.
                b"stts" if body + 8 <= box_end => {
                    let entries = u64::from(r.u32be(body + 4)?);
                    if entries > 0
                        && entries <= MAX_STTS_ENTRIES
                        && body + 8 + entries * 8 <= box_end
                    {
                        let table = r.exact(body + 8, (entries * 8) as usize)?;
                        let (mut samples, mut time) = (0u128, 0u128);
                        for pair in table.chunks_exact(8) {
                            let count = u32::from_be_bytes([pair[0], pair[1], pair[2], pair[3]]);
                            let delta = u32::from_be_bytes([pair[4], pair[5], pair[6], pair[7]]);
                            samples += u128::from(count);
                            time += u128::from(count) * u128::from(delta);
                        }
                        if let Some(t) = track_mut(ctx) {
                            t.sample_timing = Some((samples, time));
                        }
                    }
                }
                _ => {}
            }
            if total == 0 {
                return Ok(());
            }
            pos = pos.saturating_add(total);
        }
    }

    fn parse_stsd(r: &Reader, body: u64, box_end: u64, ctx: &mut Ctx) -> PResult<()> {
        // version/flags + entry count, then entries of
        // (size, type, ...). Only the first entry is reported.
        if body + 8 > box_end {
            return Ok(());
        }
        let count = r.u32be(body + 4)?;
        if count == 0 {
            return Ok(());
        }
        let entry = body + 8;
        if entry + 8 > box_end {
            return Ok(());
        }
        let ty = r.tag(entry + 4)?;
        let entry_end = entry
            .saturating_add(u64::from(r.u32be(entry)?))
            .min(box_end);
        let is_video_track = track_mut(ctx).map(|t| t.kind.as_deref()) == Some(Some("video"));
        if is_video_track {
            let order = visual_entry_field_order(r, entry, entry_end)?;
            if let Some(t) = track_mut(ctx) {
                t.field_order = order;
            }
        }
        let codec = String::from_utf8_lossy(&ty)
            .trim()
            .trim_matches('\0')
            .to_string();
        let is_audio_track = track_mut(ctx).map(|t| t.kind.as_deref()) == Some(Some("audio"));
        if let Some(t) = track_mut(ctx) {
            if !codec.is_empty() {
                t.codec = Some(codec);
            }
        }
        // Classic QuickTime/ISO audio sample entry layout:
        // channels at entry+24, sample rate (16.16) at entry+32.
        // Only trusted when the track is audio (video sample
        // entries lay those bytes out differently).
        if is_audio_track && entry + 36 <= box_end {
            let channels = u32::from(r.u16be(entry + 24)?);
            let rate_fixed = r.u32be(entry + 32)?;
            if let Some(t) = track_mut(ctx) {
                if (1..=64).contains(&channels) {
                    t.channels = Some(channels);
                }
                if rate_fixed >> 16 > 0 {
                    t.sample_rate = Some(f64::from(rate_fixed >> 16));
                }
            }
        }
        Ok(())
    }

    /// The `fiel` atom among a visual sample entry's child boxes,
    /// which follow the 86-byte entry header: field count (1 =
    /// progressive, 2 = interlaced) and, for 2, the order code.
    fn visual_entry_field_order(r: &Reader, entry: u64, entry_end: u64) -> PResult<Option<String>> {
        let mut pos = entry + 86;
        for _ in 0..MAX_ENTRY_CHILDREN {
            if pos + 8 > entry_end {
                return Ok(None);
            }
            let size = u64::from(r.u32be(pos)?);
            let ty = r.tag(pos + 4)?;
            if size < 8 {
                return Ok(None);
            }
            if &ty == b"fiel" && size >= 10 && pos + 10 <= entry_end {
                let fields = r.u8(pos + 8)?;
                let detail = r.u8(pos + 9)?;
                return Ok(match fields {
                    1 => field_order_name(0),
                    2 => field_order_name(u64::from(detail)),
                    _ => None,
                });
            }
            pos = pos.saturating_add(size);
        }
        Ok(None)
    }

    /// mdhd language: three 5-bit values, each + 0x60 gives the
    /// ASCII letter of an ISO 639-2 code.
    fn decode_packed_language(packed: u16) -> String {
        // Only 1..=26 map to 'a'..='z'; anything else (QuickTime's
        // 0x7FFF "unspecified", for one) yields no letter, so the
        // caller's three-letter check rejects the code.
        let c = |shift: u16| -> char {
            let v = (packed >> shift) & 0x1F;
            if (1..=26).contains(&v) {
                char::from_u32(u32::from(v) + 0x60).unwrap_or('\0')
            } else {
                '\0'
            }
        };
        [c(10), c(5), c(0)]
            .into_iter()
            .filter(|&ch| ch != '\0')
            .collect()
    }
}

// ----------------------------------------------------------------
// Matroska / WebM (EBML subset)
// ----------------------------------------------------------------

pub mod matroska {
    use super::*;

    const ID_EBML: u32 = 0x1A45_DFA3;
    const ID_SEGMENT: u32 = 0x1853_8067;
    const ID_INFO: u32 = 0x1549_A966;
    const ID_TRACKS: u32 = 0x1654_AE6B;
    const ID_TRACK_ENTRY: u32 = 0xAE;
    const ID_VIDEO: u32 = 0xE0;
    const ID_AUDIO: u32 = 0xE1;

    const MAX_ELEMENTS: usize = 8192;

    #[derive(Default)]
    struct Ctx {
        elements: usize,
        timecode_scale: u64,
        duration_units: Option<f64>,
        tracks: Vec<StreamInfo>,
        doc_type: String,
    }

    /// Read an EBML element ID at `pos`: 1-4 bytes, marker bits
    /// kept. Returns (id, byte length).
    fn read_id(r: &Reader, pos: u64, end: u64) -> PResult<Option<(u32, u64)>> {
        if pos >= end || pos >= r.len() {
            return Ok(None);
        }
        let first = r.u8(pos)?;
        if first == 0 {
            return Err(parse_err(format!(
                "invalid EBML element ID at offset {pos}"
            )));
        }
        let len = u64::from(first.leading_zeros() + 1);
        if len > 4 {
            return Err(parse_err(format!(
                "EBML element ID too long at offset {pos}"
            )));
        }
        if pos + len > end {
            return Err(parse_err("EBML element ID runs past its parent"));
        }
        let bytes = r.exact(pos, len as usize)?;
        let mut id: u32 = 0;
        for b in &bytes {
            id = (id << 8) | u32::from(*b);
        }
        Ok(Some((id, len)))
    }

    /// Read an EBML data size at `pos`: a variable-length int
    /// whose all-ones value means "unknown size" (None).
    fn read_size(r: &Reader, pos: u64, end: u64) -> PResult<(Option<u64>, u64)> {
        if pos >= end || pos >= r.len() {
            return Err(parse_err("EBML size field past end of input"));
        }
        let first = r.u8(pos)?;
        if first == 0 {
            return Err(parse_err(format!("invalid EBML size at offset {pos}")));
        }
        let len = u64::from(first.leading_zeros() + 1);
        if pos + len > end {
            return Err(parse_err("EBML size field runs past its parent"));
        }
        let bytes = r.exact(pos, len as usize)?;
        // Value bits of the first byte: everything below the
        // marker bit. (Written as a mask so len == 8 cannot
        // shift a u8 by its full width.)
        let mut value: u64 = u64::from(first) & ((1u64 << (8 - len)) - 1);
        for b in &bytes[1..] {
            value = (value << 8) | u64::from(*b);
        }
        let unknown = value == (1u64 << (7 * len)) - 1;
        Ok((if unknown { None } else { Some(value) }, len))
    }

    fn read_uint(r: &Reader, pos: u64, size: u64) -> PResult<u64> {
        if size > 8 {
            return Err(parse_err("EBML unsigned integer longer than 8 bytes"));
        }
        let bytes = r.exact(pos, size as usize)?;
        let mut v: u64 = 0;
        for b in &bytes {
            v = (v << 8) | u64::from(*b);
        }
        Ok(v)
    }

    fn read_float(r: &Reader, pos: u64, size: u64) -> PResult<f64> {
        match size {
            4 => Ok(f64::from(f32::from_bits(r.u32be(pos)?))),
            8 => Ok(f64::from_bits(r.u64be(pos)?)),
            _ => Err(parse_err(format!(
                "EBML float with unsupported size {size}"
            ))),
        }
    }

    fn read_string(r: &Reader, pos: u64, size: u64) -> PResult<String> {
        if size > 4096 {
            return Err(parse_err("EBML string element exceeds the probe cap"));
        }
        let bytes = r.exact(pos, size as usize)?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    fn count(ctx: &mut Ctx) -> PResult<()> {
        ctx.elements += 1;
        if ctx.elements > MAX_ELEMENTS {
            return Err(parse_err("EBML element count exceeds the probe budget"));
        }
        Ok(())
    }

    pub fn probe(r: &Reader) -> PResult<ProbeResult> {
        let mut ctx = Ctx {
            timecode_scale: 1_000_000,
            doc_type: String::new(),
            ..Default::default()
        };
        // EBML header.
        let (id, id_len) = read_id(r, 0, r.len())?.ok_or_else(|| parse_err("empty EBML input"))?;
        if id != ID_EBML {
            return Err(parse_err("missing EBML header element"));
        }
        let (size, size_len) = read_size(r, id_len, r.len())?;
        let head_start = id_len + size_len;
        let head_end = match size {
            Some(s) => head_start.saturating_add(s).min(r.len()),
            None => r.len(),
        };
        parse_ebml_header(r, head_start, head_end, &mut ctx)?;
        // Segment.
        let (seg_id, seg_id_len) = read_id(r, head_end, r.len())?
            .ok_or_else(|| parse_err("Matroska file has no Segment element"))?;
        if seg_id != ID_SEGMENT {
            return Err(parse_err(format!(
                "expected Segment element, found 0x{seg_id:08x}"
            )));
        }
        let (seg_size, seg_size_len) = read_size(r, head_end + seg_id_len, r.len())?;
        let seg_start = head_end + seg_id_len + seg_size_len;
        let seg_end = match seg_size {
            Some(s) => seg_start.saturating_add(s).min(r.len()),
            None => r.len(),
        };
        parse_segment(r, seg_start, seg_end, &mut ctx)?;

        let container = if ctx.doc_type == "webm" {
            "webm"
        } else {
            "matroska"
        };
        let mut out = empty_result(container);
        if let Some(units) = ctx.duration_units {
            // Duration is in TimecodeScale units; the scale is
            // nanoseconds per unit, so ms = units*scale / 1e6.
            let ms = units * ctx.timecode_scale as f64 / 1_000_000.0;
            if ms.is_finite() && ms >= 0.0 {
                out.duration_ms = Some(ms.round() as u64);
            }
        }
        out.streams = ctx.tracks;
        Ok(out)
    }

    fn parse_ebml_header(r: &Reader, start: u64, end: u64, ctx: &mut Ctx) -> PResult<()> {
        let mut pos = start;
        while pos < end {
            count(ctx)?;
            let Some((id, id_len)) = read_id(r, pos, end)? else {
                break;
            };
            let (size, size_len) = read_size(r, pos + id_len, end)?;
            let body = pos + id_len + size_len;
            let size = size.unwrap_or(end.saturating_sub(body));
            if id == 0x4282 {
                // DocType.
                ctx.doc_type = read_string(r, body, size)?;
            }
            pos = body + size;
        }
        Ok(())
    }

    fn parse_segment(r: &Reader, start: u64, end: u64, ctx: &mut Ctx) -> PResult<()> {
        let mut pos = start;
        let mut have_info = false;
        let mut have_tracks = false;
        while pos < end && !(have_info && have_tracks) {
            count(ctx)?;
            let Some((id, id_len)) = read_id(r, pos, end)? else {
                break;
            };
            let (size, size_len) = read_size(r, pos + id_len, end)?;
            let body = pos + id_len + size_len;
            match (id, size) {
                (ID_INFO, Some(s)) => {
                    parse_info(r, body, (body + s).min(end), ctx)?;
                    have_info = true;
                    pos = body + s;
                }
                (ID_TRACKS, Some(s)) => {
                    parse_tracks(r, body, (body + s).min(end), ctx)?;
                    have_tracks = true;
                    pos = body + s;
                }
                // Clusters and everything else are skipped by
                // size (offset arithmetic only, no reads). An
                // unknown size here cannot be skipped, so the
                // scan stops — Info/Tracks after it are not
                // reachable by bounded probing.
                (_, Some(s)) => pos = body + s,
                (_, None) => break,
            }
        }
        Ok(())
    }

    fn parse_info(r: &Reader, start: u64, end: u64, ctx: &mut Ctx) -> PResult<()> {
        let mut pos = start;
        while pos < end {
            count(ctx)?;
            let Some((id, id_len)) = read_id(r, pos, end)? else {
                break;
            };
            let (size, size_len) = read_size(r, pos + id_len, end)?;
            let body = pos + id_len + size_len;
            let size = size.unwrap_or(end.saturating_sub(body));
            match id {
                0x2AD7B1 => ctx.timecode_scale = read_uint(r, body, size)?, // TimecodeScale
                0x4489 => ctx.duration_units = Some(read_float(r, body, size)?), // Duration
                _ => {}
            }
            pos = body + size;
        }
        Ok(())
    }

    fn parse_tracks(r: &Reader, start: u64, end: u64, ctx: &mut Ctx) -> PResult<()> {
        let mut pos = start;
        while pos < end {
            count(ctx)?;
            let Some((id, id_len)) = read_id(r, pos, end)? else {
                break;
            };
            let (size, size_len) = read_size(r, pos + id_len, end)?;
            let body = pos + id_len + size_len;
            let size = size.unwrap_or(end.saturating_sub(body));
            if id == ID_TRACK_ENTRY {
                let stream = parse_track_entry(r, body, (body + size).min(end), ctx)?;
                ctx.tracks.push(stream);
            }
            pos = body + size;
        }
        Ok(())
    }

    fn parse_track_entry(r: &Reader, start: u64, end: u64, ctx: &mut Ctx) -> PResult<StreamInfo> {
        let mut stream = StreamInfo {
            kind: "other".to_string(),
            ..Default::default()
        };
        let mut default_duration = 0u64;
        let mut pos = start;
        while pos < end {
            count(ctx)?;
            let Some((id, id_len)) = read_id(r, pos, end)? else {
                break;
            };
            let (size, size_len) = read_size(r, pos + id_len, end)?;
            let body = pos + id_len + size_len;
            let size = size.unwrap_or(end.saturating_sub(body));
            match id {
                0x83 => {
                    // TrackType.
                    let t = read_uint(r, body, size)?;
                    stream.kind = match t {
                        1 => "video",
                        2 => "audio",
                        17 => "subtitle",
                        _ => "other",
                    }
                    .to_string();
                }
                0x86 => stream.codec = Some(read_string(r, body, size)?), // CodecID
                0x23E383 => default_duration = read_uint(r, body, size)?, // DefaultDuration (ns)
                0x22B59C => stream.language = Some(read_string(r, body, size)?), // Language
                ID_VIDEO => parse_video(r, body, (body + size).min(end), &mut stream, ctx)?,
                ID_AUDIO => parse_audio(r, body, (body + size).min(end), &mut stream, ctx)?,
                _ => {}
            }
            pos = body + size;
        }
        // DefaultDuration is nanoseconds per frame.
        if stream.kind == "video" {
            stream.frame_rate = reduced_ratio(1_000_000_000, u128::from(default_duration));
        } else {
            stream.field_order = None;
        }
        Ok(stream)
    }

    fn parse_video(
        r: &Reader,
        start: u64,
        end: u64,
        stream: &mut StreamInfo,
        ctx: &mut Ctx,
    ) -> PResult<()> {
        let mut flag_interlaced = None;
        let mut field_order = None;
        let mut pos = start;
        while pos < end {
            count(ctx)?;
            let Some((id, id_len)) = read_id(r, pos, end)? else {
                break;
            };
            let (size, size_len) = read_size(r, pos + id_len, end)?;
            let body = pos + id_len + size_len;
            let size = size.unwrap_or(end.saturating_sub(body));
            match id {
                0xB0 => stream.width = Some(read_uint(r, body, size)? as u32), // PixelWidth
                0xBA => stream.height = Some(read_uint(r, body, size)? as u32), // PixelHeight
                0x9A => flag_interlaced = Some(read_uint(r, body, size)?),     // FlagInterlaced
                0x9D => field_order = Some(read_uint(r, body, size)?),         // FieldOrder
                _ => {}
            }
            pos = body + size;
        }
        // FlagInterlaced 2 means progressive; otherwise only a stored
        // FieldOrder says which field comes first (2, the default,
        // means undetermined and stays null).
        stream.field_order = match (flag_interlaced, field_order) {
            (Some(2), _) => field_order_name(0),
            (_, Some(code)) => field_order_name(code),
            _ => None,
        };
        Ok(())
    }

    fn parse_audio(
        r: &Reader,
        start: u64,
        end: u64,
        stream: &mut StreamInfo,
        ctx: &mut Ctx,
    ) -> PResult<()> {
        let mut pos = start;
        while pos < end {
            count(ctx)?;
            let Some((id, id_len)) = read_id(r, pos, end)? else {
                break;
            };
            let (size, size_len) = read_size(r, pos + id_len, end)?;
            let body = pos + id_len + size_len;
            let size = size.unwrap_or(end.saturating_sub(body));
            match id {
                0xB5 => stream.sample_rate = Some(read_float(r, body, size)?), // SamplingFrequency
                0x9F => stream.channels = Some(read_uint(r, body, size)? as u32), // Channels
                _ => {}
            }
            pos = body + size;
        }
        Ok(())
    }
}
