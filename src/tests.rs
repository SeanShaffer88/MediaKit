//! Format tests. Every fixture is built byte-by-byte here so
//! the expected values are known exactly, and most tests go
//! through `probe_bytes` (no filesystem, no path-in-JSON
//! hazards). Truncation tests feed every prefix of a fixture
//! and require a clean Ok/Err, never a panic.

use super::*;

fn probe(data: &[u8]) -> json::Value {
    let text = probe_bytes(data).unwrap_or_else(|(c, m)| panic!("probe failed ({c}): {m}"));
    json::parse(&text).unwrap()
}

fn streams(v: &json::Value) -> &Vec<json::Value> {
    v.get("streams").unwrap().as_array().unwrap()
}

// ------------------------------------------------------------
// WAV
// ------------------------------------------------------------

fn wav_fixture(tag: u16, channels: u16, rate: u32, data_bytes: usize) -> Vec<u8> {
    let bits: u16 = 16;
    let byte_rate = rate * u32::from(channels) * u32::from(bits) / 8;
    let mut v = Vec::new();
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&((36 + data_bytes) as u32).to_le_bytes());
    v.extend_from_slice(b"WAVE");
    v.extend_from_slice(b"fmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&tag.to_le_bytes());
    v.extend_from_slice(&channels.to_le_bytes());
    v.extend_from_slice(&rate.to_le_bytes());
    v.extend_from_slice(&byte_rate.to_le_bytes());
    v.extend_from_slice(&(channels * (bits / 8)).to_le_bytes());
    v.extend_from_slice(&bits.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&(data_bytes as u32).to_le_bytes());
    v.resize(v.len() + data_bytes, 0);
    v
}

#[test]
fn wav_pcm_exact_duration_and_stream() {
    // 44.1 kHz stereo 16-bit: byte rate 176400; 176400 data
    // bytes is exactly one second.
    let v = probe(&wav_fixture(1, 2, 44100, 176_400));
    assert_eq!(v.get("container").unwrap().as_str(), Some("wav"));
    assert_eq!(v.get("duration_ms").unwrap().as_i64(), Some(1000));
    assert_eq!(
        v.get("duration_is_estimate").unwrap().as_bool(),
        Some(false)
    );
    assert_eq!(v.get("bitrate_bps").unwrap().as_i64(), Some(176_400 * 8));
    let st = streams(&v);
    assert_eq!(st.len(), 1);
    assert_eq!(st[0].get("kind").unwrap().as_str(), Some("audio"));
    assert_eq!(st[0].get("codec").unwrap().as_str(), Some("pcm"));
    assert_eq!(st[0].get("sample_rate").unwrap().as_i64(), Some(44100));
    assert_eq!(st[0].get("channels").unwrap().as_i64(), Some(2));
}

#[test]
fn wav_float_tag_and_missing_fmt() {
    // Fixture byte rate is 48000 * 1 ch * 2 bytes = 96000 B/s,
    // so 96000 data bytes is exactly one second.
    let v = probe(&wav_fixture(3, 1, 48000, 96_000));
    assert_eq!(
        streams(&v)[0].get("codec").unwrap().as_str(),
        Some("ieee_float")
    );
    assert_eq!(v.get("duration_ms").unwrap().as_i64(), Some(1000));

    // RIFF/WAVE with a data chunk but no fmt chunk: parse error.
    let mut bad = Vec::new();
    bad.extend_from_slice(b"RIFF");
    bad.extend_from_slice(&20u32.to_le_bytes());
    bad.extend_from_slice(b"WAVE");
    bad.extend_from_slice(b"data");
    bad.extend_from_slice(&4u32.to_le_bytes());
    bad.extend_from_slice(&[0; 4]);
    let err = probe_bytes(&bad).unwrap_err();
    assert_eq!(err.0, ERR_PARSE, "error was: {}", err.1);
}

