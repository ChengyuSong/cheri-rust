//! Memory allocation APIs

#![stable(feature = "alloc_module", since = "1.28.0")]

#[stable(feature = "alloc_module", since = "1.28.0")]
#[doc(inline)]
pub use core::alloc::*;
use core::mem::Alignment;
use core::ptr::{self, NonNull};
use core::{cmp, hint};
#[cfg(all(target_family = "cheri", target_abi = "purecap", not(cheri_no_heap_bounds)))]
use core::fmt;

unsafe extern "Rust" {
    // These are the magic symbols to call the global allocator. rustc generates
    // them to call the global allocator if there is a `#[global_allocator]` attribute
    // (the code expanding that attribute macro generates those functions), or to call
    // the default implementations in std (`__rdl_alloc` etc. in `library/std/src/alloc.rs`)
    // otherwise.
    #[rustc_allocator]
    #[rustc_nounwind]
    #[rustc_std_internal_symbol]
    #[rustc_allocator_zeroed_variant = "__rust_alloc_zeroed"]
    fn __rust_alloc(size: usize, align: Alignment) -> *mut u8;
    #[rustc_deallocator]
    #[rustc_nounwind]
    #[rustc_std_internal_symbol]
    fn __rust_dealloc(ptr: NonNull<u8>, size: usize, align: Alignment);
    #[rustc_reallocator]
    #[rustc_nounwind]
    #[rustc_std_internal_symbol]
    fn __rust_realloc(
        ptr: NonNull<u8>,
        old_size: usize,
        align: Alignment,
        new_size: usize,
    ) -> *mut u8;
    #[rustc_allocator_zeroed]
    #[rustc_nounwind]
    #[rustc_std_internal_symbol]
    fn __rust_alloc_zeroed(size: usize, align: Alignment) -> *mut u8;

    #[rustc_nounwind]
    #[rustc_std_internal_symbol]
    fn __rust_no_alloc_shim_is_unstable_v2();
}

/// The global memory allocator.
///
/// This type implements the [`Allocator`] trait by forwarding calls
/// to the allocator registered with the `#[global_allocator]` attribute
/// if there is one, or the `std` crate’s default.
///
/// Similar to [`alloc`], [`dealloc`], and the other global allocation functions,
/// the way in which calls are forwarded to the allocator registered with
/// `#[global_allocator]` is unspecified. See their safety docs for more information.
///
/// This allocator must be treated like an opaque allocator that only guarantees
/// the contract described in the docs of [`Allocator`], as well as the following
/// things:
/// * All instances of `Global` are [*equivalent*].
/// * Allocations from `Global` are only invalidated by calls to de-/reallocating functions.
///   If no such call is made, then the allocation will live for the rest of the program.
/// * The global allocation functions are equivalent to the methods on `Global`, except
///   that they disallow zero-sized allocations, and implicitly ignore any returned
///   excess size.
///
/// Note that the current implementation of `Global` does not take advantage of
/// some features of [`Allocator`], such as zero-sized allocations (which currently always
/// return a dangling pointer) and overallocating.
///
/// This may change in the future. You must not rely on it for correctness!
///
/// [*equivalent*]: Allocator#equivalent-allocators
#[stable(feature = "allocator_api", since = "CURRENT_RUSTC_VERSION")]
#[derive(Copy, Debug)]
#[derive_const(Clone, Default)]
// the compiler needs to know when a Box uses the global allocator vs a custom one
#[lang = "global_alloc_ty"]
pub struct Global;

#[unstable(feature = "allocator_ext", issue = "163177", implied_by = "allocator_api")]
unsafe impl core::alloc::AllocatorClone for Global {}

#[unstable(feature = "allocator_ext", issue = "163177", implied_by = "allocator_api")]
unsafe impl core::alloc::StaticAllocator for Global {}

/// Allocates memory with the global allocator.
///
/// This function forwards calls to the [`GlobalAlloc::alloc`] method
/// of the allocator registered with the `#[global_allocator]` attribute
/// if there is one, or the `std` crate’s default.
///
/// Note, however, that invoking this function is *not* equivalent to invoking the underlying
/// [`GlobalAlloc::alloc`] method of the registered allocator directly. Users of this function
/// cannot assume anything about what the allocator does, other than the documented requirements.
/// This means:
///
/// - This function may non-deterministically entirely skip the underlying allocator, e.g. if the
///   compiler can show that this allocation can be replaced by a stack variable. The compiler may
///   also merge multiple allocation operations into one, as long as it can also adjust all
///   corresponding deallocation operations accordingly.
/// - An allocation created by invoking this function has exactly the size and minimum alignment
///   defined by `layout`, even if the underlying allocator makes stronger promises.
/// - The allocation can only be freed by invoking [`dealloc`] or [`realloc`]. In particular,
///   passing a pointer to such an allocation directly to the underlying method on [`GlobalAlloc`] is
///   not permitted. Until one of those functions is called, it is undefined behavior to access the
///   memory that backs this allocation with any pointer not derived from the return value of this
///   function (e.g., with internal pointers the allocator might keep around).
/// - This function de-initializes the contents of the allocation before handing it to the user. So even
///   if you control the underlying allocator and know that it explicitly initialized this memory,
///   you cannot rely on it being initialized.
///
/// Users of this function have to consider that in the future, allocators may be allowed to unwind.
///
/// This function is expected to be deprecated in favor of the `allocate` method
/// of the [`Global`] type when it and the [`Allocator`] trait become stable.
///
/// # Safety
///
/// See [`GlobalAlloc::alloc`].
///
/// # Examples
///
/// ```
/// use std::alloc::{alloc, dealloc, handle_alloc_error, Layout};
///
/// unsafe {
///     let layout = Layout::new::<u16>();
///     let ptr = alloc(layout);
///     if ptr.is_null() {
///         handle_alloc_error(layout);
///     }
///
///     *(ptr as *mut u16) = 42;
///     assert_eq!(*(ptr as *mut u16), 42);
///
///     dealloc(ptr, layout);
/// }
/// ```
#[stable(feature = "global_alloc", since = "1.28.0")]
#[must_use = "losing the pointer will leak memory"]
#[inline]
#[cfg_attr(miri, track_caller)] // even without panics, this helps for Miri backtraces
pub unsafe fn alloc(layout: Layout) -> *mut u8 {
    // SAFETY: Upheld by caller.
    unsafe {
        // Make sure we don't accidentally allow omitting the allocator shim in
        // stable code until it is actually stabilized.
        __rust_no_alloc_shim_is_unstable_v2();

        #[cfg(all(target_family = "cheri", target_abi = "purecap", not(cheri_no_heap_bounds)))]
        let p = {
            let (len, align) = cheri_heap::padded(layout.size(), layout.alignment());
            if len < layout.size() {
                ptr::null_mut()
            } else {
                cheri_heap::bound(__rust_alloc(len, align), len)
            }
        };
        #[cfg(not(all(target_family = "cheri", target_abi = "purecap", not(cheri_no_heap_bounds))))]
        let p = __rust_alloc(layout.size(), layout.alignment());
        p
    }
}

