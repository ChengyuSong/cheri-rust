//! [LLM-generated] M7.2g: callee store marks (`M7.2f-ESCAPE-DESIGN.md` §3 decision 1).
//!
//! CHERI research prototype on this fork (see AGENTS.md). Not for upstream.
//!
//! A function whose body stores pointer parameter `p` (or a pointer it reads through a
//! shared reference `p`) into memory at a constant offset from pointer parameter `q`
//! gets a parameter mark. rustc only states the fact; the late CHERI pass in each
//! caller turns the mark at a direct call into a caller-side `mem.store_into_slot`
//! fact, which it lowers like a local one (compare and null at the free of the
//! stored pointer's target).
//!
//! The same mark is produced from a written `#[rustc_cheri_spec(stores(p, into = q.f))]`
//! when the place is reached through exactly one pointer level of `q`.
//!
//! The marks are found on the instance's optimized MIR. Stores in the body count, and
//! so do calls (up to three levels) to functions whose own MIR shows such a store
//! (`ptr::write` and `<*mut T>::add` before the MIR inliner runs, at O0 and O1, or a
//! small helper); the callee's MIR is read, its name is not. Each traced local must
//! have a single definition, and the parameters must never be reassigned. A store
//! whose offset is not a constant gets no mark (the registration fallback is a
//! later piece).

use std::cell::RefCell;
use std::rc::Rc;

use rustc_data_structures::fx::FxHashMap;
use rustc_middle::mir::visit::{PlaceContext, Visitor};
use rustc_middle::mir::{self, BinOp, CastKind, Operand, ProjectionElem, Rvalue, StatementKind};
use rustc_middle::ty::cheri_spec::CheriStoreMark;
use rustc_middle::ty::layout::LayoutOf;
use rustc_middle::ty::{self, Instance, Ty, TyCtxt};
use rustc_span::sym;

/// One mark: the body stores the pointer value given by `src` into the 16-byte slot
/// at byte offset `off` from parameter `dst`. Parameter numbers are 0-based Rust
/// argument indices. `src_deref` is `None` when the stored value is parameter `src`
/// itself, or `Some(d)` when it is the pointer read at byte offset `d` through the
/// shared reference `src`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct StoreMark {
    pub src: usize,
    pub dst: usize,
    pub off: u64,
    pub src_deref: Option<u64>,
}

/// At most this many marks per function (the rest are dropped: fail-safe, no clear).
const MAX_MARKS: usize = 8;
const MAX_DEPTH: u32 = 16;

/// Whether `-Zcheri-stores-into-marks` is in effect for this session.
pub fn enabled(tcx: TyCtxt<'_>) -> bool {
    let o = &tcx.sess.opts.unstable_opts;
    tcx.sess.target.is_like_cheri && o.cheri_annotate && o.cheri_stores_into_marks
}

/// The store marks of `instance`, sorted and deduplicated. Empty when the switch is
/// off, the item is not a plain function of this crate, or its MIR is not available
/// (M7.2h: except that a non-generic function of an upstream crate gets the marks
/// recorded in that crate's metadata).
pub fn store_marks<'tcx>(tcx: TyCtxt<'tcx>, instance: Instance<'tcx>) -> Vec<StoreMark> {
    if !enabled(tcx) {
        return Vec::new();
    }
    let ty::InstanceKind::Item(def_id) = instance.def else { return Vec::new() };
    let mut out = spec_marks(tcx, instance, def_id);
    if filter_ok(tcx, def_id) && tcx.is_mir_available(def_id) {
        let body = tcx.instance_mir(instance.def);
        let cache = Cache::default();
        Analysis::new(tcx, instance, body, 0, &cache).run(&mut out);
    }
    // [LLM-generated] M7.2h: a non-generic function of an upstream crate brings the
    // marks its own crate computed (crate metadata).
    if !def_id.is_local() {
        out.extend(extern_marks(tcx, def_id));
    }
    out.sort();
    out.dedup();
    out.truncate(MAX_MARKS);
    out
}

/// [LLM-generated] M7.2h: whether marks are kept in and read from crate metadata
/// (`-Zcheri-stores-into-marks-xcrate`).
pub fn xcrate_enabled(tcx: TyCtxt<'_>) -> bool {
    enabled(tcx) && tcx.sess.opts.unstable_opts.cheri_stores_into_marks_xcrate
}