#[test]
fn wav_probe_from_disk_path() {
    // Exercises me_probe's file path (positioned reads, bounded
    // IO). The path is a C string, never JSON, so no escaping
    // concerns.
    let dir = std::env::temp_dir().join(format!("mediakit-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("tone.wav");
    std::fs::write(&file, wav_fixture(1, 1, 8000, 8000 * 2)).unwrap();
    let text = probe_path(&file.to_string_lossy()).unwrap();
    let v = json::parse(&text).unwrap();
    assert_eq!(v.get("container").unwrap().as_str(), Some("wav"));
    assert_eq!(v.get("duration_ms").unwrap().as_i64(), Some(1000));
    std::fs::remove_dir_all(&dir).ok();
}

// ------------------------------------------------------------
// MP3
// ------------------------------------------------------------

/// One MPEG-1 Layer III frame: 128 kbps, 44.1 kHz, stereo.
/// Frame length = floor(144 * 128000 / 44100) = 417.
fn mp3_frame() -> Vec<u8> {
    let mut f = vec![0u8; 417];
    f[0] = 0xFF;
    f[1] = 0xFB;
    f[2] = 0x90;
    f[3] = 0x00;
    f
}

#[test]
fn mp3_xing_header_gives_exact_duration() {
    let mut data = mp3_frame();
    // Xing at 4 (header) + 32 (MPEG-1 stereo side info) = 36:
    // flags = frames present, frame count = 100.
    data[36..40].copy_from_slice(b"Xing");
    data[40..44].copy_from_slice(&1u32.to_be_bytes());
    data[44..48].copy_from_slice(&100u32.to_be_bytes());
    data.extend_from_slice(&mp3_frame());
    data.extend_from_slice(&mp3_frame());
    let v = probe(&data);
    assert_eq!(v.get("container").unwrap().as_str(), Some("mp3"));
    // 100 frames * 1152 samples / 44100 Hz = 2612 ms.
    assert_eq!(v.get("duration_ms").unwrap().as_i64(), Some(2612));
    assert_eq!(
        v.get("duration_is_estimate").unwrap().as_bool(),
        Some(false)
    );
    assert_eq!(v.get("bitrate_bps").unwrap().as_i64(), Some(128_000));
    let st = streams(&v);
    assert_eq!(st[0].get("codec").unwrap().as_str(), Some("mp3"));
    assert_eq!(st[0].get("sample_rate").unwrap().as_i64(), Some(44100));
    assert_eq!(st[0].get("channels").unwrap().as_i64(), Some(2));
}

#[test]
fn mp3_id3_skip_and_size_estimate() {
    let mut data = Vec::new();
    data.extend_from_slice(b"ID3");
    data.extend_from_slice(&[4, 0]); // version 2.4
    data.push(0); // flags
    data.extend_from_slice(&[0, 0, 0, 20]); // synchsafe size 20
    data.extend_from_slice(&[0u8; 20]); // tag payload
    for _ in 0..10 {
        data.extend_from_slice(&mp3_frame());
    }
    let v = probe(&data);
    assert_eq!(v.get("container").unwrap().as_str(), Some("mp3"));
    // Estimate: 10 frames * 417 bytes = 4170 audio bytes,
    // 4170*8/128 = 260 ms.
    assert_eq!(v.get("duration_ms").unwrap().as_i64(), Some(260));
    assert_eq!(v.get("duration_is_estimate").unwrap().as_bool(), Some(true));
}

#[test]
fn mp3_no_frames_is_a_clean_error() {
    let mut data = Vec::new();
    data.extend_from_slice(b"ID3");
    data.extend_from_slice(&[3, 0, 0, 0, 0, 0, 5]);
    data.extend_from_slice(&[0u8; 5]);
    let err = probe_bytes(&data).unwrap_err();
    assert_eq!(err.0, ERR_PARSE, "error was: {}", err.1);
}

// ------------------------------------------------------------
// FLAC
// ------------------------------------------------------------

fn flac_fixture(sample_rate: u32, channels: u32, total_samples: u64) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(b"fLaC");
    v.push(0x80); // last block, type 0 (STREAMINFO)
    v.extend_from_slice(&[0, 0, 34]);
    let mut info = vec![0u8; 34];
    info[0..2].copy_from_slice(&4096u16.to_be_bytes());
    info[2..4].copy_from_slice(&4096u16.to_be_bytes());
    // Packed: rate (20 bits), channels-1 (3), bps-1 (5),
    // total samples (36).
    let packed: u64 = (u64::from(sample_rate) << 44)
        | (u64::from(channels - 1) << 41)
        | (15u64 << 36)
        | total_samples;
    info[16..24].copy_from_slice(&packed.to_be_bytes());
    v.extend_from_slice(&info);
    v
}

#[test]
fn flac_streaminfo_exact_duration() {
    let v = probe(&flac_fixture(44100, 2, 44100));
    assert_eq!(v.get("container").unwrap().as_str(), Some("flac"));
    assert_eq!(v.get("duration_ms").unwrap().as_i64(), Some(1000));
    assert_eq!(
        v.get("duration_is_estimate").unwrap().as_bool(),
        Some(false)
    );
    let st = streams(&v);
    assert_eq!(st[0].get("codec").unwrap().as_str(), Some("flac"));
    assert_eq!(st[0].get("sample_rate").unwrap().as_i64(), Some(44100));
    assert_eq!(st[0].get("channels").unwrap().as_i64(), Some(2));
}

#[test]
fn flac_first_block_must_be_streaminfo() {
    let mut data = flac_fixture(44100, 2, 44100);
    data[4] = 0x84; // last block, type 4 (VORBIS_COMMENT)
    let err = probe_bytes(&data).unwrap_err();
    assert_eq!(err.0, ERR_PARSE, "error was: {}", err.1);
}

#[test]
fn flac_truncation_prefixes_never_panic() {
    let data = flac_fixture(48000, 1, 96000);
    for n in 0..data.len() {
        let _ = probe_bytes(&data[..n]);
    }
}

// ------------------------------------------------------------
// OGG
// ------------------------------------------------------------

fn ogg_page(serial: u32, seq: u32, granule: u64, htype: u8, payload: &[u8]) -> Vec<u8> {
    assert!(payload.len() < 255);
    let mut v = Vec::new();
    v.extend_from_slice(b"OggS");
    v.push(0); // version
    v.push(htype);
    v.extend_from_slice(&granule.to_le_bytes());
    v.extend_from_slice(&serial.to_le_bytes());
    v.extend_from_slice(&seq.to_le_bytes());
    v.extend_from_slice(&0u32.to_le_bytes()); // CRC (not verified)
    v.push(1); // one segment
    v.push(payload.len() as u8);
    v.extend_from_slice(payload);
    v
}

fn vorbis_id_header(channels: u8, rate: u32) -> Vec<u8> {
    let mut p = vec![0u8; 30];
    p[0] = 0x01;
    p[1..7].copy_from_slice(b"vorbis");
    p[11] = channels;
    p[12..16].copy_from_slice(&rate.to_le_bytes());
    p
}

#[test]
fn ogg_vorbis_stream_and_granule_duration() {
    let mut data = ogg_page(7, 0, 0, 0x02, &vorbis_id_header(2, 44100));
    data.extend_from_slice(&ogg_page(7, 1, 22050, 0, &[0x03; 10]));
    data.extend_from_slice(&ogg_page(7, 2, 44100, 0x04, &[0x05; 5]));
    let v = probe(&data);
    assert_eq!(v.get("container").unwrap().as_str(), Some("ogg"));
    assert_eq!(v.get("duration_ms").unwrap().as_i64(), Some(1000));
    assert_eq!(
        v.get("duration_is_estimate").unwrap().as_bool(),
        Some(false)
    );
    let st = streams(&v);
    assert_eq!(st[0].get("codec").unwrap().as_str(), Some("vorbis"));
    assert_eq!(st[0].get("sample_rate").unwrap().as_i64(), Some(44100));
    assert_eq!(st[0].get("channels").unwrap().as_i64(), Some(2));
}

#[test]
fn ogg_opus_duration_at_48k() {
    let mut head = Vec::new();
    head.extend_from_slice(b"OpusHead");
    head.push(1); // version
    head.push(2); // channels
    head.extend_from_slice(&0u16.to_le_bytes()); // preskip 0
    head.extend_from_slice(&48000u32.to_le_bytes()); // input rate
    head.extend_from_slice(&[0, 0, 0]); // gain + mapping family
    let mut data = ogg_page(9, 0, 0, 0x02, &head);
    data.extend_from_slice(&ogg_page(9, 1, 48000, 0x04, &[0x07; 4]));
    let v = probe(&data);
    let st = streams(&v);
    assert_eq!(st[0].get("codec").unwrap().as_str(), Some("opus"));
    assert_eq!(st[0].get("channels").unwrap().as_i64(), Some(2));
    assert_eq!(st[0].get("sample_rate").unwrap().as_i64(), Some(48000));
    assert_eq!(v.get("duration_ms").unwrap().as_i64(), Some(1000));
}

#[test]
fn ogg_unknown_codec_is_honest_other() {
    let data = ogg_page(3, 0, 100, 0x02, b"\x7fsome-codec-we-do-not-know");
    let v = probe(&data);
    assert_eq!(v.get("container").unwrap().as_str(), Some("ogg"));
    let st = streams(&v);
    assert_eq!(st[0].get("kind").unwrap().as_str(), Some("other"));
    assert!(st[0].get("codec").unwrap() == &json::Value::Null);
    assert!(v.get("duration_ms").unwrap() == &json::Value::Null);
}

// ------------------------------------------------------------
// MP4 / MOV
// ------------------------------------------------------------

fn mp4_box(ty: &str, payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(&((payload.len() + 8) as u32).to_be_bytes());
    v.extend_from_slice(ty.as_bytes());
    v.extend_from_slice(payload);
    v
}

fn mp4_ftyp(major: &str) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(major.as_bytes());
    p.extend_from_slice(&0u32.to_be_bytes());
    p.extend_from_slice(b"isomiso2");
    mp4_box("ftyp", &p)
}

