//! Offline assertions about the CI triggers and the README's CI claim.
//!
//! Why this file exists
//! --------------------
//! This repository shipped a hardcoded "CI: passing" badge over a workflow that
//! named a branch which has never existed. `ci.yml` said `branches: [main]`;
//! the default branch is `master`. The workflow could not fire on anything, so
//! it produced zero runs, while the badge kept claiming green.
//!
//! These tests run inside `cargo test`, so the ordinary test command notices
//! the next time someone reintroduces a filter that cannot match. They need no
//! network. The live counterpart — which reads `default_branch` and the branch
//! list from the GitHub API on every push — is
//! `.github/workflows/ci-trigger-guard.yml`. Between them: the offline test
//! stops the repo drifting from itself, and the guard stops the repo drifting
//! from GitHub.
//!
//! One rule governs the design of both: assert that the thing you measured was
//! actually measured. A check that cannot fail is a comment.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// This repository's default branch, as reported by
/// `GET /repos/SuperInstance/quilt-jetson`.
///
/// This constant is a *claim*. The measurement that backs it is
/// `ci-trigger-guard.yml`, which reads `default_branch` from the API on every
/// run and goes red if this file disagrees with GitHub. If the default branch
/// ever changes, change it here in the same commit — and let the guard be the
/// receipt.
const DEFAULT_BRANCH: &str = "master";

const CI_WORKFLOW: &str = ".github/workflows/ci.yml";
const GUARD_WORKFLOW: &str = ".github/workflows/ci-trigger-guard.yml";
const ALIAS_FILE: &str = ".github/branch-aliases.txt";
const README: &str = "README.md";

fn repo_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
}

fn read(relative: &str) -> String {
    let path = repo_path(relative);
    fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("could not read {}: {error}", path.display()))
}

/// Drop a trailing `#` comment. Branch names cannot contain `#`.
fn without_comment(line: &str) -> &str {
    match line.find('#') {
        Some(index) => &line[..index],
        None => line,
    }
}

/// Collect every branch named by a `branches: [a, b]` trigger filter.
///
/// This is a line scanner on purpose. Adding a YAML dependency to a repository
/// whose CI has just been shown to be unmeasured would be a poor trade, and the
/// shape of a trigger block is small and stable.
fn branch_filter_names(workflow: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for line in workflow.lines() {
        let line = without_comment(line);
        let Some(rest) = line.trim_start().strip_prefix("branches:") else {
            continue;
        };
        let rest = rest.trim().trim_start_matches('[').trim_end_matches(']');
        for name in rest.split(',') {
            let name = name.trim().trim_matches(['\'', '"']);
            if !name.is_empty() {
                names.insert(name.to_string());
            }
        }
    }
    names
}

/// Branches a workflow may reference before they exist.
fn declared_aliases() -> BTreeSet<String> {
    read(ALIAS_FILE)
        .lines()
        .map(without_comment)
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| line.trim_matches(['\'', '"']).to_string())
        .collect()
}

#[test]
fn ci_triggers_cover_the_default_branch() {
    let names = branch_filter_names(&read(CI_WORKFLOW));

    assert!(
        names.contains(DEFAULT_BRANCH),
        "{CI_WORKFLOW} must name the default branch `{DEFAULT_BRANCH}` in its `branches:` \
         filter, or the workflow cannot fire on anything. It names: {names:?}. \
         A filter that cannot match is a green badge over zero jobs."
    );

    // A push trigger and a pull_request trigger, or the workflow only runs half
    // the time. Count the occurrences rather than the distinct names.
    let push_and_pr = read(CI_WORKFLOW).matches("branches:").count();
    assert!(
        push_and_pr >= 2,
        "{CI_WORKFLOW} should filter both `push` and `pull_request`; found {push_and_pr} \
         `branches:` filters."
    );
}

#[test]
fn every_named_branch_exists_or_is_a_declared_alias() {
    let aliases = declared_aliases();
    for name in branch_filter_names(&read(CI_WORKFLOW)) {
        if name == DEFAULT_BRANCH {
            continue;
        }
        assert!(
            aliases.contains(&name),
            "{CI_WORKFLOW} names the branch `{name}`, which is neither the default branch \
             nor declared in {ALIAS_FILE}. A `branches:` filter is a union, so a phantom \
             name adds no matches -- but it is almost always a typo or a branch that \
             was renamed away. Declare it, or delete it."
        );
    }
}

#[test]
fn the_guard_workflow_is_not_itself_filtered() {
    // The guard exists to prove ci.yml's filter can match. If the guard carried
    // a branch filter, it would be silenced by the very defect it audits --
    // a guard that shares the failure mode of the thing it guards is a comment.
    let guard = read(GUARD_WORKFLOW);
    assert!(
        !guard
            .lines()
            .any(|line| without_comment(line).contains("branches:")),
        "{GUARD_WORKFLOW} must keep UNFILTERED triggers. Adding a `branches:` filter here \
         re-creates the defect this workflow was written to catch."
    );

    for event in ["push:", "pull_request:"] {
        assert!(
            guard.contains(event),
            "{GUARD_WORKFLOW} should listen to `{event}` so it fires on every push and every \
             pull request regardless of branch name."
        );
    }
}

#[test]
fn the_ci_badge_points_at_a_real_run() {
    let readme = read(README);

    assert!(
        !readme.contains("img.shields.io/badge/CI-"),
        "the README's CI badge is a hardcoded shields.io label. That endpoint renders the \
         text it is handed and can never report a failing run; it is a claim, not a \
         receipt. Point it at actions/workflows/ci.yml/badge.svg."
    );

    let expected = format!("actions/workflows/ci.yml/badge.svg?branch={DEFAULT_BRANCH}");
    assert!(
        readme.contains(&expected),
        "the README's CI badge must query the real workflow on the default branch: expected \
         `{expected}`."
    );
}

#[test]
fn the_push_claim_is_true() {
    let readme = read(README);

    assert!(
        !readme.contains("test files, CI runs on every push."),
        "the README still claims CI runs on every push. It does not: `ci.yml` is filtered to \
         the default branch, and a push to a topic branch deliberately runs nothing."
    );

    assert!(
        readme.contains(&format!("CI runs on every push to `{DEFAULT_BRANCH}`")),
        "the README should state what CI actually triggers on, naming `{DEFAULT_BRANCH}`."
    );
}