/// [LLM-generated] M7.2h: a function whose marks can travel in crate metadata: a
/// plain non-generic function (its one instance is the item itself).
fn xcrate_item(tcx: TyCtxt<'_>, def_id: rustc_span::def_id::DefId) -> bool {
    matches!(tcx.def_kind(def_id), rustc_hir::def::DefKind::Fn | rustc_hir::def::DefKind::AssocFn)
        && tcx.intrinsic(def_id).is_none()
        && !tcx.is_foreign_item(def_id)
        && !tcx.generics_of(def_id).requires_monomorphization(tcx)
}

/// [LLM-generated] M7.2h: the marks recorded in the metadata of the upstream crate
/// that defines `def_id` (empty for local, generic or unmarked items).
fn extern_marks(tcx: TyCtxt<'_>, def_id: rustc_span::def_id::DefId) -> Vec<StoreMark> {
    if def_id.is_local() || !xcrate_enabled(tcx) || !xcrate_item(tcx, def_id) {
        return Vec::new();
    }
    tcx.cheri_store_marks(def_id)
        .iter()
        .map(|m| StoreMark {
            src: m.src as usize,
            dst: m.dst as usize,
            off: m.off,
            src_deref: m.src_deref,
        })
        .collect()
}

/// [LLM-generated] M7.2h: local provider of the `cheri_store_marks` query, which the
/// metadata encoder records: the marks of a non-generic function of this crate (the
/// same list its declaration gets in this crate).
fn cheri_store_marks_local<'tcx>(
    tcx: TyCtxt<'tcx>,
    def_id: rustc_span::def_id::LocalDefId,
) -> &'tcx [CheriStoreMark] {
    let did = def_id.to_def_id();
    if !xcrate_enabled(tcx) || !xcrate_item(tcx, did) {
        return &[];
    }
    let marks = store_marks(tcx, Instance::mono(tcx, did));
    tcx.arena.alloc_from_iter(marks.iter().filter_map(|m| {
        Some(CheriStoreMark {
            src: u32::try_from(m.src).ok()?,
            dst: u32::try_from(m.dst).ok()?,
            off: m.off,
            src_deref: m.src_deref,
        })
    }))
}

pub(crate) fn provide(providers: &mut rustc_middle::query::Providers) {
    providers.cheri_store_marks = cheri_store_marks_local;
}

/// The same filter as the body's own `store_into` facts: user-crate items only, not
/// core / alloc / compiler_builtins, the panic handler or `fmt` methods.
fn filter_ok(tcx: TyCtxt<'_>, def_id: rustc_span::def_id::DefId) -> bool {
    if !def_id.is_local() {
        return false;
    }
    let name = tcx.crate_name(rustc_span::def_id::LOCAL_CRATE);
    if matches!(name.as_str(), "core" | "alloc" | "compiler_builtins") {
        return false;
    }
    if tcx.lang_items().panic_impl() == Some(def_id) {
        return false;
    }
    match tcx.opt_item_name(def_id) {
        Some(n) => !matches!(n.as_str(), "fmt" | "rust_begin_unwind"),
        None => true,
    }
}

fn mono<'tcx>(tcx: TyCtxt<'tcx>, instance: Instance<'tcx>, ty: Ty<'tcx>) -> Ty<'tcx> {
    instance.instantiate_mir_and_normalize_erasing_regions(
        tcx,
        ty::TypingEnv::fully_monomorphized(),
        ty::EarlyBinder::bind(tcx, ty),
    )
}

fn is_cap_ptr(ty: Ty<'_>) -> bool {
    match *ty.kind() {
        ty::Ref(..) => true,
        ty::RawPtr(..) => !ty.is_fn_ptr(),
        _ => false,
    }
}

/// Byte offset of the field path `elems` (Field projections only, structs and tuples)
/// inside a value of type `ty`.
fn fields_offset<'tcx>(
    tcx: TyCtxt<'tcx>,
    instance: Instance<'tcx>,
    mut ty: Ty<'tcx>,
    elems: &[mir::PlaceElem<'tcx>],
) -> Option<(u64, Ty<'tcx>)> {
    let cx = rustc_middle::ty::layout::LayoutCx::new(tcx, ty::TypingEnv::fully_monomorphized());
    let mut off = 0u64;
    for e in elems {
        let ProjectionElem::Field(f, fty) = *e else { return None };
        let ok = match ty.kind() {
            ty::Adt(def, _) => def.is_struct(),
            ty::Tuple(_) => true,
            _ => false,
        };
        if !ok {
            return None;
        }
        let layout = cx.layout_of(ty).ok()?;
        if f.index() >= layout.fields.count() {
            return None;
        }
        off = off.checked_add(layout.fields.offset(f.index()).bytes())?;
        ty = mono(tcx, instance, fty);
    }
    Some((off, ty))
}

