//! M5r R1 (RELAYER-DESIGN.md): target-neutral memory annotations emitted by codegen.
//!
//! CHERI research prototype on this fork (see AGENTS.md). Not for upstream.
//!
//! rustc only *passes information* to LLVM here; it makes no CHERI decision (no
//! `csetbounds`, no narrow skipping, no check elision). The late CHERI passes in LLVM
//! read these markers and decide (R2/R3). Emitted markers:
//! - `"mem.check"(base, len_elems, elem_size, site, kind)` operand bundle on the
//!   `panic_bounds_check` call of every `Assert(BoundsCheck)`, with `!mem.guard !{site}`
//!   on the index GEP of the access the MIR analysis matched (`find_access`), and a
//!   `"mem.alloc_inbounds"(base)` fact (in the panic block) when the base is proved
//!   allocation-bounded. Unmatched checks carry a null base and `elem_size = 0`.
//! - `%q = llvm.mem.claim(p, bytes, flags)` at the former M3.x narrow points
//!   (export entry, untrusted call returns, `&*raw`, transmute raw->ref, memcpy/memset
//!   operands, the library `cheri_contract_narrow` intrinsic).
//! - `"mem.free"(p)` operand bundle on deallocator calls (`#[rustc_deallocator]`).
//! - `!mem.last_use !{loan}` on the last use of simple loans (see `find_last_uses`).
//! - `call void @llvm.mem.fact() [ "mem.loan_into"(ptr owner_ptr_field, ptr loan_val, i64 id) ]`
//!   after a call that may have stored loan `id` into a local `Vec`/`Box` (M7.1b). The
//!   late pass re-reads the header and nulls only slots equal to `loan_val`. Same-value
//!   collisions (another longer loan of the same place, or an unknown reference, in that
//!   owner) emit nothing.
//! - `call void @llvm.mem.fact() [ "mem.move"(ptr) ]` after a move of `Box` or `Vec`
//!   (or `Option` of those) out of a local memory place (M7.1a). The operand is the
//!   source slot. A scalar load records a read. A memory operand records the slot
//!   and does not emit yet. Each later full-size copy into a different pointer, or
//!   a by-value call that is handed the slot, moves the single fact to after that
//!   read. A partial copy retires the slot with no fact. `-Zcheri-annotate` on a
//!   CHERI target gates it, the same as the loan facts.
//!
//! - `call void @llvm.mem.fact() [ "mem.store_into"(ptr dest, ptr cap) ]` after a
//!   store of a capability into a nameable local destination (M7.1d B2a): a
//!   `ptr::read` dest, a `ptr::write` into a local, or a field of a live local
//!   struct. The late CHERI pass may null `dest` at the source `mem.free` if
//!   it still holds `cap`. Statics, FFI, and `Box::leak` destinations are not
//!   emitted. Gated by `-Zcheri-annotate` and `-Zcheri-store-into` (default on;
//!   `no` is byte-identical to f88cb241).
//!
//! The MIR analysis of `a[i]` sites (`find_access`, `match_index`, the M2 raw-length and
//! trusted-allocation rules) is carried over unchanged from M1/M2 (`local/m5`
//! `cheri_bounds.rs`), except that borrow sites are always matched (flagged in `kind`).

use rustc_abi::Align;
use rustc_data_structures::fx::{FxHashMap, FxHashSet};
use rustc_index::IndexVec;
use rustc_middle::mir::visit::{PlaceContext, Visitor};
use rustc_middle::mir::{self, AssertKind, BasicBlock, Operand, PlaceElem, Rvalue, StatementKind};
use rustc_middle::ty::consts::ConstExt;
use rustc_middle::ty::layout::{HasTyCtxt, HasTypingEnv, LayoutOf};
use rustc_middle::ty::{self, Ty, TyCtxt, TypeVisitableExt};
use rustc_target::callconv::FnAbi;

use super::operand::{OperandRef, OperandValue};
use super::{FunctionCx, LocalRef};
use crate::traits::*;

// Claim flags (llvm/IR/MemAnnot.h `ClaimKind` / `ClaimFlags`).
/// [LLM-generated] R5 (D35): bit 63 marked a loan start in the R5 `LoanEnd` encoding. Since
/// R6 (D43) loan starts are `NonDivergingIntrinsic::LoanStart` statements and the bit only
/// exists in the borrowck side table; R7 (D56): kept here only to mask ids defensively.
pub(crate) const CHERI_LOAN_START: u64 = 1 << 63;
pub(crate) const CK_REF: u64 = 0;
pub(crate) const CK_SLICE: u64 = 1;
pub(crate) const CK_RAW_REF: u64 = 5;
pub(crate) const CK_TRANSMUTE: u64 = 6;
pub(crate) const CK_MEM_OP: u64 = 7;
pub(crate) const CK_RET: u64 = 8;
pub(crate) const CK_PARAM: u64 = 9;
pub(crate) const CF_UNTRUSTED: u64 = 1 << 8;
pub(crate) const CF_NULLABLE: u64 = 1 << 9;
/// R1 addition: the claimed pointer comes from a verified producer (M3.4 inlined
/// `Vec`/`Box` `Global` scope); the late pass may discharge the claim.
pub(crate) const CF_VERIFIED: u64 = 1 << 10;

// Check kinds (`CheckKind`), plus the R1 borrow flag.
const CHK_INDEX: u64 = 0;
const CHK_RAW_CHECKED: u64 = 5;
/// R1 addition: the guarded operation is a borrow `&a[i]` (not an access), so the check
/// cannot be replaced by a hardware fault at the access.
const CHKF_BORROW: u64 = 1 << 8;

/// One eligible `a[i]` site, found on the MIR before codegen.
#[derive(Copy, Clone, Debug)]
pub(crate) struct CheriIndexSite<'tcx> {
    /// The block holding the access (the `Assert`'s only successor).
    pub target: BasicBlock,
    /// Index of the accessing statement in `target`.
    pub stmt: usize,
    /// The accessed place (it may continue past the index, e.g. `(*s)[i].0`).
    pub place: mir::Place<'tcx>,
    /// Position of the `Index`/`ConstantIndex` projection in `place.projection`.
    pub index_pos: usize,
    /// M2: the base is a raw fat pointer `(*r)[i]` whose metadata is SSA-equal to the
    /// check's `len` (the RefProp case).
    pub raw_checked: bool,
    /// M2: the base capability is allocation-bounded (reference/array, not derived from
    /// a raw pointer in this body).
    pub alloc_inbounds: bool,
    /// The access is a borrow `&a[i]` / `&raw a[i]`.
    pub borrow: bool,
}

/// The `!mem.guard` to put on the index GEP of a site's access statement.
#[derive(Copy, Clone, Debug)]
pub(crate) struct MemGuard<'tcx> {
    pub site: CheriIndexSite<'tcx>,
    pub id: u64,
    pub used: bool,
}

/// Where a loan's last use is: a statement, or the block's terminator (a call).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum UseAt {
    Stmt(usize),
    Term,
}

/// Simple-case loan last uses of a body (see `find_last_uses`).
#[derive(Default, Debug)]
pub(crate) struct LastUses {
    /// (block, position) -> [(reference local, loan id)].
    pub at: FxHashMap<(BasicBlock, UseAt), Vec<(mir::Local, u64)>>,
    pub loans_total: u32,
    pub loans_eligible: u32,
}

/// Static view of whether `len` bytes are exactly representable from the base
/// (census output only; R1 makes no decision from it).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Exactness {
    /// Exact for this base: `csetboundsexact` keeps the tag.
    Always,
    /// The length itself is not representable; no base makes it exact.
    Never,
    /// Depends on the run-time base (or length).
    Dynamic,
}

/// RV64 ISAv9 (MW=14) alignment mask, as in LLVM's `RV64YCapabilityFormat::getAlignmentMaskImpl`.
fn rv64_llvm_mask(len: u64) -> u64 {
    if len == 0 {
        return !0;
    }
    let slice = len >> 13;
    let e = if slice != 0 { 64 - slice.leading_zeros() } else { 0 };
    let ie = e != 0 || (len >> 12) & 1 != 0;
    let ep = if ie { e + 3 } else { 0 };
    !0u64 << ep
}

/// Hardware-correct RV64 alignment mask. LLVM's mask is one bit too weak just below each
/// power of two from 8 KiB (probe P1/host check: `0x1ff9` needs 16 bytes, LLVM says 8);
/// applying it again to the padded length matches cc128 on every length in `[1, 2^28)`.
fn rv64_alignment_mask(len: u64) -> u64 {
    let m = rv64_llvm_mask(len);
    let padded = len.wrapping_add(!m) & m;
    rv64_llvm_mask(padded)
}

fn static_exactness(pointer_bits: u64, bytes: u64, base_align: Align) -> Exactness {
    match pointer_bits {
        64 => {
            // Confirmed on QEMU-CHERI and Toooba: any base and any length <= 4095 is exact.
            if bytes <= 4095 {
                return Exactness::Always;
            }
            let mask = rv64_alignment_mask(bytes);
            if bytes & !mask != 0 {
                Exactness::Never
            } else if base_align.bytes() >= (!mask).wrapping_add(1) {
                Exactness::Always
            } else {
                Exactness::Dynamic
            }
        }
        // CHERIoT: exact for any base up to 511 bytes; above that leave it to the tag test.
        _ => {
            if bytes <= 511 {
                Exactness::Always
            } else {
                Exactness::Dynamic
            }
        }
    }
}


impl<'a, 'tcx, Bx: BuilderMethods<'a, 'tcx>> FunctionCx<'a, 'tcx, Bx> {
    /// Whether to emit the M5r markers (`-Zcheri-annotate`, CHERI targets only).
    pub(crate) fn mem_annotate(&self) -> bool {
        let sess = self.cx.tcx().sess;
        sess.target.is_like_cheri && sess.opts.unstable_opts.cheri_annotate
    }

    /// Whether to emit conversion claims (`-Zcheri-contract-narrow`, now emission-only).
    pub(crate) fn mem_claims(&self) -> bool {
        self.mem_annotate() && self.cx.tcx().sess.opts.unstable_opts.cheri_contract_narrow
    }

    /// A module-unique id: a hash of this instance's symbol in the high bits, `n` low.
    fn mem_id(&self, n: usize) -> u64 {
        use std::hash::{Hash, Hasher};
        let tcx = self.cx.tcx();
        let mut h = rustc_data_structures::fx::FxHasher::default();
        tcx.symbol_name(self.instance).name.hash(&mut h);
        ((h.finish() & 0x7_ffff_ffff) << 24) | (n as u64 & 0xff_ffff)
    }

    /// The claim size of a reference to `pointee`, given its pointer operand value:
    /// `Some((bytes, kind))`, or `None` (ZST, `dyn`, unknown metadata shape).
    fn mem_claim_bytes(
        &self,
        bx: &mut Bx,
        pointee: Ty<'tcx>,
        val: OperandValue<Bx::Value>,
    ) -> Option<(Bx::Value, u64)> {
        let tcx = self.cx.tcx();
        if pointee.is_sized(tcx, bx.typing_env()) {
            let n = bx.layout_of(pointee).size.bytes();
            if n == 0 {
                return None;
            }
            return Some((bx.const_usize(n), CK_REF));
        }
        let OperandValue::Pair(_, meta) = val else { return None };
        match *pointee.kind() {
            ty::Slice(elem) => {
                let es = bx.layout_of(elem).size.bytes();
                if es == 0 {
                    return None;
                }
                let esv = bx.const_usize(es);
                Some((if es == 1 { meta } else { bx.unchecked_umul(meta, esv) }, CK_SLICE))
            }
            ty::Str => Some((meta, CK_SLICE + 1)),
            _ => None,
        }
    }

    /// Claim the data pointer of `val` (immediate or pair) for `bytes`.
    fn mem_claim_val(
        &self,
        bx: &mut Bx,
        val: OperandValue<Bx::Value>,
        bytes: Bx::Value,
        flags: u64,
    ) -> OperandValue<Bx::Value> {
        match val {
            OperandValue::Immediate(p) => OperandValue::Immediate(bx.mem_claim(p, bytes, flags)),
            OperandValue::Pair(p, m) => OperandValue::Pair(bx.mem_claim(p, bytes, flags), m),
            other => other,
        }
    }

    /// M3.0 export trust, now a claim: a definition with a non-Rust ABI can be called from
    /// C with a capability wider than the referent, so each `&T`/`&mut T`/`&[T]`/`&str`/
    /// `Box<T>`/`Option<&T>`/`Option<Box<T>>` parameter gets an untrusted entry claim
    /// (`CK_Param | CF_Untrusted [| CF_Nullable]`). The late pass decides whether to narrow.
    /// R2: a parameter living in memory is reloaded, claimed and stored back.
    pub(super) fn mem_export_entry_claims(
        &mut self,
        bx: &mut Bx,
        args: &mut [LocalRef<'tcx, Bx::Value>],
    ) {
        let tcx = self.cx.tcx();
        if !self.mem_annotate()
            || !tcx.sess.opts.unstable_opts.cheri_export_entry_bounds
            || self.fn_abi.conv.is_rustic_abi()
        {
            return;
        }
        let mir = self.mir;
        for (i, local) in mir.args_iter().enumerate() {
            let Some(lref) = args.get_mut(i) else { break };
            let ty = self.monomorphize(mir.local_decls[local].ty);
            let (pointee, nullable) = match *ty.kind() {
                ty::Ref(_, pointee, _) => (pointee, false),
                _ if ty.is_box_global(tcx) => (ty.expect_boxed_ty(), false),
                ty::Adt(adt, gargs)
                    if tcx.is_diagnostic_item(rustc_span::sym::Option, adt.did()) =>
                {
                    let inner = gargs.type_at(0);
                    match *inner.kind() {
                        ty::Ref(_, p, _) => (p, true),
                        _ if inner.is_box_global(tcx) => (inner.expect_boxed_ty(), true),
                        _ => continue,
                    }
                }
                _ => continue,
            };
            let flags = CK_PARAM | CF_UNTRUSTED | if nullable { CF_NULLABLE } else { 0 };
            match lref {
                LocalRef::Operand(op) => {
                    let Some((bytes, _)) = self.mem_claim_bytes(bx, pointee, op.val) else {
                        continue;
                    };
                    let v = self.mem_claim_val(bx, op.val, bytes, flags);
                    *op = OperandRef { val: v, layout: op.layout, move_annotation: None };
                }
                // M5r R2 [LLM-generated]: a parameter living in memory (passed indirectly,
                // or address-taken): reload, claim, store back. With the claim stripped the
                // store of the reloaded value is a no-op that EarlyCSE/DSE remove.
                LocalRef::Place(place) if place.layout.is_sized() => {
                    let place = *place;
                    let op = bx.load_operand(place);
                    let Some((bytes, _)) = self.mem_claim_bytes(bx, pointee, op.val) else {
                        continue;
                    };
                    let v = self.mem_claim_val(bx, op.val, bytes, flags);
                    OperandRef { val: v, layout: op.layout, move_annotation: None }
                        .val
                        .store(bx, place);
                }
                _ => continue,
            }
        }
    }

    /// M3.3/M4.0 untrusted call return, now a claim (`CK_Ret | CF_Untrusted`): a typed
    /// `&T`/`Box<T>`/`Option<&T>`/`Option<Box<T>>` returned by a foreign or non-Rust-ABI
    /// callee.
    pub(crate) fn mem_call_return_claim(
        &self,
        bx: &mut Bx,
        llret: Bx::Value,
        fn_abi: &FnAbi<'tcx, Ty<'tcx>>,
        instance: Option<ty::Instance<'tcx>>,
    ) -> Bx::Value {
        let tcx = self.cx.tcx();
        if !self.mem_claims() {
            return llret;
        }
        let untrusted = match instance {
            Some(inst) => tcx.is_foreign_item(inst.def_id()) || !fn_abi.conv.is_rustic_abi(),
            None => true,
        };
        if !untrusted {
            return llret;
        }
        let ret_ty = fn_abi.ret.layout.ty;
        let (pointee, nullable) = if ret_ty.is_box_global(tcx) {
            (ret_ty.expect_boxed_ty(), false)
        } else if let ty::Ref(_, p, _) = *ret_ty.kind() {
            (p, false)
        } else if let ty::Adt(adt, gargs) = *ret_ty.kind()
            && tcx.is_diagnostic_item(rustc_span::sym::Option, adt.did())
        {
            let inner = gargs.type_at(0);
            match *inner.kind() {
                ty::Ref(_, p, _) => (p, true),
                _ if inner.is_box_global(tcx) => (inner.expect_boxed_ty(), true),
                _ => return llret,
            }
        } else {
            return llret;
        };
        if !pointee.is_sized(tcx, bx.typing_env())
            || bx.cx().type_kind(bx.cx().val_ty(llret)) != crate::common::TypeKind::Pointer
        {
            return llret;
        }
        let n = bx.layout_of(pointee).size.bytes();
        if n == 0 {
            return llret;
        }
        let flags = CK_RET | CF_UNTRUSTED | if nullable { CF_NULLABLE } else { 0 };
        bx.mem_claim(llret, bx.const_usize(n), flags)
    }

    /// M3.3 `&*raw` / `&mut *raw`, now a claim (`CK_RawRef | CF_Untrusted`, plus
    /// `CF_Verified` inside a verified producer scope).
    pub(crate) fn mem_raw_ref_claim(
        &mut self,
        bx: &mut Bx,
        place: mir::Place<'tcx>,
        op: OperandRef<'tcx, Bx::Value>,
    ) -> OperandRef<'tcx, Bx::Value> {
        if !self.mem_claims() {
            return op;
        }
        let local_ty = self.monomorphize(self.mir.local_decls[place.local].ty);
        if place.projection.len() != 1
            || !matches!(place.projection[0], PlaceElem::Deref)
            || !matches!(local_ty.kind(), ty::RawPtr(..))
        {
            return op;
        }
        let pointee = local_ty.builtin_deref(true).unwrap();
        let Some((bytes, _)) = self.mem_claim_bytes(bx, pointee, op.val) else { return op };
        let verified = self.cheri_verified_producer(self.current_source_scope.get());
        let flags = CK_RAW_REF | CF_UNTRUSTED | if verified { CF_VERIFIED } else { 0 };
        let val = self.mem_claim_val(bx, op.val, bytes, flags);
        OperandRef { val, layout: op.layout, move_annotation: None }
    }

