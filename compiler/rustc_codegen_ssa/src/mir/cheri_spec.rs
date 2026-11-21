//! [LLM-generated] CHERI M7.2e: lowering of `#[rustc_cheri_spec]` memory specs to
//! `mem.wipe_cap_words` facts (see `rustc_middle::ty::cheri_spec` for the grammar).
//!
//! This replaces the M7.2a / M7.2c / M7.2d hooks that found the library frees by
//! function name, path and `rustc_diagnostic_item` markers. At each call:
//!
//! * The callee is a *free* if it has a `frees(block, size)` spec (its own, or the trait
//!   method's it implements), or it is the drop glue of a type whose `Drop::drop` has
//!   one and whose contents need no drop (`Box<MaybeUninit<_>>`, `ThinBox`'s guard).
//! * Inline form: walk the MIR-inlined frames of the call from the innermost out
//!   (then this function), skipping *forwarders* (frees and `relocates` functions:
//!   allocator plumbing). If the first other frame has a `dealloc_of(ty ..)` spec
//!   without `block`, wipe the callee's freed block, as `block` / `size` of its
//!   `frees` spec on the call's arguments say (or, with `fields`, only the capability
//!   words of `ty` / `or`).
//! * Root form: if the function being compiled has `dealloc_of(ty, block [, size])`,
//!   every free it calls (after inlining) frees `block` of its own parameters.
//!
//! Every form needs `ty` to possibly hold capabilities. Statement specs (`live`, and
//! the relocation rewrite) are lowered in MIR building to
//! `NonDivergingIntrinsic::CheriSpec` and codegen'd by `mem_cheri_spec_stmt`.

use rustc_abi::{Align, BackendRepr, FieldsShape, Size};
use rustc_middle::mir;
use rustc_middle::ty::cheri_spec::{
    CheriSpec, SpecBinOp, SpecExpr, SpecPlace, SpecTy, field_index, fn_specs, generic_ty,
    pointee_of,
};
use rustc_middle::ty::layout::{HasTyCtxt, HasTypingEnv, LayoutOf, TyAndLayout};
use rustc_middle::ty::{self, Instance, Ty};
use rustc_span::{Symbol, bug};
use rustc_span::def_id::DefId;
use rustc_target::callconv::{FnAbi, PassMode};

use super::operand::OperandValue;
use super::{FunctionCx, LocalRef};
use crate::MemFlags;
use crate::common::{IntPredicate, PreserveCheriTags};
use crate::traits::*;

/// A value met while evaluating a spec place.
#[derive(Clone, Copy)]
enum SVal<V> {
    Imm(V),
    Pair(V, V),
    /// A value stored at `base + off`; `align` is the alignment of `base`.
    Mem { base: V, off: u64, align: Align },
    /// Zero-sized, or a value this lowering cannot read.
    None,
}

/// A spec byte count: folded while constant.
#[derive(Clone, Copy)]
enum EVal<V> {
    Const(u64),
    Val(V),
}

/// The parameters a spec is evaluated over.
struct SpecEnv<'tcx, V> {
    /// The function whose parameter names and generics the spec text uses.
    owner: DefId,
    /// The monomorphic instance (for generic parameters); only used when it is `owner`.
    inst: Instance<'tcx>,
    names: Vec<Option<Symbol>>,
    params: Vec<(Ty<'tcx>, SVal<V>)>,
}

/// The `frees` spec of a callee.
struct Frees {
    /// Found through drop glue (the root form ignores these: a container's own
    /// body may drop its contents, which are not its block).
    glue: bool,
    owner: DefId,
    block: SpecPlace,
    size: SpecExpr,
}

impl<'a, 'tcx, Bx: BuilderMethods<'a, 'tcx>> FunctionCx<'a, 'tcx, Bx> {
    /// `-Zcheri-annotate` and `-Zcheri-wipe-cap-words` (default on).
    fn wipe_cap_words_enabled(&self) -> bool {
        self.mem_annotate() && self.cx.tcx().sess.opts.unstable_opts.cheri_wipe_cap_words
    }

    fn spec_enabled(&self) -> bool {
        self.wipe_cap_words_enabled() && self.cx.tcx().sess.opts.unstable_opts.cheri_spec
    }

