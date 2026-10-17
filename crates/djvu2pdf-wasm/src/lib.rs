//! WebAssembly entry points (target `wasm32-unknown-unknown`). Conversion runs entirely
//! in the browser; documents never leave the user's machine.
//!
//! Plain C ABI, no bindings generator. One conversion per instance:
//!
//! 1. `ptr = input_alloc(len)`, copy the DjVu bytes to `memory[ptr..ptr + len]`;
//! 2. `status = convert(ptr, len)` (0 = ok, 1 = error), calling the imported
//!    `djvu2pdf.progress(pages_done, pages_total)` along the way;
//! 3. read `memory[result_ptr()..result_ptr() + result_len()]`: the PDF, or the error
//!    message (UTF-8) if `status` is 1; `stats_ptr()`/`stats_len()` give a JSON summary.
//!
//! A panic traps; its message is then available from `panic_ptr()`/`panic_len()`.

use std::cell::RefCell;

use djvu2pdf::convert::{Options, convert_with_progress};

#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "djvu2pdf")]
unsafe extern "C" {
    fn progress(done: u32, total: u32);
}

#[cfg(not(target_arch = "wasm32"))]
unsafe fn progress(_done: u32, _total: u32) {}

thread_local! {
    static RESULT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    static STATS: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    static PANIC: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

/// Allocate `len` bytes for the input document.
#[unsafe(no_mangle)]
pub extern "C" fn input_alloc(len: usize) -> *mut u8 {
    let mut v = Vec::<u8>::with_capacity(len);
    let p = v.as_mut_ptr();
    std::mem::forget(v);
    p
}

/// Convert the DjVu document at `ptr` (from [`input_alloc`], `len` bytes, consumed).
///
/// # Safety
/// `ptr` must come from `input_alloc(len)` with all `len` bytes written.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn convert(ptr: *mut u8, len: usize) -> u32 {
    std::panic::set_hook(Box::new(|info| {
        PANIC.with(|p| *p.borrow_mut() = info.to_string().into_bytes());
    }));
    // SAFETY: allocated by `input_alloc(len)` and fully initialized by the caller.
    let input = unsafe { Vec::from_raw_parts(ptr, len, len) };
    let result = convert_with_progress(&input, Vec::new(), &Options::default(), |done, total| {
        // SAFETY: imported JS function taking two integers.
        unsafe { progress(done as u32, total as u32) }
    });
    drop(input);
    let (status, bytes) = match result {
        Ok((pdf, s)) => {
            let json = format!(
                r#"{{"pages":{},"compound_pages":{},"text_pages":{},"bookmarks":{},"links":{}}}"#,
                s.pages, s.compound_pages, s.text_pages, s.bookmarks, s.links
            );
            STATS.with(|st| *st.borrow_mut() = json.into_bytes());
            (0, pdf)
        }
        Err(e) => (1, e.to_string().into_bytes()),
    };
    RESULT.with(|r| *r.borrow_mut() = bytes);
    status
}

#[unsafe(no_mangle)]
pub extern "C" fn result_ptr() -> *const u8 {
    RESULT.with(|r| r.borrow().as_ptr())
}

#[unsafe(no_mangle)]
pub extern "C" fn result_len() -> usize {
    RESULT.with(|r| r.borrow().len())
}

#[unsafe(no_mangle)]
pub extern "C" fn stats_ptr() -> *const u8 {
    STATS.with(|r| r.borrow().as_ptr())
}

#[unsafe(no_mangle)]
pub extern "C" fn stats_len() -> usize {
    STATS.with(|r| r.borrow().len())
}

#[unsafe(no_mangle)]
pub extern "C" fn panic_ptr() -> *const u8 {
    PANIC.with(|r| r.borrow().as_ptr())
}

#[unsafe(no_mangle)]
pub extern "C" fn panic_len() -> usize {
    PANIC.with(|r| r.borrow().len())
}
