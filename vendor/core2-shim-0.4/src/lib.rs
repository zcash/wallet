//! Temporary `core2` 0.4.x shim that re-exports [`corez`].
//!
//! See `../core2-shim-0.3/README.md` and the workspace-root
//! `[patch.crates-io]` table for context and removal criteria. This crate
//! exists solely to satisfy transitive dependencies pinned to `core2 ^0.4`
//! while all `core2` releases on crates.io are yanked.
//!
//! Do not depend on this crate from any Zallet code.

#![no_std]

pub use corez::*;