    /// Called right before each call: the inline form, else the root form.
    pub(super) fn mem_wipe_cap_words_prepare(
        &mut self,
        bx: &mut Bx,
        fn_abi: &FnAbi<'tcx, Ty<'tcx>>,
        llargs: &[Bx::Value],
        instance: Option<Instance<'tcx>>,
        call_scope: mir::SourceScope,
    ) {
        if !self.spec_enabled() {
            return;
        }
        let Some(inst) = instance else {
            return;
        };
        let Some(frees) = self.spec_frees_of(inst) else {
            return;
        };
        if self.spec_inline_form(bx, fn_abi, llargs, &frees, call_scope) {
            return;
        }
        self.spec_root_form(bx, fn_abi, llargs, &frees);
    }

    // -----------------------------------------------------------------------------
    // Spec lookup
    // -----------------------------------------------------------------------------

    /// The `frees` spec that governs a call to `inst`, if it is a free.
    fn spec_frees_of(&self, inst: Instance<'tcx>) -> Option<Frees> {
        let tcx = self.cx.tcx();
        let mut glue = false;
        let did = match inst.def {
            ty::InstanceKind::Item(did) => did,
            ty::InstanceKind::Shim(ty::ShimKind::DropGlue(_, Some(dty))) => {
                // The glue of a type whose own `Drop::drop` frees and whose contents
                // need no drop is that `drop`.
                let ty::Adt(def, args) = *dty.kind() else {
                    return None;
                };
                let dtor = tcx.adt_destructor(def.did())?;
                let typing_env = self.cx.typing_env();
                if def.is_box() && args.type_at(0).needs_drop(tcx, typing_env) {
                    return None;
                }
                if def
                    .all_fields()
                    .any(|f| f.ty(tcx, args).skip_norm_wip().needs_drop(tcx, typing_env))
                {
                    return None;
                }
                glue = true;
                dtor.did
            }
            _ => return None,
        };
        let (owner, specs) = fn_specs(tcx, did);
        specs.into_iter().find_map(|s| match s {
            CheriSpec::Frees { block, size } => Some(Frees { glue, owner, block, size }),
            _ => None,
        })
    }

    /// Frees and `relocates` functions: allocator plumbing between a free and the
    /// function that asked for it.
    fn spec_is_forwarder(&self, f: Instance<'tcx>) -> bool {
        if self.spec_frees_of(f).is_some() {
            return true;
        }
        let ty::InstanceKind::Item(did) = f.def else {
            return false;
        };
        fn_specs(self.cx.tcx(), did).1.iter().any(|s| matches!(s, CheriSpec::Relocates { .. }))
    }

    /// The function's own `dealloc_of` spec (not one inherited from a trait).
    fn spec_dealloc_of(&self, f: Instance<'tcx>) -> Option<(DefId, CheriSpec)> {
        let ty::InstanceKind::Item(did) = f.def else {
            return None;
        };
        let (owner, specs) = fn_specs(self.cx.tcx(), did);
        if owner != did {
            return None;
        }
        specs
            .into_iter()
            .find(|s| matches!(s, CheriSpec::DeallocOf { .. }))
            .map(|s| (did, s))
    }

    fn spec_names(&self, owner: DefId) -> Vec<Option<Symbol>> {
        self.cx.tcx().fn_arg_idents(owner).iter().map(|i| i.map(|i| i.name)).collect()
    }