/// Deallocates memory with the global allocator.
///
/// This function forwards calls to the [`GlobalAlloc::dealloc`] method
/// of the allocator registered with the `#[global_allocator]` attribute
/// if there is one, or the `std` crate’s default.
///
/// Note, however, that invoking this function is *not* equivalent to invoking the underlying
/// [`GlobalAlloc::dealloc`] method of the registered allocator directly. Users of this function
/// cannot assume anything about what the allocator does, other than the documented requirements.
/// This means:
///
/// - This function may non-deterministically entirely skip the underlying allocator, e.g. if the
///   compiler can show that this allocation can be replaced by a stack variable. The compiler may
///   also merge multiple allocation operations into one, as long as it can also adjust all
///   corresponding deallocation operations accordingly.
/// - The pointer passed to this function must have been obtained by invoking [`alloc`],
///   [`alloc_zeroed`], or [`realloc`]. In particular, passing a pointer returned by the underlying
///   methods on [`GlobalAlloc`] is not permitted.
/// - This function de-initializes the contents of the allocation before handing it to the allocator.
///   So even if you know that the program previously initialized that memory, the allocator cannot
///   rely on it being initialized.
///
/// Users of this function have to consider that in the future, allocators may be allowed to unwind.
///
/// This function is expected to be deprecated in favor of the `deallocate` method
/// of the [`Global`] type when it and the [`Allocator`] trait become stable.
///
/// # Safety
///
/// See [`GlobalAlloc::dealloc`].
#[stable(feature = "global_alloc", since = "1.28.0")]
#[inline]
#[cfg_attr(miri, track_caller)] // even without panics, this helps for Miri backtraces
pub unsafe fn dealloc(ptr: *mut u8, layout: Layout) {
    // SAFETY: Upheld by caller.
    unsafe { dealloc_nonnull(NonNull::new_unchecked(ptr), layout) }
}

/// Same as [`dealloc`] but when you already have a non-null pointer
#[inline]
#[cfg_attr(miri, track_caller)] // even without panics, this helps for Miri backtraces
unsafe fn dealloc_nonnull(ptr: NonNull<u8>, layout: Layout) {
    // On CHERI purecap, `alloc` asked the global allocator for the padded layout.
    #[cfg(all(target_family = "cheri", target_abi = "purecap", not(cheri_no_heap_bounds)))]
    let (size, align) = cheri_heap::padded(layout.size(), layout.alignment());
    #[cfg(not(all(target_family = "cheri", target_abi = "purecap", not(cheri_no_heap_bounds))))]
    let (size, align) = (layout.size(), layout.alignment());
    // SAFETY: Upheld by caller.
    unsafe { __rust_dealloc(ptr, size, align) }
}

/// Reallocates memory with the global allocator.
///
/// This function forwards calls to the [`GlobalAlloc::realloc`] method
/// of the allocator registered with the `#[global_allocator]` attribute
/// if there is one, or the `std` crate’s default.
///
/// Note, however, that invoking this function is *not* equivalent to invoking the underlying
/// [`GlobalAlloc::realloc`] method of the registered allocator directly. Users of this function
/// cannot assume anything about what the allocator does, other than the documented requirements.
/// This means:
///
/// - This function may non-deterministically entirely skip the underlying allocator, e.g. if the
///   compiler can show that this allocation can be replaced by a stack variable. The compiler may
///   also merge multiple allocation operations into one, as long as it can also adjust all
///   corresponding deallocation operations accordingly.
/// - The pointer passed to this function must have been obtained by invoking [`alloc`],
///   [`alloc_zeroed`], or [`realloc`]. In particular, passing a pointer returned by the underlying
///   methods on [`GlobalAlloc`] is not permitted.
/// - An allocation created by invoking this function has exactly the size and minimum alignment
///   defined by `layout`, even if the underlying allocator makes stronger promises.
/// - The allocation can only be freed by invoking [`dealloc`] or [`realloc`]. In particular,
///   passing a pointer to such an allocation directly to the underlying method on [`GlobalAlloc`] is
///   not permitted. Until one of those functions is called, it is undefined behavior to access the
///   memory that backs this allocation with any pointer not derived from the return value of this
///   function (e.g., with internal pointers the allocator might keep around).
/// - If this grows the allocation, the contents of the grown part of the new allocation allocation
///   are de-initialized by this function before returning.
/// - If this shrinks the allocation, the contents of the removed part of the old allocation are
///   de-initialized by this function before invoking the underlying allocator.
///
/// Users of this function have to consider that in the future, allocators may be allowed to unwind.
///
/// This function is expected to be deprecated in favor of the `grow` and `shrink` methods
/// of the [`Global`] type when it and the [`Allocator`] trait become stable.
///
/// # Safety
///
/// See [`GlobalAlloc::realloc`].
#[stable(feature = "global_alloc", since = "1.28.0")]
#[must_use = "losing the pointer will leak memory"]
#[inline]
#[cfg_attr(miri, track_caller)] // even without panics, this helps for Miri backtraces
pub unsafe fn realloc(ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
    // SAFETY: Upheld by caller.
    unsafe { realloc_nonnull(NonNull::new_unchecked(ptr), layout, new_size) }
}

