//! The differential harness without Lean: the minimiser on synthetic predicates, and the batching
//! and scenario-file functions it relies on. Nothing here runs `lake`.

use std::convert::Infallible;
use strata_verification::drt::{ddmin, minimise, parse_cases, render_cases, Case};
use strata_verification::lean::{scenarios, ScenarioStream};

fn case(name: &str, ops: &[&str]) -> Case {
    Case {
        name: name.to_string(),
        ops: ops.iter().map(|o| o.to_string()).collect(),
    }
}

#[test]
fn ddmin_keeps_exactly_the_items_that_matter() {
    let items: Vec<u32> = (0..20).collect();
    let kept = ddmin(&items, &mut |s: &[u32]| {
        Ok::<bool, Infallible>(s.contains(&3) && s.contains(&17))
    })
    .unwrap();
    assert_eq!(kept, vec![3, 17]);
}

#[test]
fn ddmin_returns_its_input_when_the_input_does_not_fail() {
    let items = vec![1, 2, 3];
    let kept = ddmin(&items, &mut |_: &[i32]| Ok::<bool, Infallible>(false)).unwrap();
    assert_eq!(kept, items);
}

#[test]
fn a_failing_test_stops_the_search_with_its_error() {
    let items: Vec<u32> = (0..8).collect();
    let result = ddmin(&items, &mut |s: &[u32]| -> Result<bool, &str> {
        if s.len() < 4 {
            Err("boom")
        } else {
            Ok(true)
        }
    });
    assert_eq!(result, Err("boom"));
}

#[test]
fn minimise_keeps_one_scenario_and_the_two_operations_that_fail() {
    let cases = vec![
        case("a", &["op 1 100 x1", "op 1 100 x2"]),
        case(
            "b",
            &["op 1 100 x0", "op 1 100 x2", "op 1 100 x5", "op 1 100 x9"],
        ),
        case("c", &["op 1 100 x5"]),
    ];
    let kept = minimise(&cases, &mut |cs: &[Case]| {
        Ok::<bool, Infallible>(cs.iter().any(|c| {
            c.name == "b"
                && c.ops.iter().any(|o| o.ends_with("x2"))
                && c.ops.iter().any(|o| o.ends_with("x5"))
        }))
    })
    .unwrap();
    assert_eq!(kept, vec![case("b", &["op 1 100 x2", "op 1 100 x5"])]);
}

#[test]
fn minimise_leaves_a_case_that_cannot_shrink_alone() {
    let cases = vec![case("only", &["op 1 100 x1", "op 1 100 x2"])];
    let kept = minimise(&cases, &mut |cs: &[Case]| {
        Ok::<bool, Infallible>(cs.iter().map(|c| c.ops.len()).sum::<usize>() == 2)
    })
    .unwrap();
    assert_eq!(kept, cases);
}

#[test]
fn minimise_returns_a_predicate_error() {
    let cases = vec![case("a", &["op 1 100 x1"]), case("b", &["op 1 100 x9"])];
    let result = minimise(&cases, &mut |cs: &[Case]| -> Result<bool, &str> {
        if cs.len() == 1 {
            Err("lake failed")
        } else {
            Ok(true)
        }
    });
    assert_eq!(result, Err("lake failed"));
}

#[test]
fn batches_of_one_stream_are_the_generator_output() {
    let mut stream = ScenarioStream::new(20_261_125);
    let joined = format!("{}{}", stream.batch(7), stream.batch(5));
    assert_eq!(joined, scenarios(20_261_125, 12));
}

#[test]
fn parsing_and_rendering_a_scenario_file_is_the_identity() {
    let text = scenarios(3, 15);
    let cases = parse_cases(&text);
    assert_eq!(cases.len(), 15);
    assert!(cases.iter().all(|c| c.ops.len() >= 10));
    assert_eq!(render_cases(&cases), text);
}

#[test]
fn full_batches_of_one_stream_are_the_full_generator_output() {
    let mut stream = ScenarioStream::full(9);
    let joined = format!("{}{}", stream.batch(4), stream.batch(3));
    assert_eq!(joined, ScenarioStream::full(9).batch(7));
}

/// The `i`th token of an operation line, or `""`.
fn at<'a>(tokens: &[&'a str], i: usize) -> &'a str {
    tokens.get(i).copied().unwrap_or("")
}

