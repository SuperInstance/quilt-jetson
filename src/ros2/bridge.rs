//! The live ROS2 bridge — **NOT IMPLEMENTED**.
//!
//! # Why this file exists at all
//!
//! `src/ros2/mod.rs` declares `#[cfg(feature = "ros2")] pub mod bridge;`
//! and documents "the real implementation lives in `bridge.rs`". **The file
//! did not exist.** So:
//!
//! * `cargo fmt --check` failed, because rustfmt resolves EVERY `mod`
//!   declaration including cfg-gated ones — and the Lint & format job had
//!   been red with "failed to resolve mod `bridge`" ever since;
//! * `cargo build --features ros2` could not work, ever, because the
//!   module it names was absent.
//!
//! So the `ros2` feature was ASPIRATIONAL: named in the manifest, described
//! in the docs, and impossible to build.
//!
//! # Why it is a `compile_error!` and not an implementation
//!
//! An implementation would need `rclrs` 0.1's API, and **it could not be
//! compiled or tested in the environment where this was written**. Shipping
//! an unverifiable 100-line Rust file is exactly the kind of thing this
//! repository's own CI exists to catch. So the feature **FAILS LOUDLY**,
//! with a message that says what to do:
//!
//! * `cargo build` (no features) — **passes**. The default build is
//!   self-contained and always was.
//! * `cargo build --features ros2` — **fails, with this message.** Not
//!   with "file not found", and not silently with a stub that pretends.
//!
//! The stub in `mod.rs` is the real, working, default path: it logs what it
//! would do and connects to nothing. That behaviour is unchanged.
//!
//! # To actually implement it
//!
//! 1. Implement `subscribe_live` and `publish_live` against `rclrs` 0.1
//!    (`rclrs::Context`, `rclrs::node::Node`, `rclrs::subscription::Subscription`,
//!    `rclrs::publisher::Publisher`).
//! 2. Delete the `compile_error!` below.
//! 3. Un-gate the `rclrs` dependency check in `Cargo.toml` if it is not
//!    already correct, and run `cargo build --features ros2` **and**
//!    `cargo test --features ros2` on a machine that has them.
//!
//! **Do not delete the `compile_error!` without step 1.** A `ros2` feature
//! that builds and does nothing is worse than one that refuses to build.

#[cfg(feature = "ros2")]
compile_error!(
    "The live ROS2 bridge is NOT IMPLEMENTED in this repository.\n\n\
     `src/ros2/mod.rs` names this module and documents a real rclrs-backed\n\
     implementation, but the file did not exist -- so `--features ros2` could\n\
     never build, and `cargo fmt --check` was red with 'failed to resolve\n\
     mod bridge'.\n\n\
     This file now exists so the fmt check passes and so the feature fails\n\
     with a MESSAGE instead of a missing-file error.\n\n\
     To implement it: write subscribe_live and publish_live against rclrs 0.1,\n\
     delete the compile_error! below, and run `cargo build --features ros2`\n\
     AND `cargo test --features ros2` on a machine that has them.\n\n\
     The default build (no features) is unaffected: the stub in mod.rs is the\n\
     working path, and it is unchanged."
);
