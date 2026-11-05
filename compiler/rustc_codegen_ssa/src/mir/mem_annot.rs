//! M5r R1 (RELAYER-DESIGN.md): target-neutral memory annotations emitted by codegen.
//!
//! LLM-generated research prototype (personal-use exemption in AGENTS.md); not for upstream.
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
use rustc_middle::ty::{self, Ty};
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
                | StatementKind::Nop => {
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
            Rvalue::Aggregate(_, ops) if self.cx.tcx().sess.opts.unstable_opts.cheri_slot_aggregate => {
                let tcx = self.cx.tcx();
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
            "MEM-STATS\t{}\tsites_matched={}\tsites_unmatched={}\tguards={}\tguards_missed={}\tfrees={}\tloans_total={}\tloans_eligible={}\tlast_use_points={}\tlast_use_stmts={}\tlast_use_stmts_missed={}\tlast_use_calls={}\tloan_ends={}\tloan_ends_placed={}\tloan_ends_on_call={}\tloan_ends_at_return={}\tloan_ends_missed={}\tloan_ends_carried={}\tloan_starts={}\tloan_slot_stores={}\tloan_slot_copies={}\tloan_starts_missed={}\tloan_slot_ssa={}\tloan_ends_inline_call={}\tloan_ends_backward={}\tloan_link_paired={}\tloan_link_lost={}\tloan_slot_aggregate={}\n",
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
        );
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = f.write_all(line.as_bytes());
        }
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