    /// M3.6 transmute raw/NonNull -> `&T`/`&[T]`/`Box<T>`, now a claim (`CK_Transmute`).
    pub(crate) fn mem_transmute_claim(
        &mut self,
        bx: &mut Bx,
        src_ty: Ty<'tcx>,
        op: OperandRef<'tcx, Bx::Value>,
    ) -> OperandRef<'tcx, Bx::Value> {
        let tcx = self.cx.tcx();
        if !self.mem_claims() {
            return op;
        }
        let src_ty = src_ty.peel_refs();
        let src_ok = match *src_ty.kind() {
            ty::RawPtr(..) => true,
            ty::Adt(adt, _) => {
                tcx.is_lang_item(adt.did(), rustc_hir::attrs::lang_items::LangItem::NonNull)
            }
            _ => false,
        };
        if !src_ok {
            return op;
        }
        let pointee = match *op.layout.ty.kind() {
            ty::Ref(_, p, _) => p,
            ty::Adt(adt, args) if adt.is_box() => args.type_at(0),
            _ => return op,
        };
        let Some((bytes, _)) = self.mem_claim_bytes(bx, pointee, op.val) else { return op };
        let verified = self.cheri_verified_producer(self.current_source_scope.get());
        let flags = CK_TRANSMUTE | CF_UNTRUSTED | if verified { CF_VERIFIED } else { 0 };
        let val = self.mem_claim_val(bx, op.val, bytes, flags);
        OperandRef { val, layout: op.layout, move_annotation: None }
    }

    /// The library `cheri_contract_narrow(data, len)` intrinsic (from_raw_parts,
    /// Box::from_raw, ptr::read/write, ...), now a claim. The intrinsic keeps its name in
    /// R1 (renaming it is deferred).
    pub(crate) fn mem_contract_claim(
        &self,
        bx: &mut Bx,
        val: OperandValue<Bx::Value>,
        len: Bx::Value,
        scope: mir::SourceScope,
    ) -> OperandValue<Bx::Value> {
        if !self.mem_claims() {
            return val;
        }
        let kind = if matches!(val, OperandValue::Pair(..)) { CK_SLICE } else { CK_REF };
        let verified = self.cheri_verified_producer(scope);
        let flags = kind | CF_UNTRUSTED | if verified { CF_VERIFIED } else { 0 };
        self.mem_claim_val(bx, val, len, flags)
    }

    /// The `"mem.check"` marker for the `Assert(BoundsCheck)` terminating `bb`, emitted
    /// in its panic block (cold) and attached to the panic call by `do_call`. Also records
    /// the `!mem.guard` for the matched access.
    pub(crate) fn mem_check_marker(&mut self, bx: &mut Bx, bb: BasicBlock, len: Bx::Value) {
        if !self.mem_annotate() {
            return;
        }
        let id = self.mem_id(bb.index());
        let i64t = bx.cx().type_i64();
        let len = if bx.cx().val_ty(len) == i64t { len } else { bx.intcast(len, i64t, false) };
        let site = self.cheri_sites.get(&bb).copied();
        let (base, esz, kind) = match site {
            Some(site) => {
                let base_ref = mir::PlaceRef {
                    local: site.place.local,
                    projection: &site.place.projection[..site.index_pos],
                };
                let base = self.codegen_place(bx, base_ref);
                let elem = base.layout.field(bx.cx(), 0);
                let p = base.val.llval;
                if site.alloc_inbounds {
                    bx.mem_fact("mem.alloc_inbounds", &[p]);
                }
                let kind = if site.raw_checked { CHK_RAW_CHECKED } else { CHK_INDEX }
                    | if site.borrow { CHKF_BORROW } else { 0 };
                self.mem_guards.insert(site.target, MemGuard { site, id, used: false });
                self.mem_stats.sites_matched += 1;
                (p, elem.size.bytes(), kind)
            }
            None => {
                self.mem_stats.sites_unmatched += 1;
                (bx.cx().const_null(bx.cx().type_ptr()), 0, CHK_INDEX)
            }
        };
        let cx = bx.cx();
        let ops = [base, len, cx.const_u64(esz), cx.const_u64(id), cx.const_u64(kind)];
        self.mem_call_bundle = Some(("mem.check", ops.to_vec()));
    }

    /// Called by `do_call` right before the call/invoke: hand the pending marker bundle
    /// and last-use loans to the builder, and add `"mem.free"` on deallocator calls.
    pub(crate) fn mem_prepare_call(
        &mut self,
        bx: &mut Bx,
        llargs: &[Bx::Value],
        instance: Option<ty::Instance<'tcx>>,
    ) {
        if let Some((tag, ops)) = self.mem_call_bundle.take() {
            bx.set_next_call_mem_bundle(tag, &ops);
        } else if self.mem_annotate()
            && let Some(inst) = instance
            && let Some(&p) = llargs.first()
            && self.cx.tcx().codegen_fn_attrs(inst.def_id()).flags.contains(
                rustc_middle::middle::codegen_fn_attrs::CodegenFnAttrFlags::DEALLOCATOR,
            )
        {
            self.mem_stats.frees += 1;
            bx.set_next_call_mem_bundle("mem.free", &[p]);
        }
        if let Some(loans) = self.mem_call_last_use.take() {
            self.mem_stats.last_use_calls += 1;
            bx.set_next_call_mem_last_use(&loans);
        }
    }

    /// M7.1a: a `Box` / `Vec` (or `Option` of those) was just loaded or aliased by
    /// `Operand::Move`. The slot is tracked. A scalar load is one full read. A
    /// memory operand is not consumed until a later full-size copy into a different
    /// pointer, or a by-value call that is handed the slot. Each such read moves
    /// the single fact to immediately after itself, so an earlier copy or an
    /// inlined return cannot leave `mem.move` before a later `memcpy`
    /// (`from_elem`, `CString::spec_new_impl`). A partial copy drops the fact.
    pub(crate) fn mem_note_owner_move(
        &mut self,
        bx: &mut Bx,
        place: mir::PlaceRef<'tcx>,
        val: &OperandRef<'tcx, Bx::Value>,
    ) {
        if !self.mem_annotate() || !self.mem_move_place_precise(place) {
            return;
        }
        let ty = self.monomorphized_place_ty(place);
        if ty.has_param()
            || ty.has_infer()
            || ty.has_placeholders()
            || !self.mem_is_unique_owner(ty)
        {
            return;
        }
        match val.val {
            OperandValue::Immediate(_) | OperandValue::Pair(_, _) => {
                let cg = self.codegen_place(bx, place);
                if cg.layout.is_unsized() || cg.val.llextra.is_some() {
                    return;
                }
                let bytes = cg.layout.size.bytes();
                bx.mem_move_track(cg.val.llval, bytes);
                // The load copied the owner into a register. `const_null` is a
                // destination that is not the slot, so this counts as a full read.
                // A later full `memcpy` of the slot moves the fact to after that copy.
                let elsewhere = bx.const_null(bx.type_ptr());
                bx.mem_move_read(cg.val.llval, elsewhere, Some(bytes));
            }
            OperandValue::Ref(pv) if pv.llextra.is_none() => {
                let bytes = bx.layout_of(ty).size.bytes();
                bx.mem_move_track(pv.llval, bytes);
            }
            OperandValue::Ref(_) | OperandValue::ZeroSized => {}
        }
    }

    /// `memcpy %scratch <- %slot` while passing `%slot` indirectly at `-O0`.
    ///
    /// That copy is `OperandValue::Ref::store_with_flags` → `typed_place_copy`
    /// → `Builder::memcpy`. The memcpy hook only emits when `%slot` is still a
    /// live track and the size operand is a constant wide enough for it. An
    /// `intcast` of the length, or a copy taken because the operand is not
    /// `Operand::Move`, leaves no fact: the hook either never saw the slot or
    /// retired it. This is the copy `eat_vec` actually reads (`move_vec`,
    /// `move_repeat`'s `%c`). Record the source and place the fact immediately
    /// after the store, using the layout size rather than the cast length.
    pub(crate) fn mem_move_after_arg_copy(
        &mut self,
        bx: &mut Bx,
        ty: Ty<'tcx>,
        src: Bx::Value,
        dest: Bx::Value,
    ) {
        if !self.mem_annotate() {
            return;
        }
        let bytes = bx.layout_of(ty).size.bytes();
        if bytes == 0 {
            return;
        }
        if !self.mem_is_unique_owner(ty) {
            // Still relocate a track that was recorded for this pointer.
            bx.mem_move_read(src, dest, Some(bytes));
            return;
        }
        bx.mem_move_track(src, bytes);
        bx.mem_move_read(src, dest, Some(bytes));
    }

    /// The place is a local stack slot, or a constant-offset field of one, or a
    /// dereference of a local pointer. Dynamic indices and subslices are skipped:
    /// the resulting address is not a slot this pass can name precisely.
    fn mem_move_place_precise(&self, place: mir::PlaceRef<'tcx>) -> bool {
        let projections_ok = place.projection.iter().all(|elem| {
            matches!(
                elem,
                mir::ProjectionElem::Field(..)
                    | mir::ProjectionElem::Downcast(..)
                    | mir::ProjectionElem::Deref
                    | mir::ProjectionElem::ConstantIndex { from_end: false, .. }
            )
        });
        if !projections_ok {
            return false;
        }
        match self.locals[place.local] {
            LocalRef::Place(_) => true,
            // An SSA local has no alloca. A leading deref is the pointee slot,
            // which `codegen_place` can name. Any other projection of an SSA
            // local is a register extract, not a memory slot.
            LocalRef::Operand(_) if place.is_indirect_first_projection() => true,
            LocalRef::Operand(_) | LocalRef::PendingOperand | LocalRef::UnsizedPlace(_) => false,
        }
    }

    fn mem_is_box_or_vec(&self, ty: Ty<'tcx>) -> bool {
        if ty.is_box() {
            return true;
        }
        match ty.kind() {
            ty::Adt(def, _) => self.cx.tcx().is_diagnostic_item(rustc_span::sym::Vec, def.did()),
            _ => false,
        }
    }

    /// `Box` and `Vec` are the unique owners whose bit-copy is a move. `Option`
    /// of those is included because the niche is the owner capability itself
    /// (`Option<Box<T>>` in an escaped slot). Copy types and raw pointers are not.
    fn mem_is_unique_owner(&self, ty: Ty<'tcx>) -> bool {
        let tcx = self.cx.tcx();
        if ty.is_raw_ptr() || ty.is_ref() || ty.is_fn_ptr() {
            return false;
        }
        if tcx.type_is_copy_modulo_regions(self.cx.typing_env(), ty) {
            return false;
        }
        if self.mem_is_box_or_vec(ty) {
            return true;
        }
        if let ty::Adt(def, args) = ty.kind()
            && tcx.is_diagnostic_item(rustc_span::sym::Option, def.did())
            && let Some(inner) = args.types().next()
        {
            return self.mem_is_box_or_vec(inner);
        }
        false
    }

    /// `!mem.guard` on the index GEP of the site access being emitted (`codegen_place`).
    pub(crate) fn mem_guard_place(
        &mut self,
        bx: &mut Bx,
        place_ref: mir::PlaceRef<'tcx>,
        pos: usize,
        llval: Bx::Value,
    ) {
        if let Some(g) = self.mem_guard_active.as_mut()
            && g.site.place.as_ref() == place_ref
            && g.site.index_pos == pos
        {
            g.used = true;
            bx.mem_guard(llval, g.id);
        }
    }

    /// M3.3: whether `scope` is (transitively) inside a verified producer of an
    /// allocation-bounded pointer (`Vec::<_, Global>::as_slice` / `as_mut_slice`).
    pub(crate) fn cheri_verified_producer(&self, scope: mir::SourceScope) -> bool {
        self.in_trusted_vec_scope(scope)
    }

    /// M5.0 census (`-Zcheri-check-census`): one stderr line per software check site of this
    /// instance, before LLVM. Format (tab-separated):
    /// `CHERI-CENSUS compiling-crate sym def-crate site kind loc detail`, where `site` is `assert` or `call`.
    pub(crate) fn cheri_check_census(&self) {
        let tcx = self.cx.tcx();
        if !tcx.sess.opts.unstable_opts.cheri_check_census {
            return;
        }
        let sym = tcx.symbol_name(self.instance).name;
        let krate = tcx.crate_name(self.instance.def_id().krate);
        let local = tcx.crate_name(rustc_span::def_id::LOCAL_CRATE);
        let sm = tcx.sess.source_map();
        let loc_of = |si: mir::SourceInfo| {
            // `#[track_caller]` instance whose location comes from its caller: `<caller>`.
            let caller = self.instance.def.requires_caller_location(tcx).then_some(None);
            let Some(span) = self.mir.caller_location_span(si, caller, tcx, Some) else {
                return "<caller>:0:0".to_string();
            };
            let lo = sm.lookup_char_pos(span.lo());
            format!(
                "{}:{}:{}",
                lo.file.name.prefer_remapped_unconditionally(),
                lo.line,
                lo.col_display + 1
            )
        };
        let pointer_bits = tcx.data_layout.address_size().bits();
        let mut out: Vec<String> = Vec::new();
        for (bb, data) in self.mir.basic_blocks.iter_enumerated() {
            let term = data.terminator();
            match &term.kind {
                mir::TerminatorKind::Assert { msg, .. } => {
                    let kind = match &**msg {
                        AssertKind::BoundsCheck { .. } => "bounds".to_string(),
                        AssertKind::Overflow(op, ..) => {
                            if !tcx.sess.overflow_checks() && msg.is_optional_overflow_check() {
                                format!("overflow-off:{op:?}")
                            } else {
                                format!("overflow:{op:?}")
                            }
                        }
                        AssertKind::OverflowNeg(..) => {
                            if !tcx.sess.overflow_checks() {
                                "overflow-off:Neg".to_string()
                            } else {
                                "overflow:Neg".to_string()
                            }
                        }
                        AssertKind::DivisionByZero(..) => "div_zero".to_string(),
                        AssertKind::RemainderByZero(..) => "rem_zero".to_string(),
                        AssertKind::MisalignedPointerDereference { .. } => "misaligned".to_string(),
                        AssertKind::NullPointerDereference => "null_deref".to_string(),
                        other => format!("assert_other:{:?}", std::mem::discriminant(other)),
                    };
                    let detail = if let AssertKind::BoundsCheck { .. } = &**msg {
                        match self.cheri_sites.get(&bb) {
                            None => "unmatched".to_string(),
                            Some(site) => {
                                let base = mir::PlaceRef {
                                    local: site.place.local,
                                    projection: &site.place.projection[..site.index_pos],
                                };
                                let via_deref =
                                    base.projection.iter().any(|e| matches!(e, PlaceElem::Deref));
                                let bty = self.monomorphize(base.ty(self.mir, tcx).ty);
                                let layout = self.cx.layout_of(bty);
                                let (ex, size) = if layout.is_unsized() {
                                    (Exactness::Dynamic, "dyn".to_string())
                                } else {
                                    let n = layout.size.bytes();
                                    (static_exactness(pointer_bits, n, layout.align.abi), n.to_string())
                                };
                                let elem = layout.field(self.cx, 0).size.bytes();
                                rustc_middle::ty::print::with_no_trimmed_paths!(format!(
                                    "matched exact={ex:?} bytes={size} elem={elem} base={} raw_checked={} alloc_inbounds={} ty={}",
                                    if via_deref { "deref" } else { "local" },
                                    site.raw_checked,
                                    site.alloc_inbounds,
                                    bty
                                ))
                            }
                        }
                    } else {
                        String::new()
                    };
                    out.push(format!(
                        "CHERI-CENSUS\t{local}\t{sym}\t{krate}\tassert\t{kind}\t{}\t{detail}",
                        loc_of(term.source_info)
                    ));
                }
                mir::TerminatorKind::Call { func, target, .. } => {
                    let Some((def_id, _)) = func.const_fn_def() else { continue };
                    let path = rustc_middle::ty::print::with_no_trimmed_paths!(tcx.def_path_str(def_id));
                    let last = tcx.item_name(def_id);
                    let chk = matches!(
                        last.as_str(),
                        "index" | "index_mut" | "unwrap" | "expect" | "borrow" | "borrow_mut"
                            | "split_at" | "split_at_mut" | "copy_from_slice" | "clone_from_slice"
                            | "unwrap_unchecked" | "get_unchecked" | "get_unchecked_mut"
                    );
                    if target.is_some() && !chk {
                        continue;
                    }
                    let site = if target.is_none() { "divcall" } else { "chkcall" };
                    out.push(format!(
                        "CHERI-CENSUS\t{local}\t{sym}\t{krate}\tcall\t{site}\t{}\t{path}",
                        loc_of(term.source_info)
                    ));
                }
                _ => {}
            }
        }
        if out.is_empty() {
            return;
        }
        let mut text = out.join("\n");
        text.push('\n');
        // `CHERI_CENSUS_FILE`: append there (cargo hides stderr of dependency builds).
        match std::env::var_os("CHERI_CENSUS_FILE") {
            Some(path) => {
                use std::io::Write;
                let mut f = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                    .expect("CHERI_CENSUS_FILE");
                f.write_all(text.as_bytes()).expect("CHERI_CENSUS_FILE write");
            }
            None => eprint!("{text}"),
        }
    }

    /// Finds the `a[i]` sites of this body, keyed by the block whose terminator is the check.
    pub(crate) fn find_cheri_index_sites(&self) -> FxHashMap<BasicBlock, CheriIndexSite<'tcx>> {
        let mut sites = FxHashMap::default();
        if !self.mem_annotate() {
            return sites;
        }
        let mir = self.mir;
        let preds = mir.basic_blocks.predecessors();
        let uses = count_uses(mir);
        for (bb, data) in mir.basic_blocks.iter_enumerated() {
            let mir::TerminatorKind::Assert { expected: true, ref msg, target, .. } =
                data.terminator().kind
            else {
                continue;
            };
            let AssertKind::BoundsCheck { ref len, ref index } = **msg else { continue };
            if target == bb || preds[target].len() != 1 {
                continue;
            }
            if let Some(site) = self.find_access(target, len, index, &uses) {
                sites.insert(bb, site);
            }
        }
        sites
    }

    fn find_access(
        &self,
        target: BasicBlock,
        len: &Operand<'tcx>,
        index: &Operand<'tcx>,
        uses: &IndexVec<mir::Local, u32>,
    ) -> Option<CheriIndexSite<'tcx>> {
        for (k, stmt) in self.mir[target].statements.iter().enumerate() {
            match &stmt.kind {
                StatementKind::StorageLive(_)
                | StatementKind::StorageDead(_)
                | StatementKind::Nop
                // Borrowck inserts these immediately before the guarded `Rvalue::Ref` on a
                // CHERI target. They are execution no-ops (codegen still emits `!mem.loan` /
                // `!mem.last_use` from them), so they must not hide that borrow. Any other
                // statement is still observable and stops the walk.
                | StatementKind::Intrinsic(
                    mir::NonDivergingIntrinsic::LoanStart(..)
                    | mir::NonDivergingIntrinsic::LoanEnd(..),
                ) => {
                    continue;
                }
                StatementKind::Assign(assign) => {
                    let (lhs, rvalue) = &**assign;
                    let mut found = None;
                    let mut ambiguous = false;
                    let mut consider = |place: &mir::Place<'tcx>| {
                        if let Some(pos) = self.match_index(place, len, index, uses) {
                            // `s[i] += 1` reads and writes the same place, which is fine.
                            if found.is_some_and(|(p, _)| p != *place) {
                                ambiguous = true;
                            }
                            found = Some((*place, pos));
                        }
                    };
                    consider(lhs);
                    for op in access_operands(rvalue) {
                        if let Operand::Copy(p) | Operand::Move(p) = op {
                            consider(p);
                        }
                    }
                    let mut borrow_hit = false;
                    if let Rvalue::Ref(_, _, p) | Rvalue::RawPtr(_, p) = rvalue {
                        consider(p);
                        borrow_hit = true;
                    }
                    drop(consider);
                    let via_borrow = borrow_hit && found.is_some_and(|(q, _)| {
                        matches!(rvalue, Rvalue::Ref(_, _, p) | Rvalue::RawPtr(_, p) if *p == q)
                    });
                    if ambiguous {
                        return None;
                    }
                    let (place, index_pos) = found?;
                    // LLVM may delete an access whose result is never used (or a store to a
                    // local that is never read), which would lose the fault along with the
                    // panic. Keep the check when MIR already shows that the access is dead.
                    // This does not cover accesses that become dead only after LLVM's own
                    // optimizations.
                    let is_store = *lhs == place;
                    if !is_store
                        && let Some(dest) = lhs.as_local()
                        && uses[dest] < 2
                    {
                        return None;
                    }
                    if is_store && !place.is_indirect_first_projection() && uses[place.local] < 2 {
                        return None;
                    }
                    let (index_pos, raw_checked, trusted) = index_pos;
                    // R1: borrows are always sites (flagged `CHKF_BORROW`); whether the
                    // check may go is the late pass's decision.
                    let alloc_inbounds =
                        !raw_checked && (trusted || !self.base_from_raw(place.local, 4));
                    return Some(CheriIndexSite {
                        target,
                        stmt: k,
                        place,
                        index_pos,
                        raw_checked,
                        alloc_inbounds,
                        borrow: via_borrow,
                    });
                }
                // Anything else could be observable before the access.
                _ => return None,
            }
        }
        None
    }

    /// Returns the position of the projection indexed by the check's `index`, if `place`
    /// is an eligible access for the check.
    fn match_index(
        &self,
        place: &mir::Place<'tcx>,
        len: &Operand<'tcx>,
        index: &Operand<'tcx>,
        uses: &IndexVec<mir::Local, u32>,
    ) -> Option<(usize, bool, bool)> {
        let tcx = self.cx.tcx();
        let typing_env = self.cx.typing_env();
        let index_const = index
            .constant()
            .and_then(|c| self.monomorphize(c.const_).try_eval_target_usize(tcx, typing_env));
        let index_local = index.place().and_then(|p| p.as_local());

        let mut pos = None;
        for (i, elem) in place.projection.iter().enumerate() {
            let hit = match elem {
                PlaceElem::Index(l) => Some(l) == index_local,
                PlaceElem::ConstantIndex { offset, from_end: false, .. } => {
                    Some(offset) == index_const
                }
                _ => false,
            };
            if hit {
                if pos.is_some() {
                    return None;
                }
                pos = Some(i);
            }
        }
        let pos = pos?;

        // The base must be a reference's pointee or a local, never behind a raw pointer
        // or `Box` (those are lowered to raw-pointer derefs by now). M2 exception: the
        // exact shape `(*r)[i]` with `r: *const [T]` whose metadata is SSA-equal to the
        // check's `len` is recorded as `raw_checked` (tagged, never narrowed).
        let mut raw_checked = false;
        let mut trusted = false;
        let mut place_ty =
            mir::PlaceTy::from_ty(self.monomorphize(self.mir.local_decls[place.local].ty));
        for (i, elem) in place.projection[..pos].iter().enumerate() {
            if let PlaceElem::Deref = elem
                && !matches!(place_ty.ty.kind(), ty::Ref(..))
            {
                let r = place.local;
                if i != 0 || pos != 1 {
                    return None;
                }
                if self.raw_len_rule(r, len, uses) {
                    // Raw slice whose metadata is the checked length. Trusted when
                    // liballoc minted it from a `Vec<T, Global>`.
                    if self.trusted_alloc_ptr(r) {
                        trusted = true;
                    } else {
                        raw_checked = true;
                    }
                } else {
                    return None;
                }
            }
            place_ty = place_ty.projection_ty(tcx, *elem);
        }
        let base_ty = place_ty.ty;
        let elem_ty = match *base_ty.kind() {
            ty::Slice(elem) => {
                // The narrowing uses the place's own length (the slice metadata), which is the
                // value the MIR builder compares against; a constant length means this is not
                // the check for this slice.
                if len.constant().is_some() {
                    return None;
                }
                elem
            }
            ty::Array(elem, n) => {
                let n = self.monomorphize(n).try_to_target_usize(tcx)?;
                let len_c = len.constant().and_then(|c| {
                    self.monomorphize(c.const_).try_eval_target_usize(tcx, typing_env)
                })?;
                if len_c != n {
                    return None;
                }
                elem
            }
            _ => return None,
        };
        // No memory access, no hardware fault: keep the software check.
        let full_ty: Ty<'tcx> = place.ty(self.mir, tcx).ty;
        for ty in [elem_ty, self.monomorphize(full_ty)] {
            let layout = self.cx.layout_of(ty);
            if layout.is_zst() || layout.is_uninhabited() || layout.is_unsized() {
                return None;
            }
        }
        if (raw_checked || trusted) && !matches!(base_ty.kind(), ty::Slice(_)) {
            return None;
        }
        Some((pos, raw_checked, trusted))
    }

    /// M2 SSA length rule: `r` is defined exactly once, as `*const [T] from (data, copy m)`,
    /// and the check's `len` is `copy m` (or `m` is `PtrMetadata(copy r)`), where `m` has at
    /// most one definition and neither local has its address taken. Then the check compared
    /// the index against `r`'s own metadata.
    fn raw_len_rule(
        &self,
        r: mir::Local,
        len: &Operand<'tcx>,
        uses: &IndexVec<mir::Local, u32>,
    ) -> bool {
        let _ = uses;
        let Some(m) = len.place().and_then(|p| p.as_local()) else { return false };
        let r_ty = self.monomorphize(self.mir.local_decls[r].ty);
        if !matches!(r_ty.kind(), ty::RawPtr(inner, _) if matches!(inner.kind(), ty::Slice(_))) {
            return false;
        }
        let defs = local_defs(self.mir);
        if defs.borrowed.contains(&r) || defs.borrowed.contains(&m) {
            return false;
        }
        if [r, m].iter().any(|l| defs.other_writes.contains(l) || defs.call_defs.contains(l)) {
            return false;
        }
        let r_defs: &[&Rvalue<'tcx>] = defs.defs.get(&r).map(|v| &v[..]).unwrap_or(&[]);
        let m_defs: &[&Rvalue<'tcx>] = defs.defs.get(&m).map(|v| &v[..]).unwrap_or(&[]);
        if r_defs.len() != 1 || m_defs.len() > 1 {
            return false;
        }
        if let Rvalue::Aggregate(kind, ops) = r_defs[0]
            && matches!(**kind, mir::AggregateKind::RawPtr(..))
            && let Some(meta) = ops.iter().nth(1)
            && let Operand::Copy(mp) | Operand::Move(mp) = meta
            && mp.as_local() == Some(m)
        {
            return true;
        }
        if let [Rvalue::UnaryOp(mir::UnOp::PtrMetadata, Operand::Copy(rp) | Operand::Move(rp))] =
            *m_defs
            && rp.as_local() == Some(r)
        {
            return true;
        }
        false
    }

    /// M2: whether `local` (the root of a site's place) may hold a reference derived from a
    /// raw pointer in this body (`&*raw`, casts, transmutes). Trust model: safe references
    /// and arrays reaching this body (arguments, call results, loads) are allocation-bounded
    /// under M1r; only references minted here from raw pointers are not. Unknown shapes count
    /// as derived-from-raw, which only keeps the narrowing.
    fn base_from_raw(&self, local: mir::Local, depth: u32) -> bool {
        let ty = self.monomorphize(self.mir.local_decls[local].ty);
        if !matches!(ty.kind(), ty::Ref(..) | ty::Array(..)) {
            return true;
        }
        if depth == 0 {
            return true;
        }
        let defs = local_defs(self.mir);
        if defs.other_writes.contains(&local) {
            return true;
        }
        for rv in defs.defs.get(&local).map(|v| &v[..]).unwrap_or(&[]) {
            let bad = match rv {
                Rvalue::Ref(_, _, src) => self.place_through_raw(src, depth - 1),
                Rvalue::Use(Operand::Copy(src) | Operand::Move(src), _) => match src.as_local() {
                    Some(l) => self.base_from_raw(l, depth - 1),
                    None => self.place_through_raw(src, depth - 1),
                },
                // Array values built in place (`[x; N]`, `[a, b]`) live in a bounded alloca.
                Rvalue::Repeat(..) | Rvalue::Aggregate(..) => false,
                Rvalue::Use(Operand::Constant(_), _) => false,
                _ => true,
            };
            if bad {
                return true;
            }
        }
        false
    }

    /// Whether `place` goes through a raw pointer, or through a local reference that
    /// [`Self::base_from_raw`] does not trust.
    fn place_through_raw(&self, place: &mir::Place<'tcx>, depth: u32) -> bool {
        let tcx = self.cx.tcx();
        let mut place_ty =
            mir::PlaceTy::from_ty(self.monomorphize(self.mir.local_decls[place.local].ty));
        for (i, elem) in place.projection.iter().enumerate() {
            if let PlaceElem::Deref = elem {
                match place_ty.ty.kind() {
                    ty::Ref(..) => {
                        if i == 0 && self.base_from_raw(place.local, depth) {
                            return true;
                        }
                    }
                    ty::RawPtr(..) if i == 0 && self.trusted_alloc_ptr(place.local) => {}
                    _ => return true,
                }
            }
            place_ty = place_ty.projection_ty(tcx, elem);
        }
        false
    }

    /// M2.3 trust boundary for liballoc collections. `r` is a raw slice pointer defined
    /// once (never borrowed, written otherwise, or call-defined) either
    /// - by an aggregate `*const/*mut [T] from (data, len)` that MIR inlining placed in
    ///   the scope of `Vec::<T, Global>::as_slice` / `as_mut_slice` (also reached through
    ///   `String::as_bytes` / `Deref` / `Index`), or
    /// - by `BoxDerefTransmute` of a `Box<[T], Global>`'s pointer field (this trusts
    ///   reborrows `&*b`; direct `b[i]` is not a site because MIR re-derives the pointer
    ///   after the check).
    /// Such a pointer came from liballoc's `Global` allocation path, which bounds the
    /// capability to the (padded) allocation under M1r, and its metadata is `len <= cap`
    /// (Vec invariant) or the allocation's own length (Box). The same holds for values
    /// built by `Vec::from_raw_parts` / `Box::from_raw` *because their safety contract*
    /// requires a `Global` allocation of that layout: we rely on that contract. A custom
    /// allocator `A != Global` is never trusted, nor is a raw slice built by user code.
    /// Disable with `-Zcheri-alloc-trusted-slices=no`.
    fn trusted_alloc_ptr(&self, r: mir::Local) -> bool {
        let tcx = self.cx.tcx();
        if !tcx.sess.opts.unstable_opts.cheri_alloc_trusted_slices {
            return false;
        }
        let r_ty = self.monomorphize(self.mir.local_decls[r].ty);
        if !matches!(r_ty.kind(), ty::RawPtr(inner, _) if matches!(inner.kind(), ty::Slice(_))) {
            return false;
        }
        let defs = local_defs(self.mir);
        if defs.borrowed.contains(&r) || defs.other_writes.contains(&r) || defs.call_defs.contains(&r)
        {
            return false;
        }
        let Some((rv, info)) = self.single_def(r) else { return false };
        match rv {
            Rvalue::Aggregate(kind, _) if matches!(**kind, mir::AggregateKind::RawPtr(..)) => {
                self.in_trusted_vec_scope(info.scope)
            }
            Rvalue::Cast(mir::CastKind::BoxDerefTransmute, Operand::Copy(b) | Operand::Move(b), _) => {
                self.global_box_ptr(b, 2)
            }
            _ => false,
        }
    }

    /// The single assignment to `local` and its source info.
    fn single_def(&self, local: mir::Local) -> Option<(&Rvalue<'tcx>, mir::SourceInfo)> {
        let mut found = None;
        for data in self.mir.basic_blocks.iter() {
            for stmt in &data.statements {
                if let StatementKind::Assign(assign) = &stmt.kind
                    && assign.0.as_local() == Some(local)
                {
                    if found.is_some() {
                        return None;
                    }
                    found = Some((&assign.1, stmt.source_info));
                }
            }
        }
        found
    }

    /// Whether `scope` is (transitively) inside an inlined `Vec::<_, Global>::as_slice` or
    /// `as_mut_slice`.
    fn in_trusted_vec_scope(&self, scope: mir::SourceScope) -> bool {
        let tcx = self.cx.tcx();
        let mut cur = Some(scope);
        while let Some(sc) = cur {
            let data = &self.mir.source_scopes[sc];
            if let Some((inst, _)) = data.inlined
                && tcx
                    .get_diagnostic_name(inst.def_id())
                    .is_some_and(|n| matches!(n.as_str(), "vec_as_slice" | "vec_as_mut_slice"))
            {
                let args = self.monomorphize(inst.args);
                return args.len() == 2 && self.is_global_alloc(args.type_at(1));
            }
            cur = data.parent_scope;
        }
        false
    }

    fn is_global_alloc(&self, ty: Ty<'tcx>) -> bool {
        let tcx = self.cx.tcx();
        ty.ty_adt_def().is_some_and(|d| tcx.is_lang_item(d.did(), rustc_hir::attrs::lang_items::LangItem::GlobalAlloc))
    }

    /// Whether `p` holds a `Box<[T], Global>`'s pointer field: the place `box.0` itself,
    /// or a once-defined, never-borrowed local copied from such a place.
    fn global_box_ptr(&self, p: &mir::Place<'tcx>, depth: u32) -> bool {
        let tcx = self.cx.tcx();
        if let Some(l) = p.as_local() {
            if depth == 0 {
                return false;
            }
            let defs = local_defs(self.mir);
            if defs.borrowed.contains(&l) || defs.other_writes.contains(&l) || defs.call_defs.contains(&l)
            {
                return false;
            }
            return match self.single_def(l) {
                Some((Rvalue::Use(Operand::Copy(q) | Operand::Move(q), _), _)) => {
                    self.global_box_ptr(q, depth - 1)
                }
                _ => false,
            };
        }
        let Some((PlaceElem::Field(..), base)) =
            p.projection.split_last().map(|(e, rest)| (*e, rest))
        else {
            return false;
        };
        let box_ty = mir::PlaceRef { local: p.local, projection: base }.ty(self.mir, tcx).ty;
        let box_ty = self.monomorphize(box_ty);
        if !box_ty.is_box() {
            return false;
        }
        let ty::Adt(_, args) = box_ty.kind() else { return false };
        args.len() == 2
            && matches!(box_ty.boxed_ty().map(|t| t.kind()), Some(ty::Slice(_)))
            && self.is_global_alloc(args.type_at(1))
    }

}

/// Operands of `rvalue` that are read from memory when it is evaluated. Borrows,
/// address-of, discriminant reads and metadata reads are not accesses of the element, and
/// `[x; 0]` need not evaluate `x` at all.
fn access_operands<'a, 'tcx>(rvalue: &'a Rvalue<'tcx>) -> Vec<&'a Operand<'tcx>> {
    match rvalue {
        Rvalue::Use(op, _) | Rvalue::Cast(_, op, _) | Rvalue::UnaryOp(_, op) => {
            vec![op]
        }
        Rvalue::BinaryOp(_, ops) => vec![&ops.0, &ops.1],
        Rvalue::Aggregate(_, ops) => ops.iter().collect(),
        _ => vec![],
    }
}

/// Number of uses of each local, not counting storage markers and debuginfo.
fn count_uses(body: &mir::Body<'_>) -> IndexVec<mir::Local, u32> {
    struct Count(IndexVec<mir::Local, u32>);
    impl<'tcx> Visitor<'tcx> for Count {
        fn visit_local(&mut self, local: mir::Local, ctx: PlaceContext, _: mir::Location) {
            if !matches!(ctx, PlaceContext::NonUse(_)) {
                self.0[local] += 1;
            }
        }
    }
    let mut c = Count(IndexVec::from_elem(0, &body.local_decls));
    c.visit_body(body);
    c.0
}

/// Definitions (assignments) of each local, and the set of locals whose address is taken.
struct LocalDefs<'a, 'tcx> {
    defs: FxHashMap<mir::Local, Vec<&'a Rvalue<'tcx>>>,
    borrowed: rustc_data_structures::fx::FxHashSet<mir::Local>,
    /// Written other than by a whole-local assignment (field writes, inline asm, yields).
    other_writes: rustc_data_structures::fx::FxHashSet<mir::Local>,
    /// Defined by a call return (trusted for safe references, never SSA for the len rule).
    call_defs: rustc_data_structures::fx::FxHashSet<mir::Local>,
}

fn local_defs<'a, 'tcx>(body: &'a mir::Body<'tcx>) -> LocalDefs<'a, 'tcx> {
    use rustc_middle::mir::visit::MutatingUseContext;
    let mut d = LocalDefs {
        defs: FxHashMap::default(),
        borrowed: Default::default(),
        other_writes: Default::default(),
        call_defs: Default::default(),
    };
    for data in body.basic_blocks.iter() {
        for stmt in &data.statements {
            if let StatementKind::Assign(assign) = &stmt.kind {
                let (lhs, rv) = &**assign;
                if let Some(l) = lhs.as_local() {
                    d.defs.entry(l).or_default().push(rv);
                } else if !lhs.is_indirect_first_projection() {
                    d.other_writes.insert(lhs.local);
                }
                match rv {
                    Rvalue::Ref(_, _, src) | Rvalue::RawPtr(_, src)
                        if !src.is_indirect_first_projection() =>
                    {
                        d.borrowed.insert(src.local);
                    }
                    _ => {}
                }
            }
        }
        // Calls and other terminators writing a local.
        struct W<'b> {
            other: &'b mut rustc_data_structures::fx::FxHashSet<mir::Local>,
            calls: &'b mut rustc_data_structures::fx::FxHashSet<mir::Local>,
        }
        impl<'tcx> Visitor<'tcx> for W<'_> {
            fn visit_place(&mut self, place: &mir::Place<'tcx>, ctx: PlaceContext, _: mir::Location) {
                match ctx {
                    PlaceContext::MutatingUse(MutatingUseContext::Call)
                        if place.as_local().is_some() =>
                    {
                        self.calls.insert(place.local);
                    }
                    PlaceContext::MutatingUse(
                        MutatingUseContext::Call
                        | MutatingUseContext::AsmOutput
                        | MutatingUseContext::Yield,
                    ) if !place.is_indirect_first_projection() => {
                        self.other.insert(place.local);
                    }
                    _ => {}
                }
            }
        }
        if let Some(term) = &data.terminator {
            W { other: &mut d.other_writes, calls: &mut d.call_defs }
                .visit_terminator(term, mir::Location::START);
        }
    }
    d
}