struct Defs {
    count: Vec<u32>,
}

impl<'tcx> Visitor<'tcx> for Defs {
    fn visit_place(&mut self, place: &mir::Place<'tcx>, ctx: PlaceContext, _loc: mir::Location) {
        // A write through `*L` does not redefine `L`.
        let through_deref = matches!(place.projection.first(), Some(ProjectionElem::Deref));
        if ctx.is_mutating_use() && !through_deref {
            self.count[place.local.index()] += 1;
        }
    }
}

/// A pointer traced to an argument: `arg + off`, plus `var = (b, scale)` meaning
/// `+ arg_b * scale` (only inside a callee summary, e.g. `<*mut T>::add`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct PtrSym {
    arg: usize,
    off: u64,
    var: Option<(usize, u64)>,
}

/// Calls are followed this many levels deep (`add`, `write`, `cast` at O0/O1, where
/// the MIR inliner has not run, and small user helpers), into callees of at most
/// this many blocks and statements.
const MAX_CALL_DEPTH: u32 = 5;
const MAX_CALLEE_BLOCKS: usize = 16;
const MAX_CALLEE_STMTS: usize = 128;

/// Callee summaries computed during one `store_marks` query: the callee's store
/// marks and the pointer its return value traces to.
type Cache<'tcx> = RefCell<FxHashMap<Instance<'tcx>, (Rc<Vec<StoreMark>>, Option<PtrSym>)>>;

type CallArgs<'tcx> = [rustc_span::Spanned<Operand<'tcx>>];

struct Analysis<'a, 'tcx> {
    tcx: TyCtxt<'tcx>,
    instance: Instance<'tcx>,
    body: &'a mir::Body<'tcx>,
    call_depth: u32,
    cache: &'a Cache<'tcx>,
    defs: Vec<u32>,
    /// The defining rvalues (whole-local assignments) of each local.
    def_stmt: FxHashMap<mir::Local, Vec<&'a Rvalue<'tcx>>>,
    /// The defining calls (`func`, arguments) of each local.
    def_call: FxHashMap<mir::Local, Vec<(&'a Operand<'tcx>, &'a CallArgs<'tcx>)>>,
}

impl<'a, 'tcx> Analysis<'a, 'tcx> {
    fn new(
        tcx: TyCtxt<'tcx>,
        instance: Instance<'tcx>,
        body: &'a mir::Body<'tcx>,
        call_depth: u32,
        cache: &'a Cache<'tcx>,
    ) -> Self {
        let mut d = Defs { count: vec![0; body.local_decls.len()] };
        d.visit_body(body);
        let mut def_stmt: FxHashMap<_, Vec<_>> = FxHashMap::default();
        let mut def_call: FxHashMap<_, Vec<_>> = FxHashMap::default();
        for bb in body.basic_blocks.iter() {
            for st in &bb.statements {
                if let StatementKind::Assign(a) = &st.kind
                    && a.0.projection.is_empty()
                {
                    def_stmt.entry(a.0.local).or_default().push(&a.1);
                }
            }
            if let Some(t) = &bb.terminator
                && let mir::TerminatorKind::Call { func, args, destination, .. } = &t.kind
                && destination.projection.is_empty()
            {
                def_call.entry(destination.local).or_default().push((func, &args[..]));
            }
        }
        Analysis { tcx, instance, body, call_depth, cache, defs: d.count, def_stmt, def_call }
    }

