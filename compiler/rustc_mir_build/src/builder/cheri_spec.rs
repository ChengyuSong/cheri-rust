//! [LLM-generated] CHERI M7.2e: MIR lowering of the statement spec
//! `#[rustc_cheri_spec = "live(block = .., bytes = .., held = .. [, ty = ..])"]`.
//!
//! The spec sits on a `let` whose initializer calls a `relocates` function. At that
//! call MIR building emits, before the call:
//!
//! * `NonDivergingIntrinsic::CheriSpec(WipeDead { block, live: bytes, size, held })`,
//!   with `size` the callee's `relocates(size = ..)` over the call's arguments.
//!   Codegen wipes `[block + bytes, block + size)` if `held` may hold capabilities.
//! * With `-Zcheri-spec-relocate-rewrite` and `relocates(alloc = .., free = ..)`: a
//!   branch on `cheri_ty_may_contain_caps::<ty>()` whose true arm replaces the call by
//!   `alloc(..)`, then (on `Ok`) a `Relocated { from: block, to, keep, size }` copy +
//!   wipe of the whole old block, then `free(..)`. The false arm is the original call.
//!
//! Nothing is emitted unless the session is a CHERI target with `-Zcheri-annotate`,
//! `-Zcheri-wipe-cap-words`, `-Zcheri-wipe-alloc-hints` and `-Zcheri-spec`, so other
//! builds get unchanged MIR. Malformed statement specs are errors on every target.

use rustc_abi::{FieldIdx, VariantIdx};
use rustc_hir::HirId;
use rustc_hir::def::{DefKind, Res};
use rustc_middle::mir::*;
use rustc_middle::thir::{ExprId, ExprKind};
use rustc_middle::ty::cheri_spec::{
    CheriSpec, SpecBinOp, SpecExpr, SpecPlace, SpecTy, field_index, fn_specs, generic_ty,
    parse_spec, pointee_of, show_place, sibling_method,
};
use rustc_middle::ty::{self, Ty};
use rustc_span::{Span, Spanned, Symbol, sym};

use crate::builder::{BlockAnd, BlockAndExtension, Builder, LocalsForNode};

/// A resolved `live` spec waiting for its call.
pub(crate) struct PendingLive<'tcx> {
    block: Place<'tcx>,
    bytes: Vec<CheriSpecItem<'tcx>>,
    held: Ty<'tcx>,
    live_ty: Ty<'tcx>,
}

impl<'a, 'tcx> Builder<'a, 'tcx> {
    fn cheri_spec_mir_enabled(&self) -> bool {
        let sess = self.tcx.sess;
        let o = &sess.opts.unstable_opts;
        sess.target.is_like_cheri
            && o.cheri_annotate
            && o.cheri_wipe_cap_words
            && o.cheri_wipe_alloc_hints
            && o.cheri_spec
    }

    /// A `let` statement: resolve its `live` specs (if any) and remember them for
    /// the initializer's call.
    pub(crate) fn cheri_note_live(&mut self, hir_id: HirId, init: ExprId) {
        let tcx = self.tcx;
        let Some(items) = rustc_hir::find_attr!(tcx, hir_id, RustcCheriSpec(v) => v.clone()) else {
            return;
        };
        for (s, span) in items.iter() {
            let Ok(CheriSpec::Live { block, bytes, held, ty }) = parse_spec(s.as_str()) else {
                // Reported by `check_attr`.
                continue;
            };
            match self.cheri_resolve_live(init, &block, &bytes, &held, ty.as_ref()) {
                Ok((call, p)) => {
                    if self.cheri_spec_mir_enabled() {
                        self.cheri_live.insert(call, p);
                    }
                }
                Err(e) => {
                    tcx.dcx().span_err(*span, format!("invalid `rustc_cheri_spec`: {e}"));
                }
            }
        }
    }

