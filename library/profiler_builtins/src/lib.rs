// tidy-alphabetical-start
#![allow(internal_features)]
#![allow(unknown_lints, mem_contract_undischarged)] // CHERI M3.5: warn-by-default; stdlib silenced

#![feature(no_core)]
#![feature(profiler_runtime)]
#![feature(staged_api)]
// tidy-alphabetical-end

// Other attributes:
#![no_core]
#![profiler_runtime]
#![unstable(
    feature = "profiler_runtime_lib",
    reason = "internal implementation detail of rustc right now",
    issue = "none"
)]