/// Same as [`realloc`] but when you already have a non-null pointer
#[inline]
#[cfg_attr(miri, track_caller)] // even without panics, this helps for Miri backtraces
unsafe fn realloc_nonnull(ptr: NonNull<u8>, layout: Layout, new_size: usize) -> *mut u8 {
    // SAFETY: Upheld by caller.
    // SAFETY: Upheld by caller.
    #[cfg(all(target_family = "cheri", target_abi = "purecap", not(cheri_no_heap_bounds)))]
    let p = unsafe { cheri_heap::resize(&cheri_heap::Global, ptr.as_ptr(), layout, new_size) };
    // SAFETY: Upheld by caller.
    #[cfg(not(all(target_family = "cheri", target_abi = "purecap", not(cheri_no_heap_bounds))))]
    let p = unsafe { __rust_realloc(ptr, layout.size(), layout.alignment(), new_size) };
    p
}

/// Allocates zero-initialized memory with the global allocator.
///
/// This function forwards calls to the [`GlobalAlloc::alloc_zeroed`] method
/// of the allocator registered with the `#[global_allocator]` attribute
/// if there is one, or the `std` crate’s default.
///
/// Note, however, that invoking this function is *not* equivalent to invoking the underlying
/// [`GlobalAlloc::alloc_zeroed`] method of the registered allocator directly. Users of this
/// function cannot assume anything about what the allocator does, other than the documented
/// requirements. This means:
///
/// - This function may non-deterministically entirely skip the underlying allocator, e.g. if the
///   compiler can show that this allocation can be replaced by a stack variable. The compiler may
///   also merge multiple allocation operations into one, as long as it can also adjust all
///   corresponding deallocation operations accordingly.
/// - The allocation can only be freed by invoking [`dealloc`] or [`realloc`]. In particular,
///   passing a pointer to such an allocation directly to the underlying method on [`GlobalAlloc`] is
///   not permitted. Until one of those functions is called, it is undefined behavior to access the
///   memory that backs this allocation with any pointer not derived from the return value of this
///   function (e.g., with internal pointers the allocator might keep around).
/// - An allocation created by invoking this function has exactly the size and minimum alignment
///   defined by `layout`, even if the underlying allocator makes stronger promises.
///
/// Users of this function have to consider that in the future, allocators may be allowed to unwind.
///
/// This function is expected to be deprecated in favor of the `allocate_zeroed` method
/// of the [`Global`] type when it and the [`Allocator`] trait become stable.
///
/// # Safety
///
/// See [`GlobalAlloc::alloc_zeroed`].
///
/// # Examples
///
/// ```
/// use std::alloc::{alloc_zeroed, dealloc, handle_alloc_error, Layout};
///
/// unsafe {
///     let layout = Layout::new::<u16>();
///     let ptr = alloc_zeroed(layout);
///     if ptr.is_null() {
///         handle_alloc_error(layout);
///     }
///
///     assert_eq!(*(ptr as *mut u16), 0);
///
///     dealloc(ptr, layout);
/// }
/// ```
#[stable(feature = "global_alloc", since = "1.28.0")]
#[must_use = "losing the pointer will leak memory"]
#[inline]
#[cfg_attr(miri, track_caller)] // even without panics, this helps for Miri backtraces
pub unsafe fn alloc_zeroed(layout: Layout) -> *mut u8 {
    // SAFETY: Upheld by caller.
    unsafe {
        // Make sure we don't accidentally allow omitting the allocator shim in
        // stable code until it is actually stabilized.
        __rust_no_alloc_shim_is_unstable_v2();

        #[cfg(all(target_family = "cheri", target_abi = "purecap", not(cheri_no_heap_bounds)))]
        let p = {
            let (len, align) = cheri_heap::padded(layout.size(), layout.alignment());
            if len < layout.size() {
                ptr::null_mut()
            } else {
                cheri_heap::bound(__rust_alloc_zeroed(len, align), len)
            }
        };
        #[cfg(not(all(target_family = "cheri", target_abi = "purecap", not(cheri_no_heap_bounds))))]
        let p = __rust_alloc_zeroed(layout.size(), layout.alignment());
        p
    }
}

