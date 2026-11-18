// M7.2c BTree node wipe [LLM-generated]: BTree nodes are freed by a direct
// `Allocator::deallocate` in alloc::collections::btree::node, not through a
// Box / RawVec / Rc drop. rustc now emits
//   call addrspace(200) void @llvm.mem.fact() [ "mem.wipe_cap_words"(ptr addrspace(200) <node>, i64 <size>) ]
// right before that deallocate, with the size taken from the Layout (the
// `size` field, not `align`). `-Zcheri-wipe-btree-nodes=no` (or
// `-Zcheri-wipe-cap-words=no`) emits nothing there.
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

// pop_internal_level frees an InternalNode<u32, u32> (304 bytes).
// CHECK-LABEL: define{{.*}}pop_internal_level
// on: "mem.wipe_cap_words"(ptr addrspace(200) %{{.*}}, i64 304)
// on: call {{.*}}Allocator10deallocate{{.*}}%top, i64 16, i64 304)
// off-NOT: "mem.wipe_cap_words"
// off: call {{.*}}Allocator10deallocate{{.*}}%top, i64 16, i64 304)
// offall-NOT: "mem.wipe_cap_words"
// offall: call {{.*}}Allocator10deallocate{{.*}}%top, i64 16, i64 304)