/// Per-instance marker counts, appended to `$MEM_ANNOT_STATS_FILE` when set (R1 coverage).
#[derive(Default, Debug)]
pub(crate) struct MemStats {
    pub sites_matched: u32,
    pub sites_unmatched: u32,
    pub guards: u32,
    pub guards_missed: u32,
    pub frees: u32,
    pub last_use_points: u32,
    pub last_use_stmts: u32,
    pub last_use_stmts_missed: u32,
    pub last_use_calls: u32,
    /// D17 `LoanEnd` markers reached at codegen, and where their ids went.
    pub loan_ends: u32,
    pub loan_ends_placed: u32,
    pub loan_ends_on_call: u32,
    pub loan_ends_at_return: u32,
    pub loan_ends_missed: u32,
    pub loan_ends_carried: u32,
    /// R5 (D35): loan starts reached at codegen, slot-fact stores tagged, starts whose
    /// next statement was not a `Ref` into memory.
    pub loan_starts: u32,
    pub loan_slot_stores: u32,
    pub loan_slot_copies: u32,
    pub loan_starts_missed: u32,
    pub loan_slot_ssa: u32,
    pub loan_slot_aggregate: u32,
    pub loan_ends_inline_call: u32,
    pub loan_ends_backward: u32,
    /// R9 (D65): linked starts paired with their local's assignment / with a lost link.
    pub loan_link_paired: u32,
    pub loan_link_lost: u32,
    /// M7.1b: `mem.loan_into` facts emitted, and facts skipped by the same-value guard
    /// or because the owner/value could not be named.
    pub loan_into: u32,
    pub loan_into_skipped: u32,
    /// M7.1d B2a: `mem.store_into` facts emitted, and dests skipped because they
    /// were not a nameable local (or had no memory address).
    pub store_into: u32,
    pub store_into_skipped: u32,
}