/// Allocation-level capability bounds for the global allocator on CHERI purecap targets.
///
/// Every block handed out by [`alloc`], [`alloc_zeroed`] and [`realloc`] (and so by
/// `Box`, `Vec`, `String`, `Rc` and `Arc` through [`Global`]) is a capability whose bounds
/// are exactly the block. The request is padded and aligned so that the bounds are
/// representable: the length is rounded up with `CRRL`, and the alignment is raised to
/// what `CRAM` requires for the rounded length. The global allocator sees the padded
/// layout in `alloc`, `realloc` and `dealloc`, so the layouts it receives stay consistent.
///
/// The capability given back to `GlobalAlloc::dealloc` and `GlobalAlloc::realloc` is the
/// bounded one. An allocator that needs more than the block itself at that point (a
/// header in front of the block, or a free list whose nodes are later merged) must
/// re-derive a wider capability from a root it owns; [`CheriBoundedAlloc::with_root`]
/// does that. `realloc` keeps the block when the padded layout is unchanged, otherwise
/// asks `GlobalAlloc::realloc` to resize it and bounds the result from the widest
/// capability available; it allocates, copies and frees only when the alignment must
/// grow, or when a block grown in place comes back with only the narrowed capability.
///
/// Build the standard library with `--cfg cheri_no_heap_bounds` to turn this off.
#[cfg(all(target_family = "cheri", target_abi = "purecap", not(cheri_no_heap_bounds)))]
mod cheri_heap {
    use core::alloc::Layout;
    use core::arch::cheri;
    use core::mem::Alignment;
    use core::ptr::{self, NonNull};

    /// The padded `(length, alignment)` for a block of `size` bytes aligned to `align`.
    ///
    /// The length is 0 or less than `size` only if `size` is too large to be represented.
    #[inline]
    pub(super) fn padded(size: usize, align: Alignment) -> (usize, Alignment) {
        let len = cheri::round_representable_length(size);
        // CRAM of the rounded length is the alignment the bounds of `size` bytes need.
        // Using the rounded length also sidesteps LLVM folding CRAM of a constant `size`
        // with a mask that is one bit too weak just below powers of two.
        let req = (!cheri::representable_alignment_mask(len)).wrapping_add(1);
        match Alignment::new(req) {
            Some(r) if r > align => (len, r),
            _ => (len, align),
        }
    }

    /// Bound `p` to exactly `len` bytes. A null `p` stays null.
    ///
    /// `p` is aligned as [`padded`] asked, so the bounds are exact. If the allocator broke
    /// that contract, `CSetBoundsExact` clears the tag and the first access faults.
    #[inline]
    pub(super) unsafe fn bound(p: *mut u8, len: usize) -> *mut u8 {
        if p.is_null() {
            return p;
        }
        // SAFETY: narrowing a capability is always allowed.
        unsafe { cheri::bounds_set_exact(p, len) as *mut u8 }
    }

    /// The allocator under the bounding layer, in padded sizes.
    pub(super) trait Inner {
        unsafe fn alloc(&self, len: usize, align: Alignment) -> *mut u8;
        unsafe fn realloc(&self, p: *mut u8, len: usize, align: Alignment, new_len: usize)
        -> *mut u8;
        unsafe fn dealloc(&self, p: *mut u8, len: usize, align: Alignment);
        /// The widest capability this layer can get for the block at `p`'s address
        /// (`CheriBoundedAlloc::with_root`); `p` itself if there is none.
        fn widen(&self, p: *mut u8) -> *mut u8 {
            p
        }
    }

    /// The global allocator (`__rust_*`).
    pub(super) struct Global;

    impl Inner for Global {
        #[inline]
        unsafe fn alloc(&self, len: usize, align: Alignment) -> *mut u8 {
            // SAFETY: Upheld by caller.
            unsafe { super::__rust_alloc(len, align) }
        }
        #[inline]
        unsafe fn realloc(&self, p: *mut u8, len: usize, align: Alignment, new_len: usize) -> *mut u8 {
            // SAFETY: Upheld by caller (`p` is a live, non-null block).
            unsafe { super::__rust_realloc(NonNull::new_unchecked(p), len, align, new_len) }
        }
        #[inline]
        unsafe fn dealloc(&self, p: *mut u8, len: usize, align: Alignment) {
            // SAFETY: Upheld by caller.
            unsafe { super::__rust_dealloc(NonNull::new_unchecked(p), len, align) }
        }
    }

    /// Whether `c` is tagged and its bounds cover `[address, address + len)`.
    #[inline]
    fn covers(c: *mut u8, len: usize) -> bool {
        // Not `top_get`: QEMU-CHERI (ISAv9) has no `CGetTop`.
        let (a, b) = (cheri::address_get(c), cheri::base_get(c));
        cheri::tag_get(c) && b <= a && (a - b).checked_add(len).is_some_and(|e| e <= cheri::length_get(c))
    }

    /// `realloc` of a bounded block `p` (allocated for `layout`) to `new_size` bytes.
    ///
    /// The inner allocator always sees padded layouts, so `dealloc` stays consistent:
    /// 1. Same padded length and alignment: return `p` unchanged; its bounds already
    ///    cover the padding.
    /// 2. Same alignment, different padded length (grow past the padding, or shrink): ask
    ///    the inner allocator to resize. Bounds can only shrink, so the result is bounded
    ///    from the widest capability available (the inner allocator's result, or one
    ///    re-derived from a root). If the block stayed in place but only the narrowed
    ///    capability came back, the block is moved instead.
    /// 3. Otherwise (stricter alignment needed): allocate, copy and free.
    #[inline]
    pub(super) unsafe fn resize<I: Inner + ?Sized>(
        inner: &I,
        p: *mut u8,
        layout: Layout,
        new_size: usize,
    ) -> *mut u8 {
        let (old_len, old_align) = padded(layout.size(), layout.alignment());
        let (new_len, new_align) = padded(new_size, layout.alignment());
        if new_len < new_size {
            return ptr::null_mut();
        }
        if new_len == old_len && new_align == old_align {
            return p;
        }
        let keep = cmp_min(layout.size(), new_size);
        // SAFETY: Upheld by caller; every layout passed to `inner` is the padded one it
        // allocated (or, for a resized block, the one it resized to).
        unsafe {
            if new_align == old_align {
                let r = inner.realloc(inner.widen(p), old_len, old_align, new_len);
                if r.is_null() {
                    return r;
                }
                let w = inner.widen(r);
                if covers(w, new_len) {
                    return bound(w, new_len);
                }
                // Resized in place, but no capability covers the new length. `r` still
                // covers the old block (`p` is dead after `realloc`), so copy out of it.
                let n = inner.alloc(new_len, new_align);
                if n.is_null() {
                    // Best effort: give the inner block its old size back.
                    let _ = inner.realloc(w, new_len, old_align, old_len);
                    return ptr::null_mut();
                }
                ptr::copy_nonoverlapping(r, n, keep);
                inner.dealloc(w, new_len, old_align);
                return bound(n, new_len);
            }
            let n = inner.alloc(new_len, new_align);
            if n.is_null() {
                return n;
            }
            ptr::copy_nonoverlapping(p, n, keep);
            inner.dealloc(inner.widen(p), old_len, old_align);
            bound(n, new_len)
        }
    }