    fn cheri_resolve_live(
        &mut self,
        init: ExprId,
        block: &SpecPlace,
        bytes: &SpecExpr,
        held: &SpecTy,
        ty: Option<&SpecTy>,
    ) -> Result<(ExprId, PendingLive<'tcx>), String> {
        let tcx = self.tcx;
        let mut e = init;
        loop {
            match self.thir[e].kind {
                ExprKind::Scope { value, .. } => e = value,
                _ => break,
            }
        }
        let ExprKind::Call { fun, .. } = self.thir[e].kind else {
            return Err("`live` must be on a `let` whose initializer is a call".to_string());
        };
        let ty::FnDef(callee, _) = *self.thir[fun].ty.kind() else {
            return Err("`live`: the initializer does not call a named function".to_string());
        };
        if !fn_specs(tcx, callee).1.iter().any(|s| matches!(s, CheriSpec::Relocates { .. })) {
            return Err(format!(
                "`live`: `{}` has no `relocates` spec",
                tcx.def_path_str(callee)
            ));
        }
        let block = self.cheri_local_place(block)?;
        let bty = block.ty(&self.local_decls, tcx).ty;
        if pointee_of(tcx, bty).is_none() {
            return Err(format!("`block`: `{bty}` is not a pointer"));
        }
        let mut items = Vec::new();
        self.cheri_local_expr(bytes, &mut items)?;
        let held = self.cheri_local_ty(held)?;
        let live_ty = match ty {
            Some(t) => self.cheri_local_ty(t)?,
            None => held,
        };
        Ok((e, PendingLive { block, bytes: items, held, live_ty }))
    }

    /// The unique local variable named `name` declared so far.
    fn cheri_local(&self, name: Symbol) -> Result<Local, String> {
        let mut found = None;
        // The result does not depend on the iteration order: two matches are an error.
        #[allow(rustc::potential_query_instability)]
        for (var, locals) in self.var_indices.iter() {
            if self.tcx.hir_name(var.0) == name {
                let LocalsForNode::One(l) = *locals else {
                    return Err(format!("`{name}` is a match-guard binding"));
                };
                if found.is_some() {
                    return Err(format!("more than one local variable is named `{name}`"));
                }
                found = Some(l);
            }
        }
        found.ok_or_else(|| format!("no local variable named `{name}`"))
    }