    fn spec_normalize(&self, t: Ty<'tcx>) -> Ty<'tcx> {
        let tcx = self.cx.tcx();
        tcx.try_normalize_erasing_regions(self.cx.typing_env(), ty::Unnormalized::new_wip(t))
            .unwrap_or(t)
    }

    /// Monomorphic parameter types of an inlined frame (types only, no values).
    fn spec_frame_env(&self, f: Instance<'tcx>, owner: DefId) -> SpecEnv<'tcx, Bx::Value> {
        let tcx = self.cx.tcx();
        let sig = tcx.fn_sig(f.def_id()).instantiate(tcx, f.args).skip_norm_wip();
        let sig = tcx.instantiate_bound_regions_with_erased(sig);
        let params =
            sig.inputs().iter().map(|&t| (self.spec_normalize(t), SVal::None)).collect();
        SpecEnv { owner, inst: f, names: self.spec_names(owner), params }
    }

    /// The parameters of the function being compiled, from its argument locals.
    fn spec_root_env(&self, owner: DefId) -> SpecEnv<'tcx, Bx::Value> {
        let mut params = Vec::new();
        for i in 0..self.mir.arg_count {
            let local = mir::Local::from_usize(i + 1);
            let t = self.monomorphize(self.mir.local_decls[local].ty);
            let v = match &self.locals[local] {
                LocalRef::Operand(op) => match op.val {
                    OperandValue::Immediate(v) => SVal::Imm(v),
                    OperandValue::Pair(a, b) => SVal::Pair(a, b),
                    OperandValue::Ref(pv) => SVal::Mem { base: pv.llval, off: 0, align: pv.align },
                    OperandValue::ZeroSized => SVal::None,
                },
                LocalRef::Place(p) => SVal::Mem { base: p.val.llval, off: 0, align: p.val.align },
                LocalRef::UnsizedPlace(_) | LocalRef::PendingOperand => SVal::None,
            };
            params.push((t, v));
        }
        SpecEnv { owner, inst: self.instance, names: self.spec_names(owner), params }
    }

    /// The callee's parameters, from the call's ABI arguments.
    fn spec_args_env(
        &self,
        fn_abi: &FnAbi<'tcx, Ty<'tcx>>,
        llargs: &[Bx::Value],
        owner: DefId,
        inst: Instance<'tcx>,
    ) -> SpecEnv<'tcx, Bx::Value> {
        let mut idx = if matches!(fn_abi.ret.mode, PassMode::Indirect { .. }) { 1 } else { 0 };
        let mut params = Vec::new();
        for arg in fn_abi.args.iter() {
            let t = arg.layout.ty;
            let (v, n) = match arg.mode {
                PassMode::Ignore => (SVal::None, 0),
                PassMode::Direct(_) if idx < llargs.len() => (SVal::Imm(llargs[idx]), 1),
                PassMode::Pair(..) if idx + 1 < llargs.len() => {
                    (SVal::Pair(llargs[idx], llargs[idx + 1]), 2)
                }
                PassMode::Indirect { meta_attrs, .. } if idx < llargs.len() => (
                    SVal::Mem { base: llargs[idx], off: 0, align: arg.layout.align.abi },
                    if meta_attrs.is_some() { 2 } else { 1 },
                ),
                PassMode::Pair(..) => (SVal::None, 2),
                _ => (SVal::None, 1),
            };
            idx += n;
            params.push((t, v));
        }
        SpecEnv { owner, inst, names: self.spec_names(owner), params }
    }

    // -----------------------------------------------------------------------------
    // Types
    // -----------------------------------------------------------------------------

    fn spec_param(&self, env: &SpecEnv<'tcx, Bx::Value>, name: Symbol) -> Option<usize> {
        let i = env.names.iter().position(|n| *n == Some(name))?;
        (i < env.params.len()).then_some(i)
    }

    /// One field step on a monomorphic type: look through pointers, then take `name`.
    /// Returns the deref count and the field's layout index.
    fn spec_field_ty(&self, mut t: Ty<'tcx>, name: Symbol) -> Option<(Ty<'tcx>, u32, usize)> {
        let tcx = self.cx.tcx();
        for derefs in 0..5 {
            if let ty::Adt(def, _) = *t.kind()
                && let Some(i) = field_index(def, name)
            {
                return Some((self.cx.layout_of(t).field(self.cx, i).ty, derefs, i));
            }
            t = self.spec_normalize(pointee_of(tcx, t)?);
        }
        None
    }