    #[inline]
    fn cmp_min(a: usize, b: usize) -> usize {
        if a < b { a } else { b }
    }
}

/// A [`GlobalAlloc`] adapter for CHERI purecap targets that bounds every block to an
/// exactly representable allocation and, given a root, re-derives the allocator's own
/// capability before handing a block back.
///
/// ```ignore (cheri-purecap-only)
/// static mut HEAP: [u8; 1 << 20] = [0; 1 << 20];
/// #[global_allocator]
/// static A: CheriBoundedAlloc<MyAlloc> =
///     unsafe { CheriBoundedAlloc::with_root(MyAlloc::new(), (&raw mut HEAP).cast()) };
/// ```
///
/// `new` passes the bounded capability back to the inner allocator in `dealloc` and
/// `realloc`; use it for allocators that only touch the block itself (for example, a bump
/// allocator). `with_root` re-derives a capability from `root` at the block's address,
/// so the inner allocator gets back the bounds it handed out (for example, for a free-list
/// allocator that writes headers or merges neighbouring blocks).
#[cfg(all(target_family = "cheri", target_abi = "purecap", not(cheri_no_heap_bounds)))]
#[unstable(feature = "cheri_bounded_alloc", issue = "none")]
pub struct CheriBoundedAlloc<A> {
    inner: A,
    root: *mut u8,
}

#[cfg(all(target_family = "cheri", target_abi = "purecap", not(cheri_no_heap_bounds)))]
#[unstable(feature = "cheri_bounded_alloc", issue = "none")]
impl<A: fmt::Debug> fmt::Debug for CheriBoundedAlloc<A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CheriBoundedAlloc").field("inner", &self.inner).field("root", &self.root).finish()
    }
}

#[cfg(all(target_family = "cheri", target_abi = "purecap", not(cheri_no_heap_bounds)))]
#[unstable(feature = "cheri_bounded_alloc", issue = "none")]
// SAFETY: `root` is only used to re-derive pointers for `inner`, which is itself `Sync`.
unsafe impl<A: Sync> Sync for CheriBoundedAlloc<A> {}

#[cfg(all(target_family = "cheri", target_abi = "purecap", not(cheri_no_heap_bounds)))]
impl<A> CheriBoundedAlloc<A> {
    /// Wrap `inner`. Bounded capabilities are passed back to it in `dealloc`/`realloc`.
    #[unstable(feature = "cheri_bounded_alloc", issue = "none")]
    pub const fn new(inner: A) -> Self {
        CheriBoundedAlloc { inner, root: ptr::null_mut() }
    }

    /// Wrap `inner`, re-deriving capabilities from `root` before passing them back.
    ///
    /// # Safety
    ///
    /// `root` must be a capability whose bounds and permissions cover every block that
    /// `inner` hands out.
    #[unstable(feature = "cheri_bounded_alloc", issue = "none")]
    pub const unsafe fn with_root(inner: A, root: *mut u8) -> Self {
        CheriBoundedAlloc { inner, root }
    }

    #[inline]
    fn rederive(&self, p: *mut u8) -> *mut u8 {
        if self.root.is_null() {
            p
        } else {
            // SAFETY: `root` covers the block (see `with_root`).
            unsafe {
                core::arch::cheri::address_set(self.root, core::arch::cheri::address_get(p))
                    as *mut u8
            }
        }
    }
}

#[cfg(all(target_family = "cheri", target_abi = "purecap", not(cheri_no_heap_bounds)))]
#[unstable(feature = "cheri_bounded_alloc", issue = "none")]
unsafe impl<A: GlobalAlloc> GlobalAlloc for CheriBoundedAlloc<A> {
    #[inline]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let (len, align) = cheri_heap::padded(layout.size(), layout.alignment());
        if len < layout.size() {
            return ptr::null_mut();
        }
        // SAFETY: `len` is non-zero and `align` a power of two not below `layout.align()`.
        unsafe {
            let l = Layout::from_size_alignment_unchecked(len, align);
            cheri_heap::bound(self.inner.alloc(l), len)
        }
    }

    #[inline]
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let (len, align) = cheri_heap::padded(layout.size(), layout.alignment());
        if len < layout.size() {
            return ptr::null_mut();
        }
        // SAFETY: as in `alloc`.
        unsafe {
            let l = Layout::from_size_alignment_unchecked(len, align);
            cheri_heap::bound(self.inner.alloc_zeroed(l), len)
        }
    }

    #[inline]
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        let (len, align) = cheri_heap::padded(layout.size(), layout.alignment());
        // SAFETY: `alloc` asked `inner` for exactly this layout.
        unsafe {
            self.inner.dealloc(self.rederive(p), Layout::from_size_alignment_unchecked(len, align))
        }
    }

    #[inline]
    unsafe fn realloc(&self, p: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: Upheld by caller.
        unsafe { cheri_heap::resize(self, p, layout, new_size) }
    }
}