    fn cheri_local_place(&self, p: &SpecPlace) -> Result<Place<'tcx>, String> {
        let tcx = self.tcx;
        let mut place = Place::from(self.cheri_local(p.base)?);
        for f in &p.fields {
            let mut t = place.ty(&self.local_decls, tcx).ty;
            // Look through references only (a statement place is a MIR place).
            while let ty::Ref(_, inner, _) = *t.kind() {
                place = place.project_deeper(&[ProjectionElem::Deref], tcx);
                t = inner;
            }
            let ty::Adt(def, _) = *t.kind() else {
                return Err(format!("`{}`: `{t}` has no field `{f}`", show_place(p)));
            };
            let Some(i) = field_index(def, *f) else {
                return Err(format!("`{}`: `{t}` has no field `{f}`", show_place(p)));
            };
            place = place.project_to_field(FieldIdx::from_usize(i), &self.local_decls, tcx);
        }
        Ok(place)
    }

    fn cheri_local_ty(&self, t: &SpecTy) -> Result<Ty<'tcx>, String> {
        let tcx = self.tcx;
        match t {
            SpecTy::Param(n) => generic_ty(tcx, self.def_id.to_def_id(), *n)
                .ok_or_else(|| format!("`{n}` is not a generic type parameter or `Self` here")),
            SpecTy::Pointee(p) => {
                let pt = self.cheri_local_place(p)?.ty(&self.local_decls, tcx).ty;
                pointee_of(tcx, pt)
                    .ok_or_else(|| format!("`*{}`: `{pt}` is not a pointer", show_place(p)))
            }
        }
    }

    fn cheri_local_expr(
        &self,
        e: &SpecExpr,
        out: &mut Vec<CheriSpecItem<'tcx>>,
    ) -> Result<(), String> {
        let tcx = self.tcx;
        match e {
            SpecExpr::Lit(n) => out.push(CheriSpecItem::Const(*n)),
            SpecExpr::Place(p) => {
                let place = self.cheri_local_place(p)?;
                let t = place.ty(&self.local_decls, tcx).ty;
                if !rustc_middle::ty::cheri_spec::is_integer_like(tcx, t) {
                    return Err(format!("`{}` has type `{t}`, not an integer", show_place(p)));
                }
                out.push(CheriSpecItem::Operand(Operand::Copy(place)));
            }
            SpecExpr::SizeOf(t) => out.push(CheriSpecItem::SizeOf(self.cheri_local_ty(t)?)),
            SpecExpr::SizeOfPointee(p) => {
                let t = self.cheri_local_ty(&SpecTy::Pointee(p.clone()))?;
                out.push(CheriSpecItem::SizeOf(t));
            }
            SpecExpr::Bin(op, a, b) => {
                self.cheri_local_expr(a, out)?;
                self.cheri_local_expr(b, out)?;
                out.push(match op {
                    SpecBinOp::Add => CheriSpecItem::Add,
                    SpecBinOp::Sub => CheriSpecItem::Sub,
                    SpecBinOp::Mul => CheriSpecItem::Mul,
                    SpecBinOp::Rem => CheriSpecItem::Rem,
                });
            }
            SpecExpr::Old(_) => return Err("`old(..)` is only allowed in `vacates`".to_string()),
        }
        Ok(())
    }

    /// The callee parameter `name` as an operand that can be read again (a copy of
    /// the argument's place).
    fn cheri_arg(
        &self,
        names: &[Option<Symbol>],
        args: &[Spanned<Operand<'tcx>>],
        name: Symbol,
    ) -> Option<Place<'tcx>> {
        let i = names.iter().position(|n| *n == Some(name))?;
        let place = args.get(i)?.node.place()?;
        let t = place.ty(&self.local_decls, self.tcx).ty;
        self.tcx.type_is_copy_modulo_regions(self.typing_env(), t).then_some(place)
    }

    fn cheri_arg_expr(
        &self,
        names: &[Option<Symbol>],
        args: &[Spanned<Operand<'tcx>>],
        e: &SpecExpr,
        out: &mut Vec<CheriSpecItem<'tcx>>,
    ) -> Option<()> {
        let tcx = self.tcx;
        match e {
            SpecExpr::Lit(n) => out.push(CheriSpecItem::Const(*n)),
            SpecExpr::Place(p) => {
                let mut place = self.cheri_arg(names, args, p.base)?;
                for f in &p.fields {
                    let mut t = place.ty(&self.local_decls, tcx).ty;
                    while let ty::Ref(_, inner, _) = *t.kind() {
                        place = place.project_deeper(&[ProjectionElem::Deref], tcx);
                        t = inner;
                    }
                    let ty::Adt(def, _) = *t.kind() else { return None };
                    let i = field_index(def, *f)?;
                    place = place.project_to_field(FieldIdx::from_usize(i), &self.local_decls, tcx);
                }
                out.push(CheriSpecItem::Operand(Operand::Copy(place)));
            }
            // Types over the callee's generics are not needed by the library specs.
            SpecExpr::SizeOf(_) | SpecExpr::SizeOfPointee(_) => return None,
            SpecExpr::Bin(op, a, b) => {
                self.cheri_arg_expr(names, args, a, out)?;
                self.cheri_arg_expr(names, args, b, out)?;
                out.push(match op {
                    SpecBinOp::Add => CheriSpecItem::Add,
                    SpecBinOp::Sub => CheriSpecItem::Sub,
                    SpecBinOp::Mul => CheriSpecItem::Mul,
                    SpecBinOp::Rem => CheriSpecItem::Rem,
                });
            }
            SpecExpr::Old(_) => return None,
        }
        Some(())
    }

    /// `core::intrinsics::cheri_ty_may_contain_caps`.
    fn cheri_caps_intrinsic(&self) -> Option<rustc_span::def_id::DefId> {
        let tcx = self.tcx;
        let core = tcx.crates(()).iter().copied().find(|&c| tcx.crate_name(c) == sym::core)?;
        let root = core.as_def_id();
        let intr = tcx.module_children(root).iter().find_map(|c| match c.res {
            Res::Def(DefKind::Mod, d) if c.ident.name == sym::intrinsics => Some(d),
            _ => None,
        })?;
        tcx.module_children(intr).iter().find_map(|c| match c.res {
            Res::Def(DefKind::Fn, d) if c.ident.name == sym::cheri_ty_may_contain_caps => Some(d),
            _ => None,
        })
    }

    /// At a call with a pending `live` spec, after its arguments are lowered and
    /// before it is terminated: emit the dead-range wipe and, if enabled, the
    /// relocation rewrite. Returns the block to put the original call in.
    pub(crate) fn cheri_live_at_call(
        &mut self,
        mut block: BasicBlock,
        call: ExprId,
        fun: &Operand<'tcx>,
        args: &[Spanned<Operand<'tcx>>],
        destination: Place<'tcx>,
        source_info: SourceInfo,
        fn_span: Span,
        success: BasicBlock,
    ) -> BlockAnd<()> {
        let Some(p) = self.cheri_live.remove(&call) else {
            return block.unit();
        };
        let tcx = self.tcx;
        let Some((callee, callee_args)) = fun.const_fn_def() else {
            return block.unit();
        };
        let (owner, specs) = fn_specs(tcx, callee);
        let Some(CheriSpec::Relocates { block: rblock, size, keep, alloc, free }) =
            specs.into_iter().find(|s| matches!(s, CheriSpec::Relocates { .. }))
        else {
            return block.unit();
        };
        let names: Vec<Option<Symbol>> =
            tcx.fn_arg_idents(owner).iter().map(|i| i.map(|i| i.name)).collect();
        let mut size_items = Vec::new();
        if self.cheri_arg_expr(&names, args, &size, &mut size_items).is_none() {
            return block.unit();
        }
        self.cfg.push(
            block,
            Statement::new(
                source_info,
                StatementKind::Intrinsic(Box::new(NonDivergingIntrinsic::CheriSpec(Box::new(
                    CheriSpecStmt::WipeDead {
                        block: Operand::Copy(p.block),
                        live: CheriSpecExpr(p.bytes),
                        size: CheriSpecExpr(size_items.clone()),
                        held: p.held,
                    },
                )))),
            ),
        );

        // The relocation rewrite.
        if !tcx.sess.opts.unstable_opts.cheri_spec_relocate_rewrite {
            return block.unit();
        }
        let (Some(alloc), Some(free)) = (alloc, free) else {
            return block.unit();
        };
        let Some(trait_did) = tcx.trait_of_assoc(callee) else {
            return block.unit();
        };
        let (Some(alloc_fn), Some(free_fn), Some(caps_fn)) = (
            sibling_method(tcx, callee, alloc.name),
            sibling_method(tcx, callee, free.name),
            self.cheri_caps_intrinsic(),
        ) else {
            return block.unit();
        };
        let trait_args = tcx.mk_args(&callee_args[..tcx.generics_of(trait_did).count()]);
        let mut keep_items = Vec::new();
        let ops = |b: &Self, call: &rustc_middle::ty::cheri_spec::SpecCall| {
            call.args
                .iter()
                .map(|a| b.cheri_arg(&names, args, *a).map(Operand::Copy))
                .collect::<Option<Vec<_>>>()
        };
        let (Some(alloc_ops), Some(free_ops), Some(from), Some(())) = (
            ops(self, &alloc),
            ops(self, &free),
            self.cheri_arg(&names, args, rblock.base).filter(|_| rblock.fields.is_empty()),
            self.cheri_arg_expr(&names, args, &keep, &mut keep_items),
        ) else {
            return block.unit();
        };
        // `alloc` must return the call's type, a `Result` whose `Ok` holds the new block.
        let dest_ty = destination.ty(&self.local_decls, tcx).ty;
        let alloc_sig = tcx.fn_sig(alloc_fn).instantiate(tcx, trait_args).skip_norm_wip();
        let alloc_out = tcx.instantiate_bound_regions_with_erased(alloc_sig).output();
        if tcx.erase_and_anonymize_regions(alloc_out) != tcx.erase_and_anonymize_regions(dest_ty) {
            return block.unit();
        }
        let ty::Adt(res_def, res_args) = *dest_ty.kind() else {
            return block.unit();
        };
        let Some(ok_idx) = res_def.variants().iter_enumerated().find_map(|(i, v)| {
            (v.name == sym::Ok && v.fields.len() == 1).then_some(i)
        }) else {
            return block.unit();
        };
        let new_ty = res_def.variant(ok_idx).fields[FieldIdx::ZERO].ty(tcx, res_args).skip_norm_wip();
        let ok_discr = res_def.discriminant_for_variant(tcx, ok_idx).val;
        let span = source_info.span;

        // block: _c = cheri_ty_may_contain_caps::<ty>() -> pick
        let c = self.temp(tcx.types.bool, span);
        let pick = self.cfg.start_new_block();
        self.cfg.terminate(
            block,
            source_info,
            TerminatorKind::Call {
                func: Operand::function_handle(tcx, caps_fn, &[p.live_ty.into()], span),
                args: Box::new([]),
                destination: c,
                target: Some(pick),
                unwind: UnwindAction::Unreachable,
                call_source: CallSource::Misc,
                fn_span,
            },
        );
        // pick: switchInt(_c) [0: original call, otherwise: substitute]
        let orig = self.cfg.start_new_block();
        let subst = self.cfg.start_new_block();
        self.cfg.terminate(
            pick,
            source_info,
            TerminatorKind::SwitchInt {
                discr: Operand::Copy(c),
                targets: SwitchTargets::static_if(0, orig, subst),
            },
        );
        // subst: dest = alloc(..) -> check
        let check = self.cfg.start_new_block();
        let spanned = |ops: Vec<Operand<'tcx>>| -> Box<[Spanned<Operand<'tcx>>]> {
            ops.into_iter().map(|node| Spanned { node, span }).collect()
        };
        self.cfg.terminate(
            subst,
            source_info,
            TerminatorKind::Call {
                func: Operand::function_handle(tcx, alloc_fn, trait_args, span),
                args: spanned(alloc_ops),
                destination,
                target: Some(check),
                unwind: UnwindAction::Continue,
                call_source: CallSource::Misc,
                fn_span,
            },
        );
        self.diverge_from(subst);
        // check: switchInt(discriminant(dest)) [Ok: copy, otherwise: success]
        let d = self.temp(dest_ty.discriminant_ty(tcx), span);
        self.cfg.push_assign(check, source_info, d, Rvalue::Discriminant(destination));
        let copy = self.cfg.start_new_block();
        self.cfg.terminate(
            check,
            source_info,
            TerminatorKind::SwitchInt {
                discr: Operand::Copy(d),
                targets: SwitchTargets::static_if(ok_discr, copy, success),
            },
        );
        // copy: _n = (dest as Ok).0; Relocated; free(..) -> success
        let n = self.temp(new_ty, span);
        let ok_place = destination.project_deeper(
            &[
                ProjectionElem::Downcast(Some(sym::Ok), ok_idx),
                ProjectionElem::Field(FieldIdx::ZERO, new_ty),
            ],
            tcx,
        );
        self.cfg.push_assign(copy, source_info, n, Rvalue::Use(Operand::Copy(ok_place), WithRetag::Yes));
        self.cfg.push(
            copy,
            Statement::new(
                source_info,
                StatementKind::Intrinsic(Box::new(NonDivergingIntrinsic::CheriSpec(Box::new(
                    CheriSpecStmt::Relocated {
                        from: Operand::Copy(from),
                        to: Operand::Copy(n),
                        keep: CheriSpecExpr(keep_items),
                        size: CheriSpecExpr(size_items),
                        ty: p.live_ty,
                    },
                )))),
            ),
        );
        let unit = self.temp(tcx.types.unit, span);
        self.cfg.terminate(
            copy,
            source_info,
            TerminatorKind::Call {
                func: Operand::function_handle(tcx, free_fn, trait_args, span),
                args: spanned(free_ops),
                destination: unit,
                target: Some(success),
                unwind: UnwindAction::Continue,
                call_source: CallSource::Misc,
                fn_span,
            },
        );
        self.diverge_from(copy);
        block = orig;
        let _ = VariantIdx::ZERO;
        block.unit()
    }
}