    fn local_ty(&self, l: mir::Local) -> Ty<'tcx> {
        mono(self.tcx, self.instance, self.body.local_decls[l].ty)
    }

    fn is_arg(&self, l: mir::Local) -> bool {
        l.index() >= 1 && l.index() <= self.body.arg_count
    }

    /// An argument local that is never reassigned, as a 0-based argument index.
    fn arg_of(&self, l: mir::Local) -> Option<usize> {
        (self.is_arg(l) && self.defs[l.index()] == 0).then(|| l.index() - 1)
    }

    /// Whether every write to `l` is a whole-local assignment or call result (at most
    /// four of them, e.g. the two arms of `cheri_narrow_elems`), and nothing else
    /// (no `&mut l`, no partial write).
    fn defs_ok(&self, l: mir::Local) -> bool {
        let n = self.defs[l.index()] as usize;
        let k = self.def_stmt.get(&l).map_or(0, |v| v.len())
            + self.def_call.get(&l).map_or(0, |v| v.len());
        !self.is_arg(l) && n >= 1 && n <= 4 && n == k
    }

    /// The common value of `f` over all definitions of `l`, if they all agree.
    fn all_defs<T: PartialEq>(
        &self,
        l: mir::Local,
        mut f_stmt: impl FnMut(&Rvalue<'tcx>) -> Option<T>,
        mut f_call: impl FnMut(&Operand<'tcx>, &CallArgs<'tcx>) -> Option<T>,
    ) -> Option<T> {
        if !self.defs_ok(l) {
            return None;
        }
        let mut res: Option<T> = None;
        let mut add = |v: Option<T>| -> bool {
            match (v, &res) {
                (None, _) => false,
                (Some(v), None) => {
                    res = Some(v);
                    true
                }
                (Some(v), Some(r)) => v == *r,
            }
        };
        for rv in self.def_stmt.get(&l).into_iter().flatten() {
            if !add(f_stmt(rv)) {
                return None;
            }
        }
        for (func, args) in self.def_call.get(&l).into_iter().flatten() {
            if !add(f_call(func, args)) {
                return None;
            }
        }
        res
    }

    fn plain_local(op: &Operand<'tcx>) -> Option<mir::Local> {
        match op {
            Operand::Copy(p) | Operand::Move(p) => p.as_local(),
            _ => None,
        }
    }

    fn const_usize(&self, op: &Operand<'tcx>) -> Option<u64> {
        let env = ty::TypingEnv::fully_monomorphized();
        let c = self.instance.instantiate_mir_and_normalize_erasing_regions(
            self.tcx,
            env,
            ty::EarlyBinder::bind(self.tcx, op.constant()?.const_),
        );
        c.try_eval_target_usize(self.tcx, env)
    }

    fn size_of(&self, ty: Ty<'tcx>) -> Option<u64> {
        let cx = rustc_middle::ty::layout::LayoutCx::new(self.tcx, ty::TypingEnv::fully_monomorphized());
        Some(cx.layout_of(ty).ok()?.size.bytes())
    }

    /// The callee of a direct call, resolved, with its MIR available.
    fn callee(&self, func: &Operand<'tcx>) -> Option<Instance<'tcx>> {
        if self.call_depth >= MAX_CALL_DEPTH {
            return None;
        }
        let fty = mono(self.tcx, self.instance, func.ty(self.body, self.tcx));
        let ty::FnDef(did, args) = *fty.kind() else { return None };
        if !matches!(
            self.tcx.def_kind(did),
            rustc_hir::def::DefKind::Fn | rustc_hir::def::DefKind::AssocFn
        ) || self.tcx.intrinsic(did).is_some()
        {
            return None;
        }
        let inst = Instance::try_resolve(self.tcx, ty::TypingEnv::fully_monomorphized(), did, args.no_bound_vars()?)
            .ok()??;
        let ty::InstanceKind::Item(cdid) = inst.def else { return None };
        if self.tcx.is_foreign_item(cdid) {
            return None;
        }
        // [LLM-generated] M7.2h: an upstream callee without MIR is summarized by the
        // marks of its crate's metadata (see `summary`).
        if !self.tcx.is_mir_available(cdid) && extern_marks(self.tcx, cdid).is_empty() {
            return None;
        }
        Some(inst)
    }

    /// The store marks and return-pointer trace of a callee (cached; small bodies only).
    fn summary(&self, inst: Instance<'tcx>) -> (Rc<Vec<StoreMark>>, Option<PtrSym>) {
        if let Some(s) = self.cache.borrow().get(&inst) {
            return s.clone();
        }
        // Mark the callee first, so a recursive call sees an empty summary.
        self.cache.borrow_mut().insert(inst, (Rc::new(Vec::new()), None));
        // [LLM-generated] M7.2h: no MIR here (an upstream non-generic function): its
        // metadata marks, and no return-pointer trace.
        if !self.tcx.is_mir_available(inst.def_id()) {
            let res = (Rc::new(extern_marks(self.tcx, inst.def_id())), None);
            self.cache.borrow_mut().insert(inst, res.clone());
            return res;
        }
        let body = self.tcx.instance_mir(inst.def);
        let nstmts: usize = body.basic_blocks.iter().map(|b| b.statements.len()).sum();
        let res = if body.basic_blocks.len() > MAX_CALLEE_BLOCKS || nstmts > MAX_CALLEE_STMTS {
            (Rc::new(Vec::new()), None)
        } else {
            let sub = Analysis::new(self.tcx, inst, body, self.call_depth + 1, self.cache);
            let mut marks = Vec::new();
            sub.run(&mut marks);
            marks.sort();
            marks.dedup();
            let ret = sub.trace_ptr(mir::RETURN_PLACE, 0);
            (Rc::new(marks), ret)
        };
        self.cache.borrow_mut().insert(inst, res.clone());
        res
    }

    /// Map an operand of a call to the caller's terms: an argument offset, a constant
    /// (for a scaled variable part), or nothing.
    fn op_ptr(&self, op: &Operand<'tcx>, depth: u32) -> Option<PtrSym> {
        self.trace_ptr(Self::plain_local(op)?, depth + 1)
    }

    /// Pointer local derived from a pointer argument by copies, pointer casts,
    /// `offset`, field addresses, the `cheri_contract_narrow` intrinsic (which keeps
    /// the address) and calls whose own return value is so derived.
    fn trace_ptr(&self, l: mir::Local, depth: u32) -> Option<PtrSym> {
        if depth > MAX_DEPTH {
            return None;
        }
        if let Some(a) = self.arg_of(l) {
            return is_cap_ptr(self.local_ty(l)).then_some(PtrSym { arg: a, off: 0, var: None });
        }
        self.all_defs(
            l,
            |rv| self.trace_ptr_rv(rv, depth),
            |func, args| self.trace_ptr_call(l, func, args, depth),
        )
    }

    fn trace_ptr_rv(&self, rv: &Rvalue<'tcx>, depth: u32) -> Option<PtrSym> {
        match rv {
            Rvalue::Use(op, _) | Rvalue::Cast(CastKind::PtrToPtr, op, _) => self.op_ptr(op, depth),
            Rvalue::BinaryOp(BinOp::Offset, ops) => {
                let (base, n) = &**ops;
                let b = Self::plain_local(base)?;
                let sz = self.size_of(self.local_ty(b).builtin_deref(true)?)?;
                let mut s = self.trace_ptr(b, depth + 1)?;
                if let Some(k) = self.const_usize(n) {
                    s.off = s.off.checked_add(k.checked_mul(sz)?)?;
                } else {
                    // A variable count is kept only when it is an argument and no
                    // other variable part is present (callee summaries).
                    let nl = Self::plain_local(n)?;
                    let na = self.arg_of(nl)?;
                    if s.var.is_some() {
                        return None;
                    }
                    s.var = Some((na, sz));
                }
                Some(s)
            }
            Rvalue::RawPtr(_, p) | Rvalue::Ref(_, _, p) => self.trace_place_addr(*p, depth),
            _ => None,
        }
    }

    fn trace_ptr_call(
        &self,
        l: mir::Local,
        func: &Operand<'tcx>,
        args: &CallArgs<'tcx>,
        depth: u32,
    ) -> Option<PtrSym> {
        let fty = mono(self.tcx, self.instance, func.ty(self.body, self.tcx));
        if let ty::FnDef(did, _) = *fty.kind()
            && self.tcx.intrinsic(did).map(|i| i.name) == Some(sym::cheri_contract_narrow)
        {
            return self.op_ptr(&args.first()?.node, depth);
        }
        // A call whose return value is an argument plus an offset.
        if !is_cap_ptr(self.local_ty(l)) {
            return None;
        }
        let inst = self.callee(func)?;
        let r = self.summary(inst).1?;
        let mut s = self.op_ptr(&args.get(r.arg)?.node, depth)?;
        s.off = s.off.checked_add(r.off)?;
        if let Some((b, scale)) = r.var {
            let bop = &args.get(b)?.node;
            if let Some(k) = self.const_usize(bop) {
                s.off = s.off.checked_add(k.checked_mul(scale)?)?;
            } else {
                let na = self.arg_of(Self::plain_local(bop)?)?;
                if s.var.is_some() {
                    return None;
                }
                s.var = Some((na, scale));
            }
        }
        Some(s)
    }

    /// Address of `(*L).f1.f2…` (or `(*L)`): `L`'s trace plus the field offsets.
    fn trace_place_addr(&self, p: mir::Place<'tcx>, depth: u32) -> Option<PtrSym> {
        let (first, rest) = p.projection.split_first()?;
        if !matches!(first, ProjectionElem::Deref) {
            return None;
        }
        let pointee = self.local_ty(p.local).builtin_deref(true)?;
        let (fo, _) = fields_offset(self.tcx, self.instance, pointee, rest)?;
        let mut s = self.trace_ptr(p.local, depth + 1)?;
        s.off = s.off.checked_add(fo)?;
        Some(s)
    }

    /// Whether argument `a` is a shared reference to a `Freeze` type (so the caller
    /// reads, after the call, the same value the callee read through it).
    fn shared_freeze_arg(&self, a: usize) -> bool {
        let aty = self.local_ty(mir::Local::from_usize(a + 1));
        matches!(*aty.kind(), ty::Ref(_, pointee, mir::Mutability::Not)
            if pointee.is_freeze(self.tcx, ty::TypingEnv::fully_monomorphized()))
    }

    /// The stored value: an argument itself, or the pointer read at a constant offset
    /// through a shared-reference argument.
    fn trace_cap(&self, op: &Operand<'tcx>, depth: u32) -> Option<(usize, Option<u64>)> {
        if depth > MAX_DEPTH {
            return None;
        }
        let place = match op {
            Operand::Copy(p) | Operand::Move(p) => *p,
            _ => return None,
        };
        if let Some(l) = place.as_local() {
            if let Some(a) = self.arg_of(l) {
                return is_cap_ptr(self.local_ty(l)).then_some((a, None));
            }
            return self.all_defs(l, |rv| self.trace_cap_rv(rv, depth), |_, _| None);
        }
        self.trace_cap_read(place)
    }

    fn trace_cap_rv(&self, rv: &Rvalue<'tcx>, depth: u32) -> Option<(usize, Option<u64>)> {
        match rv {
            Rvalue::Use(op, _)
            | Rvalue::Cast(
                CastKind::PtrToPtr | CastKind::Transmute | CastKind::BoxDerefTransmute,
                op,
                _,
            ) => self.trace_cap(op, depth + 1),
            // `&raw const *x` / `&*x`: the same pointer value.
            Rvalue::RawPtr(_, p) | Rvalue::Ref(_, _, p)
                if p.projection.len() == 1
                    && matches!(p.projection[0], ProjectionElem::Deref) =>
            {
                self.trace_cap(&Operand::Copy(mir::Place::from(p.local)), depth + 1)
            }
            _ => None,
        }
    }

    /// A read `(*a).f1.f2…` through a shared reference argument.
    fn trace_cap_read(&self, place: mir::Place<'tcx>) -> Option<(usize, Option<u64>)> {
        let (first, rest) = place.projection.split_first()?;
        if !matches!(first, ProjectionElem::Deref) {
            return None;
        }
        let a = self.arg_of(place.local)?;
        if !self.shared_freeze_arg(a) {
            return None;
        }
        let pointee = self.local_ty(place.local).builtin_deref(true)?;
        let (off, fty) = fields_offset(self.tcx, self.instance, pointee, rest)?;
        // The loaded value must be pointer-sized (a pointer, or a newtype of one).
        if self.size_of(fty)? != self.size_of(Ty::new_imm_ptr(self.tcx, self.tcx.types.u8))? {
            return None;
        }
        Some((a, Some(off)))
    }

    fn push(out: &mut Vec<StoreMark>, dst: PtrSym, src: (usize, Option<u64>)) {
        if dst.var.is_some() || src.0 == dst.arg {
            return;
        }
        out.push(StoreMark { src: src.0, dst: dst.arg, off: dst.off, src_deref: src.1 });
    }

    fn run(&self, out: &mut Vec<StoreMark>) {
        for bb in self.body.basic_blocks.iter() {
            for st in &bb.statements {
                let StatementKind::Assign(a) = &st.kind else { continue };
                let (place, rv) = &**a;
                let op = match rv {
                    Rvalue::Use(op, _) | Rvalue::Cast(CastKind::PtrToPtr, op, _) => op,
                    _ => continue,
                };
                let pty = mono(self.tcx, self.instance, place.ty(self.body, self.tcx).ty);
                if !is_cap_ptr(pty) {
                    continue;
                }
                let Some(dst) = self.trace_place_addr(*place, 0) else { continue };
                let Some(src) = self.trace_cap(op, 0) else { continue };
                Self::push(out, dst, src);
            }
            // A call to a function that itself stores one argument into another
            // (`ptr::write`, `<*mut T>::write` before MIR inlining, a user helper).
            let Some(t) = &bb.terminator else { continue };
            let mir::TerminatorKind::Call { func, args, .. } = &t.kind else { continue };
            // Only calls that pass two or more pointers can store one into another.
            let nptr = args
                .iter()
                .filter(|a| is_cap_ptr(mono(self.tcx, self.instance, a.node.ty(self.body, self.tcx))))
                .count();
            if nptr < 2 {
                continue;
            }
            let Some(inst) = self.callee(func) else { continue };
            let inner = self.summary(inst).0;
            for &m in inner.iter() {
                let (Some(da), Some(sa)) = (args.get(m.dst), args.get(m.src)) else { continue };
                let Some(mut dst) = self.op_ptr(&da.node, 0) else { continue };
                let Some(o) = dst.off.checked_add(m.off) else { continue };
                dst.off = o;
                let src = match m.src_deref {
                    None => self.trace_cap(&sa.node, 0),
                    // The callee reads through its argument: the caller's operand
                    // must trace to a shared-reference argument plus an offset.
                    Some(d) => self.op_ptr(&sa.node, 0).and_then(|s| {
                        (s.var.is_none() && self.shared_freeze_arg(s.arg))
                            .then(|| s.off.checked_add(d).map(|o| (s.arg, Some(o))))
                            .flatten()
                    }),
                };
                let Some(src) = src else { continue };
                Self::push(out, dst, src);
            }
        }
    }
}

/// Marks from written `stores(p, into = q.f1.f2…)` specs: the place must be reached
/// through exactly one pointer level of `q` (so the slot is at a constant offset from
/// `q`), and `p` must be a pointer.
fn spec_marks<'tcx>(
    tcx: TyCtxt<'tcx>,
    instance: Instance<'tcx>,
    def_id: rustc_span::def_id::DefId,
) -> Vec<StoreMark> {
    use rustc_middle::ty::cheri_spec::{CheriSpec, field_index, fn_params, fn_specs, param_index, pointee_of};
    let o = &tcx.sess.opts.unstable_opts;
    if !o.cheri_spec {
        return Vec::new();
    }
    let (spec_did, specs) = fn_specs(tcx, def_id);
    if specs.is_empty() {
        return Vec::new();
    }
    let params = fn_params(tcx, spec_did);
    let sig = tcx.fn_sig(def_id).instantiate(tcx, instance.args).skip_norm_wip();
    let sig = tcx.instantiate_bound_regions_with_erased(sig);
    let inputs = sig.inputs();
    let cx = rustc_middle::ty::layout::LayoutCx::new(tcx, ty::TypingEnv::fully_monomorphized());
    let mut out = Vec::new();
    for s in &specs {
        let CheriSpec::Stores { param, into } = s else { continue };
        let (Some(src), Some(dst)) = (param_index(&params, *param), param_index(&params, into.base))
        else {
            continue;
        };
        if src == dst || src >= inputs.len() || dst >= inputs.len() {
            continue;
        }
        let norm = |t: Ty<'tcx>| tcx.normalize_erasing_regions(ty::TypingEnv::fully_monomorphized(), ty::Unnormalized::new_wip(t));
        if !is_cap_ptr(norm(inputs[src])) || !is_cap_ptr(norm(inputs[dst])) {
            continue;
        }
        // Walk the fields: the first step looks through `dst` (one pointer level);
        // any further pointer level makes the offset unknown (no mark).
        let Some(mut ty) = pointee_of(tcx, norm(inputs[dst])) else { continue };
        let mut off = 0u64;
        let mut ok = true;
        for f in &into.fields {
            let ty::Adt(def, args) = *ty.kind() else { ok = false; break };
            let Some(i) = field_index(def, *f) else { ok = false; break };
            let Ok(layout) = cx.layout_of(ty) else { ok = false; break };
            off += layout.fields.offset(i).bytes();
            let fd = &def.non_enum_variant().fields[rustc_abi::FieldIdx::from_usize(i)];
            ty = norm(fd.ty(tcx, args).skip_norm_wip());
        }
        if ok {
            out.push(StoreMark { src, dst, off, src_deref: None });
        }
    }
    out
}