impl<'a, 'tcx, Bx: BuilderMethods<'a, 'tcx>> FunctionCx<'a, 'tcx, Bx> {
    /// R1 simple-case loan last uses, computed on the optimized MIR at codegen.
    ///
    /// Borrowck's loan liveness is *not* carried through the MIR passes in R1. Instead a
    /// loan is approximated by the reference local that holds it, restricted to shapes
    /// where the local's last use is the loan's last use:
    /// - `r: &T`/`&mut T`, not an argument, assigned exactly once, by `Rvalue::Ref` of a
    ///   place without a `Deref` projection (a borrow of a local's own memory; so the loan
    ///   cannot be `'static` or a reborrow of a longer-lived reference),
    /// - `r` itself is never borrowed, copied or moved into another value, except as a
    ///   call argument where every argument and the return type are reference-free
    ///   scalars (so the callee cannot store `r` anywhere that outlives the call),
    /// - every other use is a read or write *through* `r` (`(*r)…` in a place that is
    ///   not itself borrowed).
    /// A use is a last use when no other use of `r` is reachable from it in the CFG.
    /// Anything else is not annotated (fail-safe: a missing last use only loses an
    /// invalidation, which is the M5.0 behaviour).
    pub(crate) fn find_last_uses(&self) -> LastUses {
        let mut out = LastUses::default();
        if !self.mem_annotate() {
            return out;
        }
        let tcx = self.cx.tcx();
        let mir = self.mir;
        if mir.basic_blocks.len() > 4000 {
            return out;
        }
        let defs = local_defs(mir);
        let scalar = |ty: Ty<'tcx>| {
            let ty = self.monomorphize(ty);
            matches!(
                ty.kind(),
                ty::Bool | ty::Char | ty::Int(_) | ty::Uint(_) | ty::Float(_) | ty::Never
            ) || ty.is_unit()
        };
        for local in mir.local_decls.indices() {
            let ty = self.monomorphize(mir.local_decls[local].ty);
            if !matches!(ty.kind(), ty::Ref(..)) {
                continue;
            }
            let rdefs = defs.defs.get(&local).map(|v| &v[..]).unwrap_or(&[]);
            if rdefs.iter().any(|rv| matches!(rv, Rvalue::Ref(..))) {
                out.loans_total += 1;
            } else {
                continue;
            }
            if local.index() <= mir.arg_count
                || rdefs.len() != 1
                || defs.borrowed.contains(&local)
                || defs.other_writes.contains(&local)
                || defs.call_defs.contains(&local)
            {
                continue;
            }
            let Rvalue::Ref(_, _, src) = rdefs[0] else { continue };
            if src.projection.iter().any(|e| matches!(e, PlaceElem::Deref)) {
                continue;
            }
            // Collect the uses; bail on any non-simple one.
            let mut uses: Vec<(BasicBlock, UseAt)> = Vec::new();
            let mut ok = true;
            'blocks: for (bb, data) in mir.basic_blocks.iter_enumerated() {
                for (k, stmt) in data.statements.iter().enumerate() {
                    match classify_stmt_use(stmt, local) {
                        StmtUse::None => {}
                        StmtUse::Through => uses.push((bb, UseAt::Stmt(k))),
                        StmtUse::Def => {}
                        StmtUse::Other => {
                            ok = false;
                            break 'blocks;
                        }
                    }
                }
                let term = data.terminator();
                let mut m = Mentions { local, any: false };
                m.visit_terminator(term, mir::Location { block: bb, statement_index: 0 });
                if !m.any {
                    continue;
                }
                match &term.kind {
                    mir::TerminatorKind::Call { func, args, destination, .. } => {
                        let direct = args.iter().any(|a| {
                            matches!(&a.node, Operand::Copy(p) | Operand::Move(p) if p.as_local() == Some(local))
                        });
                        let others_scalar = args.iter().all(|a| {
                            matches!(&a.node, Operand::Copy(p) | Operand::Move(p) if p.as_local() == Some(local))
                                || scalar(a.node.ty(mir, tcx))
                        });
                        let callee_ok = func.const_fn_def().is_some_and(|(d, _)| tcx.intrinsic(d).is_none());
                        let through_dest = destination.local == local;
                        if direct
                            && others_scalar
                            && callee_ok
                            && !through_dest
                            && scalar(destination.ty(mir, tcx).ty)
                        {
                            uses.push((bb, UseAt::Term));
                        } else {
                            ok = false;
                            break 'blocks;
                        }
                    }
                    _ => {
                        ok = false;
                        break 'blocks;
                    }
                }
            }
            if !ok || uses.is_empty() {
                continue;
            }
            out.loans_eligible += 1;
            let id = self.mem_id(local.index()) | (1 << 62);
            let use_set: FxHashSet<(BasicBlock, UseAt)> = uses.iter().copied().collect();
            let use_blocks: FxHashSet<BasicBlock> = uses.iter().map(|&(b, _)| b).collect();
            for &(bb, at) in &uses {
                if !use_reachable_after(mir, &use_set, &use_blocks, bb, at) {
                    out.at.entry((bb, at)).or_default().push((local, id));
                }
            }
        }
        out
    }

    /// Statement codegen hook: tag the last use of the loans ending at this statement.
    pub(crate) fn mem_last_use_stmt(&mut self, bx: &mut Bx, mark: Option<Bx::Value>, bb: BasicBlock, k: usize) {
        let Some(loans) = self.mem_last_uses.at.get(&(bb, UseAt::Stmt(k))) else { return };
        let loans = loans.clone();
        for (local, id) in loans {
            self.mem_stats.last_use_points += 1;
            let ok = match &self.locals[local] {
                LocalRef::Operand(op) => match op.val {
                    OperandValue::Immediate(p) => bx.mem_last_use_since(mark, p, &[id]),
                    _ => false,
                },
                _ => false,
            };
            if ok {
                self.mem_stats.last_use_stmts += 1;
            } else {
                self.mem_stats.last_use_stmts_missed += 1;
                if std::env::var_os("MEM_ANNOT_DEBUG").is_some() {
                    let kind = match &self.locals[local] {
                        LocalRef::Operand(op) => format!("operand {:?}", op.val),
                        LocalRef::Place(_) => "place".to_string(),
                        LocalRef::UnsizedPlace(_) => "unsized".to_string(),
                        LocalRef::PendingOperand => "pending".to_string(),
                    };
                    rustc_middle::ty::print::with_no_trimmed_paths!(eprintln!(
                        "MEM-LU-MISS {} {:?} {:?}[{}]: {:?} ({kind})",
                        self.cx.tcx().symbol_name(self.instance).name,
                        local,
                        bb,
                        k,
                        self.mir[bb].statements[k]
                    ));
                }
            }
        }
    }

    /// [LLM-generated] M5r R3 (D17): a `LoanEnd` marker. The loan's ids wait for the first
    /// memory access or call emitted at or after this point on the same straight-line path
    /// (see `mem_loan_flush` / `mem_loan_block_end`). Placing it later than the real last
    /// use only delays invalidation; it is never placed earlier.
    pub(crate) fn mem_loan_end(&mut self, bx: &mut Bx, id: u64) {
        if !self.mem_annotate() {
            return;
        }
        self.mem_stats.loan_ends += 1;
        let id = self.mem_loan_key(id);
        // Older pending ids found no carrier up to here, so they can share the new mark.
        self.mem_loan_flush(bx);
        let mark = bx.mem_mark();
        match &mut self.mem_loan_pending {
            Some((m, ids)) => {
                *m = mark;
                ids.push(id);
            }
            None => self.mem_loan_pending = Some((mark, vec![id])),
        }
    }

    /// [LLM-generated] R11 (D74): locals assigned exactly once in the body.
    pub(crate) fn mem_single_assigned(&self) -> rustc_data_structures::fx::FxHashSet<mir::Local> {
        let mut n: rustc_data_structures::fx::FxHashMap<mir::Local, u32> = Default::default();
        for bb in self.mir.basic_blocks.iter() {
            for st in &bb.statements {
                if let StatementKind::Assign(a) = &st.kind {
                    *n.entry(a.0.local).or_default() += 1;
                }
            }
            if let mir::TerminatorKind::Call { destination, .. } = &bb.terminator().kind {
                *n.entry(destination.local).or_default() += 1;
            }
        }
        #[allow(rustc::potential_query_instability)] // set membership only, order unused
        let r = n.into_iter().filter(|&(_, c)| c == 1).map(|(l, _)| l).collect();
        r
    }

    /// The `!mem.last_use` / `!mem.loan` id of a MIR loan id (start flag ignored).
    fn mem_loan_key(&self, id: u64) -> u64 {
        use std::hash::{Hash, Hasher};
        let id = id & !CHERI_LOAN_START;
        let mut h = rustc_data_structures::fx::FxHasher::default();
        self.cx.tcx().symbol_name(self.instance).name.hash(&mut h);
        id.hash(&mut h);
        (h.finish() & 0x1fff_ffff_ffff_ffff) | (1 << 61)
    }

    /// [LLM-generated] M5r R5 (D35): a loan start. The next statement, if it is the
    /// loan's `Rvalue::Ref` assigned into memory, tags the store of the reference with
    /// `!mem.loan !{id}` (the slot fact for the late CHERI pass's loan-end invalidation).
    pub(crate) fn mem_loan_start(&mut self, id: u64, scope: mir::SourceScope, link: mir::Local) {
        if !self.mem_annotate() {
            return;
        }
        self.mem_stats.loan_starts += 1;
        if self.cx.tcx().sess.opts.unstable_opts.cheri_slot_start_link {
            // R9 (D65): explicit link; a lost link drops the start (fail-safe).
            if link == mir::Local::MAX {
                self.mem_stats.loan_starts_missed += 1;
                self.mem_stats.loan_link_lost += 1;
            } else {
                self.mem_slot_links.push((self.mem_loan_key(id), link));
            }
            return;
        }
        if self.mem_slot_pending.is_some() {
            self.mem_stats.loan_starts_missed += 1;
        }
        self.mem_slot_pending = Some(self.mem_loan_key(id));
        self.mem_slot_pending_age = 0;
        self.mem_slot_pending_scope = Some(scope);
    }

    /// Statement hook (before codegen): the loans whose reference this statement stores
    /// into memory. A pending loan start waits (within its block) for the next `Ref`
    /// (MIR opts may put e.g. a Box-deref temp in between); a `Ref` or a copy of a
    /// reference local into an SSA (operand) local only records the loans for the local,
    /// so a later copy of it into memory is tagged. A copy with a pending start (a
    /// reborrow that GVN turned into a copy) carries both loans (D35: the slot's region
    /// is contained in each).
    pub(crate) fn mem_slot_before(&mut self, stmt: &mir::Statement<'tcx>) -> Option<Vec<u64>> {
        let StatementKind::Assign(assign) = &stmt.kind else { return None };
        self.mem_slot_pending_age += 1;
        // [LLM-generated] R9 (D65): with the explicit link, the first assignment to a
        // start's linked local pairs with it: a `Ref` (the start's own, or a reborrow of the
        // same loan), or a copy of a reference (D63); anything else drops the start.
        if self.cx.tcx().sess.opts.unstable_opts.cheri_slot_start_link {
            let dest = assign.0.local;
            if let Some(k) = self.mem_slot_links.iter().position(|&(_, l)| l == dest) {
                let (id, _) = self.mem_slot_links.remove(k);
                let pairs = match &assign.1 {
                    Rvalue::Ref(..) => true,
                    Rvalue::Use(Operand::Copy(s) | Operand::Move(s), _) => {
                        s.projection.is_empty()
                            && self.cx.tcx().sess.opts.unstable_opts.cheri_slot_copy_start
                            && self.mir.local_decls[s.local].ty.is_ref()
                    }
                    _ => false,
                };
                if pairs && assign.0.projection.is_empty() {
                    self.mem_slot_pending = Some(id);
                    self.mem_slot_pending_age = 1;
                    self.mem_slot_pending_scope = Some(stmt.source_info.scope);
                    self.mem_stats.loan_link_paired += 1;
                } else {
                    self.mem_stats.loan_starts_missed += 1;
                }
            }
        }
        // [LLM-generated] R8 (D64): a loan start pairs only with the assignment right
        // after it, in the same source scope. With MIR inlining + GVN a start's own `Ref`
        // can be folded away (e.g. `&self.filename` of an inlined `as_ptr`), and the R5
        // "next `Ref` of the block" rule then tagged an unrelated later reference (seen on
        // `Location::file()` in a panic handler: the slot was cleared while `fmt::Arguments`
        // still used it). A start that does not pair is dropped (fail-safe: no clear).
        if self.mem_slot_pending.is_some()
            && self.cx.tcx().sess.opts.unstable_opts.cheri_slot_start_strict
            && !self.cx.tcx().sess.opts.unstable_opts.cheri_slot_start_link
            && (self.mem_slot_pending_age != 1
                || self.mem_slot_pending_scope != Some(stmt.source_info.scope))
        {
            self.mem_slot_pending = None;
            self.mem_stats.loan_starts_missed += 1;
        }
        let (place, rv) = &**assign;
        let in_mem = matches!(self.locals[place.local], LocalRef::Place(_))
            && !place.projection.iter().any(|e| matches!(e, PlaceElem::Deref));
        let ids = match rv {
            Rvalue::Ref(..) => vec![self.mem_slot_pending.take()?],
            Rvalue::Use(Operand::Copy(src) | Operand::Move(src), _) if src.projection.is_empty() => {
                let mut ids = match self.mem_slot_loans.get(&src.local).cloned() {
                    Some(ids) => ids,
                    // [LLM-generated] R8 (D63): a reborrow that GVN turned into a copy of a
                    // reference whose own loan is not known (e.g. it came from an inlined
                    // `Index::index`): the copy right after the loan start (first
                    // assignment since) carries that loan alone.
                    None if self.cx.tcx().sess.opts.unstable_opts.cheri_slot_copy_start
                        && self.mem_slot_pending.is_some()
                        && self.mem_slot_pending_age == 1
                        && self.mir.local_decls[src.local].ty.is_ref() =>
                    {
                        Vec::new()
                    }
                    None => return None,
                };
                if let Some(id) = self.mem_slot_pending.take() {
                    ids.push(id);
                }
                ids
            }
            // [LLM-generated] R10 (D71): `&'a T` held in a struct/enum/closure field.
            // Only one pointer-carrying operand: the other fields are plain data, so a
            // clear at the loan's end cannot hit a field with a longer-lived pointer.
            Rvalue::Aggregate(kind, ops) if self.cx.tcx().sess.opts.unstable_opts.cheri_slot_aggregate => {
                let tcx = self.cx.tcx();
                // [LLM-generated] R11 (D75): an ADT whose only generic parameter is one
                // lifetime is dead once that lifetime ends, and the lifetime lies within
                // every loan of a reference stored in it, so the loans of all its
                // reference operands may clear the whole value.
                if let mir::AggregateKind::Adt(def, ..) = &**kind
                    && let g = tcx.generics_of(*def)
                    && g.parent_count == 0
                    && g.own_params.len() == 1
                    && g.own_counts().lifetimes == 1
                {
                    let mut ids = Vec::new();
                    for o in ops.iter() {
                        if let Operand::Copy(src) | Operand::Move(src) = o
                            && src.projection.is_empty()
                            && let Some(l) = self.mem_slot_loans.get(&src.local)
                        {
                            ids.extend(l.iter().copied());
                        }
                    }
                    if ids.is_empty() {
                        return None;
                    }
                    self.mem_stats.loan_slot_aggregate += 1;
                    ids
                } else {
                let mut ptrish = ops.iter().filter(|o| {
                    let t = self.monomorphize(o.ty(&self.mir.local_decls, tcx));
                    !(t.is_integral() || t.is_floating_point() || t.is_bool() || t.is_char()
                        || t.is_unit())
                });
                let (Some(op), None) = (ptrish.next(), ptrish.next()) else { return None };
                let (Operand::Copy(src) | Operand::Move(src)) = op else { return None };
                if !src.projection.is_empty() || !self.mir.local_decls[src.local].ty.is_ref() {
                    return None;
                }
                let ids = self.mem_slot_loans.get(&src.local).cloned()?;
                self.mem_stats.loan_slot_aggregate += 1;
                ids
                }
            }
            _ => return None,
        };
        if in_mem {
            return Some(ids);
        }
        match place.as_local() {
            Some(l) if !matches!(self.locals[l], LocalRef::Place(_)) => {
                self.mem_stats.loan_slot_ssa += 1;
                self.mem_slot_loans.insert(l, ids);
            }
            _ => self.mem_stats.loan_starts_missed += 1,
        }
        None
    }

    /// Statement hook (after codegen): tag the reference stores emitted since `mark`.
    pub(crate) fn mem_slot_after(
        &mut self,
        bx: &mut Bx,
        stmt: &mir::Statement<'tcx>,
        mark: Option<Bx::Value>,
        ids: Vec<u64>,
    ) {
        let mir::StatementKind::Assign(assign) = &stmt.kind else { return };
        let n = bx.mem_loan_slot_since(mark, &ids);
        if n == 0 {
            self.mem_stats.loan_starts_missed += 1;
            return;
        }
        if matches!(assign.1, mir::Rvalue::Ref(..)) {
            self.mem_stats.loan_slot_stores += n;
        } else {
            self.mem_stats.loan_slot_copies += n;
        }
        if let Some(l) = assign.0.as_local() {
            self.mem_slot_loans.insert(l, ids);
        }
    }

    /// Try to place the pending loan ends on an instruction emitted since their mark.
    pub(crate) fn mem_loan_flush(&mut self, bx: &mut Bx) {
        if let Some((mark, ids)) = &self.mem_loan_pending {
            if bx.mem_last_use_first_since(*mark, ids) {
                self.mem_stats.loan_ends_placed += ids.len() as u32;
                self.mem_loan_pending = None;
            }
        }
    }

    /// Before the terminator: flush; a call/drop terminator carries what is left.
    pub(crate) fn mem_loan_before_term(&mut self, bx: &mut Bx, term: &mir::Terminator<'tcx>) {
        self.mem_loan_flush(bx);
        if matches!(term.kind, mir::TerminatorKind::Call { .. } | mir::TerminatorKind::Drop { .. })
            && let Some((_, ids)) = self.mem_loan_pending.take()
        {
            self.mem_stats.loan_ends_on_call += ids.len() as u32;
            self.mem_call_last_use.get_or_insert_with(Vec::new).extend(ids);
        }
    }

    /// After the terminator: unless the successor is merged into this backend block, the
    /// ids that found no carrier are dropped (counted; at a return the frame dies anyway).
    pub(crate) fn mem_loan_block_end(&mut self, bx: &mut Bx, term: &mir::Terminator<'tcx>, merged: bool) {
        if merged {
            return;
        }
        if let Some((_, ids)) = self.mem_loan_pending.take() {
            use mir::TerminatorKind as T;
            if matches!(term.kind, T::Return) {
                self.mem_stats.loan_ends_at_return += ids.len() as u32;
                // [LLM-generated] R8 (D62): a loan that ends only at the return (its
                // last use is the final call or access of the body) gets the last
                // carrier in this backend block, as D42 does at a goto: nothing between
                // that carrier and the end point touches memory, so the fact is exact.
                if self.cx.tcx().sess.opts.unstable_opts.cheri_loan_end_at_return
                    && bx.mem_last_use_last_in_block(&ids)
                {
                    self.mem_stats.loan_ends_backward += ids.len() as u32;
                }
                return;
            }
            // Carry the ids into each normal (non-unwind) successor whose only MIR
            // predecessor is this block: such a block is entered only from here and is
            // codegen'd after it (RPO), so its first access is still forward of the
            // loan end on every path. Other successors lose the fact (fail-safe).
            let succs: Vec<mir::BasicBlock> = match &term.kind {
                T::Goto { target } | T::Assert { target, .. } => vec![*target],
                T::SwitchInt { targets, .. } => targets.all_targets().to_vec(),
                T::FalseEdge { real_target, .. } | T::FalseUnwind { real_target, .. } => {
                    vec![*real_target]
                }
                _ => vec![],
            };
            let preds = self.mir.basic_blocks.predecessors();
            let mut carried = false;
            for succ in succs {
                if succ != mir::START_BLOCK && preds[succ].len() == 1 {
                    self.mem_loan_carry.entry(succ).or_default().extend(ids.iter().copied());
                    carried = true;
                }
            }
            if carried {
                self.mem_stats.loan_ends_carried += ids.len() as u32;
            } else if matches!(term.kind, T::Goto { .. } | T::SwitchInt { .. })
                && bx.mem_last_use_last_in_block(&ids)
            {
                // R6 (D42): the last carrier before the end point in this block.
                self.mem_stats.loan_ends_backward += ids.len() as u32;
            } else {
                self.mem_stats.loan_ends_missed += ids.len() as u32;
            }
        }
    }

    /// Block start: ids carried from the unique predecessor become pending again.
    pub(crate) fn mem_loan_block_start(&mut self, bb: mir::BasicBlock) {
        if let Some(ids) = self.mem_loan_carry.remove(&bb) {
            match &mut self.mem_loan_pending {
                Some((_, v)) => v.extend(ids),
                None => self.mem_loan_pending = Some((None, ids)),
            }
        }
    }

    /// Terminator codegen hook: the loans whose last use is this call.
    pub(crate) fn mem_last_use_term(&mut self, bb: BasicBlock) {
        if let Some(loans) = self.mem_last_uses.at.get(&(bb, UseAt::Term)) {
            self.mem_stats.last_use_points += loans.len() as u32;
            self.mem_call_last_use = Some(loans.iter().map(|&(_, id)| id).collect());
        }
    }

    pub(crate) fn mem_dump_stats(&self) {
        let Some(path) = std::env::var_os("MEM_ANNOT_STATS_FILE") else { return };
        if !self.mem_annotate() {
            return;
        }
        let s = &self.mem_stats;
        let tcx = self.cx.tcx();
        let line = format!(
            "MEM-STATS\t{}\tsites_matched={}\tsites_unmatched={}\tguards={}\tguards_missed={}\tfrees={}\tloans_total={}\tloans_eligible={}\tlast_use_points={}\tlast_use_stmts={}\tlast_use_stmts_missed={}\tlast_use_calls={}\tloan_ends={}\tloan_ends_placed={}\tloan_ends_on_call={}\tloan_ends_at_return={}\tloan_ends_missed={}\tloan_ends_carried={}\tloan_starts={}\tloan_slot_stores={}\tloan_slot_copies={}\tloan_starts_missed={}\tloan_slot_ssa={}\tloan_ends_inline_call={}\tloan_ends_backward={}\tloan_link_paired={}\tloan_link_lost={}\tloan_slot_aggregate={}\tloan_into={}\tloan_into_skipped={}\tstore_into={}\tstore_into_skipped={}\n",
            tcx.symbol_name(self.instance).name,
            s.sites_matched,
            s.sites_unmatched,
            s.guards,
            s.guards_missed,
            s.frees,
            self.mem_last_uses.loans_total,
            self.mem_last_uses.loans_eligible,
            s.last_use_points,
            s.last_use_stmts,
            s.last_use_stmts_missed,
            s.last_use_calls,
            s.loan_ends,
            s.loan_ends_placed,
            s.loan_ends_on_call,
            s.loan_ends_at_return,
            s.loan_ends_missed,
            s.loan_ends_carried,
            s.loan_starts,
            s.loan_slot_stores,
            s.loan_slot_copies,
            s.loan_starts_missed,
            s.loan_slot_ssa,
            s.loan_ends_inline_call,
            s.loan_ends_backward,
            s.loan_link_paired,
            s.loan_link_lost,
            s.loan_slot_aggregate,
            s.loan_into,
            s.loan_into_skipped,
            s.store_into,
            s.store_into_skipped,
        );
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = f.write_all(line.as_bytes());
        }
    }
}

