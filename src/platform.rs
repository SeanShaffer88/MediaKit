//! Platform layer: the only place Windows APIs are touched.
//! On non-Windows targets everything returns ERR_NOT_SUPPORTED so the
//! pure logic and FFI contract stay testable anywhere.
//!
//! Every extern block MUST carry #[link(name = "...")] naming its import
//! library (advapi32 / shell32 / kernel32 / user32 …). Without it the
//! build fails at LINK time with LNK2019 — check and clippy cannot
//! catch it; CI's real build can.

use crate::ERR_NOT_SUPPORTED;

pub type PResult<T> = Result<T, (i32, String)>;

pub fn not_supported<T>(what: &str) -> PResult<T> {
    Err((
        ERR_NOT_SUPPORTED,
        format!("{what} is only available on Windows"),
    ))
}

/// Positioned read: fill `buf` from `offset` without moving
/// any shared seek position, returning the number of bytes
/// actually read (short at end of file). This is what lets the
/// probe core read headers and tails of very large files
/// without ever slurping them.
#[cfg(unix)]
pub fn file_read_at(file: &std::fs::File, offset: u64, buf: &mut [u8]) -> std::io::Result<usize> {
    use std::os::unix::fs::FileExt;
    file.read_at(buf, offset)
}

#[cfg(windows)]
pub fn file_read_at(file: &std::fs::File, offset: u64, buf: &mut [u8]) -> std::io::Result<usize> {
    use std::os::windows::fs::FileExt;
    file.seek_read(buf, offset)
}

#[cfg(not(any(unix, windows)))]
pub fn file_read_at(
    _file: &std::fs::File,
    _offset: u64,
    _buf: &mut [u8],
) -> std::io::Result<usize> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "positioned file reads are not available on this platform",
    ))
}

#[cfg(windows)]
pub fn example() -> PResult<u32> {
    // Replace with a real call. Pattern for a hand-declared binding:
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcessId() -> u32;
    }
    // SAFETY: GetCurrentProcessId takes no arguments and cannot fail.
    Ok(unsafe { GetCurrentProcessId() })
}

#[cfg(not(windows))]
pub fn example() -> PResult<u32> {
    not_supported("example")
}