/// Every operation form the full generator must emit at least once, as predicates on the tokens of
/// an `op` line: `op <tenant> <principal> <intent> [...]`.
fn operation_forms() -> Vec<(&'static str, fn(&[&str]) -> bool)> {
    vec![
        ("create", |t: &[&str]| at(t, 3) == "create"),
        ("create with no field named", |t: &[&str]| {
            at(t, 3) == "create" && t.len() == 4
        }),
        ("update with a version", |t: &[&str]| {
            at(t, 3) == "update" && at(t, 5).starts_with("v=")
        }),
        ("update without a version", |t: &[&str]| {
            at(t, 3) == "update" && !at(t, 5).starts_with("v=")
        }),
        ("patch with a version", |t: &[&str]| {
            at(t, 3) == "patch" && at(t, 5).starts_with("v=")
        }),
        ("patch without a version", |t: &[&str]| {
            at(t, 3) == "patch" && !at(t, 5).starts_with("v=")
        }),
        ("patch that removes a field", |t: &[&str]| {
            at(t, 3) == "patch" && t.iter().any(|x| x.ends_with("=n"))
        }),
        ("delete with a version", |t: &[&str]| {
            at(t, 3) == "delete" && at(t, 5).starts_with("v=")
        }),
        ("delete without a version", |t: &[&str]| {
            at(t, 3) == "delete" && !at(t, 5).starts_with("v=")
        }),
        ("require strict", |t: &[&str]| {
            at(t, 3) == "require" && at(t, 5) == "strict"
        }),
        ("require lax", |t: &[&str]| {
            at(t, 3) == "require" && at(t, 5) == "lax"
        }),
        ("require of a reference field", |t: &[&str]| {
            at(t, 3) == "require" && matches!(at(t, 4), "4" | "5")
        }),
        ("require of a list field", |t: &[&str]| {
            at(t, 3) == "require" && matches!(at(t, 4), "0" | "7" | "8")
        }),
        ("reference in field 4", |t: &[&str]| {
            t.iter().any(|x| x.starts_with("4=r:"))
        }),
        ("reference in field 5", |t: &[&str]| {
            t.iter().any(|x| x.starts_with("5=r:"))
        }),
        ("list of references in field 0", |t: &[&str]| {
            t.iter().any(|x| x.starts_with("0=l:r:"))
        }),
        ("list of references in field 8", |t: &[&str]| {
            t.iter().any(|x| x.starts_with("8=l:r:"))
        }),
        ("list of scalars in field 7", |t: &[&str]| {
            t.iter().any(|x| x.starts_with("7=l:s:"))
        }),
        ("empty list", |t: &[&str]| {
            t.iter().any(|x| matches!(*x, "0=l:" | "7=l:" | "8=l:"))
        }),
        ("list at the length limit", |t: &[&str]| {
            t.iter().any(|x| x.contains("l*1000:"))
        }),
        ("list over the length limit", |t: &[&str]| {
            t.iter().any(|x| x.contains("l*1001:"))
        }),
        ("string at the length limit", |t: &[&str]| {
            t.iter().any(|x| x.contains("s*65536:"))
        }),
        ("string over the length limit", |t: &[&str]| {
            t.iter().any(|x| x.contains("s*65537:"))
        }),
        ("reference to a never-issued id", |t: &[&str]| {
            t.iter().any(|x| x.contains("r:9"))
        }),
        ("unknown field", |t: &[&str]| {
            t.iter().any(|x| x.starts_with("9="))
        }),
        ("null field value", |t: &[&str]| {
            t.iter().any(|x| x.ends_with("=n"))
        }),
        ("tenant with no data", |t: &[&str]| at(t, 1) == "3"),
        ("stranger principal", |t: &[&str]| at(t, 2) == "999"),
        ("member of the other tenant", |t: &[&str]| {
            let (tenant, principal) = (at(t, 1), at(t, 2));
            (tenant == "1" && principal.starts_with("20"))
                || (tenant == "2" && principal.starts_with("10"))
        }),
    ]
}

#[test]
fn the_full_generator_emits_every_operation_form() {
    let text = ScenarioStream::full(20_261_125).batch(300);
    let ops: Vec<String> = parse_cases(&text).into_iter().flat_map(|c| c.ops).collect();
    assert!(ops.len() > 300 * 10, "only {} operations", ops.len());
    let missing: Vec<&str> = operation_forms()
        .into_iter()
        .filter(|(_, form)| {
            !ops.iter().any(|line| {
                let tokens: Vec<&str> = line.split_whitespace().collect();
                form(&tokens[..])
            })
        })
        .map(|(name, _)| name)
        .collect();
    assert!(
        missing.is_empty(),
        "the full generator never emits: {missing:?}"
    );
}
