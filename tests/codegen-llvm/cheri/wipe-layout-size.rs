// M7.2c follow-up [LLM-generated]: the fallback that takes a wipe length
// from a deallocate call's `Layout` (used for Box of an unsized pointee,
// whose size is not a compile-time constant) must use the Layout's `size`,
// not its alignment. The `Layout` scalar pair is passed in memory order,
// (align, size), so element 0 is the alignment.
// M7.2e: the length now comes from the `frees(block, size = size_of_val(*self.0))`
// spec on `Box::drop` (and the `deallocate` spec's `layout.size`), so the
// `-Zcheri-wipe-layout-size=no` legacy reading (element 0) is gone and the switch
// has no effect; the `old` revision was removed.
//
//@ compile-flags: -Cdebuginfo=0 -Copt-level=0
//@ only-riscv64cheri-unknown-none-purecap

#![crate_type = "lib"]
#![no_std]

extern crate alloc;

use alloc::boxed::Box;

pub trait Tr {
    fn f(&self) -> usize;
}

#[no_mangle]
pub fn drop_box_dyn(b: Box<dyn Tr>) {
    drop(b)
}

#[no_mangle]
pub fn drop_box_slice(b: Box<[&'static u8]>) {
    drop(b)
}

// Box<dyn Tr>: size and align both come from the vtable.
// CHECK-LABEL: define{{.*}}3BoxDNtC{{.*}}2TrEL_E{{.*}}4Drop4drop
// CHECK: "mem.wipe_cap_words"(ptr addrspace(200) %{{.*}}, i64 %size)
// CHECK-NEXT: ; call <alloc::alloc::Global as core::alloc::Allocator>::deallocate
// CHECK-NEXT: call {{.*}}Allocator10deallocate{{.*}}, i64 %align, i64 %size)

// Box<[&u8]>: the alignment is the constant 16, the size is computed.
// CHECK-LABEL: define{{.*}}3BoxSRhE{{.*}}4Drop4drop
// CHECK: "mem.wipe_cap_words"(ptr addrspace(200) %{{.*}}, i64 %size)
// CHECK-NEXT: ; call <alloc::alloc::Global as core::alloc::Allocator>::deallocate
// CHECK-NEXT: call {{.*}}Allocator10deallocate{{.*}}, i64 16, i64 %size)
