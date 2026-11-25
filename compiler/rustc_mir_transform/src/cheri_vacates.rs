//! [LLM-generated] CHERI M7.3: lowering of the function spec
//! `#[rustc_cheri_spec = "vacates(ty, block, from, to)"]` (see
//! `rustc_middle::ty::cheri_spec::CheriSpec::Vacates`). Runs on the runtime MIR of the
//! annotated function right after the analysis-to-runtime passes (so the copies through
//! `NonNull` can use `Transmute`, and borrowck has already accepted the body); the result
//! is part of the function's optimized MIR and so travels with it into MIR inlining and
//! into every crate that instantiates it. Nothing is emitted unless the session is a
//! CHERI target with `-Zcheri-annotate`, `-Zcheri-wipe-cap-words`,
//! `-Zcheri-wipe-alloc-hints`, `-Zcheri-spec` and `-Zcheri-spec-vacates`.

use rustc_abi::FieldIdx;
use rustc_hir::def::DefKind;
use rustc_middle::mir::*;
use rustc_middle::ty::cheri_spec::{
    CheriSpec, SpecBinOp, SpecExpr, SpecPlace, SpecTy, field_index, parse_spec, pointee_of,
    show_place,
};
use rustc_middle::ty::{self, Ty, TyCtxt};
use rustc_span::{Span, Symbol};

/// [LLM-generated] M7.3: the session gate of `Builder::cheri_spec_mir_enabled` (MIR building), plus
/// `-Zcheri-spec-vacates`, for the `vacates` lowering .
fn cheri_vacates_enabled(tcx: TyCtxt<'_>) -> bool {
    let sess = tcx.sess;
    let o = &sess.opts.unstable_opts;
    sess.target.is_like_cheri
        && o.cheri_annotate
        && o.cheri_wipe_cap_words
        && o.cheri_wipe_alloc_hints
        && o.cheri_spec
        && o.cheri_spec_vacates
}

/// [LLM-generated] M7.3: lower the `vacates(ty, block, from, to)` specs written on the
/// function whose MIR was just built. On entry (start of `START_BLOCK`) the block pointer
/// and every `old(..)` operand are copied into fresh temporaries; before each `Return`
/// the other operands are read into temporaries and a
/// `CheriSpec(WipeDead { block, live: from, size: to, held: ty })` is emitted. Because
/// the statements are in the function's own MIR, they move with it when it is inlined
/// (MIR or LLVM), so the caller of e.g. `Vec::pop` gets the wipe without a spec of its
/// own. Codegen wipes nothing unless `ty` may hold capabilities.
///
/// Places look through references and raw pointers (a deref) and through pointer-like
/// structs such as `NonNull` / `Unique` (copied out by a `Transmute` to a raw pointer
/// first, as their field is a pattern type). Integer operands are read through
/// single-field structs (`WrappedIndex`) and widened to `usize`.
pub(crate) fn lower_vacates<'tcx>(
    tcx: TyCtxt<'tcx>,
    def: rustc_span::def_id::LocalDefId,
    body: &mut Body<'tcx>,
) {
    let def_id = def.to_def_id();
    if !matches!(tcx.def_kind(def_id), DefKind::Fn | DefKind::AssocFn) {
        return;
    }
    let own = rustc_middle::ty::cheri_spec::own_spec_strs(tcx, def_id);
    let specs: Vec<(CheriSpec, Span)> = own
        .iter()
        .filter_map(|(s, sp)| match parse_spec(s.as_str()) {
            Ok(v @ CheriSpec::Vacates { .. }) => Some((v, *sp)),
            _ => None,
        })
        .collect();
    if specs.is_empty() || !cheri_vacates_enabled(tcx) {
        return;
    }
    let names: Vec<Option<Symbol>> =
        tcx.fn_arg_idents(def_id).iter().map(|i| i.map(|i| i.name)).collect();
    let returns: Vec<BasicBlock> = body
        .basic_blocks
        .iter_enumerated()
        .filter(|(_, b)| matches!(b.terminator().kind, TerminatorKind::Return))
        .map(|(bb, _)| bb)
        .collect();
    let mut entry = Vec::new();
    let mut exit = Vec::new();
    for (spec, span) in &specs {
        let mut v = Vac { tcx, def_id, body: &mut *body, names: &names, span: *span };
        if let Err(e) = v.lower(spec, &mut entry, &mut exit) {
            tcx.dcx().span_err(*span, format!("invalid `rustc_cheri_spec`: {e}"));
            return;
        }
    }
    let bbs = body.basic_blocks.as_mut();
    let start = &mut bbs[START_BLOCK].statements;
    start.splice(0..0, entry);
    for bb in returns {
        bbs[bb].statements.extend(exit.iter().cloned());
    }
}