#[cfg(all(target_family = "cheri", target_abi = "purecap", not(cheri_no_heap_bounds)))]
impl<A: GlobalAlloc> cheri_heap::Inner for CheriBoundedAlloc<A> {
    #[inline]
    unsafe fn alloc(&self, len: usize, align: Alignment) -> *mut u8 {
        // SAFETY: Upheld by caller (`len` non-zero).
        unsafe { self.inner.alloc(Layout::from_size_alignment_unchecked(len, align)) }
    }
    #[inline]
    unsafe fn realloc(&self, p: *mut u8, len: usize, align: Alignment, new_len: usize) -> *mut u8 {
        // SAFETY: Upheld by caller.
        unsafe { self.inner.realloc(p, Layout::from_size_alignment_unchecked(len, align), new_len) }
    }
    #[inline]
    unsafe fn dealloc(&self, p: *mut u8, len: usize, align: Alignment) {
        // SAFETY: Upheld by caller.
        unsafe { self.inner.dealloc(p, Layout::from_size_alignment_unchecked(len, align)) }
    }
    #[inline]
    fn widen(&self, p: *mut u8) -> *mut u8 {
        self.rederive(p)
    }
}

impl Global {
    #[inline]
    #[cfg_attr(miri, track_caller)] // even without panics, this helps for Miri backtraces
    fn alloc_impl_runtime(layout: Layout, zeroed: bool) -> Result<NonNull<[u8]>, AllocError> {
        match layout.size() {
            0 => Ok(layout.dangling_ptr().cast_slice(0)),
            // SAFETY: `layout` is non-zero in size,
            size => unsafe {
                let raw_ptr = if zeroed { alloc_zeroed(layout) } else { alloc(layout) };
                let ptr = NonNull::new(raw_ptr).ok_or(AllocError)?;
                Ok(ptr.cast_slice(size))
            },
        }
    }

    #[inline]
    #[cfg_attr(miri, track_caller)] // even without panics, this helps for Miri backtraces
    fn deallocate_impl_runtime(ptr: NonNull<u8>, layout: Layout) {
        if layout.size() != 0 {
            // SAFETY:
            // * We have checked that `layout` is non-zero in size.
            // * The caller is obligated to provide a layout that "fits", and in this case,
            //   "fit" always means a layout that is equal to the original, because our
            //   `allocate()`, `grow()`, and `shrink()` implementations never returns a larger
            //   allocation than requested.
            // * Other conditions must be upheld by the caller, as per `Allocator::deallocate()`'s
            //   safety documentation.
            unsafe { dealloc_nonnull(ptr, layout) }
        }
    }

    // SAFETY: Same as `Allocator::grow`
    #[inline]
    #[cfg_attr(miri, track_caller)] // even without panics, this helps for Miri backtraces
    fn grow_impl_runtime(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
        zeroed: bool,
    ) -> Result<NonNull<[u8]>, AllocError> {
        debug_assert!(
            new_layout.size() >= old_layout.size(),
            "`new_layout.size()` must be greater than or equal to `old_layout.size()`"
        );

        match old_layout.size() {
            0 => self.alloc_impl(new_layout, zeroed),

            // SAFETY: `new_size` is non-zero as `old_size` is greater than or equal to `new_size`
            // as required by safety conditions. Other conditions must be upheld by the caller
            old_size if old_layout.align() == new_layout.align() => unsafe {
                let new_size = new_layout.size();

                // `realloc` probably checks for `new_size >= old_layout.size()` or something similar.
                hint::assert_unchecked(new_size >= old_layout.size());

                let raw_ptr = realloc_nonnull(ptr, old_layout, new_size);
                let ptr = NonNull::new(raw_ptr).ok_or(AllocError)?;
                if zeroed {
                    raw_ptr.add(old_size).write_bytes(0, new_size - old_size);
                }
                Ok(ptr.cast_slice(new_size))
            },

            // SAFETY: because `new_layout.size()` must be greater than or equal to `old_size`,
            // both the old and new memory allocation are valid for reads and writes for `old_size`
            // bytes. Also, because the old allocation wasn't yet deallocated, it cannot overlap
            // `new_ptr`. Thus, the call to `copy_nonoverlapping` is safe. The safety contract
            // for `dealloc` must be upheld by the caller.
            old_size => unsafe {
                let new_ptr = self.alloc_impl(new_layout, zeroed)?;
                ptr::copy_nonoverlapping(ptr.as_ptr(), new_ptr.as_mut_ptr(), old_size);
                self.deallocate(ptr, old_layout);
                Ok(new_ptr)
            },
        }
    }

