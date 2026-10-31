#![allow(unknown_lints, mem_contract_undischarged)] // CHERI M3.5: warn-by-default; stdlib silenced
compile_error!("This crate should never be compiled");
