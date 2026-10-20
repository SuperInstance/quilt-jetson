//! The MSRV in `Cargo.toml` and the toolchain the CI job installs must be
//! the same number.
//!
//! Why this file exists
//! --------------------
//! `Cargo.toml` declared `rust-version = "1.75"`. The `msrv` CI job also
//! named 1.75. Both agreed, and both were wrong: the dependency set
//! requires rustc 1.89, and cargo 1.75 cannot even parse
//! `indexmap-2.14.2`'s manifest because it declares `edition2024`.
//!
//! Two numbers agreeing with each other is not evidence that either is
//! right. They agreed because nobody had checked either against the
//! dependency graph. This test does the thing the two declarations cannot:
//! it reads the manifest, reads the workflow, and requires that they name
//! the same version -- so the next Dependabot bump of one and not the
//! other is caught by a test instead of by a red badge nobody reads.
//!
//! It deliberately does NOT verify that the number is correct in the
//! abstract. That cannot be checked offline: it depends on which versions
//! the floating dependency ranges resolve to on the day. What it can check
//! is that the manifest and the workflow say the same thing, and that the
//! declared version is one that could plausibly have been released.

use std::fs;
use std::path::{Path, PathBuf};

/// Repository root, from the manifest directory.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

/// The `rust-version = "..."` value from `Cargo.toml`.
fn manifest_msrv() -> String {
    let text = fs::read_to_string(repo_root().join("Cargo.toml")).expect("Cargo.toml is readable");
    text.lines()
        .map(str::trim)
        .find_map(|l| l.strip_prefix("rust-version"))
        .and_then(|rest| rest.trim().strip_prefix('='))
        .map(|rest| rest.trim().trim_matches('"').to_string())
        .expect("Cargo.toml declares a rust-version")
}

/// The toolchain version the `msrv:` job installs, from `ci.yml`.
///
/// The step is `dtolnay/rust-toolchain@<version>`, so the version is the
/// text after the `@`.
fn workflow_msrv() -> String {
    let text = fs::read_to_string(repo_root().join(".github/workflows/ci.yml"))
        .expect("ci.yml is readable");

    let mut in_msrv_job = false;
    for line in text.lines() {
        // A job key sits at two spaces of indentation; a step at four.
        if line.starts_with("  msrv:") {
            in_msrv_job = true;
            continue;
        }
        if in_msrv_job
            && line.starts_with("  ")
            && !line.starts_with("   ")
            && line.trim_end().ends_with(':')
        {
            in_msrv_job = false;
        }
        if in_msrv_job {
            if let Some(rest) = line.trim().strip_prefix("- uses: dtolnay/rust-toolchain@") {
                return rest.trim().to_string();
            }
        }
    }
    panic!("ci.yml has no `dtolnay/rust-toolchain@<version>` step inside the `msrv` job");
}

#[test]
fn manifest_and_workflow_agree_on_the_msrv() {
    let manifest = manifest_msrv();
    let workflow = workflow_msrv();
    assert_eq!(
        manifest, workflow,
        "Cargo.toml says rust-version = \"{manifest}\" but ci.yml's msrv job installs \
         rust-toolchain@{workflow}. One Dependabot bump can move only one of these, \
         and the disagreement is invisible until the job fails for a reason that has \
         nothing to do with the code."
    );
}

#[test]
fn the_declared_msrv_was_actually_released() {
    let msrv = manifest_msrv();
    let parts: Vec<&str> = msrv.split('.').collect();
    assert!(
        parts.len() == 2 || parts.len() == 3,
        "rust-version \"{msrv}\" is not a version number"
    );

    // The manifest once declared 1.120, a version that does not exist,
    // because a Dependabot bump landed in the job NAME rather than the
    // toolchain. A release-series sanity check catches that shape of
    // error without needing the network.
    let minor: u32 = parts[1]
        .parse()
        .unwrap_or_else(|_| panic!("rust-version \"{msrv}\" has a non-numeric minor version"));

    assert!(
        (1..=99).contains(&minor),
        "rust-version \"{msrv}\" has an implausible minor version. Toolchain versions \
         are not released with a minor number in the hundreds -- check whether a \
         Dependabot bump was pasted into the wrong field."
    );
}

#[test]
fn the_msrv_is_not_the_value_that_was_measured_wrong() {
    // 1.75 was the declared value from the initial release and was never
    // true: `indexmap = "2"` resolves to a crate whose manifest requires
    // `edition2024`, which cargo 1.75 cannot parse. If this ever returns to
    // 1.75, something has pinned the whole dependency graph backwards, and
    // that deserves a deliberate decision rather than a silent revert.
    let msrv = manifest_msrv();
    assert_ne!(
        msrv, "1.75",
        "rust-version is back at 1.75. That value was measured wrong on 2026-10-20 \
         (cargo 1.75 cannot parse indexmap 2.14.2's edition2024 manifest). If the \
         dependency graph has genuinely been pinned to support it, say so here."
    );
}