    // SAFETY: Same as `Allocator::grow`
    #[inline]
    #[cfg_attr(miri, track_caller)] // even without panics, this helps for Miri backtraces
    fn shrink_impl_runtime(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
        _zeroed: bool,
    ) -> Result<NonNull<[u8]>, AllocError> {
        debug_assert!(
            new_layout.size() <= old_layout.size(),
            "`new_layout.size()` must be smaller than or equal to `old_layout.size()`"
        );

        match new_layout.size() {
            // SAFETY: conditions must be upheld by the caller
            0 => unsafe {
                self.deallocate(ptr, old_layout);
                Ok(new_layout.dangling_ptr().cast_slice(0))
            },

            // SAFETY: `new_size` is non-zero. Other conditions must be upheld by the caller
            new_size if old_layout.align() == new_layout.align() => unsafe {
                // `realloc` probably checks for `new_size <= old_layout.size()` or something similar.
                hint::assert_unchecked(new_size <= old_layout.size());

                let raw_ptr = realloc_nonnull(ptr, old_layout, new_size);
                let ptr = NonNull::new(raw_ptr).ok_or(AllocError)?;
                Ok(ptr.cast_slice(new_size))
            },

            // SAFETY: because `new_size` must be smaller than or equal to `old_layout.size()`,
            // both the old and new memory allocation are valid for reads and writes for `new_size`
            // bytes. Also, because the old allocation wasn't yet deallocated, it cannot overlap
            // `new_ptr`. Thus, the call to `copy_nonoverlapping` is safe. The safety contract
            // for `dealloc` must be upheld by the caller.
            new_size => unsafe {
                let new_ptr = self.allocate(new_layout)?;
                ptr::copy_nonoverlapping(ptr.as_ptr(), new_ptr.as_mut_ptr(), new_size);
                self.deallocate(ptr, old_layout);
                Ok(new_ptr)
            },
        }
    }

    // SAFETY: Same as `Allocator::allocate`
    #[inline]
    #[cfg_attr(miri, track_caller)] // even without panics, this helps for Miri backtraces
    #[rustc_const_unstable(feature = "const_heap", issue = "79597")]
    const fn alloc_impl(&self, layout: Layout, zeroed: bool) -> Result<NonNull<[u8]>, AllocError> {
        core::intrinsics::const_eval_select(
            (layout, zeroed),
            Global::alloc_impl_const,
            Global::alloc_impl_runtime,
        )
    }

    // SAFETY: Same as `Allocator::deallocate`
    #[inline]
    #[cfg_attr(miri, track_caller)] // even without panics, this helps for Miri backtraces
    #[rustc_const_unstable(feature = "const_heap", issue = "79597")]
    const unsafe fn deallocate_impl(&self, ptr: NonNull<u8>, layout: Layout) {
        core::intrinsics::const_eval_select(
            (ptr, layout),
            Global::deallocate_impl_const,
            Global::deallocate_impl_runtime,
        )
    }

    // SAFETY: Same as `Allocator::grow`
    #[inline]
    #[cfg_attr(miri, track_caller)] // even without panics, this helps for Miri backtraces
    #[rustc_const_unstable(feature = "const_heap", issue = "79597")]
    const unsafe fn grow_impl(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
        zeroed: bool,
    ) -> Result<NonNull<[u8]>, AllocError> {
        core::intrinsics::const_eval_select(
            (self, ptr, old_layout, new_layout, zeroed),
            Global::grow_shrink_impl_const,
            Global::grow_impl_runtime,
        )
    }

    // SAFETY: Same as `Allocator::shrink`
    #[inline]
    #[cfg_attr(miri, track_caller)] // even without panics, this helps for Miri backtraces
    #[rustc_const_unstable(feature = "const_heap", issue = "79597")]
    const unsafe fn shrink_impl(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        core::intrinsics::const_eval_select(
            (self, ptr, old_layout, new_layout, false),
            Global::grow_shrink_impl_const,
            Global::shrink_impl_runtime,
        )
    }

    #[inline]
    #[cfg_attr(miri, track_caller)] // even without panics, this helps for Miri backtraces
    #[rustc_const_unstable(feature = "const_heap", issue = "79597")]
    const fn alloc_impl_const(layout: Layout, zeroed: bool) -> Result<NonNull<[u8]>, AllocError> {
        match layout.size() {
            0 => Ok(layout.dangling_ptr().cast_slice(0)),
            // SAFETY: `layout` is non-zero in size,
            size => unsafe {
                let raw_ptr = core::intrinsics::const_allocate(layout.size(), layout.align());
                let ptr = NonNull::new(raw_ptr).ok_or(AllocError)?;
                if zeroed {
                    // SAFETY: the pointer returned by `const_allocate` is valid to write to.
                    ptr.write_bytes(0, size);
                }
                Ok(ptr.cast_slice(size))
            },
        }
    }

    #[inline]
    #[cfg_attr(miri, track_caller)] // even without panics, this helps for Miri backtraces
    #[rustc_const_unstable(feature = "const_heap", issue = "79597")]
    const fn deallocate_impl_const(ptr: NonNull<u8>, layout: Layout) {
        if layout.size() != 0 {
            // SAFETY: We checked for nonzero size; other preconditions must be upheld by caller.
            unsafe {
                core::intrinsics::const_deallocate(ptr.as_ptr(), layout.size(), layout.align());
            }
        }
    }

    #[inline]
    #[cfg_attr(miri, track_caller)] // even without panics, this helps for Miri backtraces
    #[rustc_const_unstable(feature = "const_heap", issue = "79597")]
    const fn grow_shrink_impl_const(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
        zeroed: bool,
    ) -> Result<NonNull<[u8]>, AllocError> {
        let new_ptr = self.alloc_impl(new_layout, zeroed)?;
        // SAFETY: both pointers are valid and this operations is in bounds.
        unsafe {
            ptr::copy_nonoverlapping(
                ptr.as_ptr(),
                new_ptr.as_mut_ptr(),
                cmp::min(old_layout.size(), new_layout.size()),
            );
        }
        // SAFETY: Caller ensures the ptr & layout are correct.
        unsafe {
            self.deallocate_impl(ptr, old_layout);
        }
        Ok(new_ptr)
    }
}

