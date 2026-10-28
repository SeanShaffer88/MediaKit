//! C ABI surface — everything declared in include/mediakit.h.
//! Conventions (house standard):
//! - `*mut c_char` results are caller-owned; free with `me_free_string`.
//! - Failures return null / an ERR_* code + a thread-local last-error message.
//! - Nothing unwinds across the FFI boundary (catch_unwind everywhere;
//!   release profile also sets panic = "abort").

#![allow(clippy::not_unsafe_ptr_arg_deref)]

use std::cell::RefCell;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int};
use std::panic::catch_unwind;

use crate::{ERR_INTERNAL, ERR_INVALID_ARG, ME_OK};

thread_local! {
    static LAST_ERROR: RefCell<(i32, CString)> =
        RefCell::new((0, CString::new("").unwrap()));
}

pub fn set_error(code: i32, msg: &str) {
    let clean: String = msg.chars().filter(|&c| c != '\0').collect();
    LAST_ERROR.with(|e| {
        *e.borrow_mut() = (
            code,
            CString::new(clean).unwrap_or_else(|_| CString::new("error").unwrap()),
        );
    });
}

fn clear_error() {
    set_error(ME_OK, "");
}

fn out_json(result: crate::MeResult<String>) -> *mut c_char {
    match result {
        Ok(text) => match CString::new(text) {
            Ok(s) => {
                clear_error();
                s.into_raw()
            }
            Err(_) => {
                set_error(ERR_INTERNAL, "result string contained a NUL byte");
                std::ptr::null_mut()
            }
        },
        Err((code, msg)) => {
            set_error(code, &msg);
            std::ptr::null_mut()
        }
    }
}

fn guard_json<F: FnOnce() -> *mut c_char>(f: F) -> *mut c_char {
    match catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(p) => p,
        Err(_) => {
            set_error(ERR_INTERNAL, "internal panic caught at the FFI boundary");
            std::ptr::null_mut()
        }
    }
}

#[no_mangle]
pub extern "C" fn me_version() -> *const c_char {
    static VERSION_C: &[u8] = b"0.1.0\0";
    VERSION_C.as_ptr() as *const c_char
}

#[no_mangle]
pub extern "C" fn me_last_error() -> *const c_char {
    LAST_ERROR.with(|e| e.borrow().1.as_ptr())
}

#[no_mangle]
pub extern "C" fn me_last_error_code() -> c_int {
    LAST_ERROR.with(|e| e.borrow().0) as c_int
}

/// # Safety (caller contract): `p` is null or a string this library
/// returned and has not been freed yet. Documented in the header.
#[no_mangle]
pub extern "C" fn me_free_string(p: *mut c_char) {
    if !p.is_null() {
        unsafe {
            drop(CString::from_raw(p));
        }
    }
}

/// Probe a media file on disk. Returns a caller-owned JSON
/// string (free with `me_free_string`), or null + last-error.
#[no_mangle]
pub extern "C" fn me_probe(path: *const c_char) -> *mut c_char {
    guard_json(|| {
        if path.is_null() {
            set_error(ERR_INVALID_ARG, "path must not be null");
            return std::ptr::null_mut();
        }
        let path_str = match unsafe { CStr::from_ptr(path) }.to_str() {
            Ok(s) => s,
            Err(_) => {
                set_error(ERR_INVALID_ARG, "path must be valid UTF-8");
                return std::ptr::null_mut();
            }
        };
        out_json(crate::probe_path(path_str))
    })
}

/// Probe media bytes already in memory. `data` must point to
/// `len` readable bytes. Same result contract as `me_probe`.
#[no_mangle]
pub extern "C" fn me_probe_bytes(data: *const u8, len: usize) -> *mut c_char {
    guard_json(|| {
        if data.is_null() {
            set_error(ERR_INVALID_ARG, "data must not be null");
            return std::ptr::null_mut();
        }
        if len == 0 {
            set_error(ERR_INVALID_ARG, "len must not be 0");
            return std::ptr::null_mut();
        }
        let slice = unsafe { std::slice::from_raw_parts(data, len) };
        out_json(crate::probe_bytes(slice))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json;

    fn take(p: *mut c_char) -> String {
        assert!(
            !p.is_null(),
            "FFI call returned null: {}",
            unsafe { CStr::from_ptr(me_last_error()) }.to_str().unwrap()
        );
        let s = unsafe { CStr::from_ptr(p) }.to_str().unwrap().to_string();
        me_free_string(p);
        s
    }

    /// Minimal valid WAV: 8000 Hz mono 8-bit PCM, 8000 data
    /// bytes = exactly 1000 ms.
    fn wav_fixture() -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(b"RIFF");
        v.extend_from_slice(&(36u32 + 8000).to_le_bytes());
        v.extend_from_slice(b"WAVE");
        v.extend_from_slice(b"fmt ");
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes()); // PCM
        v.extend_from_slice(&1u16.to_le_bytes()); // mono
        v.extend_from_slice(&8000u32.to_le_bytes()); // rate
        v.extend_from_slice(&8000u32.to_le_bytes()); // byte rate
        v.extend_from_slice(&1u16.to_le_bytes()); // block align
        v.extend_from_slice(&8u16.to_le_bytes()); // bits
        v.extend_from_slice(b"data");
        v.extend_from_slice(&8000u32.to_le_bytes());
        v.extend_from_slice(&[128u8; 8000]);
        v
    }

    #[test]
    fn ffi_probe_bytes_round_trip() {
        let data = wav_fixture();
        let text = take(me_probe_bytes(data.as_ptr(), data.len()));
        let v = json::parse(&text).unwrap();
        assert_eq!(v.get("container").unwrap().as_str(), Some("wav"));
        assert_eq!(v.get("duration_ms").unwrap().as_i64(), Some(1000));
        assert_eq!(me_last_error_code(), ME_OK as c_int);
    }

    #[test]
    fn ffi_probe_rejects_null_and_bad_args() {
        assert!(me_probe(std::ptr::null()).is_null());
        assert_eq!(me_last_error_code(), ERR_INVALID_ARG as c_int);
        assert!(me_probe_bytes(std::ptr::null(), 4).is_null());
        assert_eq!(me_last_error_code(), ERR_INVALID_ARG as c_int);
        assert!(me_probe_bytes(wav_fixture().as_ptr(), 0).is_null());
        assert_eq!(me_last_error_code(), ERR_INVALID_ARG as c_int);
    }

    #[test]
    fn ffi_probe_missing_file_is_not_found() {
        let path = CString::new("mediakit-no-such-file-xyz.wav").unwrap();
        assert!(me_probe(path.as_ptr()).is_null());
        assert_eq!(
            me_last_error_code(),
            crate::ERR_NOT_FOUND as c_int,
            "error was: {}",
            unsafe { CStr::from_ptr(me_last_error()) }.to_str().unwrap()
        );
    }

    #[test]
    fn ffi_probe_unknown_bytes_reports_first_bytes() {
        let junk = [0xDEu8, 0xAD, 0xBE, 0xEF, 1, 2, 3, 4];
        assert!(me_probe_bytes(junk.as_ptr(), junk.len()).is_null());
        assert_eq!(me_last_error_code(), crate::ERR_NOT_SUPPORTED as c_int);
        let msg = unsafe { CStr::from_ptr(me_last_error()) }
            .to_str()
            .unwrap()
            .to_string();
        assert!(msg.contains("deadbeef"), "message was: {msg}");
    }
}