fn mvhd_v0(timescale: u32, duration: u32) -> Vec<u8> {
    let mut p = vec![0u8; 100];
    p[12..16].copy_from_slice(&timescale.to_be_bytes());
    p[16..20].copy_from_slice(&duration.to_be_bytes());
    mp4_box("mvhd", &p)
}

fn tkhd(width: u32, height: u32) -> Vec<u8> {
    let mut p = vec![0u8; 92];
    p[84..88].copy_from_slice(&(width << 16).to_be_bytes());
    p[88..92].copy_from_slice(&(height << 16).to_be_bytes());
    mp4_box("tkhd", &p)
}

fn mdhd_v0(timescale: u32, duration: u32, packed_lang: u16) -> Vec<u8> {
    let mut p = vec![0u8; 24];
    p[12..16].copy_from_slice(&timescale.to_be_bytes());
    p[16..20].copy_from_slice(&duration.to_be_bytes());
    p[20..22].copy_from_slice(&packed_lang.to_be_bytes());
    mp4_box("mdhd", &p)
}

fn hdlr(kind: &str) -> Vec<u8> {
    let mut p = vec![0u8; 24];
    p[8..12].copy_from_slice(kind.as_bytes());
    mp4_box("hdlr", &p)
}

fn stsd_video(codec: &str) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&[0; 4]); // version/flags
    p.extend_from_slice(&1u32.to_be_bytes()); // entry count
    p.extend_from_slice(&16u32.to_be_bytes()); // entry size
    p.extend_from_slice(codec.as_bytes());
    p.extend_from_slice(&[0; 8]); // reserved + data reference
    mp4_box("stsd", &p)
}

fn stsd_audio(codec: &str, channels: u16, rate: u32) -> Vec<u8> {
    let mut entry = vec![0u8; 36];
    entry[0..4].copy_from_slice(&36u32.to_be_bytes());
    entry[4..8].copy_from_slice(codec.as_bytes());
    entry[24..26].copy_from_slice(&channels.to_be_bytes());
    entry[26..28].copy_from_slice(&16u16.to_be_bytes());
    entry[32..36].copy_from_slice(&(rate << 16).to_be_bytes());
    let mut p = Vec::new();
    p.extend_from_slice(&[0; 4]);
    p.extend_from_slice(&1u32.to_be_bytes());
    p.extend_from_slice(&entry);
    mp4_box("stsd", &p)
}

fn mp4_video_fixture(major: &str) -> Vec<u8> {
    let stbl = mp4_box("stbl", &stsd_video("avc1"));
    let minf = mp4_box("minf", &stbl);
    let mut mdia_p = Vec::new();
    mdia_p.extend_from_slice(&mdhd_v0(600, 1500, 5575)); // "eng"
    mdia_p.extend_from_slice(&hdlr("vide"));
    mdia_p.extend_from_slice(&minf);
    let mdia = mp4_box("mdia", &mdia_p);
    let mut trak_p = Vec::new();
    trak_p.extend_from_slice(&tkhd(320, 240));
    trak_p.extend_from_slice(&mdia);
    let trak = mp4_box("trak", &trak_p);
    let mut moov_p = Vec::new();
    moov_p.extend_from_slice(&mvhd_v0(1000, 2500));
    moov_p.extend_from_slice(&trak);
    let mut file = mp4_ftyp(major);
    file.extend_from_slice(&mp4_box("moov", &moov_p));
    file
}

