//! Measures the cost of one call into compiled Lean for a small and a 1 MB payload.
//! `cargo run -p strata-lean-kernel-ffi --release --example overhead`
use std::time::Instant;
use strata_lean_kernel_ffi::{identity, step, step_reference};

fn time(label: &str, input: &[u8], iters: u32) {
    step(input);
    let t = Instant::now();
    for _ in 0..iters {
        std::hint::black_box(step(std::hint::black_box(input)));
    }
    let lean = t.elapsed() / iters;
    let t = Instant::now();
    for _ in 0..iters {
        std::hint::black_box(step_reference(std::hint::black_box(input)));
    }
    let rust = t.elapsed() / iters;
    let t = Instant::now();
    for _ in 0..iters {
        std::hint::black_box(identity(std::hint::black_box(input)));
    }
    let boundary = t.elapsed() / iters;
    println!(
        "{label}: boundary alone {boundary:?}, Lean step {lean:?}, same step in Rust {rust:?}"
    );
}

fn main() {
    time("16 bytes", &[7u8; 16], 200_000);
    let big: Vec<u8> = (0..1_048_576u32).map(|i| (i % 251) as u8).collect();
    time("1 MiB", &big, 50);
}
