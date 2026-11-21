// M7.2d wipe hints [LLM-generated]: three library frees that no container
// drop covers get `mem.wipe_cap_words` facts.
// M7.2e: the library now carries `#[rustc_cheri_spec]` attributes instead of the
// `rustc_diagnostic_item` markers: `dealloc_of(ty = T)` on `Box::map` /
// `Box::try_map` / `WithHeader::drop`, a `frees` spec on the ThinBox `DropGuard`
// drop, and a `live(..)` statement spec on the in-place `collect` shrink.
// * `Box::map`: before the old block's `deallocate` (size changed).
// * `Box::try_map` error path: before the drop of the reused
//   `Box<MaybeUninit<U>>` (same size as `T`; `U` holds no caps here).
// * `ThinBox<T>` drop: before the drop of the `WithHeader::drop` guard
//   (value bytes, read from the guard's `ptr` / `value_layout.size`).
// * in-place `collect`: before the shrink, the bytes past the live elements,
//   `[len * size_of::<U>(), old)` (`WipeDead`), if the old element type may hold
//   capabilities. If the new element type may hold capabilities, the shrink is
//   replaced (MIR branch on `cheri_ty_may_contain_caps::<U>()`) by allocate +
//   copy + wipe of the whole old block + free (`Relocated`), so a block the
//   allocator would move keeps no capability.
// Only when the element types may hold capabilities (`Plain` / `PSrc` get
// nothing). `-Zcheri-wipe-alloc-hints=no` (or `-Zcheri-wipe-cap-words=no`)
// emits none of these; `-Zcheri-spec-relocate-rewrite=no` drops the rewrite's
// wipe (the MIR branch itself follows the flags the library was built with).
// The RawVec drop facts (`dealloc_of` on `RawVec::drop`) stay (and go with
// `-Zcheri-wipe-cap-words=no`).
//
//@ revisions: o0 mir off offall rwoff
//@ compile-flags: -Cdebuginfo=0
//@[o0] compile-flags: -Copt-level=0
//@[mir] compile-flags: -Copt-level=2 -Cno-prepopulate-passes
//@[off] compile-flags: -Copt-level=0 -Zcheri-wipe-alloc-hints=no
//@[offall] compile-flags: -Copt-level=0 -Zcheri-wipe-cap-words=no
//@[rwoff] compile-flags: -Copt-level=0 -Zcheri-spec-relocate-rewrite=no
//@ only-riscv64cheri-unknown-none-purecap

#![crate_type = "lib"]
#![no_std]
#![feature(thin_box, smart_pointer_try_map)]
extern crate alloc;
use alloc::boxed::{Box, ThinBox};
use alloc::vec::Vec;