#[test]
fn mp4_video_track_full_extraction() {
    let v = probe(&mp4_video_fixture("isom"));
    assert_eq!(v.get("container").unwrap().as_str(), Some("mp4"));
    assert_eq!(v.get("duration_ms").unwrap().as_i64(), Some(2500));
    assert_eq!(
        v.get("duration_is_estimate").unwrap().as_bool(),
        Some(false)
    );
    let st = streams(&v);
    assert_eq!(st.len(), 1);
    assert_eq!(st[0].get("kind").unwrap().as_str(), Some("video"));
    assert_eq!(st[0].get("codec").unwrap().as_str(), Some("avc1"));
    assert_eq!(st[0].get("width").unwrap().as_i64(), Some(320));
    assert_eq!(st[0].get("height").unwrap().as_i64(), Some(240));
    assert_eq!(st[0].get("language").unwrap().as_str(), Some("eng"));
}

#[test]
fn mp4_audio_track_and_duration_fallback() {
    // No mvhd: the movie duration falls back to the track's
    // mdhd (48000 units at a 48000 timescale = 1000 ms).
    let stbl = mp4_box("stbl", &stsd_audio("mp4a", 2, 48000));
    let minf = mp4_box("minf", &stbl);
    let mut mdia_p = Vec::new();
    mdia_p.extend_from_slice(&mdhd_v0(48000, 48000, 0));
    mdia_p.extend_from_slice(&hdlr("soun"));
    mdia_p.extend_from_slice(&minf);
    let mdia = mp4_box("mdia", &mdia_p);
    let trak = mp4_box("trak", &mdia);
    let moov = mp4_box("moov", &trak);
    let mut file = mp4_ftyp("isom");
    file.extend_from_slice(&moov);
    let v = probe(&file);
    assert_eq!(v.get("duration_ms").unwrap().as_i64(), Some(1000));
    let st = streams(&v);
    assert_eq!(st[0].get("kind").unwrap().as_str(), Some("audio"));
    assert_eq!(st[0].get("codec").unwrap().as_str(), Some("mp4a"));
    assert_eq!(st[0].get("sample_rate").unwrap().as_i64(), Some(48000));
    assert_eq!(st[0].get("channels").unwrap().as_i64(), Some(2));
    assert!(st[0].get("language").unwrap() == &json::Value::Null);
}

#[test]
fn mp4_quicktime_brand_reports_mov() {
    let v = probe(&mp4_video_fixture("qt  "));
    assert_eq!(v.get("container").unwrap().as_str(), Some("mov"));
    assert_eq!(v.get("duration_ms").unwrap().as_i64(), Some(2500));
}

#[test]
fn mp4_truncation_prefixes_never_panic() {
    let data = mp4_video_fixture("isom");
    for n in 0..data.len() {
        let _ = probe_bytes(&data[..n]);
    }
    // The complete fixture must still parse afterwards.
    assert!(probe_bytes(&data).is_ok());
}

#[test]
fn mp4_oversized_box_size_is_a_clean_error() {
    let mut data = mp4_ftyp("isom");
    // A box claiming size 1 (64-bit size follows) with a huge
    // 64-bit size and no payload.
    data.extend_from_slice(&1u32.to_be_bytes());
    data.extend_from_slice(b"free");
    data.extend_from_slice(&u64::MAX.to_be_bytes());
    for n in 0..data.len() {
        let _ = probe_bytes(&data[..n]);
    }
}

// ------------------------------------------------------------
// Matroska / WebM
// ------------------------------------------------------------

fn ebml_el(id: &[u8], payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(id);
    if payload.len() < 127 {
        v.push(0x80 | payload.len() as u8);
    } else {
        let len = payload.len();
        assert!(len < 16383);
        v.push(0x40 | ((len >> 8) as u8));
        v.push((len & 0xFF) as u8);
    }
    v.extend_from_slice(payload);
    v
}

fn ebml_uint(id: &[u8], mut val: u64, nbytes: usize) -> Vec<u8> {
    let mut p = vec![0u8; nbytes];
    for i in (0..nbytes).rev() {
        p[i] = (val & 0xFF) as u8;
        val >>= 8;
    }
    ebml_el(id, &p)
}

fn ebml_str(id: &[u8], s: &str) -> Vec<u8> {
    ebml_el(id, s.as_bytes())
}

fn ebml_f64(id: &[u8], f: f64) -> Vec<u8> {
    ebml_el(id, &f.to_be_bytes())
}