/// One `mem.loan_into` to emit after a call, or again after the owner header moves.
#[derive(Clone)]
pub(crate) struct LoanIntoEmit {
    /// Raw borrowck loan key (`mem_loan_key` is applied at emission).
    id: u64,
    owner: mir::Local,
    ptr_offset: u64,
    /// Local that holds the reference value, and the field path inside it.
    value_local: mir::Local,
    value_fields: Vec<u16>,
}

#[derive(Default)]
pub(crate) struct LoanIntoPlan {
    by_bb: FxHashMap<BasicBlock, Vec<LoanIntoEmit>>,
    by_owner: FxHashMap<mir::Local, Vec<LoanIntoEmit>>,
}

#[derive(Clone)]
struct Carried {
    id: u64,
    fields: Vec<u16>,
}

impl<'a, 'tcx, Bx: BuilderMethods<'a, 'tcx>> FunctionCx<'a, 'tcx, Bx> {
    /// [LLM-generated] M7.1b: which calls store a loan into a local `Vec`/`Box`.
    ///
    /// The heap write itself is inside `push` / `grow_one`, which stay calls in this
    /// function's MIR. The fact names the owner's buffer-pointer field in this frame.
    /// A second loan of the same place that outlives this one, or a reference with no
    /// loan id, suppresses the fact (the late pass nulls every slot equal to the value).
    pub(crate) fn prepare_loan_into(&mut self) {
        if !self.mem_annotate() {
            return;
        }
        let Some(def) = self.mir.source.def_id().as_local() else { return };
        let overlap = self.cx.tcx().cheri_loan_overlap.lock().remove(&def).unwrap_or_default();
        if overlap.iter().any(|&(a, b)| a == u64::MAX && b == u64::MAX) {
            self.mem_stats.loan_into_skipped += 1;
            return;
        }
        let mut longer_of: FxHashMap<u64, Vec<u64>> = FxHashMap::default();
        for (shorter, longer) in overlap {
            longer_of.entry(shorter).or_default().push(longer);
        }

        let mir = self.mir;
        let mut carries: FxHashMap<mir::Local, Vec<Carried>> = Default::default();
        let mut unknown: FxHashSet<mir::Local> = Default::default();
        let mut ref_of: FxHashMap<mir::Local, mir::Place<'tcx>> = Default::default();
        // A local that holds the buffer pointer (or a slice/ref of it) of an owner.
        let mut buf_owner: FxHashMap<mir::Local, mir::Local> = Default::default();
        // bb, owner, ptr-field offset, local that holds the reference, loan.
        let mut direct: Vec<(BasicBlock, mir::Local, u64, mir::Local, Carried)> = Vec::new();
        let mut tainted: FxHashSet<mir::Local> = Default::default();
        let mut propagate: Vec<(BasicBlock, mir::Local, mir::Local, u64)> = Vec::new();
        for (bb, data) in mir.basic_blocks.iter_enumerated() {
            // Only loans started in this block. Copy-prop parks every reborrow of an
            // argument on that argument, so a later push must not repeat earlier ids.
            carries.clear();
            for stmt in &data.statements {
                // Copy-prop retargets a loan onto the argument it collapsed into, which is
                // not the destination of a `Ref`. The link local still carries the id.
                if let StatementKind::Intrinsic(mir::NonDivergingIntrinsic::LoanStart(id, link)) =
                    stmt.kind
                    && link != mir::Local::MAX
                {
                    let slot = carries.entry(link).or_default();
                    if !slot.iter().any(|c| c.id == id && c.fields.is_empty()) {
                        slot.push(Carried { id, fields: Vec::new() });
                    }
                    continue;
                }
                let StatementKind::Assign(assign) = &stmt.kind else { continue };
                let (place, rv) = &**assign;
                if place.projection.is_empty() {
                    self.note_buf_owner(rv, place.local, &mut buf_owner);
                }
                if !place.projection.is_empty() {
                    // A struct literal is often lowered to `_s.field = copy r`.
                    if let Rvalue::Use(Operand::Copy(src) | Operand::Move(src), _) = rv
                        && src.projection.is_empty()
                        && place.projection.iter().all(|e| matches!(e, PlaceElem::Field(..)))
                        && let Some(cs) = carries.get(&src.local).cloned()
                    {
                        let prefix: Vec<u16> = place
                            .projection
                            .iter()
                            .map(|e| match e {
                                PlaceElem::Field(f, _) => f.as_usize() as u16,
                                _ => 0,
                            })
                            .collect();
                        let slot = carries.entry(place.local).or_default();
                        for c in cs {
                            let mut fields = prefix.clone();
                            fields.extend_from_slice(&c.fields);
                            if !slot.iter().any(|e| e.id == c.id && e.fields == fields) {
                                slot.push(Carried { id: c.id, fields });
                            }
                        }
                    }
                    continue;
                }
                match rv {
                    Rvalue::Ref(_, _, borrowed) => {
                        if let Some(owner) = self.ref_buf_owner(*borrowed, &buf_owner) {
                            ref_of.insert(place.local, mir::Place::from(owner));
                        } else {
                            ref_of.insert(place.local, *borrowed);
                        }
                    }
                    Rvalue::Use(Operand::Copy(src) | Operand::Move(src), _)
                        if src.projection.is_empty() =>
                    {
                        if let Some(p) = ref_of.get(&src.local).copied() {
                            ref_of.insert(place.local, p);
                        }
                        if let Some(c) = carries.get(&src.local).cloned() {
                            carries.insert(place.local, c);
                        } else if unknown.contains(&src.local) {
                            unknown.insert(place.local);
                        }
                    }
                    Rvalue::Cast(_, Operand::Copy(src) | Operand::Move(src), _)
                        if src.projection.is_empty() =>
                    {
                        if let Some(p) = ref_of.get(&src.local).copied() {
                            ref_of.insert(place.local, p);
                        }
                        if let Some(c) = carries.get(&src.local).cloned() {
                            carries.insert(place.local, c);
                        } else if unknown.contains(&src.local) {
                            unknown.insert(place.local);
                        }
                    }
                    Rvalue::Aggregate(_, ops) => {
                        let mut acc = Vec::new();
                        let mut bad = false;
                        for (i, op) in ops.iter().enumerate() {
                            let p = match op {
                                Operand::Copy(p) | Operand::Move(p) => *p,
                                other => {
                                    if self.operand_has_ref(other) {
                                        bad = true;
                                    }
                                    continue;
                                }
                            };
                            if !p.projection.is_empty() {
                                if self.place_has_ref(p) {
                                    bad = true;
                                }
                                continue;
                            }
                            if let Some(cs) = carries.get(&p.local) {
                                for c in cs {
                                    let mut fields = vec![i as u16];
                                    fields.extend_from_slice(&c.fields);
                                    acc.push(Carried { id: c.id, fields });
                                }
                            } else if unknown.contains(&p.local) || self.local_has_ref(p.local) {
                                bad = true;
                            }
                        }
                        carries.insert(place.local, acc);
                        if bad {
                            unknown.insert(place.local);
                        }
                    }
                    _ => {}
                }
            }
            let mir::TerminatorKind::Call { func, args, destination, .. } =
                &data.terminator().kind
            else {
                continue;
            };
            self.note_loan_deref(func, args, *destination, &mut ref_of);
            let Some(mode) = self.loan_into_call_mode(func) else { continue };
            let Some(recv) = args.first().and_then(|a| operand_local(&a.node)) else { continue };
            let Some(owner) = self.loan_into_owner(recv, &ref_of) else { continue };
            let offset = self.loan_into_ptr_offset(mir.local_decls[owner].ty);
            let Some(offset) = offset else { continue };
            match mode {
                LoanIntoMode::Store => {
                    for arg in args.iter().skip(1) {
                        let Some(local) = operand_local(&arg.node) else {
                            // A reference we cannot name may alias a loan value.
                            // A struct that merely contains one does not: its loans were
                            // recorded when it was built, and tainting here would drop them.
                            if self.monomorphize(arg.node.ty(&mir.local_decls, self.cx.tcx())).is_ref()
                            {
                                tainted.insert(owner);
                            }
                            continue;
                        };
                        if unknown.contains(&local) {
                            tainted.insert(owner);
                        }
                        match carries.get(&local) {
                            Some(cs) if !cs.is_empty() => {
                                for c in cs {
                                    direct.push((
                                        bb,
                                        owner,
                                        offset,
                                        local,
                                        Carried { id: c.id, fields: c.fields.clone() },
                                    ));
                                }
                            }
                            _ if self.monomorphize(mir.local_decls[local].ty).is_ref() => {
                                tainted.insert(owner);
                            }
                            _ => {}
                        }
                    }
                }
                LoanIntoMode::Propagate => {
                    let Some(src_op) = args.get(1).and_then(|a| operand_local(&a.node)) else {
                        continue;
                    };
                    let Some(src_owner) = self.loan_into_owner(src_op, &ref_of) else {
                        continue;
                    };
                    propagate.push((bb, owner, src_owner, offset));
                }
            }
        }

        self.finish_loan_into(direct, propagate, tainted, longer_of);
    }
}

