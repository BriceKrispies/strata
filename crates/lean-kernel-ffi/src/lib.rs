//! The boundary to compiled Lean.
//!
//! The Lean library (`lean/StrataFfi.lean`) exports functions over `ByteArray`; `lean/shim.c`
//! wraps them in a plain C ABI over pointers and lengths, compiled with Lean's own clang so that
//! Rust never depends on `lean.h`. This crate is the only place in the workspace allowed `unsafe`.
//!
//! Threading: the Lean runtime is initialised once per process ([`init`], called implicitly by
//! [`step`]), and every thread that calls into Lean must be registered with the runtime first. A
//! thread is registered on its first call and unregistered when it exits (a thread-local guard), so
//! callers on tokio worker threads need do nothing.

pub mod codec;

use std::sync::OnceLock;

mod sys {
    use std::ffi::c_void;

    #[link(name = "strata_ffi", kind = "dylib")]
    extern "C" {
        pub fn strata_ffi_init() -> i32;
        pub fn strata_ffi_thread_init();
        pub fn strata_ffi_thread_end();
        pub fn strata_ffi_step_bytes(
            input: *const u8,
            len: usize,
            out: *mut *const u8,
            out_len: *mut usize,
            handle: *mut *mut c_void,
        );
        pub fn strata_ffi_identity_bytes(
            input: *const u8,
            len: usize,
            out: *mut *const u8,
            out_len: *mut usize,
            handle: *mut *mut c_void,
        );
        pub fn strata_ffi_release(handle: *mut c_void);
    }
}

static INIT: OnceLock<Result<(), i32>> = OnceLock::new();

/// Initialises the Lean runtime and the Lean module, once per process.
pub fn init() -> Result<(), i32> {
    *INIT.get_or_init(|| {
        // SAFETY: called exactly once per process (OnceLock), before any other call into Lean.
        // The thread that initialises the runtime is registered with it by the initialisation.
        match unsafe { sys::strata_ffi_init() } {
            0 => {
                // The initialising thread is the runtime's own; it is never unregistered.
                REGISTERED.with(|r| r.set(true));
                Ok(())
            }
            code => Err(code),
        }
    })
}

thread_local! {
    static REGISTERED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static GUARD: ThreadGuard = const { ThreadGuard };
}

/// Unregisters a thread from the Lean runtime when the thread exits.
struct ThreadGuard;

impl Drop for ThreadGuard {
    fn drop(&mut self) {
        if REGISTERED.try_with(|r| r.get()).unwrap_or(false) {
            // SAFETY: this thread was registered by `ensure_thread` and holds no Lean objects
            // (every result is released before `step` returns).
            unsafe { sys::strata_ffi_thread_end() };
        }
    }
}

fn ensure_thread() {
    if !REGISTERED.with(|r| r.get()) {
        // SAFETY: the runtime is initialised (`init` succeeded) and this thread is not registered.
        unsafe { sys::strata_ffi_thread_init() };
        REGISTERED.with(|r| r.set(true));
        GUARD.with(|_| {});
    }
}

type BytesFn =
    unsafe extern "C" fn(*const u8, usize, *mut *const u8, *mut usize, *mut *mut std::ffi::c_void);

fn call(f: BytesFn, input: &[u8]) -> Vec<u8> {
    init().expect("the Lean runtime failed to initialise");
    ensure_thread();
    let mut out: *const u8 = std::ptr::null();
    let mut out_len: usize = 0;
    let mut handle: *mut std::ffi::c_void = std::ptr::null_mut();
    // SAFETY: `input` is valid for `input.len()` bytes for the duration of the call; the shim
    // copies it into a Lean object before returning. The out-pointers are valid locals.
    unsafe {
        f(
            input.as_ptr(),
            input.len(),
            &mut out,
            &mut out_len,
            &mut handle,
        )
    };
    let bytes = if out_len == 0 {
        Vec::new()
    } else {
        // SAFETY: the shim set `out` to `out_len` bytes inside the Lean object `handle`, which
        // stays alive until it is released below.
        unsafe { std::slice::from_raw_parts(out, out_len) }.to_vec()
    };
    // SAFETY: `handle` is the owned result object returned by this call, released exactly once.
    unsafe { sys::strata_ffi_release(handle) };
    bytes
}

/// Runs the Lean step on `input` and returns its output bytes.
///
/// The spike's step reverses the bytes and appends their sum modulo 256; the kernel will replace it
/// with `transition` over the canonical encoding.
pub fn step(input: &[u8]) -> Vec<u8> {
    call(sys::strata_ffi_step_bytes, input)
}

/// Sends `input` through Lean unchanged: the cost of the boundary alone.
pub fn identity(input: &[u8]) -> Vec<u8> {
    call(sys::strata_ffi_identity_bytes, input)
}

/// The same function as the Lean step, written in Rust, for tests.
pub fn step_reference(input: &[u8]) -> Vec<u8> {
    let mut v: Vec<u8> = input.iter().rev().copied().collect();
    v.push(input.iter().fold(0u8, |s, b| s.wrapping_add(*b)));
    v
}