#[stable(feature = "allocator_api", since = "CURRENT_RUSTC_VERSION")]
#[rustc_const_unstable(feature = "const_heap", issue = "79597")]
const unsafe impl Allocator for Global {
    #[inline]
    #[cfg_attr(miri, track_caller)] // even without panics, this helps for Miri backtraces
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        self.alloc_impl(layout, false)
    }

    #[inline]
    #[cfg_attr(miri, track_caller)] // even without panics, this helps for Miri backtraces
    fn allocate_zeroed(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        self.alloc_impl(layout, true)
    }

    #[inline]
    #[cfg_attr(miri, track_caller)] // even without panics, this helps for Miri backtraces
    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        // SAFETY: all conditions must be upheld by the caller
        unsafe { self.deallocate_impl(ptr, layout) }
    }

    #[inline]
    #[cfg_attr(miri, track_caller)] // even without panics, this helps for Miri backtraces
    unsafe fn grow(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        // SAFETY: all conditions must be upheld by the caller
        unsafe { self.grow_impl(ptr, old_layout, new_layout, false) }
    }

    #[inline]
    #[cfg_attr(miri, track_caller)] // even without panics, this helps for Miri backtraces
    unsafe fn grow_zeroed(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        // SAFETY: all conditions must be upheld by the caller
        unsafe { self.grow_impl(ptr, old_layout, new_layout, true) }
    }

    #[inline]
    #[cfg_attr(miri, track_caller)] // even without panics, this helps for Miri backtraces
    unsafe fn shrink(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        // SAFETY: all conditions must be upheld by the caller
        unsafe { self.shrink_impl(ptr, old_layout, new_layout) }
    }
}

// # Allocation error handler

#[cfg(not(no_global_oom_handling))]
unsafe extern "Rust" {
    // This is the magic symbol to call the global alloc error handler. rustc generates
    // it to call `__rg_oom` if there is a `#[alloc_error_handler]`, or to call the
    // default implementations below (`__rdl_alloc_error_handler`) otherwise.
    #[rustc_std_internal_symbol]
    fn __rust_alloc_error_handler(size: usize, align: usize) -> !;
}

/// Signals a memory allocation error.
///
/// Callers of memory allocation APIs wishing to cease execution
/// in response to an allocation error are encouraged to call this function,
/// rather than directly invoking [`panic!`] or similar.
///
/// This function is guaranteed to diverge (not return normally with a value), but depending on
/// global configuration, it may either panic (resulting in unwinding or aborting as per
/// configuration for all panics), or abort the process (with no unwinding).
///
/// The default behavior is:
///
///  * If the binary links against `std` (typically the case), then
///   print a message to standard error and abort the process.
///   This behavior can be replaced with [`set_alloc_error_hook`] and [`take_alloc_error_hook`].
///   Future versions of Rust may panic by default instead.
///
/// * If the binary does not link against `std` (all of its crates are marked
///   [`#![no_std]`][no_std]), then call [`panic!`] with a message.
///   [The panic handler] applies as to any panic.
///
/// [`set_alloc_error_hook`]: ../../std/alloc/fn.set_alloc_error_hook.html
/// [`take_alloc_error_hook`]: ../../std/alloc/fn.take_alloc_error_hook.html
/// [The panic handler]: https://doc.rust-lang.org/reference/runtime.html#the-panic_handler-attribute
/// [no_std]: https://doc.rust-lang.org/reference/names/preludes.html#the-no_std-attribute
#[stable(feature = "global_alloc", since = "1.28.0")]
#[rustc_const_unstable(feature = "const_alloc_error", issue = "92523")]
#[cfg(not(no_global_oom_handling))]
#[cold]
#[optimize(size)]
pub const fn handle_alloc_error(layout: Layout) -> ! {
    const fn ct_error(_: Layout) -> ! {
        panic!("allocation failed");
    }

    #[inline]
    fn rt_error(layout: Layout) -> ! {
        // SAFETY: Safe to call; we control this function.
        unsafe {
            __rust_alloc_error_handler(layout.size(), layout.align());
        }
    }

    #[cfg(not(panic = "immediate-abort"))]
    {
        core::intrinsics::const_eval_select((layout,), ct_error, rt_error)
    }

    #[cfg(panic = "immediate-abort")]
    ct_error(layout)
}

#[cfg(not(no_global_oom_handling))]
#[doc(hidden)]
#[unstable(feature = "alloc_internals", issue = "none")]
pub mod __alloc_error_handler {
    // called via generated `__rust_alloc_error_handler` if there is no
    // `#[alloc_error_handler]`.
    #[rustc_std_internal_symbol]
    pub unsafe fn __rdl_alloc_error_handler(size: usize, _align: usize) -> ! {
        core::panicking::panic_nounwind_fmt(
            format_args!("memory allocation of {size} bytes failed"),
            /* force_no_backtrace */ false,
        )
    }
}

/// Allocator marker trait that is implemented only on `Global`, except when
/// the allocator feature gate is enabled (in which case it is implemented
/// for all allocators).
///
/// This is to prevent stable code from e.g. constructing `Arc<T, NotGlobal>`
/// using the `From<Box<T, A>> for Arc<T, A>` impl.
///
/// Note that this trait cannot appear in specialization impls (even if not
/// specialized on).
///
/// This trait should be used as a bound whenever a function constructing
/// a type with an `#[unstable] A: Allocator = Global` parameter may be
/// callable for `A != Global`.
#[marker]
#[unstable(feature = "allocator_ext", issue = "163177", implied_by = "allocator_api")]
#[doc(hidden)]
pub trait AllocatorNightly: Allocator {}

#[unstable(feature = "allocator_ext", issue = "163177", implied_by = "allocator_api")]
#[unstable_feature_bound(allocator_ext)]
impl<A: Allocator + ?Sized> AllocatorNightly for A {}

#[unstable(feature = "allocator_ext", issue = "163177", implied_by = "allocator_api")]
impl AllocatorNightly for Global {}
