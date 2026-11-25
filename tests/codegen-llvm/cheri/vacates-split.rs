// M7.3b [LLM-generated]: the B-tree node split (`Handle::split_leaf_data`, used by the
// leaf and internal `split` on insert) carries `vacates` specs for the key and value
// slots `[idx, old_len)` it moves to the new node. Instantiated here through
// `BTreeMap::insert`: `u64` keys hold no capability (no key wipe), `Box<u64>` values do
// (one value wipe); a map of plain data gets no wipe. Library MIR is built with the
// library's flags, so the result is the same in both revisions.
//
//@ revisions: on off
//@ compile-flags: -Copt-level=0 -Cdebuginfo=0
//@[off] compile-flags: -Zcheri-spec-vacates=no
//@ only-riscv64cheri-unknown-none-purecap

#![crate_type = "lib"]
#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use alloc::collections::BTreeMap;

// CHECK-LABEL: define {{.*}}MutyINtNt{{.*}}BoxyE{{.*}}4LeafE{{.*}}15split_leaf_data
// CHECK: mem.wipe_cap_words
// CHECK-NOT: mem.wipe_cap_words
// CHECK: ret
// CHECK-LABEL: define {{.*}}MutyINtNt{{.*}}BoxyE{{.*}}8InternalE{{.*}}15split_leaf_data
// CHECK: mem.wipe_cap_words
// CHECK-NOT: mem.wipe_cap_words
// CHECK: ret
// CHECK-LABEL: define {{.*}}MutyyNt{{.*}}4LeafE{{.*}}15split_leaf_data
// CHECK-NOT: mem.wipe_cap_words
// CHECK: ret
// CHECK-LABEL: define {{.*}}MutyyNt{{.*}}8InternalE{{.*}}15split_leaf_data
// CHECK-NOT: mem.wipe_cap_words
// CHECK: ret

#[no_mangle]
pub fn insert_box(m: &mut BTreeMap<u64, Box<u64>>, k: u64, v: Box<u64>) -> Option<Box<u64>> {
    m.insert(k, v)
}

#[no_mangle]
pub fn insert_plain(m: &mut BTreeMap<u64, u64>, k: u64, v: u64) -> Option<u64> {
    m.insert(k, v)
}