enum LoanIntoMode {
    Store,
    Propagate,
}

impl<'a, 'tcx, Bx: BuilderMethods<'a, 'tcx>> FunctionCx<'a, 'tcx, Bx> {
    fn finish_loan_into(
        &mut self,
        direct: Vec<(BasicBlock, mir::Local, u64, mir::Local, Carried)>,
        propagate: Vec<(BasicBlock, mir::Local, mir::Local, u64)>,
        tainted: FxHashSet<mir::Local>,
        longer_of: FxHashMap<u64, Vec<u64>>,
    ) {
        // bb, owner, offset, value local, carried loan.
        let mut flows = direct;
        for (bb, dest, src, offset) in propagate {
            let copied: Vec<(mir::Local, Carried)> = flows
                .iter()
                .filter(|f| f.1 == src)
                .map(|f| (f.3, Carried { id: f.4.id, fields: f.4.fields.clone() }))
                .collect();
            let mut seen = FxHashSet::default();
            for (value_local, c) in copied {
                if !seen.insert(c.id) {
                    continue;
                }
                flows.push((bb, dest, offset, value_local, c));
            }
        }
        let owners_ids: FxHashSet<(mir::Local, u64)> =
            flows.iter().map(|f| (f.1, f.4.id)).collect();
        for (bb, owner, offset, value_local, carried) in flows {
            let longer_also = longer_of.get(&carried.id).is_some_and(|longers| {
                longers.iter().any(|longer| owners_ids.contains(&(owner, *longer)))
            });
            if tainted.contains(&owner) || longer_also {
                self.mem_stats.loan_into_skipped += 1;
                continue;
            }
            let emit = LoanIntoEmit {
                id: carried.id,
                owner,
                ptr_offset: offset,
                value_local,
                value_fields: carried.fields,
            };
            self.mem_loan_into.by_bb.entry(bb).or_default().push(emit.clone());
            self.mem_loan_into.by_owner.entry(owner).or_default().push(emit);
        }
    }