struct Vac<'a, 'tcx> {
    tcx: TyCtxt<'tcx>,
    def_id: rustc_span::def_id::DefId,
    body: &'a mut Body<'tcx>,
    names: &'a [Option<Symbol>],
    span: Span,
}

impl<'a, 'tcx> Vac<'a, 'tcx> {
    fn si(&self) -> SourceInfo {
        SourceInfo::outermost(self.span)
    }

    fn temp(&mut self, t: Ty<'tcx>) -> Local {
        self.body.local_decls.push(LocalDecl::new(t, self.span))
    }

    fn assign(&mut self, out: &mut Vec<Statement<'tcx>>, t: Ty<'tcx>, rv: Rvalue<'tcx>) -> Local {
        let l = self.temp(t);
        out.push(Statement::new(self.si(), StatementKind::Assign(Box::new((Place::from(l), rv)))));
        l
    }

    fn ty_of(&self, p: Place<'tcx>) -> Ty<'tcx> {
        p.ty(&self.body.local_decls, self.tcx).ty
    }

    fn place(&mut self, p: &SpecPlace, out: &mut Vec<Statement<'tcx>>) -> Result<Place<'tcx>, String> {
        let tcx = self.tcx;
        let Some(i) = self.names.iter().position(|n| *n == Some(p.base)) else {
            return Err(format!("no parameter named `{}`", p.base));
        };
        let mut place = Place::from(Local::from_usize(i + 1));
        for f in &p.fields {
            let mut steps = 0;
            loop {
                steps += 1;
                if steps > 8 {
                    return Err(format!("`{}`: too many pointer levels", show_place(p)));
                }
                let t = self.ty_of(place);
                match *t.kind() {
                    ty::Adt(def, _) if field_index(def, *f).is_some() => {
                        let i = field_index(def, *f).unwrap();
                        place = place.project_to_field(FieldIdx::from_usize(i), &self.body.local_decls, tcx);
                        break;
                    }
                    ty::Ref(..) | ty::RawPtr(..) => {
                        place = place.project_deeper(&[ProjectionElem::Deref], tcx);
                    }
                    ty::Adt(def, _) if !def.is_box() => {
                        let Some(pt) = pointee_of(tcx, t) else {
                            return Err(format!("`{}`: `{t}` has no field `{f}`", show_place(p)));
                        };
                        let raw = Ty::new_imm_ptr(tcx, pt);
                        let l = self.assign(out, raw, Rvalue::Cast(CastKind::Transmute, Operand::Copy(place), raw));
                        place = Place::from(l).project_deeper(&[ProjectionElem::Deref], tcx);
                    }
                    _ => return Err(format!("`{}`: `{t}` has no field `{f}`", show_place(p))),
                }
            }
        }
        Ok(place)
    }

    /// `place` (integer-like) widened to `usize`, copied into a temporary now.
    fn int(&mut self, mut place: Place<'tcx>, out: &mut Vec<Statement<'tcx>>) -> Result<Local, String> {
        let tcx = self.tcx;
        for _ in 0..6 {
            let t = self.ty_of(place);
            match *t.kind() {
                ty::Uint(ty::UintTy::Usize) => {
                    return Ok(self.assign(out, t, Rvalue::Use(Operand::Copy(place), WithRetag::No)));
                }
                ty::Int(_) | ty::Uint(_) => {
                    let u = tcx.types.usize;
                    return Ok(self.assign(out, u, Rvalue::Cast(CastKind::IntToInt, Operand::Copy(place), u)));
                }
                ty::Pat(base, _) => {
                    let l = self.assign(out, base, Rvalue::Cast(CastKind::Transmute, Operand::Copy(place), base));
                    place = Place::from(l);
                }
                ty::Adt(def, args) if def.is_struct() => {
                    let mut fs = def
                        .non_enum_variant()
                        .fields
                        .iter_enumerated()
                        .filter(|(_, f)| !matches!(f.ty(tcx, args).skip_norm_wip().kind(), ty::Adt(d, _) if d.is_phantom_data()));
                    let (Some((i, _)), None) = (fs.next(), fs.next()) else {
                        return Err(format!("`{t}` is not an integer"));
                    };
                    place = place.project_to_field(i, &self.body.local_decls, tcx);
                }
                _ => return Err(format!("`{t}` is not an integer")),
            }
        }
        Err("integer operand nested too deeply".to_string())
    }

    fn ty(&self, t: &SpecTy) -> Result<Ty<'tcx>, String> {
        let params = rustc_middle::ty::cheri_spec::fn_params(self.tcx, self.def_id);
        rustc_middle::ty::cheri_spec::spec_ty(self.tcx, self.def_id, &params, t, &|_| None)
    }

    fn expr(
        &mut self,
        e: &SpecExpr,
        old: bool,
        entry: &mut Vec<Statement<'tcx>>,
        exit: &mut Vec<Statement<'tcx>>,
        items: &mut Vec<CheriSpecItem<'tcx>>,
    ) -> Result<(), String> {
        match e {
            SpecExpr::Lit(n) => items.push(CheriSpecItem::Const(*n)),
            SpecExpr::SizeOf(t) => items.push(CheriSpecItem::SizeOf(self.ty(t)?)),
            SpecExpr::SizeOfPointee(p) => {
                items.push(CheriSpecItem::SizeOf(self.ty(&SpecTy::Pointee(p.clone()))?))
            }
            SpecExpr::Place(p) => {
                let out = if old { &mut *entry } else { &mut *exit };
                let place = self.place(p, out)?;
                let l = self.int(place, out)?;
                items.push(CheriSpecItem::Operand(Operand::Copy(Place::from(l))));
            }
            SpecExpr::Old(a) => self.expr(a, true, entry, exit, items)?,
            SpecExpr::Bin(op, a, b) => {
                self.expr(a, old, entry, exit, items)?;
                self.expr(b, old, entry, exit, items)?;
                items.push(match op {
                    SpecBinOp::Add => CheriSpecItem::Add,
                    SpecBinOp::Sub => CheriSpecItem::Sub,
                    SpecBinOp::Mul => CheriSpecItem::Mul,
                    SpecBinOp::Rem => CheriSpecItem::Rem,
                });
            }
        }
        Ok(())
    }

    fn lower(
        &mut self,
        spec: &CheriSpec,
        entry: &mut Vec<Statement<'tcx>>,
        exit: &mut Vec<Statement<'tcx>>,
    ) -> Result<(), String> {
        let tcx = self.tcx;
        let CheriSpec::Vacates { ty, block, addr, from, to } = spec else { return Ok(()) };
        let held = self.ty(ty)?;
        let bplace = self.place(block, entry)?;
        let bt = self.ty_of(bplace);
        let bl = if *addr {
            let raw = Ty::new_imm_ptr(tcx, bt);
            self.assign(entry, raw, Rvalue::RawPtr(RawPtrKind::Const, bplace))
        } else {
            let Some(pt) = pointee_of(tcx, bt) else {
                return Err(format!("`block = {}`: `{bt}` is not a pointer", show_place(block)));
            };
            if matches!(bt.kind(), ty::Adt(d, _) if d.is_box()) {
                return Err(format!("`block = {}`: `Box` is not supported", show_place(block)));
            }
            if matches!(bt.kind(), ty::RawPtr(..)) {
                self.assign(entry, bt, Rvalue::Use(Operand::Copy(bplace), WithRetag::No))
            } else {
                // `&T`, `NonNull<T>`, `Unique<T>`, `Option` of those: same bits as `*const T`.
                let raw = Ty::new_imm_ptr(tcx, pt);
                self.assign(entry, raw, Rvalue::Cast(CastKind::Transmute, Operand::Copy(bplace), raw))
            }
        };
        let mut live = Vec::new();
        self.expr(from, false, entry, exit, &mut live)?;
        let mut size = Vec::new();
        self.expr(to, false, entry, exit, &mut size)?;
        exit.push(Statement::new(
            self.si(),
            StatementKind::Intrinsic(Box::new(NonDivergingIntrinsic::CheriSpec(Box::new(
                CheriSpecStmt::WipeDead {
                    block: Operand::Copy(Place::from(bl)),
                    live: CheriSpecExpr(live),
                    size: CheriSpecExpr(size),
                    held,
                },
            )))),
        ));
        Ok(())
    }
}

