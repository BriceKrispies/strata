//! Compiled Lean called from Rust: the bytes come back as the Lean function computes them, from
//! one thread and from many tokio worker threads at once.

use strata_lean_kernel_ffi::{step, step_reference};

#[test]
fn the_lean_step_returns_the_bytes_lean_computes() {
    assert_eq!(step(&[]), vec![0]);
    assert_eq!(step(&[1, 2, 3]), vec![3, 2, 1, 6]);
    assert_eq!(step(&[200, 100]), vec![100, 200, 44]);
    let big: Vec<u8> = (0..1_000_000u32).map(|i| (i % 251) as u8).collect();
    assert_eq!(step(&big), step_reference(&big));
}

#[test]
fn many_tokio_worker_threads_call_lean_at_once() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(8)
        .build()
        .unwrap();
    rt.block_on(async {
        let mut tasks = Vec::new();
        for t in 0..64u32 {
            tasks.push(tokio::task::spawn_blocking(move || {
                for i in 0..200u32 {
                    let input: Vec<u8> = (0..(t * 7 + i) % 513).map(|k| (k ^ t) as u8).collect();
                    assert_eq!(step(&input), step_reference(&input), "thread {t}, call {i}");
                }
            }));
        }
        for task in tasks {
            task.await.unwrap();
        }
    });
}
