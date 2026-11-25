//! Builds the Lean library in `lean/` and links it as a DLL with a plain C ABI.
//!
//! 1. `lake build` compiles the Lean modules to C (`lean/.lake/build/ir/*.c`).
//! 2. `leanc` (Lean's bundled clang) compiles that C and `lean/shim.c`, and links them into
//!    `strata_ffi.dll` with an import library `strata_ffi.lib`. The DLL imports the Lean runtime from
//!    `libInit_shared.dll` in the toolchain.
//! 3. Both DLLs are copied next to the test and binary outputs so Windows finds them at run time.
//!
//! Rust never includes `lean.h`: its inline functions are compiled into the shim by `leanc`.
//! Needs `lake` and `leanc` on PATH (elan).

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn run(cmd: &mut Command) -> String {
    let shown = format!("{cmd:?}");
    let out = cmd.output().unwrap_or_else(|e| {
        panic!("could not run {shown} (is Lean installed through elan and on PATH?): {e}")
    });
    if !out.status.success() {
        panic!(
            "{shown} failed:\n{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let lean_dir = manifest.join("lean");
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

    for f in [
        "StrataFfi.lean",
        "shim.c",
        "lakefile.toml",
        "lean-toolchain",
    ] {
        println!("cargo:rerun-if-changed=lean/{f}");
    }

    run(Command::new("lake").arg("build").current_dir(&lean_dir));

    let ir = lean_dir.join(".lake/build/ir/StrataFfi.c");
    let module_o = out_dir.join("StrataFfi.o");
    let shim_o = out_dir.join("shim.o");
    let dll = out_dir.join("strata_ffi.dll");
    let implib = out_dir.join("strata_ffi.lib");

    run(Command::new("leanc")
        .args(["-c", "-O2", "-o"])
        .arg(&module_o)
        .arg(&ir));
    run(Command::new("leanc")
        .args(["-c", "-O2", "-o"])
        .arg(&shim_o)
        .arg(lean_dir.join("shim.c")));
    run(Command::new("leanc")
        .arg("-shared")
        .arg("-o")
        .arg(&dll)
        .arg(&module_o)
        .arg(&shim_o)
        .arg(format!("-Wl,--out-implib,{}", implib.display())));

    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=dylib=strata_ffi");

    // Run-time location: next to the binaries of this profile (target/<profile> and its deps/).
    let prefix = PathBuf::from(run(Command::new("lean").arg("--print-prefix")));
    let runtime = prefix.join("bin").join("libInit_shared.dll");
    let profile_dir = out_dir
        .ancestors()
        .nth(3)
        .expect("OUT_DIR is target/<profile>/build/<pkg>/out")
        .to_path_buf();
    for dest in [profile_dir.clone(), profile_dir.join("deps")] {
        copy_if_newer(&dll, &dest.join("strata_ffi.dll"));
        copy_if_newer(&runtime, &dest.join("libInit_shared.dll"));
    }
    println!("cargo:rustc-env=STRATA_FFI_DLL={}", dll.display());
}

fn copy_if_newer(from: &Path, to: &Path) {
    let newer = match (std::fs::metadata(from), std::fs::metadata(to)) {
        (Ok(a), Ok(b)) => a.len() != b.len() || a.modified().ok() > b.modified().ok(),
        _ => true,
    };
    if newer {
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        // A running test binary keeps the DLL open; a failed copy of an identical file is harmless.
        if let Err(e) = std::fs::copy(from, to) {
            println!(
                "cargo:warning=could not copy {} to {}: {e}",
                from.display(),
                to.display()
            );
        }
    }
}