fn mkv_fixture(doc_type: &str, with_duration: bool) -> Vec<u8> {
    let header = ebml_el(
        &[0x1A, 0x45, 0xDF, 0xA3],
        &ebml_str(&[0x42, 0x82], doc_type),
    );
    let mut info_p = Vec::new();
    info_p.extend_from_slice(&ebml_uint(&[0x2A, 0xD7, 0xB1], 2_000_000, 3));
    if with_duration {
        info_p.extend_from_slice(&ebml_f64(&[0x44, 0x89], 750.0));
    }
    let info = ebml_el(&[0x15, 0x49, 0xA9, 0x66], &info_p);

    let mut video_p = Vec::new();
    video_p.extend_from_slice(&ebml_uint(&[0xB0], 640, 2));
    video_p.extend_from_slice(&ebml_uint(&[0xBA], 360, 2));
    let mut entry1_p = Vec::new();
    entry1_p.extend_from_slice(&ebml_uint(&[0x83], 1, 1));
    entry1_p.extend_from_slice(&ebml_str(&[0x86], "V_VP9"));
    entry1_p.extend_from_slice(&ebml_str(&[0x22, 0xB5, 0x9C], "eng"));
    entry1_p.extend_from_slice(&ebml_el(&[0xE0], &video_p));
    let entry1 = ebml_el(&[0xAE], &entry1_p);

    let mut audio_p = Vec::new();
    audio_p.extend_from_slice(&ebml_f64(&[0xB5], 48000.0));
    audio_p.extend_from_slice(&ebml_uint(&[0x9F], 2, 1));
    let mut entry2_p = Vec::new();
    entry2_p.extend_from_slice(&ebml_uint(&[0x83], 2, 1));
    entry2_p.extend_from_slice(&ebml_str(&[0x86], "A_OPUS"));
    entry2_p.extend_from_slice(&ebml_el(&[0xE1], &audio_p));
    let entry2 = ebml_el(&[0xAE], &entry2_p);

    let mut tracks_p = Vec::new();
    tracks_p.extend_from_slice(&entry1);
    tracks_p.extend_from_slice(&entry2);
    let tracks = ebml_el(&[0x16, 0x54, 0xAE, 0x6B], &tracks_p);

    let mut seg_p = Vec::new();
    seg_p.extend_from_slice(&info);
    seg_p.extend_from_slice(&tracks);
    let segment = ebml_el(&[0x18, 0x53, 0x80, 0x67], &seg_p);

    let mut file = header;
    file.extend_from_slice(&segment);
    file
}

#[test]
fn matroska_webm_full_extraction() {
    let v = probe(&mkv_fixture("webm", true));
    assert_eq!(v.get("container").unwrap().as_str(), Some("webm"));
    // 750 units * 2,000,000 ns scale / 1e6 = 1500 ms.
    assert_eq!(v.get("duration_ms").unwrap().as_i64(), Some(1500));
    assert_eq!(
        v.get("duration_is_estimate").unwrap().as_bool(),
        Some(false)
    );
    let st = streams(&v);
    assert_eq!(st.len(), 2);
    assert_eq!(st[0].get("kind").unwrap().as_str(), Some("video"));
    assert_eq!(st[0].get("codec").unwrap().as_str(), Some("V_VP9"));
    assert_eq!(st[0].get("width").unwrap().as_i64(), Some(640));
    assert_eq!(st[0].get("height").unwrap().as_i64(), Some(360));
    assert_eq!(st[0].get("language").unwrap().as_str(), Some("eng"));
    assert_eq!(st[1].get("kind").unwrap().as_str(), Some("audio"));
    assert_eq!(st[1].get("codec").unwrap().as_str(), Some("A_OPUS"));
    assert_eq!(st[1].get("sample_rate").unwrap().as_i64(), Some(48000));
    assert_eq!(st[1].get("channels").unwrap().as_i64(), Some(2));
}

#[test]
fn matroska_doctype_and_missing_duration() {
    let v = probe(&mkv_fixture("matroska", false));
    assert_eq!(v.get("container").unwrap().as_str(), Some("matroska"));
    assert!(v.get("duration_ms").unwrap() == &json::Value::Null);
    assert!(v.get("bitrate_bps").unwrap() == &json::Value::Null);
}

#[test]
fn matroska_truncation_prefixes_never_panic() {
    let data = mkv_fixture("webm", true);
    for n in 0..data.len() {
        let _ = probe_bytes(&data[..n]);
    }
    assert!(probe_bytes(&data).is_ok());
}

// ------------------------------------------------------------
// AVI
// ------------------------------------------------------------

fn riff_chunk(id: &str, payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(id.as_bytes());
    v.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    v.extend_from_slice(payload);
    if payload.len() % 2 == 1 {
        v.push(0);
    }
    v
}

fn riff_list(kind: &str, payload: &[u8]) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(kind.as_bytes());
    p.extend_from_slice(payload);
    riff_chunk("LIST", &p)
}

#[test]
fn avi_video_stream_and_duration() {
    let mut avih = vec![0u8; 56];
    avih[0..4].copy_from_slice(&33333u32.to_le_bytes()); // µs per frame
    avih[16..20].copy_from_slice(&300u32.to_le_bytes()); // total frames
    avih[24..28].copy_from_slice(&1u32.to_le_bytes()); // stream count

    let mut strh = vec![0u8; 56];
    strh[0..4].copy_from_slice(b"vids");
    strh[4..8].copy_from_slice(b"XVID");
    strh[20..24].copy_from_slice(&1u32.to_le_bytes()); // scale
    strh[24..28].copy_from_slice(&30u32.to_le_bytes()); // rate
    strh[32..36].copy_from_slice(&300u32.to_le_bytes()); // length

    let mut strf = vec![0u8; 40];
    strf[0..4].copy_from_slice(&40u32.to_le_bytes());
    strf[4..8].copy_from_slice(&640i32.to_le_bytes());
    strf[8..12].copy_from_slice(&480i32.to_le_bytes());
    strf[16..20].copy_from_slice(b"XVID");

    let mut strl_p = Vec::new();
    strl_p.extend_from_slice(&riff_chunk("strh", &strh));
    strl_p.extend_from_slice(&riff_chunk("strf", &strf));
    let mut hdrl_p = Vec::new();
    hdrl_p.extend_from_slice(&riff_chunk("avih", &avih));
    hdrl_p.extend_from_slice(&riff_list("strl", &strl_p));
    let hdrl = riff_list("hdrl", &hdrl_p);

    let mut file = Vec::new();
    file.extend_from_slice(b"RIFF");
    file.extend_from_slice(&((4 + hdrl.len()) as u32).to_le_bytes());
    file.extend_from_slice(b"AVI ");
    file.extend_from_slice(&hdrl);

    let v = probe(&file);
    assert_eq!(v.get("container").unwrap().as_str(), Some("avi"));
    // strh: 300 frames * scale 1 / rate 30 = 10 s.
    assert_eq!(v.get("duration_ms").unwrap().as_i64(), Some(10000));
    let st = streams(&v);
    assert_eq!(st.len(), 1);
    assert_eq!(st[0].get("kind").unwrap().as_str(), Some("video"));
    assert_eq!(st[0].get("codec").unwrap().as_str(), Some("XVID"));
    assert_eq!(st[0].get("width").unwrap().as_i64(), Some(640));
    assert_eq!(st[0].get("height").unwrap().as_i64(), Some(480)); // strh rate 30 / scale 1.
    assert_eq!(st[0].get("frame_rate").unwrap().as_str(), Some("30/1"));
    assert!(st[0].get("field_order").unwrap() == &json::Value::Null);
}

