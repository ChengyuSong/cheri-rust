// M7.2c BTree node wipe [LLM-generated]: BTree nodes are freed by a direct
// `Allocator::deallocate` in alloc::collections::btree::node, not through a
// Box / RawVec / Rc drop. rustc now emits `mem.wipe_cap_words` facts right
// before that deallocate. The site is found through the call's MIR inline
// scopes, from the `#[rustc_cheri_spec = "dealloc_of(.., fields)"]` specs on
// the node.rs functions that free nodes (M7.2e; was a module marker). When K and V hold no capabilities only the cap
// fields are wiped: the parent link (16 bytes) always, and for an internal
// node the 12-edge array (192 bytes) as chunks of at most 8 cap words
// (128 + 64). When K or V may hold capabilities the whole node is wiped
// (the Layout size, taken from the `size` field, not `align`).
// `-Zcheri-wipe-btree-nodes=no` (or `-Zcheri-wipe-cap-words=no`) emits
// nothing there.
//
//@ revisions: on off offall
//@ compile-flags: -Cdebuginfo=0 -Copt-level=0
//@[off] compile-flags: -Zcheri-wipe-btree-nodes=no
//@[offall] compile-flags: -Zcheri-wipe-cap-words=no
//@ only-riscv64cheri-unknown-none-purecap

#![crate_type = "lib"]
#![no_std]

extern crate alloc;

use alloc::collections::BTreeMap;

#[no_mangle]
pub fn btree_churn(n: u32) -> usize {
    let mut m: BTreeMap<u32, u32> = BTreeMap::new();
    for i in 0..n {
        m.insert(i, i);
    }
    for i in 0..n {
        m.remove(&i);
    }
    m.len()
}

#[no_mangle]
pub fn btree_churn_caps(n: u32, s: &'static str) -> usize {
    let mut m: BTreeMap<u32, &'static str> = BTreeMap::new();
    for i in 0..n {
        m.insert(i, s);
    }
    for i in 0..n {
        m.remove(&i);
    }
    m.len()
}

// pop_internal_level of BTreeMap<u32, &str>: V holds caps, whole node.
// CHECK-LABEL: define{{.*}}marker5OwnedmReNtB1a_14LeafOrInternalE18pop_internal_level
// on: "mem.wipe_cap_words"(ptr addrspace(200) %top, i64 608)
// on-NEXT: ; call <alloc::alloc::Global as core::alloc::Allocator>::deallocate
// off-NOT: "mem.wipe_cap_words"
// offall-NOT: "mem.wipe_cap_words"
// CHECK: call {{.*}}Allocator10deallocate{{.*}}%top, i64 16, i64 608)

// pop_internal_level of BTreeMap<u32, u32>: parent link + edges only.
// CHECK-LABEL: define{{.*}}marker5OwnedmmNtB1a_14LeafOrInternalE18pop_internal_level
// on: "mem.wipe_cap_words"(ptr addrspace(200) %top, i64 16)
// on-NEXT: getelementptr inbounds i8, ptr addrspace(200) %top, i64 112
// on-NEXT: "mem.wipe_cap_words"(ptr addrspace(200) %{{.*}}, i64 128)
// on-NEXT: getelementptr inbounds i8, ptr addrspace(200) %top, i64 240
// on-NEXT: "mem.wipe_cap_words"(ptr addrspace(200) %{{.*}}, i64 64)
// off-NOT: "mem.wipe_cap_words"
// offall-NOT: "mem.wipe_cap_words"
// CHECK: call {{.*}}Allocator10deallocate{{.*}}%top, i64 16, i64 304)
