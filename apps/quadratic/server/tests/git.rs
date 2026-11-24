use std::collections::BTreeSet;

use quadratic_server::git::ids;
use quadratic_server::git::watch::{
    Candidate, Commit, Head, Observed, Place, Reached, Reading, Seen, step,
};
use quadratic_server::store::{GitWatch, GitWork};

const REPOSITORY: &str = "/r/.git";

fn known(tip: &str, work: &[(&str, &str)]) -> GitWatch {
    GitWatch {
        repository: REPOSITORY.into(),
        default_branch: "main".into(),
        tip: tip.into(),
        upstream: None,
        work: work
            .iter()
            .map(|(branch, tip)| GitWork {
                branch: (*branch).into(),
                tip: (*tip).into(),
            })
            .collect(),
    }
}

fn seen(tip: &str, heads: &[(&str, &str)], outside: &[&str], reached: Reached) -> Seen {
    let mut all = vec![Head {
        branch: "main".into(),
        tip: tip.into(),
    }];
    all.extend(heads.iter().map(|(branch, tip)| Head {
        branch: (*branch).into(),
        tip: (*tip).into(),
    }));
    Seen {
        repository: REPOSITORY.into(),
        default_branch: "main".into(),
        heads: all,
        local: Reading {
            place: main_at(tip),
            outside: outside
                .iter()
                .map(|branch| (*branch).to_owned())
                .collect::<BTreeSet<_>>(),
            reached,
        },
        upstream: None,
    }
}

fn main_at(tip: &str) -> Place {
    Place {
        name: "main".into(),
        tip: tip.into(),
    }
}

fn commits(hashes: &[&str]) -> Reached {
    Reached::Since(
        hashes
            .iter()
            .map(|hash| Commit {
                hash: (*hash).into(),
                refs: Vec::new(),
            })
            .collect(),
    )
}

fn nothing_in_main(tip: &str) -> Vec<Observed> {
    vec![Observed {
        place: main_at(tip),
        branches: Vec::new(),
        trailers: Vec::new(),
    }]
}

#[test]
fn a_branch_names_the_issues_whose_ids_it_contains_between_word_boundaries() {
    assert_eq!(ids::in_branch("abc-12-short-title", "ABC"), ["ABC-12"]);
    assert_eq!(ids::in_branch("feature/ABC-7", "ABC"), ["ABC-7"]);
    assert_eq!(
        ids::in_branch("abc-12-and-abc-3", "ABC"),
        ["ABC-12", "ABC-3"]
    );
    for unrelated in ["xabc-12", "abc-12x", "abc-012", "abc-", "agent-a", "dem-12"] {
        assert_eq!(
            ids::in_branch(unrelated, "ABC"),
            Vec::<String>::new(),
            "{unrelated}"
        );
    }
}

#[test]
fn refs_trailers_name_one_or_several_issues_of_the_project() {
    let values = vec!["ABC-12, abc-13".to_owned(), "ABC-14 DEM-2".to_owned()];
    assert_eq!(
        ids::in_trailers(&values, "ABC"),
        ["ABC-12", "ABC-13", "ABC-14"]
    );
    assert_eq!(
        ids::in_trailers(&["#12 ABC-0 ABC12".to_owned()], "ABC"),
        Vec::<String>::new()
    );
}

#[test]
fn a_refs_line_in_any_paragraph_of_a_message_gives_its_value() {
    let message = "Parse the headers (#7)\n\n* feat: parse the headers\n\nRefs: ABC-12, ABC-13\n\n* fix: an empty header\n\nrefs: ABC-14\r\nREFS:ABC-15\n";
    assert_eq!(
        ids::refs_values(message),
        ["ABC-12, ABC-13", "ABC-14", "ABC-15"]
    );
}

#[test]
fn a_line_that_does_not_start_with_the_refs_key_gives_nothing() {
    for line in [
        "See Refs: ABC-12 for the history",
        "  Refs: ABC-12",
        "\tRefs: ABC-12",
        "> Refs: ABC-12",
        "* Refs: ABC-12",
        "Refs : ABC-12",
        "References: ABC-12",
        "Refs ABC-12",
        "Refs:",
    ] {
        let message = format!("Parse the headers\n\n{line}");
        assert_eq!(ids::refs_values(&message), Vec::<String>::new(), "{line}");
    }
}

#[test]
fn the_first_look_settles_nothing_and_keeps_the_branches_outside_the_default_branch() {
    let (observed, next) = step(
        None,
        &seen(
            "m1",
            &[("agent-a", "a1"), ("old", "m0")],
            &["agent-a"],
            commits(&["a1", "m1"]),
        ),
    );
    assert_eq!(observed, Vec::<Observed>::new());
    assert_eq!(next.tip, "m1");
    assert_eq!(
        next.work,
        [GitWork {
            branch: "agent-a".into(),
            tip: "a1".into(),
        }]
    );
}

#[test]
fn a_kept_branch_that_the_default_branch_now_reaches_is_a_candidate_with_its_kept_tip() {
    let (observed, next) = step(
        Some(&known("m0", &[("agent-a", "a1")])),
        &seen("m1", &[("agent-a", "a1")], &[], commits(&["m1", "a1"])),
    );
    assert_eq!(
        observed[0].branches,
        [Candidate {
            branch: "agent-a".into(),
            tip: "a1".into(),
            work: "a1".into(),
        }]
    );
    assert!(next.work.is_empty());
}

#[test]
fn a_deleted_branch_is_no_candidate() {
    let (observed, _) = step(
        Some(&known("m0", &[("agent-a", "a1")])),
        &seen("m1", &[], &[], commits(&["m1", "a1"])),
    );
    assert_eq!(observed, nothing_in_main("m1"));
}

#[test]
fn a_default_branch_that_was_rewritten_or_changed_settles_nothing() {
    let rewritten = seen("m9", &[("agent-a", "a1")], &[], Reached::Unknown);
    let (observed, _) = step(Some(&known("m0", &[("agent-a", "a1")])), &rewritten);
    assert_eq!(observed, Vec::<Observed>::new());

    let mut renamed = seen("m1", &[("agent-a", "a1")], &[], commits(&["m1", "a1"]));
    renamed.default_branch = "trunk".into();
    let (observed, next) = step(Some(&known("m0", &[("agent-a", "a1")])), &renamed);
    assert_eq!(observed, Vec::<Observed>::new());
    assert_eq!(next.default_branch, "trunk");
}

#[test]
fn each_newly_reached_commit_with_a_refs_trailer_is_a_landing() {
    let trailer = Commit {
        hash: "s1".into(),
        refs: vec!["ABC-12".into()],
    };
    let reached = Reached::Since(vec![
        trailer.clone(),
        Commit {
            hash: "s0".into(),
            refs: Vec::new(),
        },
    ]);
    let (observed, _) = step(Some(&known("m0", &[])), &seen("s1", &[], &[], reached));
    assert_eq!(observed[0].trailers, [trailer]);
}
