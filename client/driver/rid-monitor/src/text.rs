//! UTF-16 helpers. The kernel speaks counted UTF-16 strings; Rust string literals are UTF-8.

use core::ffi::c_void;

use crate::sys;

/// An ASCII literal as a UTF-16 array, converted at compile time.
pub const fn ascii16<const N: usize>(s: &str) -> [u16; N] {
    let bytes = s.as_bytes();
    let mut out = [0u16; N];
    let mut i = 0;
    while i < N {
        out[i] = bytes[i] as u16;
        i += 1;
    }
    out
}

/// `w!("text")` is a `&'static [u16]`.
macro_rules! w {
    ($s:literal) => {{
        const N: usize = $s.len();
        static WIDE: [u16; N] = $crate::text::ascii16::<N>($s);
        &WIDE as &'static [u16]
    }};
}
pub(crate) use w;

/// Lower-case; compared against the end of a new process's image path.
pub const MSIEXEC: &[u16] = &ascii16::<12>("\\msiexec.exe");

/// A counted string over a static buffer, for kernel APIs that take `PUNICODE_STRING`.
pub fn unicode(s: &'static [u16]) -> sys::UNICODE_STRING {
    let bytes = (s.len() * 2) as u16;
    sys::UNICODE_STRING { Length: bytes, MaximumLength: bytes, Buffer: s.as_ptr().cast_mut() }
}

/// The characters of a counted string the kernel handed us.
///
/// # Safety
/// `s.Buffer` must be readable for `s.Length` bytes for as long as the result is used.
pub unsafe fn slice(s: &sys::UNICODE_STRING) -> &[u16] {
    if s.Buffer.is_null() {
        return &[];
    }
    // SAFETY: guaranteed by the caller.
    unsafe { core::slice::from_raw_parts(s.Buffer, s.Length as usize / 2) }
}

/// Is this a kernel-space address? Rust has no structured exception handling, so memory that
/// may belong to the calling process is never touched; such data is simply not recorded.
pub fn kernel(address: *const c_void) -> bool {
    address as usize >= 0xFFFF_8000_0000_0000
}

/// Like [`slice`], but empty unless both the string header and its buffer are kernel memory.
///
/// # Safety
/// If the pointers are kernel addresses they must be valid, as for [`slice`].
pub unsafe fn checked<'a>(s: *const sys::UNICODE_STRING) -> &'a [u16] {
    if !kernel(s.cast()) {
        return &[];
    }
    // SAFETY: the header is kernel memory supplied by the registry; the buffer is checked next.
    unsafe { if kernel((*s).Buffer.cast()) { slice(&*s) } else { &[] } }
}

/// ASCII case-insensitive suffix test; `suffix` must be lower-case.
pub fn ends_with(text: &[u16], suffix: &[u16]) -> bool {
    text.len() >= suffix.len()
        && text[text.len() - suffix.len()..].iter().zip(suffix).all(|(&c, &s)| {
            let lower = if c >= 'A' as u16 && c <= 'Z' as u16 { c + 32 } else { c };
            lower == s
        })
}
