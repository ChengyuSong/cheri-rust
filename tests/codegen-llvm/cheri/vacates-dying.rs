// M7.3b [LLM-generated]: the B-tree's consuming paths read a key/value out of a node that
// stays allocated until the iterator leaves it (`Handle<Dying, KV>::into_key_val`, used
// by `IntoIter`) or drop it in place (`drop_key_val`, used by `Drop` of the map and of
// `IntoIter`). Both carry `vacates` specs for that one key and value slot, so the stale
// copy is wiped at once instead of when the node is freed. `u64` keys hold no
// capability (no key wipe); `Box<u64>` values do (one wipe); plain maps get none.
// Library MIR is built with the library's flags, so both revisions agree.
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

// CHECK-LABEL: define {{.*}}DyingyINtNt{{.*}}BoxyE{{.*}}12drop_key_val
// CHECK: mem.wipe_cap_words
// CHECK-NOT: mem.wipe_cap_words
// CHECK: ret void
// CHECK-LABEL: define {{.*}}DyingyINtNt{{.*}}BoxyE{{.*}}12into_key_val
// CHECK: mem.wipe_cap_words
// CHECK-NOT: mem.wipe_cap_words
// CHECK: ret
// CHECK-LABEL: define {{.*}}DyingyyNt{{.*}}12drop_key_val
// CHECK-NOT: mem.wipe_cap_words
// CHECK: ret void
// CHECK-LABEL: define {{.*}}DyingyyNt{{.*}}12into_key_val
// CHECK-NOT: mem.wipe_cap_words
// CHECK: ret

#[no_mangle]
pub fn consume_box(m: BTreeMap<u64, Box<u64>>) -> u64 {
    m.into_iter().map(|(k, v)| k + *v).sum()
}

#[no_mangle]
pub fn consume_plain(m: BTreeMap<u64, u64>) -> u64 {
    m.into_iter().map(|(k, v)| k + v).sum()
}