    /// A copied `Vec`/`Box` buffer pointer, or a cast/slice built from one.
    fn note_buf_owner(
        &self,
        rv: &Rvalue<'tcx>,
        dest: mir::Local,
        buf_owner: &mut FxHashMap<mir::Local, mir::Local>,
    ) {
        let owner = match rv {
            Rvalue::Use(Operand::Copy(p) | Operand::Move(p), _)
            | Rvalue::Cast(_, Operand::Copy(p) | Operand::Move(p), _) => {
                self.buf_owner_of(*p, buf_owner)
            }
            Rvalue::Aggregate(_, ops) => ops.iter().find_map(|op| match op {
                Operand::Copy(p) | Operand::Move(p) => self.buf_owner_of(*p, buf_owner),
                _ => None,
            }),
            _ => None,
        };
        if let Some(owner) = owner {
            buf_owner.insert(dest, owner);
        }
    }

    /// `place` is the buffer pointer of an owner, or a local already known to be one.
    fn buf_owner_of(
        &self,
        place: mir::Place<'tcx>,
        buf_owner: &FxHashMap<mir::Local, mir::Local>,
    ) -> Option<mir::Local> {
        if place.projection.is_empty() {
            return buf_owner.get(&place.local).copied();
        }
        if place.projection.iter().all(|e| matches!(e, PlaceElem::Field(..)))
            && self.is_loan_owner(self.mir.local_decls[place.local].ty)
        {
            return Some(place.local);
        }
        None
    }

    /// `&*slot` / `&slot.field` where `slot` is an owner's buffer.
    fn ref_buf_owner(
        &self,
        borrowed: mir::Place<'tcx>,
        buf_owner: &FxHashMap<mir::Local, mir::Local>,
    ) -> Option<mir::Local> {
        if borrowed.projection.len() == 1 && matches!(borrowed.projection[0], PlaceElem::Deref) {
            if let Some(owner) = buf_owner.get(&borrowed.local) {
                return Some(*owner);
            }
        }
        self.buf_owner_of(borrowed, buf_owner)
    }

    /// `deref` / `deref_mut` of a `Vec` or `Box` yields the slice; the slice local
    /// stands for that owner so `copy_from_slice` can propagate.
    fn note_loan_deref(
        &self,
        func: &mir::Operand<'tcx>,
        args: &[rustc_span::Spanned<Operand<'tcx>>],
        destination: mir::Place<'tcx>,
        ref_of: &mut FxHashMap<mir::Local, mir::Place<'tcx>>,
    ) {
        let Some(dest) = destination.as_local() else { return };
        let Some((def_id, _)) = func.const_fn_def() else { return };
        let name = self.cx.tcx().item_name(def_id);
        if name.as_str() != "deref" && name.as_str() != "deref_mut" {
            return;
        }
        let Some(src) = args.first().and_then(|a| operand_local(&a.node)) else { return };
        if self.loan_into_owner(src, ref_of).is_none() {
            return;
        }
        if let Some(place) = ref_of.get(&src).copied() {
            ref_of.insert(dest, place);
        }
    }

    fn loan_into_call_mode(&self, func: &mir::Operand<'tcx>) -> Option<LoanIntoMode> {
        let (def_id, _) = func.const_fn_def()?;
        let name = self.cx.tcx().item_name(def_id);
        match name.as_str() {
            "push" | "push_mut" => Some(LoanIntoMode::Store),
            // `-O2` inlines `copy_from_slice` / `clone_from_slice` to the impl, and
            // `Vec::clone_from` to `clone_into`. The slice args are still (dest, src).
            "copy_from_slice" | "copy_from_slice_impl" | "clone_from_slice" | "spec_clone_from"
            | "extend_from_slice" | "clone_from" | "clone_into" => Some(LoanIntoMode::Propagate),
            _ => None,
        }
    }

    fn loan_into_owner(
        &self,
        recv: mir::Local,
        ref_of: &FxHashMap<mir::Local, mir::Place<'tcx>>,
    ) -> Option<mir::Local> {
        let mut local = recv;
        for _ in 0..6 {
            let place = *ref_of.get(&local)?;
            let chased = place.projection.len() == 1
                && matches!(place.projection[0], PlaceElem::Deref);
            if chased {
                local = place.local;
                continue;
            }
            if place.projection.iter().any(|e| {
                matches!(
                    e,
                    PlaceElem::Deref
                        | PlaceElem::Index(_)
                        | PlaceElem::Subslice { .. }
                        | PlaceElem::ConstantIndex { .. }
                        | PlaceElem::Downcast(..)
                )
            }) {
                return None;
            }
            if !self.is_loan_owner(self.mir.local_decls[place.local].ty) {
                return None;
            }
            return Some(place.local);
        }
        None
    }

    fn is_loan_owner(&self, ty: Ty<'tcx>) -> bool {
        let ty = self.monomorphize(ty);
        let inner = if ty.is_box() {
            ty.boxed_ty()
        } else if let ty::Adt(def, args) = ty.kind()
            && self.cx.tcx().is_diagnostic_item(rustc_span::sym::Vec, def.did())
        {
            args.types().next()
        } else {
            None
        };
        inner.is_some_and(|t| ty_has_ref(self.cx.tcx(), self.monomorphize(t)))
    }

    fn loan_into_ptr_offset(&self, ty: Ty<'tcx>) -> Option<u64> {
        let mut layout = self.cx.layout_of(self.monomorphize(ty));
        let mut base = 0u64;
        for _ in 0..8 {
            if let rustc_abi::BackendRepr::Scalar(scalar) = layout.backend_repr
                && matches!(scalar.primitive(), rustc_abi::Primitive::Pointer(_))
            {
                return Some(base);
            }
            let mut next = None;
            for i in 0..layout.fields.count() {
                let field = layout.field(self.cx, i);
                if field.is_zst() {
                    continue;
                }
                let off = layout.fields.offset(i).bytes();
                next = Some((field, base + off));
                break;
            }
            let (field, off) = next?;
            layout = field;
            base = off;
        }
        None
    }

    fn local_has_ref(&self, local: mir::Local) -> bool {
        ty_has_ref(self.cx.tcx(), self.monomorphize(self.mir.local_decls[local].ty))
    }

    fn operand_has_ref(&self, op: &Operand<'tcx>) -> bool {
        ty_has_ref(self.cx.tcx(), self.monomorphize(op.ty(&self.mir.local_decls, self.cx.tcx())))
    }

    fn place_has_ref(&self, place: mir::Place<'tcx>) -> bool {
        let ty = place.ty(&self.mir.local_decls, self.cx.tcx()).ty;
        ty_has_ref(self.cx.tcx(), self.monomorphize(ty))
    }

    /// Load the reference values for this call's facts before arguments are consumed.
    pub(crate) fn mem_loan_into_preload(&mut self, bx: &mut Bx, bb: BasicBlock) {
        self.mem_loan_into_ready.clear();
        let Some(facts) = self.mem_loan_into.by_bb.get(&bb).cloned() else { return };
        for fact in facts {
            let Some(ptr) = self.loan_into_owner_ptr(bx, fact.owner, fact.ptr_offset) else {
                self.mem_stats.loan_into_skipped += 1;
                continue;
            };
            // A propagated fact reuses the SSA captured when the source loan was
            // emitted. Reloading the local can see a later store into the same slot.
            let val = if let Some(saved) = self.saved_loan_value(fact.id, bb) {
                saved
            } else {
                let Some(loaded) =
                    self.loan_into_value(bx, fact.value_local, &fact.value_fields, bb)
                else {
                    self.mem_stats.loan_into_skipped += 1;
                    continue;
                };
                loaded
            };
            self.mem_loan_into_saved.push((fact.id, val, bb));
            self.mem_loan_into_ready.push((ptr, val, fact.id));
        }
    }

    pub(crate) fn mem_loan_into_flush(&mut self, bx: &mut Bx) {
        let ready = std::mem::take(&mut self.mem_loan_into_ready);
        for (ptr, val, raw_id) in ready {
            let id = self.mem_loan_key(raw_id);
            bx.mem_fact("mem.loan_into", &[ptr, val, bx.const_u64(id)]);
            self.mem_stats.loan_into += 1;
        }
    }

    /// The owner header was copied to another local. Point the same loans at the
    /// destination's buffer-pointer field. No saved value means no fact.
    pub(crate) fn mem_loan_into_after_assign(
        &mut self,
        bx: &mut Bx,
        place: &mir::Place<'tcx>,
        rvalue: &Rvalue<'tcx>,
    ) {
        let Rvalue::Use(Operand::Copy(src) | Operand::Move(src), _) = rvalue else { return };
        if !src.projection.is_empty() || !place.projection.is_empty() {
            return;
        }
        let Some(dest) = place.as_local() else { return };
        if dest == src.local {
            return;
        }
        let Some(facts) = self.mem_loan_into.by_owner.get(&src.local).cloned() else { return };
        if !matches!(self.locals[dest], LocalRef::Place(_)) {
            return;
        }
        let bb = self.mem_codegen_bb;
        let dom = self.mir.basic_blocks.dominators();
        let mut added = Vec::new();
        for fact in facts {
            let Some((_, val, def_bb)) =
                self.mem_loan_into_saved.iter().rev().find(|s| s.0 == fact.id)
            else {
                self.mem_stats.loan_into_skipped += 1;
                continue;
            };
            if !dom.dominates(*def_bb, bb) {
                self.mem_stats.loan_into_skipped += 1;
                continue;
            }
            let Some(ptr) = self.loan_into_owner_ptr(bx, dest, fact.ptr_offset) else {
                self.mem_stats.loan_into_skipped += 1;
                continue;
            };
            bx.mem_fact("mem.loan_into", &[ptr, *val, bx.const_u64(self.mem_loan_key(fact.id))]);
            self.mem_stats.loan_into += 1;
            let mut moved = fact.clone();
            moved.owner = dest;
            added.push(moved);
        }
        if !added.is_empty() {
            self.mem_loan_into.by_owner.entry(dest).or_default().extend(added);
        }
    }

    fn saved_loan_value(&self, id: u64, bb: BasicBlock) -> Option<Bx::Value> {
        let dom = self.mir.basic_blocks.dominators();
        self.mem_loan_into_saved
            .iter()
            .rev()
            .find(|saved| saved.0 == id && dom.dominates(saved.2, bb))
            .map(|saved| saved.1)
    }

    fn loan_into_owner_ptr(
        &self,
        bx: &mut Bx,
        owner: mir::Local,
        offset: u64,
    ) -> Option<Bx::Value> {
        let LocalRef::Place(place) = self.locals[owner] else { return None };
        let ptr = if offset == 0 {
            place.val.llval
        } else {
            bx.inbounds_ptradd(place.val.llval, bx.const_usize(offset))
        };
        Some(ptr)
    }

    fn loan_into_value(
        &mut self,
        bx: &mut Bx,
        local: mir::Local,
        fields: &[u16],
        bb: BasicBlock,
    ) -> Option<Bx::Value> {
        let _ = bb;
        match self.locals[local] {
            LocalRef::Operand(op) => operand_field(bx, op.val, fields),
            LocalRef::Place(place) => {
                let mut pref = place;
                for &f in fields {
                    pref = pref.project_field(bx, f as usize);
                }
                let ty = bx.backend_type(pref.layout);
                Some(bx.load(ty, pref.val.llval, pref.val.align))
            }
            LocalRef::PendingOperand | LocalRef::UnsizedPlace(_) => None,
        }
    }
    /// M7.1d B2a: after a store of a capability into a nameable local dest,
    /// emit `mem.store_into`(dest, cap). The late pass decides whether to null
    /// it at the source `mem.free`. rustc does not clear.
    pub(crate) fn mem_store_into_after_assign(
        &mut self,
        bx: &mut Bx,
        place: &mir::Place<'tcx>,
        rvalue: &Rvalue<'tcx>,
    ) {
        if !self.mem_annotate() || !self.cx.tcx().sess.opts.unstable_opts.cheri_store_into {
            return;
        }
        // Whole-struct / tuple aggregate: fact each capability field of a
        // nameable local. Enums, unions, closures and raw-ptr aggregates are
        // skipped: projecting fields on an enum layout ICEs (tag is field 0).
        if let Rvalue::Aggregate(kind, fields) = rvalue {
            self.mem_store_into_aggregate(bx, place, kind, fields);
            return;
        }
        let ty = self.monomorphize(place.ty(&self.mir.local_decls, bx.tcx()).ty);
        if !is_store_into_cap_ty(ty) {
            return;
        }
        if !self.store_into_dest_ok(*place) {
            self.mem_stats.store_into_skipped += 1;
            return;
        }
        let Some(dest) = self.store_into_dest_ptr(bx, *place) else {
            self.mem_stats.store_into_skipped += 1;
            return;
        };
        let Some(cap) = self.store_into_cap_val(bx, rvalue) else {
            self.mem_stats.store_into_skipped += 1;
            return;
        };
        bx.mem_fact("mem.store_into", &[dest, cap]);
        self.mem_stats.store_into += 1;
    }

    fn mem_store_into_aggregate(
        &mut self,
        bx: &mut Bx,
        place: &mir::Place<'tcx>,
        kind: &mir::AggregateKind<'tcx>,
        fields: &rustc_index::IndexVec<rustc_abi::FieldIdx, mir::Operand<'tcx>>,
    ) {
        let kind_ok = match kind {
            mir::AggregateKind::Adt(def, ..) if self.cx.tcx().adt_def(*def).is_struct() => true,
            mir::AggregateKind::Tuple => true,
            _ => false,
        };
        if !kind_ok {
            return;
        }
        if !self.store_into_dest_ok(*place) {
            self.mem_stats.store_into_skipped += 1;
            return;
        }
        if !matches!(self.locals[place.local], LocalRef::Place(_) | LocalRef::UnsizedPlace(_))
            && !place.is_indirect()
        {
            self.mem_stats.store_into_skipped += 1;
            return;
        }
        let dest_pr = self.codegen_place(bx, place.as_ref());
        let nfields = dest_pr.layout.fields.count();
        if nfields == 0 {
            self.mem_stats.store_into_skipped += 1;
            return;
        }
        let mut emitted = false;
        for (i, op) in fields.iter_enumerated() {
            if i.as_usize() >= nfields {
                continue;
            }
            let flayout = dest_pr.layout.field(bx.cx(), i.as_usize());
            if !is_store_into_cap_ty(flayout.ty) {
                continue;
            }
            let field_pr = dest_pr.project_field(bx, i.as_usize());
            let cap_op = self.codegen_operand(bx, op);
            let Some(cap) = self.store_into_operand_cap(cap_op) else { continue };
            bx.mem_fact("mem.store_into", &[field_pr.val.llval, cap]);
            self.mem_stats.store_into += 1;
            emitted = true;
        }
        if !emitted {
            self.mem_stats.store_into_skipped += 1;
        }
    }

    /// Local, field, or constant-index field of a non-`Box` local; or a store
    /// through a pointer whose MIR origin is such a place (a GEP dest of a
    /// local `Vec` / field address). Not a static, not a `Box` heap, not an
    /// unknown / argument raw pointer.
    fn store_into_dest_ok(&self, place: mir::Place<'tcx>) -> bool {
        let local_ty = self.monomorphize(self.mir.local_decls[place.local].ty);
        if local_ty.is_box() {
            return false;
        }
        let mut ty = local_ty;
        let mut saw_deref = false;
        for elem in place.projection.iter() {
            match elem {
                mir::ProjectionElem::Deref => {
                    if ty.is_box() {
                        return false;
                    }
                    saw_deref = true;
                    match ty.builtin_deref(true) {
                        Some(inner) => ty = inner,
                        None => return false,
                    }
                }
                mir::ProjectionElem::Field(f, _) => {
                    let tcx = self.cx.tcx();
                    ty = match ty.kind() {
                        ty::Adt(def, args) => {
                            def.all_fields().nth(f.index()).map(|fd| fd.ty(tcx, args).skip_norm_wip()).unwrap_or(ty)
                        }
                        ty::Tuple(ts) => ts.get(f.index()).copied().unwrap_or(ty),
                        _ => ty,
                    };
                    ty = self.monomorphize(ty);
                    if ty.is_box() {
                        return false;
                    }
                }
                mir::ProjectionElem::Index(_)
                | mir::ProjectionElem::ConstantIndex { .. }
                | mir::ProjectionElem::Subslice { .. } => {
                    ty = ty.builtin_index().unwrap_or(ty);
                    ty = self.monomorphize(ty);
                }
                mir::ProjectionElem::Downcast(_, _)
                | mir::ProjectionElem::UnwrapUnsafeBinder(_) => {}
                mir::ProjectionElem::OpaqueCast(_)
                | mir::ProjectionElem::PhantomDeref => return false,
            }
        }
        if saw_deref {
            // Deref of a field/index of this local (a Vec buffer, etc.) is a
            // nameable container projection. Deref of a pointer local traces
            // that pointer's origin (ptr::write through &mut local.field).
            if let Some(first) = place.projection.first() {
                if !matches!(first, mir::ProjectionElem::Deref) {
                    return true;
                }
            }
            return self.store_into_ptr_origin_ok(place.local, &mut Default::default());
        }
        true
    }

    fn store_into_ptr_origin_ok(
        &self,
        local: mir::Local,
        seen: &mut rustc_data_structures::fx::FxHashSet<mir::Local>,
    ) -> bool {
        if !seen.insert(local) {
            return false;
        }
        if local.index() <= self.mir.arg_count {
            return false;
        }
        let ty = self.monomorphize(self.mir.local_decls[local].ty);
        if ty.is_box() {
            return false;
        }
        let mut found = false;
        for bb in self.mir.basic_blocks.iter() {
            for stmt in &bb.statements {
                let mir::StatementKind::Assign(assign) = &stmt.kind else { continue };
                if assign.0.as_local() != Some(local) {
                    continue;
                }
                if !self.store_into_rvalue_origin_ok(&assign.1, seen) {
                    return false;
                }
                found = true;
            }
        }
        found
    }

    fn store_into_rvalue_origin_ok(
        &self,
        rvalue: &Rvalue<'tcx>,
        seen: &mut rustc_data_structures::fx::FxHashSet<mir::Local>,
    ) -> bool {
        match rvalue {
            Rvalue::Use(op, _) | Rvalue::Cast(_, op, _) => self.store_into_operand_origin_ok(op, seen),
            Rvalue::Ref(_, _, p) | Rvalue::RawPtr(_, p) => self.store_into_place_origin_ok(*p, seen),
            Rvalue::BinaryOp(op, operands)
                if matches!(
                    *op,
                    mir::BinOp::Offset | mir::BinOp::Add | mir::BinOp::AddUnchecked
                ) =>
            {
                self.store_into_operand_origin_ok(&operands.0, seen)
            }
            Rvalue::ThreadLocalRef(_) => false,
            _ => false,
        }
    }

    fn store_into_operand_origin_ok(
        &self,
        op: &mir::Operand<'tcx>,
        seen: &mut rustc_data_structures::fx::FxHashSet<mir::Local>,
    ) -> bool {
        match op {
            mir::Operand::Copy(p) | mir::Operand::Move(p) => {
                self.store_into_place_origin_ok(*p, seen)
            }
            mir::Operand::Constant(_) | mir::Operand::RuntimeChecks(_) => false,
        }
    }

    fn store_into_place_origin_ok(
        &self,
        p: mir::Place<'tcx>,
        seen: &mut rustc_data_structures::fx::FxHashSet<mir::Local>,
    ) -> bool {
        let ty = self.monomorphize(self.mir.local_decls[p.local].ty);
        if ty.is_box() {
            return false;
        }
        if p.projection.is_empty() {
            // &mut local / &raw mut local names this local's memory, even when
            // the local itself has pointer type (a ptr::write dest slot).
            if p.local.index() <= self.mir.arg_count {
                return false;
            }
            return true;
        }
        if matches!(p.projection.first(), Some(mir::ProjectionElem::Deref)) {
            if ty.is_box() {
                return false;
            }
            return self.store_into_ptr_origin_ok(p.local, seen);
        }
        // Field / index of a non-box local (Vec buffer, struct field).
        true
    }

    fn store_into_dest_ptr(
        &mut self,
        bx: &mut Bx,
        place: mir::Place<'tcx>,
    ) -> Option<Bx::Value> {
        // An SSA-only local has no ready memory address; do not invent one.
        if place.projection.is_empty() {
            match self.locals[place.local] {
                LocalRef::Place(_) | LocalRef::UnsizedPlace(_) => {}
                LocalRef::Operand(_) | LocalRef::PendingOperand => return None,
            }
        }
        let pr = self.codegen_place(bx, place.as_ref());
        Some(pr.val.llval)
    }

    fn store_into_cap_val(&mut self, bx: &mut Bx, rvalue: &Rvalue<'tcx>) -> Option<Bx::Value> {
        match rvalue {
            Rvalue::Use(op, _) | Rvalue::Cast(_, op, _) => {
                let cap_op = self.codegen_operand(bx, op);
                self.store_into_operand_cap(cap_op)
            }
            Rvalue::Ref(_, _, p) | Rvalue::RawPtr(_, p) => {
                let pr = self.codegen_place(bx, p.as_ref());
                Some(pr.val.llval)
            }
            _ => None,
        }
    }

    fn store_into_operand_cap(
        &self,
        op: crate::mir::operand::OperandRef<'tcx, Bx::Value>,
    ) -> Option<Bx::Value> {
        match op.val {
            crate::mir::operand::OperandValue::Immediate(v) => Some(v),
            crate::mir::operand::OperandValue::Pair(a, _) => Some(a),
            crate::mir::operand::OperandValue::ZeroSized => None,
            crate::mir::operand::OperandValue::Ref(place) => Some(place.llval),
        }
    }

    fn is_ptr_write_fn(&self, instance: ty::Instance<'tcx>) -> bool {
        let s = self.cx.tcx().def_path_str(instance.def_id());
        let name = s.rsplit("::").next().unwrap_or("");
        matches!(name, "write" | "write_unaligned" | "write_volatile") && s.contains("::ptr::")
    }

    fn is_ptr_read_fn(&self, instance: ty::Instance<'tcx>) -> bool {
        let s = self.cx.tcx().def_path_str(instance.def_id());
        let name = s.rsplit("::").next().unwrap_or("");
        matches!(name, "read" | "read_unaligned" | "read_volatile") && s.contains("::ptr::")
    }

    /// After a call: fact a cap return stored into a nameable local (ptr::read),
    /// or a ptr::write into a nameable dest pointer.
    pub(crate) fn mem_store_into_on_call(
        &mut self,
        bx: &mut Bx,
        instance: Option<ty::Instance<'tcx>>,
        llargs: &[Bx::Value],
        llret: Option<Bx::Value>,
    ) {
        if !self.mem_annotate() || !self.cx.tcx().sess.opts.unstable_opts.cheri_store_into {
            self.mem_store_into_ret = None;
            self.mem_store_into_arg0 = None;
            return;
        }
        if let Some(inst) = instance
            && self.is_ptr_write_fn(inst)
            && llargs.len() >= 2
        {
            if let Some(arg0) = self.mem_store_into_arg0.take() {
                let dest_ok = match arg0 {
                    mir::Operand::Copy(p) | mir::Operand::Move(p) => {
                        // The dest pointer should originate at a nameable local.
                        let mut deref = p;
                        if !p.projection.iter().any(|e| matches!(e, mir::ProjectionElem::Deref)) {
                            deref = p.project_deeper(&[mir::ProjectionElem::Deref], self.cx.tcx());
                        }
                        self.store_into_dest_ok(p) || self.store_into_dest_ok(deref)
                            || self.store_into_place_origin_ok(p, &mut Default::default())
                    }
                    mir::Operand::Constant(_) | mir::Operand::RuntimeChecks(_) => false,
                };
                if dest_ok {
                    bx.mem_fact("mem.store_into", &[llargs[0], llargs[1]]);
                    self.mem_stats.store_into += 1;
                } else {
                    self.mem_stats.store_into_skipped += 1;
                }
            }
        } else {
            self.mem_store_into_arg0 = None;
        }
        if let (Some(place), Some(cap), Some(inst)) =
            (self.mem_store_into_ret.take(), llret, instance)
        {
            if self.is_ptr_read_fn(inst) {
                let ty = self.monomorphize(place.ty(&self.mir.local_decls, bx.tcx()).ty);
                if is_store_into_cap_ty(ty) && self.store_into_dest_ok(place) {
                    if let Some(dest) = self.store_into_dest_ptr(bx, place) {
                        bx.mem_fact("mem.store_into", &[dest, cap]);
                        self.mem_stats.store_into += 1;
                    } else {
                        self.mem_stats.store_into_skipped += 1;
                    }
                }
            }
        } else {
            self.mem_store_into_ret = None;
        }
    }
}

fn is_store_into_cap_ty(ty: Ty<'_>) -> bool {
    match ty.kind() {
        ty::Ref(..) => true,
        ty::RawPtr(_, _) if !ty.is_fn_ptr() => true,
        _ => false,
    }
}



fn operand_local(op: &Operand<'_>) -> Option<mir::Local> {
    match op {
        Operand::Copy(p) | Operand::Move(p) if p.projection.is_empty() => Some(p.local),
        _ => None,
    }
}

fn ty_has_ref<'tcx>(tcx: TyCtxt<'tcx>, ty: Ty<'tcx>) -> bool {
    fn go<'tcx>(tcx: TyCtxt<'tcx>, ty: Ty<'tcx>, depth: u32) -> bool {
        if depth == 0 {
            return false;
        }
        if ty.is_ref() {
            return true;
        }
        match *ty.kind() {
            ty::Adt(def, args) => {
                def.all_fields().any(|f| go(tcx, f.ty(tcx, args).skip_norm_wip(), depth - 1))
            }
            ty::Tuple(ts) => ts.iter().any(|t| go(tcx, t, depth - 1)),
            ty::Array(t, _) | ty::Slice(t) => go(tcx, t, depth - 1),
            _ => false,
        }
    }
    go(tcx, ty, 6)
}