#[test]
fn avi_audio_stream_from_strf() {
    let mut strh = vec![0u8; 56];
    strh[0..4].copy_from_slice(b"auds");
    strh[20..24].copy_from_slice(&1u32.to_le_bytes());
    strh[24..28].copy_from_slice(&44100u32.to_le_bytes());
    strh[32..36].copy_from_slice(&44100u32.to_le_bytes()); // 1 second of samples

    let mut strf = vec![0u8; 18];
    strf[0..2].copy_from_slice(&0x55u16.to_le_bytes()); // MP3
    strf[2..4].copy_from_slice(&2u16.to_le_bytes());
    strf[4..8].copy_from_slice(&44100u32.to_le_bytes());

    let mut strl_p = Vec::new();
    strl_p.extend_from_slice(&riff_chunk("strh", &strh));
    strl_p.extend_from_slice(&riff_chunk("strf", &strf));
    let hdrl = riff_list("hdrl", &riff_list("strl", &strl_p));
    let mut file = Vec::new();
    file.extend_from_slice(b"RIFF");
    file.extend_from_slice(&((4 + hdrl.len()) as u32).to_le_bytes());
    file.extend_from_slice(b"AVI ");
    file.extend_from_slice(&hdrl);

    let v = probe(&file);
    assert_eq!(v.get("duration_ms").unwrap().as_i64(), Some(1000));
    let st = streams(&v);
    assert_eq!(st[0].get("kind").unwrap().as_str(), Some("audio"));
    assert_eq!(st[0].get("codec").unwrap().as_str(), Some("mp3"));
    assert_eq!(st[0].get("sample_rate").unwrap().as_i64(), Some(44100));
    assert_eq!(st[0].get("channels").unwrap().as_i64(), Some(2));
}

// ------------------------------------------------------------
// Detection / errors
// ------------------------------------------------------------

#[test]
fn unknown_container_reports_first_bytes_hex() {
    let junk: [u8; 12] = [0xDE, 0xAD, 0xBE, 0xEF, 0, 1, 2, 3, 4, 5, 6, 7];
    let err = probe_bytes(&junk).unwrap_err();
    assert_eq!(err.0, ERR_NOT_SUPPORTED);
    assert!(err.1.contains("deadbeef"), "message was: {}", err.1);
}

#[test]
fn empty_input_is_invalid_arg() {
    let err = probe_bytes(&[]).unwrap_err();
    assert_eq!(err.0, ERR_INVALID_ARG);
}

#[test]
fn riff_but_not_wave_or_avi_is_unsupported() {
    let mut data = Vec::new();
    data.extend_from_slice(b"RIFF");
    data.extend_from_slice(&4u32.to_le_bytes());
    data.extend_from_slice(b"WEBP");
    let err = probe_bytes(&data).unwrap_err();
    assert_eq!(err.0, ERR_NOT_SUPPORTED);
}

// ------------------------------------------------------------
// Frame rate and field order
// ------------------------------------------------------------

fn stts(entries: &[(u32, u32)]) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&[0; 4]);
    p.extend_from_slice(&(entries.len() as u32).to_be_bytes());
    for (count, delta) in entries {
        p.extend_from_slice(&count.to_be_bytes());
        p.extend_from_slice(&delta.to_be_bytes());
    }
    mp4_box("stts", &p)
}

/// A visual sample entry with the full 86-byte header and, when
/// given, a `fiel` child box (field count, order code).
fn stsd_visual(codec: &str, fiel: Option<(u8, u8)>) -> Vec<u8> {
    let mut entry = vec![0u8; 86];
    entry[4..8].copy_from_slice(codec.as_bytes());
    if let Some((fields, detail)) = fiel {
        entry.extend_from_slice(&10u32.to_be_bytes());
        entry.extend_from_slice(b"fiel");
        entry.extend_from_slice(&[fields, detail]);
    }
    let len = entry.len() as u32;
    entry[0..4].copy_from_slice(&len.to_be_bytes());
    let mut p = Vec::new();
    p.extend_from_slice(&[0; 4]);
    p.extend_from_slice(&1u32.to_be_bytes());
    p.extend_from_slice(&entry);
    mp4_box("stsd", &p)
}

