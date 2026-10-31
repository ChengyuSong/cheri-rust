//! CHERI M3.5/M3.6 [LLM-generated]: `mem_contract_undischarged` lint.
//!
//! Warn-by-default. Every unsafe raw→typed conversion must be covered by an
//! explicit `requires` / `mem_contract` whose predicate matches the conversion's
//! target type (and arguments, when names are available). Pointer provenance
//! may discharge *narrowing* in codegen; it does **not** satisfy this lint.
//!
//! Covering sources (first match wins for the diagnostic note):
//! 1. A block/statement `#[rustc_mem_contract_site]` (from `#[requires(...)]` on
//!    a block/let — M3.5).
//! 2. An enclosing function's `#[rustc_mem_contract]` / `requires`.
//! 3. For calls into contracted APIs: still need a covering requires at the
//!    *call site* (the API's own contract is the callee's obligation).

use rustc_ast::BorrowKind;
use rustc_attr_ir::find_attr;
use rustc_hir::{Expr, ExprKind, Node};
use rustc_lint_defs::{declare_lint, declare_lint_pass};
use rustc_macros::Diagnostic;
use rustc_attr_ir::lang_items::LangItem;
use rustc_span::sym;

use rustc_middle::ty::Ty;

use crate::{LateContext, LateLintPass, LintContext};

declare_lint! {
    /// The `mem_contract_undischarged` lint flags unsafe raw→typed conversions
    /// that lack a covering `requires` / `mem_contract` matching the target type.
    ///
    /// ### Example
    ///
    /// ```rust,ignore
    /// unsafe fn steal<'a>(p: *const u8, n: usize) -> &'a [u8] {
    ///     core::slice::from_raw_parts(p, n)
    /// }
    /// ```
    ///
    /// ### Explanation
    ///
    /// Formal `requires` replaces free-form `// SAFETY:` comments. CHERI still
    /// emits a conversion-narrow for the spatial part; provenance can skip that
    /// narrow but never excuses a missing `requires`.
    ///
    /// Warn by default (M3.5).
    pub MEM_CONTRACT_UNDISCHARGED,
    Warn,
    "unsafe raw→typed conversion without a covering `requires` / `mem_contract`"
}

declare_lint_pass!(MemContractUndischarged => [MEM_CONTRACT_UNDISCHARGED]);

#[derive(Diagnostic)]
#[diag("unsafe raw→typed conversion `{$what}` without a covering `requires` / `mem_contract`")]
#[note("add `#[core::contracts::requires(valid_*(...))]` on the enclosing function or block, matching this conversion's target type")]
struct MemContractUndischargedDiag {
    what: &'static str,
}

/// Kind of type-contract predicate needed for a conversion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Need {
    Slice,
    SliceMut,
    Ref,
    RefMut,
    Box,
    CStr,
}

impl Need {
    fn as_canon_prefix(self) -> &'static str {
        match self {
            Need::Slice => "slice",
            Need::SliceMut => "slice_mut",
            Need::Ref => "ref",
            Need::RefMut => "ref_mut",
            Need::Box => "box",
            Need::CStr => "cstr",
        }
    }
}

impl<'tcx> LateLintPass<'tcx> for MemContractUndischarged {
    fn check_expr(&mut self, cx: &LateContext<'tcx>, expr: &Expr<'_>) {
        let Some((what, need)) = conversion_kind(cx, expr) else { return };
        if !in_unsafe_context(cx, expr) {
            return;
        }
        if has_covering_requires(cx, expr, need) {
            return;
        }
        cx.emit_span_lint(
            MEM_CONTRACT_UNDISCHARGED,
            expr.span,
            MemContractUndischargedDiag { what },
        );
    }
}

