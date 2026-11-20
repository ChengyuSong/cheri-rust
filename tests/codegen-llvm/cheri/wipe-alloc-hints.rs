// M7.2d wipe hints [LLM-generated]: three library frees that no container
// drop covers get `mem.wipe_cap_words` facts without any library code change.
// The library functions only carry `#[rustc_diagnostic_item]` markers
// (`cheri_wipe_box_map`, `cheri_wipe_box_try_map`, `cheri_wipe_thin_box_drop`,
// `cheri_wipe_in_place_collect` and the `InPlaceCollect` trait); rustc finds
// the marked function on the call's MIR inline scope chain and takes the old
// element type from its generic arguments.
// * `Box::map`: before the old block's `deallocate` (size changed).
// * `Box::try_map` error path: before the drop of the reused
//   `Box<MaybeUninit<U>>` (same size as `T`; `U` holds no caps here).
// * `ThinBox<T>` drop: before the drop of the `WithHeader::drop` guard
//   (value bytes, read from the guard's `ptr` / `value_layout.size`).
// * in-place `collect`: before `Global::shrink_impl_runtime`, the bytes the
//   new layout drops, `[old - old % size_of::<T>(), old)`.
// Only when the old element type may hold capabilities (`Plain` / `PSrc` get
// nothing). `-Zcheri-wipe-alloc-hints=no` (or `-Zcheri-wipe-cap-words=no`)
// emits none of these; the RawVec<Src2> drop fact in the M7.2a container
// hook stays (and goes with `-Zcheri-wipe-cap-words=no`).
//
//@ revisions: o0 mir off offall
//@ compile-flags: -Cdebuginfo=0
//@[o0] compile-flags: -Copt-level=0
//@[mir] compile-flags: -Copt-level=2 -Cno-prepopulate-passes
//@[off] compile-flags: -Copt-level=0 -Zcheri-wipe-alloc-hints=no
//@[offall] compile-flags: -Copt-level=0 -Zcheri-wipe-cap-words=no
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

// from_iter_in_place::<_, Dst2> (Src2 holds a cap): dropped tail before shrink.
// CHECK-LABEL: define{{.*}}18from_iter_in_place{{.*}}4Src2
// o0: [[REM:%.*]] = urem i64 [[OLD:%.*]], 32
// o0-NEXT: [[NEW:%.*]] = sub i64 [[OLD]], [[REM]]
// o0-NEXT: [[AT:%.*]] = getelementptr inbounds i8, ptr addrspace(200) %{{.*}}, i64 [[NEW]]
// o0-NEXT: "mem.wipe_cap_words"(ptr addrspace(200) [[AT]], i64 [[REM]])
// o0-NEXT: ; call <alloc::alloc::Global>::shrink_impl_runtime
// mir: "mem.wipe_cap_words"(ptr addrspace(200) %{{.*}}, i64 %{{.*}})
// mir-NEXT: ; call <alloc::alloc::Global>::shrink_impl_runtime
// off-NOT: "mem.wipe_cap_words"
// offall-NOT: "mem.wipe_cap_words"
// CHECK: shrink_impl_runtime