fn mp4_timed_video(handler: &str, stsd: Vec<u8>, timing: Option<Vec<u8>>) -> Vec<u8> {
    let mut stbl_p = stsd;
    if let Some(t) = timing {
        stbl_p.extend_from_slice(&t);
    }
    let minf = mp4_box("minf", &mp4_box("stbl", &stbl_p));
    let mut mdia_p = Vec::new();
    mdia_p.extend_from_slice(&mdhd_v0(600, 1500, 0));
    mdia_p.extend_from_slice(&hdlr(handler));
    mdia_p.extend_from_slice(&minf);
    let mut trak_p = tkhd(720, 480);
    trak_p.extend_from_slice(&mp4_box("mdia", &mdia_p));
    let mut file = mp4_ftyp("qt  ");
    file.extend_from_slice(&mp4_box("moov", &mp4_box("trak", &trak_p)));
    file
}

fn first_stream(data: &[u8]) -> json::Value {
    streams(&probe(data))[0].clone()
}

#[test]
fn mp4_frame_rate_from_stts_constant_and_variable() {
    // 75 samples of 20 units at a 600 timescale: 30 fps.
    let cfr = first_stream(&mp4_timed_video(
        "vide",
        stsd_video("avc1"),
        Some(stts(&[(75, 20)])),
    ));
    assert_eq!(cfr.get("frame_rate").unwrap().as_str(), Some("30/1"));
    // 50×20 + 25×40 = 2000 units for 75 samples: 75×600/2000 = 45/2.
    let vfr = first_stream(&mp4_timed_video(
        "vide",
        stsd_video("avc1"),
        Some(stts(&[(50, 20), (25, 40)])),
    ));
    assert_eq!(vfr.get("frame_rate").unwrap().as_str(), Some("45/2"));
    // NTSC-style timing: 1001 units per frame at 30000.
    let mut ntsc = mp4_timed_video("vide", stsd_video("avc1"), Some(stts(&[(300, 1001)])));
    let at = ntsc.windows(4).position(|w| w == b"mdhd").unwrap() + 4 + 12;
    ntsc[at..at + 4].copy_from_slice(&30000u32.to_be_bytes());
    assert_eq!(
        first_stream(&ntsc).get("frame_rate").unwrap().as_str(),
        Some("30000/1001")
    );
}

#[test]
fn mp4_frame_rate_is_null_without_a_readable_table_or_for_audio() {
    let none = first_stream(&mp4_timed_video("vide", stsd_video("avc1"), None));
    assert!(none.get("frame_rate").unwrap() == &json::Value::Null);
    // A table claiming more entries than the box holds is not read.
    let mut lying = stts(&[(75, 20)]);
    lying[12..16].copy_from_slice(&70_000u32.to_be_bytes());
    let over = first_stream(&mp4_timed_video("vide", stsd_video("avc1"), Some(lying)));
    assert!(over.get("frame_rate").unwrap() == &json::Value::Null);
    let audio = first_stream(&mp4_timed_video(
        "soun",
        stsd_audio("mp4a", 2, 48000),
        Some(stts(&[(75, 20)])),
    ));
    assert!(audio.get("frame_rate").unwrap() == &json::Value::Null);
    assert!(audio.get("field_order").unwrap() == &json::Value::Null);
}

#[test]
fn mp4_field_order_from_fiel() {
    let order = |fiel| {
        first_stream(&mp4_timed_video("vide", stsd_visual("dvsd", fiel), None))
            .get("field_order")
            .unwrap()
            .clone()
    };
    assert_eq!(order(Some((2, 1))).as_str(), Some("tt"));
    assert_eq!(order(Some((2, 6))).as_str(), Some("bb"));
    assert_eq!(order(Some((2, 9))).as_str(), Some("tb"));
    assert_eq!(order(Some((2, 14))).as_str(), Some("bt"));
    assert_eq!(order(Some((1, 0))).as_str(), Some("progressive"));
    assert!(
        order(Some((2, 3))) == json::Value::Null,
        "unknown code stays null"
    );
    assert!(order(None) == json::Value::Null, "no fiel atom stays null");
    let codec = first_stream(&mp4_timed_video(
        "vide",
        stsd_visual("dvsd", Some((2, 6))),
        None,
    ));
    assert_eq!(codec.get("codec").unwrap().as_str(), Some("dvsd"));
}

/// A Matroska file with one video track carrying these extra
/// TrackEntry and Video child elements.
fn mkv_video(entry_extra: &[u8], video_extra: &[u8]) -> Vec<u8> {
    let header = ebml_el(
        &[0x1A, 0x45, 0xDF, 0xA3],
        &ebml_str(&[0x42, 0x82], "matroska"),
    );
    let mut video_p = ebml_uint(&[0xB0], 720, 2);
    video_p.extend_from_slice(&ebml_uint(&[0xBA], 576, 2));
    video_p.extend_from_slice(video_extra);
    let mut entry_p = ebml_uint(&[0x83], 1, 1);
    entry_p.extend_from_slice(&ebml_str(&[0x86], "V_MS/VFW/FOURCC"));
    entry_p.extend_from_slice(entry_extra);
    entry_p.extend_from_slice(&ebml_el(&[0xE0], &video_p));
    let tracks = ebml_el(&[0x16, 0x54, 0xAE, 0x6B], &ebml_el(&[0xAE], &entry_p));
    let mut file = header;
    file.extend_from_slice(&ebml_el(&[0x18, 0x53, 0x80, 0x67], &tracks));
    file
}