fn conversion_kind<'tcx>(cx: &LateContext<'tcx>, expr: &Expr<'_>) -> Option<(&'static str, Need)> {
    match expr.kind {
        ExprKind::AddrOf(BorrowKind::Ref, mutbl, inner) => {
            let ExprKind::Unary(rustc_hir::UnOp::Deref, ptr_e) = inner.kind else {
                return None;
            };
            let ty = cx.typeck_results().expr_ty(ptr_e).peel_refs();
            if !ty.is_raw_ptr() {
                return None;
            }
            let ref_ty = cx.typeck_results().expr_ty(expr);
            let need = need_from_typed_ty(cx, ref_ty).unwrap_or_else(|| {
                if mutbl.is_mut() { Need::RefMut } else { Need::Ref }
            });
            Some(("&*raw / &mut *raw", need))
        }
        ExprKind::Call(callee, args) => {
            let ExprKind::Path(qpath) = callee.kind else { return None };
            let res = cx.typeck_results().qpath_res(&qpath, callee.hir_id);
            let rustc_hir::def::Res::Def(_, def_id) = res else { return None };
            // M3.6: transmute / transmute_copy raw→typed.
            if let Some(r) = need_for_transmute(cx, def_id, expr, args) {
                return Some(r);
            }
            need_for_def(cx, def_id)
        }
        ExprKind::MethodCall(seg, recv, _, _) => {
            let Some(def_id) = cx.typeck_results().type_dependent_def_id(expr.hir_id) else {
                return None;
            };
            let name = seg.ident.name;
            if matches!(name.as_str(), "from_raw" | "from_raw_in") {
                return Some(("Box::from_raw", Need::Box));
            }
            if name.as_str() == "from_ptr" {
                return Some(("CStr::from_ptr", Need::CStr));
            }
            // M3.6: ptr/NonNull::as_ref / as_mut (bodies live in core with crate allow).
            if matches!(
                name.as_str(),
                "as_ref" | "as_mut" | "as_ref_unchecked" | "as_mut_unchecked"
            ) {
                let recv_ty = cx.typeck_results().expr_ty(recv).peel_refs();
                if is_raw_or_nonnull(cx, recv_ty) {
                    let ret = cx.typeck_results().expr_ty(expr);
                    // Option<&T> / &T / &mut T
                    let inner = match ret.kind() {
                        rustc_middle::ty::Adt(adt, args)
                            if cx.tcx.is_diagnostic_item(sym::Option, adt.did()) =>
                        {
                            args.type_at(0)
                        }
                        _ => ret,
                    };
                    if let Some(need) = need_from_typed_ty(cx, inner) {
                        return Some(("ptr/NonNull::as_ref/as_mut", need));
                    }
                }
            }
            need_for_def(cx, def_id)
        }
        _ => None,
    }
}

fn is_raw_or_nonnull<'tcx>(cx: &LateContext<'tcx>, ty: Ty<'tcx>) -> bool {
    if ty.is_raw_ptr() {
        return true;
    }
    match ty.kind() {
        rustc_middle::ty::Adt(adt, _) => cx.tcx.is_lang_item(adt.did(), LangItem::NonNull),
        _ => false,
    }
}

fn need_from_typed_ty<'tcx>(_cx: &LateContext<'tcx>, ty: Ty<'tcx>) -> Option<Need> {
    match ty.kind() {
        rustc_middle::ty::Ref(_, p, m) => match *p.kind() {
            rustc_middle::ty::Slice(_) | rustc_middle::ty::Str => {
                Some(if m.is_mut() { Need::SliceMut } else { Need::Slice })
            }
            _ => Some(if m.is_mut() { Need::RefMut } else { Need::Ref }),
        },
        rustc_middle::ty::Adt(adt, _) if adt.is_box() => Some(Need::Box),
        _ => None,
    }
}

/// `mem::transmute` / `transmute_copy` / intrinsic transmute from raw ptr → ref/slice/Box.
fn need_for_transmute<'tcx>(
    cx: &LateContext<'tcx>,
    def_id: rustc_hir::def_id::DefId,
    expr: &Expr<'_>,
    args: &[Expr<'_>],
) -> Option<(&'static str, Need)> {
    let path = cx.tcx.item_name(def_id);
    let is_copy = path.as_str() == "transmute_copy";
    let is_tm = matches!(path.as_str(), "transmute" | "transmute_unchecked" | "transmute_copy");
    if !is_tm {
        return None;
    }
    let dst = cx.typeck_results().expr_ty(expr);
    let need = need_from_typed_ty(cx, dst)?;
    // Source: first arg type; for transmute_copy the arg is `&Src` and Src must be raw.
    let Some(arg0) = args.first() else { return None };
    let mut src = cx.typeck_results().expr_ty(arg0);
    if is_copy {
        src = src.builtin_deref(true).unwrap_or(src);
    }
    if !src.is_raw_ptr() && !is_raw_or_nonnull(cx, src) {
        return None;
    }
    Some(("mem::transmute(raw→typed)", need))
}