fn operand_field<'a, 'tcx, Bx: BuilderMethods<'a, 'tcx>>(
    bx: &mut Bx,
    val: OperandValue<Bx::Value>,
    fields: &[u16],
) -> Option<Bx::Value> {
    match (val, fields) {
        (OperandValue::Immediate(v), []) => Some(v),
        (OperandValue::Immediate(v), [0]) => Some(v),
        (OperandValue::Pair(a, _), [0] | []) => Some(a),
        (OperandValue::Pair(_, b), [1]) => Some(b),
        (OperandValue::Ref(place), []) => Some(bx.load(bx.type_ptr(), place.llval, place.align)),
        _ => None,
    }
}

enum StmtUse {
    None,
    /// Only as the root of `(*r)…` places that are read or written (not borrowed).
    Through,
    /// The defining assignment.
    Def,
    Other,
}

fn classify_stmt_use<'tcx>(stmt: &mir::Statement<'tcx>, local: mir::Local) -> StmtUse {
    match &stmt.kind {
        StatementKind::StorageLive(_) | StatementKind::StorageDead(_) | StatementKind::Nop => {
            return StmtUse::None;
        }
        StatementKind::Assign(assign) => {
            let (lhs, rv) = &**assign;
            if lhs.as_local() == Some(local) {
                return if matches!(rv, Rvalue::Ref(..)) { StmtUse::Def } else { StmtUse::Other };
            }
            let through = |p: &mir::Place<'tcx>| {
                p.local == local && matches!(p.projection.first(), Some(PlaceElem::Deref))
            };
            // Index locals inside places are values; a ref is never an index.
            let mut m = Mentions { local, any: false };
            m.visit_statement(stmt, mir::Location::START);
            if !m.any {
                return StmtUse::None;
            }
            let lhs_ok = lhs.local != local || through(lhs);
            let rv_ok = match rv {
                Rvalue::Use(op, _) | Rvalue::Cast(_, op, _) | Rvalue::UnaryOp(_, op) => {
                    op_ok(op, local, &through)
                }
                Rvalue::BinaryOp(_, ops) => {
                    op_ok(&ops.0, local, &through) && op_ok(&ops.1, local, &through)
                }
                Rvalue::Aggregate(_, ops) => ops.iter().all(|o| op_ok(o, local, &through)),
                Rvalue::Discriminant(p) | Rvalue::CopyForDeref(p) => p.local != local || through(p),
                _ => !rvalue_mentions(rv, local),
            };
            if lhs_ok && rv_ok { StmtUse::Through } else { StmtUse::Other }
        }
        _ => {
            let mut m = Mentions { local, any: false };
            m.visit_statement(stmt, mir::Location::START);
            if m.any { StmtUse::Other } else { StmtUse::None }
        }
    }
}

fn op_ok<'tcx>(
    op: &Operand<'tcx>,
    local: mir::Local,
    through: &impl Fn(&mir::Place<'tcx>) -> bool,
) -> bool {
    match op {
        Operand::Copy(p) | Operand::Move(p) => p.local != local || through(p),
        _ => true,
    }
}

fn rvalue_mentions<'tcx>(rv: &Rvalue<'tcx>, local: mir::Local) -> bool {
    let mut m = Mentions { local, any: false };
    m.visit_rvalue(rv, mir::Location::START);
    m.any
}

/// Whether `local` is mentioned (as a use, not a storage marker) anywhere visited.
struct Mentions {
    local: mir::Local,
    any: bool,
}
impl<'tcx> Visitor<'tcx> for Mentions {
    fn visit_local(&mut self, l: mir::Local, ctx: PlaceContext, _: mir::Location) {
        if l == self.local && !matches!(ctx, PlaceContext::NonUse(_)) {
            self.any = true;
        }
    }
}

/// Whether another use in `uses` is reachable from `(bb, at)` (strictly after it).
fn use_reachable_after(
    mir: &mir::Body<'_>,
    uses: &FxHashSet<(BasicBlock, UseAt)>,
    use_blocks: &FxHashSet<BasicBlock>,
    bb: BasicBlock,
    at: UseAt,
) -> bool {
    // Later in the same block.
    let n = mir[bb].statements.len();
    if let UseAt::Stmt(k) = at {
        if (k + 1..n).any(|j| uses.contains(&(bb, UseAt::Stmt(j)))) || uses.contains(&(bb, UseAt::Term)) {
            return true;
        }
    }
    // Any use in a block reachable from a successor (including `bb` itself via a loop).
    let block_has_use = |b: BasicBlock| use_blocks.contains(&b);
    let mut seen: FxHashSet<BasicBlock> = FxHashSet::default();
    let mut stack: Vec<BasicBlock> = mir[bb].terminator().successors().collect();
    while let Some(b) = stack.pop() {
        if !seen.insert(b) {
            continue;
        }
        if block_has_use(b) {
            return true;
        }
        stack.extend(mir[b].terminator().successors());
    }
    false
}

/// M3.4 memcpy/memset/copy_nonoverlapping operand, now a claim (`CK_MemOp`) used only by
/// the memory operation (it does not rewrite caller-held pointers).
pub(crate) fn mem_op_claim<'a, 'tcx, Bx: BuilderMethods<'a, 'tcx>>(
    bx: &mut Bx,
    ptr: Bx::Value,
    bytes: Bx::Value,
) -> Bx::Value {
    let sess = bx.sess();
    if !sess.target.is_like_cheri
        || !sess.opts.unstable_opts.cheri_annotate
        || !sess.opts.unstable_opts.cheri_contract_narrow
    {
        return ptr;
    }
    bx.mem_claim(ptr, bytes, CK_MEM_OP | CF_UNTRUSTED)
}
