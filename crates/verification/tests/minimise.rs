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