#[test]
fn matroska_frame_rate_from_default_duration() {
    let pal = first_stream(&mkv_video(
        &ebml_uint(&[0x23, 0xE3, 0x83], 40_000_000, 4),
        &[],
    ));
    assert_eq!(pal.get("frame_rate").unwrap().as_str(), Some("25/1"));
    // Stored nanoseconds are rounded, so the exact fraction is kept.
    let ntsc = first_stream(&mkv_video(
        &ebml_uint(&[0x23, 0xE3, 0x83], 33_366_667, 4),
        &[],
    ));
    assert_eq!(
        ntsc.get("frame_rate").unwrap().as_str(),
        Some("1000000000/33366667")
    );
    let none = first_stream(&mkv_video(&[], &[]));
    assert!(none.get("frame_rate").unwrap() == &json::Value::Null);
    // The existing fixture's audio track carries no frame rate.
    let v = probe(&mkv_fixture("webm", true));
    assert!(streams(&v)[1].get("frame_rate").unwrap() == &json::Value::Null);
}

#[test]
fn matroska_field_order_from_interlace_flags() {
    let order = |video_extra: Vec<u8>| {
        first_stream(&mkv_video(&[], &video_extra))
            .get("field_order")
            .unwrap()
            .clone()
    };
    let flags = |interlaced: Option<u64>, order: Option<u64>| {
        let mut v = Vec::new();
        if let Some(i) = interlaced {
            v.extend_from_slice(&ebml_uint(&[0x9A], i, 1));
        }
        if let Some(o) = order {
            v.extend_from_slice(&ebml_uint(&[0x9D], o, 1));
        }
        v
    };
    assert_eq!(order(flags(Some(1), Some(1))).as_str(), Some("tt"));
    assert_eq!(order(flags(Some(1), Some(6))).as_str(), Some("bb"));
    assert_eq!(order(flags(Some(1), Some(9))).as_str(), Some("tb"));
    assert_eq!(order(flags(Some(1), Some(14))).as_str(), Some("bt"));
    assert_eq!(order(flags(Some(2), None)).as_str(), Some("progressive"));
    assert!(
        order(flags(Some(1), Some(2))) == json::Value::Null,
        "undetermined stays null"
    );
    assert!(
        order(flags(Some(1), None)) == json::Value::Null,
        "interlaced, order unknown"
    );
    assert!(order(flags(None, None)) == json::Value::Null);
}

#[test]
fn matroska_frame_rate_fixture_truncation_never_panics() {
    let data = mkv_video(
        &ebml_uint(&[0x23, 0xE3, 0x83], 40_000_000, 4),
        &ebml_uint(&[0x9A], 1, 1),
    );
    for n in 0..data.len() {
        let _ = probe_bytes(&data[..n]);
    }
    assert!(probe_bytes(&data).is_ok());
}

#[test]
fn mp4_frame_rate_fixture_truncation_never_panics() {
    let data = mp4_timed_video(
        "vide",
        stsd_visual("dvsd", Some((2, 6))),
        Some(stts(&[(50, 20), (25, 40)])),
    );
    for n in 0..data.len() {
        let _ = probe_bytes(&data[..n]);
    }
    assert!(probe_bytes(&data).is_ok());
}

#[test]
fn ratios_reduce_and_refuse_zero() {
    assert_eq!(crate::parsers::reduced_ratio(12_800, 512), Some((25, 1)));
    assert_eq!(
        crate::parsers::reduced_ratio(30_000, 1001),
        Some((30000, 1001))
    );
    assert_eq!(crate::parsers::reduced_ratio(0, 5), None);
    assert_eq!(crate::parsers::reduced_ratio(5, 0), None);
    assert_eq!(
        crate::parsers::reduced_ratio(u128::MAX, 1),
        None,
        "too large to report"
    );
}

#[test]
fn quicktime_data_handler_does_not_hide_the_video_track() {
    // mdia hdlr says "vide"; the minf hdlr is a data handler
    // ("dhlr"/"alis") and must not turn the track into "other".
    let mut dhlr = vec![0u8; 24];
    dhlr[4..8].copy_from_slice(b"dhlr");
    dhlr[8..12].copy_from_slice(b"alis");
    let mut minf_p = mp4_box("hdlr", &dhlr);
    let mut stbl_p = stsd_visual("avc1", Some((2, 1)));
    stbl_p.extend_from_slice(&stts(&[(50, 512)]));
    minf_p.extend_from_slice(&mp4_box("stbl", &stbl_p));
    let mut mhlr = vec![0u8; 24];
    mhlr[4..8].copy_from_slice(b"mhlr");
    mhlr[8..12].copy_from_slice(b"vide");
    let mut mdia_p = mdhd_v0(12800, 25600, 0); // Macintosh language 0
    mdia_p.extend_from_slice(&mp4_box("hdlr", &mhlr));
    mdia_p.extend_from_slice(&mp4_box("minf", &minf_p));
    let mut trak_p = tkhd(320, 240);
    trak_p.extend_from_slice(&mp4_box("mdia", &mdia_p));
    let mut file = mp4_ftyp("qt  ");
    file.extend_from_slice(&mp4_box("moov", &mp4_box("trak", &trak_p)));

    let st = first_stream(&file);
    assert_eq!(st.get("kind").unwrap().as_str(), Some("video"));
    assert_eq!(st.get("frame_rate").unwrap().as_str(), Some("25/1"));
    assert_eq!(st.get("field_order").unwrap().as_str(), Some("tt"));
    assert!(
        st.get("language").unwrap() == &json::Value::Null,
        "a Macintosh language code is not an ISO code"
    );

    let mut unspecified = file.clone();
    let at = unspecified.windows(4).position(|w| w == b"mdhd").unwrap() + 4 + 20;
    unspecified[at..at + 2].copy_from_slice(&0x7FFFu16.to_be_bytes());
    let st = first_stream(&unspecified);
    assert!(
        st.get("language").unwrap() == &json::Value::Null,
        "QuickTime's 0x7FFF means unspecified"
    );
}