#[repr(C)]
pub struct Pair { x: u64, p: &'static u64 }
#[repr(C, align(16))]
pub struct Same([u64; 4]);
#[repr(C)]
pub struct Plain { x: u64, y: u64 }
#[repr(C)]
pub struct Src2 { a: u64, b: u64, c: u64, p: &'static u64 }
#[repr(C, align(16))]
pub struct Dst2([u64; 4]);
#[repr(C, align(16))]
pub struct PSrc { a: u64, b: u64, c: u64 }
#[repr(C, align(16))]
pub struct PDst([u64; 2]);
#[repr(C)]
pub struct Src4 { a: u64, b: u64, p: &'static u64, c: u64, d: u64 }
#[repr(C)]
pub struct Dst4 { a: u64, q: &'static u64 }

#[no_mangle]
pub fn hint_map_caps(b: Box<Pair>) -> Box<u64> { Box::map(b, |e| e.x) }
#[no_mangle]
pub fn hint_map_plain(b: Box<Plain>) -> Box<u32> { Box::map(b, |e| e.x as u32) }
#[no_mangle]
pub fn hint_try_map_caps(b: Box<Pair>) -> Result<Box<Same>, u64> {
    Box::try_map(b, |e| if e.x == 7 { Err(e.x) } else { Ok(Same([e.x, 0, 0, 0])) })
}
#[no_mangle]
pub fn hint_thin_caps(t: ThinBox<Pair>) { drop(t) }
#[no_mangle]
pub fn hint_thin_plain(t: ThinBox<Plain>) { drop(t) }
#[no_mangle]
pub fn hint_inplace_caps(v: Vec<Src2>) -> Vec<Dst2> {
    v.into_iter().map(|e| Dst2([e.a, e.b, e.c, 0])).collect()
}
#[no_mangle]
pub fn hint_inplace_rewrite(v: Vec<Src4>) -> Vec<Dst4> {
    v.into_iter().map(|e| Dst4 { a: e.a + e.b + e.c + e.d, q: e.p }).collect()
}
#[no_mangle]
pub fn hint_inplace_plain(v: Vec<PSrc>) -> Vec<PDst> {
    v.into_iter().map(|e| PDst([e.a, e.b])).collect()
}

// ThinBox<Pair>: guard drop; at O0 the guard fields are loaded, at O2 the stored
// constants fold.
// CHECK-LABEL: define{{.*}}10WithHeaderuE4dropNtCs{{.*}}4PairE
// o0: "mem.wipe_cap_words"(ptr addrspace(200) %{{.*}}, i64 %{{.*}})
// o0-NEXT: ; call core::ptr::drop_glue::<<alloc::boxed::thin::WithHeader<_>>::drop::DropGuard<()>>
// mir: "mem.wipe_cap_words"(ptr addrspace(200) %{{.*}}, i64 32)
// mir-NEXT: ; call core::ptr::drop_glue::<<alloc::boxed::thin::WithHeader<_>>::drop::DropGuard<()>>
// off-NOT: "mem.wipe_cap_words"
// offall-NOT: "mem.wipe_cap_words"
// CHECK: ret void

// ThinBox<Plain>: no caps, nothing.
// CHECK-LABEL: define{{.*}}10WithHeaderuE4dropNtCs{{.*}}5PlainE
// CHECK-NOT: "mem.wipe_cap_words"
// CHECK: ret void

// Box::map::<Pair -> u64>: old block before deallocate.
// CHECK-LABEL: define{{.*}}4PairE3map
// o0: "mem.wipe_cap_words"(ptr addrspace(200) %_8, i64 32)
// o0-NEXT: ; call <alloc::alloc::Global as core::alloc::Allocator>::deallocate
// mir: "mem.wipe_cap_words"(ptr addrspace(200) %_8, i64 32)
// mir-NEXT: ; call <alloc::alloc::Global as core::alloc::Allocator>::deallocate
// off-NOT: "mem.wipe_cap_words"
// offall-NOT: "mem.wipe_cap_words"

// Box::try_map::<Pair -> Same>: same size, error path drops Box<MaybeUninit<Same>>.
// CHECK-LABEL: define{{.*}}4PairE7try_map
// At O0 the size-changing branch is still there, with its deallocate fact.
// o0: "mem.wipe_cap_words"(ptr addrspace(200) %_8, i64 32)
// o0-NEXT: ; call <alloc::alloc::Global as core::alloc::Allocator>::deallocate
// o0: "mem.wipe_cap_words"(ptr addrspace(200) %{{.*}}, i64 32)
// o0-NEXT: ; call core::ptr::drop_glue::<alloc::boxed::Box<core::mem::maybe_uninit::MaybeUninit<{{.*}}Same>>>
// mir: "mem.wipe_cap_words"(ptr addrspace(200) %{{.*}}, i64 32)
// mir-NEXT: ; call core::ptr::drop_glue::<alloc::boxed::Box<core::mem::maybe_uninit::MaybeUninit<{{.*}}Same>>>
// off-NOT: "mem.wipe_cap_words"
// offall-NOT: "mem.wipe_cap_words"

// Box::map::<Plain -> u32>: nothing.
// CHECK-LABEL: define{{.*}}5PlainE3map
// CHECK-NOT: "mem.wipe_cap_words"

// from_iter_in_place::<_, PDst> (PSrc holds no caps): nothing.
// CHECK-LABEL: define{{.*}}18from_iter_in_place{{.*}}4PSrc
// CHECK-NOT: "mem.wipe_cap_words"

// from_iter_in_place::<_, Dst2> (Src2 holds a cap, Dst2 none): the bytes past
// the live elements, `[len * 32, old)`, before the shrink (`live` spec). No rewrite
// wipe (Dst2 holds no caps; its arm is unreachable and carries no fact).
// CHECK-LABEL: define{{.*}}18from_iter_in_place{{.*}}4Src2
// o0: [[LIVE:%.*]] = mul i64 %len, 32
// o0-NEXT: [[AT:%.*]] = getelementptr inbounds i8, ptr addrspace(200) %{{.*}}, i64 [[LIVE]]
// o0-NEXT: [[N:%.*]] = sub i64 %src_size, [[LIVE]]
// o0-NEXT: "mem.wipe_cap_words"(ptr addrspace(200) [[AT]], i64 [[N]])
// o0-NOT: "mem.wipe_cap_words"
// o0: ; call <alloc::alloc::Global>::shrink_impl_runtime
// mir: [[LIVE:%.*]] = shl i64 %len, 5
// mir-NEXT: [[AT:%.*]] = getelementptr inbounds i8, ptr addrspace(200) %self8, i64 [[LIVE]]
// mir-NEXT: [[N:%.*]] = sub i64 %src_size, [[LIVE]]
// mir-NEXT: "mem.wipe_cap_words"(ptr addrspace(200) [[AT]], i64 [[N]])
// mir-NOT: "mem.wipe_cap_words"
// mir: ; call <alloc::alloc::Global>::shrink_impl_runtime
// rwoff: "mem.wipe_cap_words"
// off-NOT: "mem.wipe_cap_words"
// offall-NOT: "mem.wipe_cap_words"
// CHECK: shrink_impl_runtime

// from_iter_in_place::<_, Dst4> (Dst4 holds a cap): the dead bytes as above, then
// the shrink rewrite: allocate, copy the kept bytes (`dst_size`), wipe the whole old
// block (`src_size`), free. `-Zcheri-spec-relocate-rewrite=no` keeps the copy but
// drops the wipe.
// CHECK-LABEL: define{{.*}}18from_iter_in_place{{.*}}4Src4
// o0: "mem.wipe_cap_words"(ptr addrspace(200) %{{.*}}, i64 %{{.*}})
// o0: ; call <alloc::alloc::Global>::alloc_impl_runtime
// o0: call {{.*}}@llvm.memcpy.p200.p200.i64(ptr addrspace(200) align 1 %{{.*}}, ptr addrspace(200) align 1 %self8, i64 %dst_size, i1 false)
// o0-NEXT: "mem.wipe_cap_words"(ptr addrspace(200) %self8, i64 %src_size)
// mir: "mem.wipe_cap_words"(ptr addrspace(200) %{{.*}}, i64 %{{.*}})
// mir: ; call <alloc::alloc::Global>::alloc_impl_runtime
// mir: call {{.*}}@llvm.memcpy.p200.p200.i64(ptr addrspace(200) nonnull align 1 %{{.*}}, ptr addrspace(200) nonnull align 1 %self8, i64 %dst_size, i1 false)
// mir-NEXT: "mem.wipe_cap_words"(ptr addrspace(200) %self8, i64 %src_size)
// mir: ; call __rustc::__rust_dealloc
// rwoff: call {{.*}}@llvm.memcpy.p200.p200.i64(ptr addrspace(200) align 1 %{{.*}}, ptr addrspace(200) align 1 %self8, i64 %dst_size, i1 false)
// rwoff-NOT: "mem.wipe_cap_words"
// rwoff: ; call __rustc::__rust_dealloc
// off-NOT: "mem.wipe_cap_words"
// offall-NOT: "mem.wipe_cap_words"
// CHECK: ret void
