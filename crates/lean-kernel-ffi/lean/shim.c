/* Plain C ABI over the Lean kernel, so Rust (MSVC) never touches lean.h's inline functions.
   Built with Lean's own clang (leanc); the toolchain ships no libc headers, so only lean.h. */
#include <lean/lean.h>

extern lean_object *strata_ffi_step(lean_object *);
extern lean_object *strata_ffi_identity(lean_object *);
extern lean_object *initialize_strataffi_StrataFfi(uint8_t builtin);
extern void lean_initialize_runtime_module(void);
extern void lean_io_mark_end_initialization(void);
/* Exported by the runtime but not declared in lean.h. */
extern void lean_initialize_thread(void);
extern void lean_finalize_thread(void);

#define API __declspec(dllexport)

API int strata_ffi_init(void) {
  lean_initialize_runtime_module();
  lean_object *res = initialize_strataffi_StrataFfi(1);
  if (!lean_io_result_is_ok(res)) {
    lean_io_result_show_error(res);
    lean_dec_ref(res);
    return 1;
  }
  lean_dec_ref(res);
  lean_io_mark_end_initialization();
  return 0;
}

API void strata_ffi_thread_init(void) { lean_initialize_thread(); }
API void strata_ffi_thread_end(void) { lean_finalize_thread(); }

typedef lean_object *(*lean_bytes_fn)(lean_object *);

/* Runs `f` on a copy of `in`. The result bytes stay inside a Lean object: `*out` points at them and
   `*handle` owns them until strata_ffi_release(*handle). */
static void call_bytes(lean_bytes_fn f, const uint8_t *in, size_t len, const uint8_t **out,
                       size_t *out_len, void **handle) {
  lean_object *arr = lean_alloc_sarray(1, len, len);
  uint8_t *dst = lean_sarray_cptr(arr);
  for (size_t i = 0; i < len; i++) dst[i] = in[i];
  lean_object *r = f(arr);
  *out = lean_sarray_cptr(r);
  *out_len = lean_sarray_size(r);
  *handle = r;
}

API void strata_ffi_step_bytes(const uint8_t *in, size_t len, const uint8_t **out, size_t *out_len,
                               void **handle) {
  call_bytes(strata_ffi_step, in, len, out, out_len, handle);
}

API void strata_ffi_identity_bytes(const uint8_t *in, size_t len, const uint8_t **out,
                                   size_t *out_len, void **handle) {
  call_bytes(strata_ffi_identity, in, len, out, out_len, handle);
}

API void strata_ffi_release(void *handle) { lean_dec((lean_object *)handle); }