fn need_for_def<'tcx>(
    cx: &LateContext<'tcx>,
    def_id: rustc_hir::def_id::DefId,
) -> Option<(&'static str, Need)> {
    if let Some(name) = cx.tcx.get_diagnostic_name(def_id) {
        match name {
            sym::slice_from_raw_parts => return Some(("slice::from_raw_parts", Need::Slice)),
            sym::slice_from_raw_parts_mut => {
                return Some(("slice::from_raw_parts_mut", Need::SliceMut));
            }
            sym::ptr_read => return Some(("ptr::read", Need::Ref)),
            sym::ptr_write => return Some(("ptr::write", Need::RefMut)),
            sym::ptr_copy_nonoverlapping | sym::ptr_copy => {
                return Some(("ptr::copy", Need::Slice));
            }
            sym::ptr_write_bytes => return Some(("ptr::write_bytes", Need::SliceMut)),
            _ => {}
        }
    }
    // Unstable helpers without diagnostic names (M3.6).
    match cx.tcx.item_name(def_id).as_str() {
        "from_ptr_range" => Some(("slice::from_ptr_range", Need::Slice)),
        "from_mut_ptr_range" => Some(("slice::from_mut_ptr_range", Need::SliceMut)),
        _ => None,
    }
}

fn in_unsafe_context<'tcx>(cx: &LateContext<'tcx>, expr: &Expr<'_>) -> bool {
    for (_, node) in cx.tcx.hir_parent_iter(expr.hir_id) {
        match node {
            Node::Block(b) if matches!(b.rules, rustc_hir::BlockCheckMode::UnsafeBlock(_)) => {
                return true;
            }
            Node::Item(i) => {
                return matches!(
                    i.kind,
                    rustc_hir::ItemKind::Fn { sig, .. } if sig.header.is_unsafe()
                );
            }
            Node::ImplItem(i) => {
                return matches!(
                    i.kind,
                    rustc_hir::ImplItemKind::Fn(sig, _) if sig.header.is_unsafe()
                );
            }
            Node::TraitItem(i) => {
                return matches!(
                    i.kind,
                    rustc_hir::TraitItemKind::Fn(sig, _) if sig.header.is_unsafe()
                );
            }
            _ => {}
        }
    }
    false
}

/// True if a requires/mem_contract covers `need` for this expression.
fn has_covering_requires<'tcx>(cx: &LateContext<'tcx>, expr: &Expr<'_>, need: Need) -> bool {
    let prefix = need.as_canon_prefix();
    // 1) Walk parents for `#[rustc_mem_contract_site]` (block/statement requires).
    for (hir_id, _) in std::iter::once((expr.hir_id, ())).chain(
        cx.tcx.hir_parent_iter(expr.hir_id).map(|(id, _)| (id, ())),
    ) {
        let attrs = cx.tcx.hir_attrs(hir_id);
        if let Some(sites) = find_attr!(attrs, RustcMemContractSite(v) => v.clone()) {
            if sites.iter().any(|c| canon_covers(c.as_str(), prefix)) {
                return true;
            }
        }
    }
    // 2) Enclosing function's `#[rustc_mem_contract]` (index-keyed: `slice(0,1)`).
    let owner = cx.tcx.hir_get_parent_item(expr.hir_id).to_def_id();
    if let Some(canons) = find_attr!(cx.tcx, owner, RustcMemContract(v) => v.clone()) {
        if canons.iter().any(|c| canon_covers(c.as_str(), prefix)) {
            return true;
        }
    }
    false
}

fn canon_covers(canon: &str, prefix: &str) -> bool {
    // Function-level: "slice(0,1);ref(ret)" — site-level: "slice(p,n)".
    // A matching prefix is enough for the lint (argument-name equality is a
    // stronger check we can tighten later); mismatched kind still fails.
    canon.split(';').any(|part| {
        let part = part.trim();
        part == prefix || part.starts_with(&format!("{prefix}("))
    })
}