    fn spec_place_ty(&self, env: &SpecEnv<'tcx, Bx::Value>, p: &SpecPlace) -> Option<Ty<'tcx>> {
        let mut t = env.params[self.spec_param(env, p.base)?].0;
        for f in &p.fields {
            t = self.spec_field_ty(t, *f)?.0;
        }
        Some(t)
    }

    fn spec_ty(&self, env: &SpecEnv<'tcx, Bx::Value>, t: &SpecTy) -> Option<Ty<'tcx>> {
        let tcx = self.cx.tcx();
        match t {
            SpecTy::Param(n) => {
                if env.inst.def_id() != env.owner {
                    return None;
                }
                let g = generic_ty(tcx, env.owner, *n)?;
                Some(tcx.instantiate_and_normalize_erasing_regions(
                    env.inst.args,
                    self.cx.typing_env(),
                    ty::EarlyBinder::bind(tcx, g),
                ))
            }
            SpecTy::Pointee(p) => {
                let pt = self.spec_place_ty(env, p)?;
                Some(self.spec_normalize(pointee_of(tcx, pt)?))
            }
        }
    }

    fn spec_sized_size(&self, t: Ty<'tcx>) -> Option<u64> {
        t.is_sized(self.cx.tcx(), self.cx.typing_env()).then(|| self.cx.layout_of(t).size.bytes())
    }

    // -----------------------------------------------------------------------------
    // Values
    // -----------------------------------------------------------------------------

    fn spec_addr(bx: &mut Bx, base: Bx::Value, off: u64) -> Bx::Value {
        if off == 0 { base } else { bx.inbounds_ptradd(base, bx.const_usize(off)) }
    }

    fn spec_ptr(&self, bx: &mut Bx, v: SVal<Bx::Value>) -> Option<Bx::Value> {
        match v {
            SVal::Imm(x) | SVal::Pair(x, _) => Some(x),
            SVal::Mem { base, off, align } => {
                let addr = Self::spec_addr(bx, base, off);
                let align = align.restrict_for_offset(Size::from_bytes(off));
                Some(bx.load(bx.type_ptr(), addr, align))
            }
            SVal::None => None,
        }
    }

    fn spec_int(&self, bx: &mut Bx, v: SVal<Bx::Value>) -> Option<Bx::Value> {
        match v {
            SVal::Imm(x) => Some(x),
            SVal::Mem { base, off, align } => {
                let addr = Self::spec_addr(bx, base, off);
                let align = align.restrict_for_offset(Size::from_bytes(off));
                Some(bx.load(bx.type_isize(), addr, align))
            }
            SVal::Pair(..) | SVal::None => None,
        }
    }

    /// One field step on a value (loads only when looking through a pointer).
    fn spec_field_val(
        &self,
        bx: &mut Bx,
        mut t: Ty<'tcx>,
        mut v: SVal<Bx::Value>,
        name: Symbol,
    ) -> Option<(Ty<'tcx>, SVal<Bx::Value>)> {
        let tcx = self.cx.tcx();
        for _ in 0..5 {
            if let ty::Adt(def, _) = *t.kind()
                && let Some(i) = field_index(def, name)
            {
                let l = self.cx.layout_of(t);
                let fl = l.field(self.cx, i);
                let foff = l.fields.offset(i).bytes();
                let nv = if fl.is_zst() {
                    SVal::None
                } else {
                    match v {
                        SVal::Mem { base, off, align } => SVal::Mem { base, off: off + foff, align },
                        SVal::Imm(x) if matches!(l.backend_repr, BackendRepr::Scalar(_)) => {
                            SVal::Imm(x)
                        }
                        SVal::Pair(a, b) if matches!(l.backend_repr, BackendRepr::ScalarPair { .. }) => {
                            if fl.size == l.size {
                                SVal::Pair(a, b)
                            } else if foff == 0 {
                                SVal::Imm(a)
                            } else {
                                SVal::Imm(b)
                            }
                        }
                        _ => return None,
                    }
                };
                return Some((fl.ty, nv));
            }
            let pt = self.spec_normalize(pointee_of(tcx, t)?);
            let p = self.spec_ptr(bx, v)?;
            let align = match self.spec_sized_size(pt) {
                Some(_) => self.cx.layout_of(pt).align.abi,
                None => Align::ONE,
            };
            t = pt;
            v = SVal::Mem { base: p, off: 0, align };
        }
        None
    }

    fn spec_place_val(
        &self,
        bx: &mut Bx,
        env: &SpecEnv<'tcx, Bx::Value>,
        p: &SpecPlace,
    ) -> Option<(Ty<'tcx>, SVal<Bx::Value>)> {
        let (mut t, mut v) = env.params[self.spec_param(env, p.base)?];
        for f in &p.fields {
            (t, v) = self.spec_field_val(bx, t, v, *f)?;
        }
        Some((t, v))
    }

    fn spec_bin(bx: &mut Bx, op: SpecBinOp, x: EVal<Bx::Value>, y: EVal<Bx::Value>) -> EVal<Bx::Value> {
        use EVal::*;
        match (op, x, y) {
            (SpecBinOp::Add, Const(a), Const(b)) => Const(a.wrapping_add(b)),
            (SpecBinOp::Sub, Const(a), Const(b)) => Const(a.wrapping_sub(b)),
            (SpecBinOp::Mul, Const(a), Const(b)) => Const(a.wrapping_mul(b)),
            (SpecBinOp::Mul, _, Const(0)) | (SpecBinOp::Mul, Const(0), _) => Const(0),
            (SpecBinOp::Mul, v, Const(1)) | (SpecBinOp::Mul, Const(1), v) => v,
            (SpecBinOp::Add | SpecBinOp::Sub, v, Const(0)) | (SpecBinOp::Add, Const(0), v) => v,
            (op, x, y) => {
                let x = Self::ev(bx, x);
                let y = Self::ev(bx, y);
                Val(match op {
                    SpecBinOp::Add => bx.add(x, y),
                    SpecBinOp::Sub => bx.sub(x, y),
                    SpecBinOp::Mul => bx.mul(x, y),
                })
            }
        }
    }

    fn ev(bx: &mut Bx, e: EVal<Bx::Value>) -> Bx::Value {
        match e {
            EVal::Const(c) => bx.const_usize(c),
            EVal::Val(v) => v,
        }
    }

    fn spec_expr(
        &self,
        bx: &mut Bx,
        env: &SpecEnv<'tcx, Bx::Value>,
        e: &SpecExpr,
    ) -> Option<EVal<Bx::Value>> {
        Some(match e {
            SpecExpr::Lit(n) => EVal::Const(*n),
            SpecExpr::Place(p) => {
                let (_, v) = self.spec_place_val(bx, env, p)?;
                EVal::Val(self.spec_int(bx, v)?)
            }
            SpecExpr::SizeOf(t) => EVal::Const(self.spec_sized_size(self.spec_ty(env, t)?)?),
            SpecExpr::SizeOfPointee(p) => {
                let pt = self.spec_place_ty(env, p)?;
                let pointee = self.spec_normalize(pointee_of(self.cx.tcx(), pt)?);
                EVal::Const(self.spec_sized_size(pointee)?)
            }
            SpecExpr::Bin(op, a, b) => {
                let x = self.spec_expr(bx, env, a)?;
                let y = self.spec_expr(bx, env, b)?;
                Self::spec_bin(bx, *op, x, y)
            }
        })
    }

    /// The freed `(block, size)` of a call to a free, from its `frees` spec.
    fn spec_freed_block(
        &self,
        bx: &mut Bx,
        fn_abi: &FnAbi<'tcx, Ty<'tcx>>,
        llargs: &[Bx::Value],
        frees: &Frees,
    ) -> Option<(Bx::Value, Bx::Value)> {
        // Generic parameters are never needed by a `frees` spec's places, only by
        // `size_of::<T>()`, which takes the owner's generics; use a dummy instance
        // that never matches the owner so a `Param` type fails cleanly.
        let env = self.spec_args_env(fn_abi, llargs, frees.owner, self.instance);
        if env.params.len() > env.names.len() + 1 {
            return None;
        }
        let (_, bv) = self.spec_place_val(bx, &env, &frees.block)?;
        let ptr = self.spec_ptr(bx, bv)?;
        let size = self.spec_expr(bx, &env, &frees.size)?;
        Some((ptr, Self::ev(bx, size)))
    }

    // -----------------------------------------------------------------------------
    // Forms
    // -----------------------------------------------------------------------------

    /// MIR-inlined frames of the call, innermost first, then this function.
    fn spec_frames(&self, call_scope: mir::SourceScope) -> Vec<Instance<'tcx>> {
        let scopes = &self.mir.source_scopes;
        let mut frames = Vec::new();
        let mut cur = Some(call_scope);
        while let Some(sc) = cur {
            let data = &scopes[sc];
            if let Some((callee, _)) = data.inlined {
                frames.push(self.monomorphize(callee));
            }
            cur = data.inlined_parent_scope;
        }
        frames.push(self.instance);
        frames
    }

    fn spec_inline_form(
        &mut self,
        bx: &mut Bx,
        fn_abi: &FnAbi<'tcx, Ty<'tcx>>,
        llargs: &[Bx::Value],
        frees: &Frees,
        call_scope: mir::SourceScope,
    ) -> bool {
        let tcx = self.cx.tcx();
        let opts = &tcx.sess.opts.unstable_opts;
        let frames = self.spec_frames(call_scope);
        let Some(&frame) = frames.iter().find(|&&f| !self.spec_is_forwarder(f)) else {
            return false;
        };
        let Some((owner, CheriSpec::DeallocOf { ty, or, fields, block: None, .. })) =
            self.spec_dealloc_of(frame)
        else {
            return false;
        };
        if if fields { !opts.cheri_wipe_btree_nodes } else { !opts.cheri_wipe_alloc_hints } {
            return false;
        }
        let env = self.spec_frame_env(frame, owner);
        let Some(ty) = self.spec_ty(&env, &ty) else {
            return false;
        };
        if !ty::ty_may_contain_caps(tcx, ty) || self.spec_sized_size(ty) == Some(0) {
            return false;
        }
        let or = match &or {
            Some(o) => match self.spec_ty(&env, o) {
                Some(t) => Some(t),
                None => return false,
            },
            None => None,
        };
        let Some((ptr, size)) = self.spec_freed_block(bx, fn_abi, llargs, frees) else {
            return false;
        };
        if fields {
            self.spec_emit_fields(bx, ptr, size, ty, or);
        } else {
            bx.mem_fact("mem.wipe_cap_words", &[ptr, size]);
        }
        true
    }

    fn spec_root_form(
        &mut self,
        bx: &mut Bx,
        fn_abi: &FnAbi<'tcx, Ty<'tcx>>,
        llargs: &[Bx::Value],
        frees: &Frees,
    ) {
        let tcx = self.cx.tcx();
        if frees.glue {
            return;
        }
        let Some((owner, CheriSpec::DeallocOf { ty, block: Some(block), size, .. })) =
            self.spec_dealloc_of(self.instance)
        else {
            return;
        };
        let env = self.spec_root_env(owner);
        let Some(ty) = self.spec_ty(&env, &ty) else {
            return;
        };
        if !ty::ty_may_contain_caps(tcx, ty) {
            return;
        }
        // Without `size`, the block's (sized) pointee size; for an unsized pointee
        // both come from the free's own spec.
        let default_size = match &size {
            Some(_) => None,
            None => {
                let Some(bt) = self.spec_place_ty(&env, &block) else {
                    return;
                };
                let Some(pointee) = pointee_of(tcx, bt) else {
                    return;
                };
                match self.spec_sized_size(self.spec_normalize(pointee)) {
                    Some(n) => Some(n),
                    None => {
                        if let Some((p, n)) = self.spec_freed_block(bx, fn_abi, llargs, frees) {
                            bx.mem_fact("mem.wipe_cap_words", &[p, n]);
                        }
                        return;
                    }
                }
            }
        };
        let Some((_, bv)) = self.spec_place_val(bx, &env, &block) else {
            return;
        };
        let Some(ptr) = self.spec_ptr(bx, bv) else {
            return;
        };
        let len = match (&size, default_size) {
            (Some(e), _) => match self.spec_expr(bx, &env, e) {
                Some(v) => Self::ev(bx, v),
                None => return,
            },
            (None, Some(n)) => bx.const_usize(n),
            (None, None) => return,
        };
        bx.mem_fact("mem.wipe_cap_words", &[ptr, len]);
    }

    // -----------------------------------------------------------------------------
    // `fields`: capability-word geometry
    // -----------------------------------------------------------------------------

    /// Byte offsets of the capability words of `ty`'s layout (every variant / union
    /// field counted), or `None` if some part cannot be laid out word by word.
    fn spec_cap_words(&self, ty: Ty<'tcx>) -> Option<Vec<u64>> {
        let tcx = self.cx.tcx();
        let word = self.cx.layout_of(Ty::new_imm_ptr(tcx, tcx.types.u8)).size.bytes();
        let mut out = Vec::new();
        if !self.spec_cap_words_rec(self.cx.layout_of(ty), 0, word, &mut out, 0) {
            return None;
        }
        out.sort_unstable();
        out.dedup();
        Some(out)
    }

    fn spec_cap_words_rec(
        &self,
        l: TyAndLayout<'tcx>,
        base: u64,
        word: u64,
        out: &mut Vec<u64>,
        depth: u32,
    ) -> bool {
        let tcx = self.cx.tcx();
        if depth > 24 || out.len() > 4096 {
            return false;
        }
        if l.is_zst() || !ty::ty_may_contain_caps(tcx, l.ty) {
            return true;
        }
        match *l.ty.kind() {
            ty::Ref(..) | ty::RawPtr(..) | ty::FnPtr(..) => {
                if l.size.bytes() == word {
                    out.push(base);
                    true
                } else {
                    false
                }
            }
            ty::Pat(t, _) => self.spec_cap_words_rec(self.cx.layout_of(t), base, word, out, depth + 1),
            ty::Adt(def, _) if def.is_enum() => {
                for v in def.variants().indices() {
                    let vl = l.for_variant(self.cx, v);
                    for i in 0..vl.fields.count() {
                        let f = vl.field(self.cx, i);
                        let off = base + vl.fields.offset(i).bytes();
                        if !self.spec_cap_words_rec(f, off, word, out, depth + 1) {
                            return false;
                        }
                    }
                }
                true
            }
            ty::Array(..) => {
                let FieldsShape::Array { stride, count } = l.fields else {
                    return false;
                };
                if count > 1024 {
                    return false;
                }
                let el = l.field(self.cx, 0);
                (0..count).all(|k| {
                    self.spec_cap_words_rec(el, base + k * stride.bytes(), word, out, depth + 1)
                })
            }
            ty::Adt(..) | ty::Tuple(..) | ty::Closure(..) => {
                if matches!(l.fields, FieldsShape::Primitive) {
                    return false;
                }
                (0..l.fields.count()).all(|i| {
                    let off = base + l.fields.offset(i).bytes();
                    self.spec_cap_words_rec(l.field(self.cx, i), off, word, out, depth + 1)
                })
            }
            _ => false,
        }
    }

    /// Emit word runs (merged, in chunks of at most eight capability words: a chunk
    /// that small is a constant-length fact the late pass unrolls in place).
    fn spec_emit_runs(bx: &mut Bx, ptr: Bx::Value, words: &[u64], word: u64) {
        let mut i = 0;
        while i < words.len() {
            let start = words[i];
            let mut n = 1;
            while i + n < words.len() && words[i + n] == start + n as u64 * word && n < 8 {
                n += 1;
            }
            let p = Self::spec_addr(bx, ptr, start);
            bx.mem_fact("mem.wipe_cap_words", &[p, bx.const_usize(n as u64 * word)]);
            i += n;
        }
    }

    /// `dealloc_of(.., fields)`: if `ty`'s generic arguments hold no capabilities, wipe
    /// only `ty`'s capability words, plus those only `or` has when the freed size is
    /// `size_of::<or>()` (a run-time size gets a branch); another constant size wipes
    /// the whole block. Otherwise the whole block.
    fn spec_emit_fields(
        &mut self,
        bx: &mut Bx,
        ptr: Bx::Value,
        size: Bx::Value,
        ty: Ty<'tcx>,
        or: Option<Ty<'tcx>>,
    ) {
        let tcx = self.cx.tcx();
        let word = self.cx.layout_of(Ty::new_imm_ptr(tcx, tcx.types.u8)).size.bytes();
        let args_free = match *ty.kind() {
            ty::Adt(_, args) => args.types().all(|t| !ty::ty_may_contain_caps(tcx, t)),
            _ => false,
        };
        let ty_words = if args_free { self.spec_cap_words(ty) } else { None };
        let or_words = match (or, &ty_words) {
            (Some(o), Some(tw)) => match self.spec_cap_words(o) {
                Some(ow) => Some((o, ow.into_iter().filter(|w| !tw.contains(w)).collect::<Vec<_>>())),
                None => {
                    bx.mem_fact("mem.wipe_cap_words", &[ptr, size]);
                    return;
                }
            },
            _ => None,
        };
        let (Some(tw), Some(ty_size)) = (ty_words, self.spec_sized_size(ty)) else {
            bx.mem_fact("mem.wipe_cap_words", &[ptr, size]);
            return;
        };
        Self::spec_emit_runs(bx, ptr, &tw, word);
        let or_size = or_words.as_ref().and_then(|(o, _)| self.spec_sized_size(*o));
        match (bx.const_to_opt_uint(size), or_words, or_size) {
            (Some(n), _, _) if n == ty_size => {}
            (Some(n), Some((_, ow)), Some(os)) if n == os => Self::spec_emit_runs(bx, ptr, &ow, word),
            (Some(_), _, _) => bx.mem_fact("mem.wipe_cap_words", &[ptr, size]),
            (None, Some((_, ow)), Some(os)) => {
                let is_or = bx.icmp(IntPredicate::IntEQ, size, bx.const_usize(os));
                let wipe_bb = bx.append_sibling_block("cheri_spec_wipe_or");
                let cont_bb = bx.append_sibling_block("cheri_spec_wipe_cont");
                bx.cond_br(is_or, wipe_bb, cont_bb);
                bx.switch_to_block(wipe_bb);
                Self::spec_emit_runs(bx, ptr, &ow, word);
                bx.br(cont_bb);
                bx.switch_to_block(cont_bb);
            }
            (None, _, _) => bx.mem_fact("mem.wipe_cap_words", &[ptr, size]),
        }
    }

    // -----------------------------------------------------------------------------
    // Statement specs
    // -----------------------------------------------------------------------------

    fn spec_mir_expr(&mut self, bx: &mut Bx, e: &mir::CheriSpecExpr<'tcx>) -> Option<EVal<Bx::Value>> {
        let mut st: Vec<EVal<Bx::Value>> = Vec::new();
        for it in &e.0 {
            match it {
                mir::CheriSpecItem::Operand(op) => {
                    let v = self.codegen_operand(bx, op).immediate();
                    st.push(match bx.const_to_opt_uint(v) {
                        Some(c) => EVal::Const(c),
                        None => EVal::Val(v),
                    });
                }
                mir::CheriSpecItem::Const(c) => st.push(EVal::Const(*c)),
                mir::CheriSpecItem::SizeOf(t) => {
                    let t = self.monomorphize(*t);
                    st.push(EVal::Const(self.spec_sized_size(t)?));
                }
                mir::CheriSpecItem::Add | mir::CheriSpecItem::Sub | mir::CheriSpecItem::Mul => {
                    let y = st.pop()?;
                    let x = st.pop()?;
                    let op = match it {
                        mir::CheriSpecItem::Add => SpecBinOp::Add,
                        mir::CheriSpecItem::Sub => SpecBinOp::Sub,
                        _ => SpecBinOp::Mul,
                    };
                    st.push(Self::spec_bin(bx, op, x, y));
                }
            }
        }
        if st.len() == 1 { st.pop() } else { None }
    }

    fn spec_mir_ptr(&mut self, bx: &mut Bx, op: &mir::Operand<'tcx>) -> Bx::Value {
        match self.codegen_operand(bx, op).val {
            OperandValue::Immediate(v) | OperandValue::Pair(v, _) => v,
            OperandValue::Ref(pv) => bx.load(bx.type_ptr(), pv.llval, pv.align),
            OperandValue::ZeroSized => bug!("CHERI spec pointer operand is zero-sized"),
        }
    }

    /// `NonDivergingIntrinsic::CheriSpec`.
    pub(crate) fn mem_cheri_spec_stmt(&mut self, bx: &mut Bx, spec: &mir::CheriSpecStmt<'tcx>) {
        match spec {
            mir::CheriSpecStmt::WipeDead { block, live, size, held } => {
                let opts = &self.cx.tcx().sess.opts.unstable_opts;
                if !self.spec_enabled() || !opts.cheri_wipe_alloc_hints {
                    return;
                }
                let held = self.monomorphize(*held);
                if !ty::ty_may_contain_caps(self.cx.tcx(), held)
                    || self.spec_sized_size(held).is_none_or(|n| n == 0)
                {
                    return;
                }
                let ptr = self.spec_mir_ptr(bx, block);
                let (Some(live), Some(size)) =
                    (self.spec_mir_expr(bx, live), self.spec_mir_expr(bx, size))
                else {
                    return;
                };
                let start = match live {
                    EVal::Const(c) => Self::spec_addr(bx, ptr, c),
                    EVal::Val(v) => bx.inbounds_ptradd(ptr, v),
                };
                let len = Self::spec_bin(bx, SpecBinOp::Sub, size, live);
                let len = Self::ev(bx, len);
                bx.mem_fact("mem.wipe_cap_words", &[start, len]);
            }
            mir::CheriSpecStmt::Relocated { from, to, keep, size, ty } => {
                // The copy is part of the program and always happens.
                let src = self.spec_mir_ptr(bx, from);
                let dst = self.spec_mir_ptr(bx, to);
                let keep = self.spec_mir_expr(bx, keep).expect("CHERI spec: bad `keep`");
                let keep = Self::ev(bx, keep);
                bx.memcpy(
                    dst,
                    Align::ONE,
                    src,
                    Align::ONE,
                    keep,
                    MemFlags::empty(),
                    None,
                    PreserveCheriTags::Required,
                );
                // The MIR branch follows the flags of the crate that built the MIR (the
                // library); the wipe follows this crate's switches.
                let opts = &self.cx.tcx().sess.opts.unstable_opts;
                if self.spec_enabled()
                    && opts.cheri_wipe_alloc_hints
                    && opts.cheri_spec_relocate_rewrite
                    && ty::ty_may_contain_caps(self.cx.tcx(), self.monomorphize(*ty))
                    && let Some(size) = self.spec_mir_expr(bx, size)
                {
                    let size = Self::ev(bx, size);
                    bx.mem_fact("mem.wipe_cap_words", &[src, size]);
                }
            }
        }
    }
}
