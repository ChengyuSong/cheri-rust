//! [LLM-generated] M7.1d CR-1: emit a typic C-root table of C-visible
//! writable data-capability statics for the late LLVM CHERI pass to walk
//! at `mem.free`.
//!
//! A slot is included when it is a typic *data* cap (`ty_is_data_cap`:
//! raw pointer / reference, not a fn-ptr-only type such as `FFI_CB`) and
//! is either stored from a local `extern "C"` body or exported as C data
//! (`#[no_mangle]` / `#[export_name]`). TLS, immutable, nested, and
//! well-known non-C-root names (`ARENA`, GOT, vtables, `__cap_relocs`)
//! stay out. core / alloc / compiler_builtins emit nothing. An empty set
//! emits nothing, so official KPI crates (typic C-root set 0) stay
//! byte-identical to the merged B2b trees. The table is `linkonce_odr` in
//! section `cheri_croot` so every CGU of a crate that has entries defines
//! the same full table (the late pass must see a constant non-zero count
//! in the module that contains `mem.free`) and CR-2 can later concatenate
//! more entries. The CR-3 symbol `cheri_register_root` is reserved as a
//! declaration only, and only when the table is non-empty (a stub body
//! would change `.text`). rustc does not clear; `-Zcheri-croot=no` emits
//! nothing.

use rustc_abi::ExternAbi;
use rustc_codegen_ssa::traits::*;
use rustc_hir::def::{DefKind, Res};
use rustc_hir::intravisit::{self, Visitor};
use rustc_hir::{Expr, ExprKind, ItemKind, Node, QPath, UnOp};
use rustc_middle::middle::codegen_fn_attrs::CodegenFnAttrFlags;
use rustc_middle::ty::TyCtxt;
use rustc_span::def_id::{DefId, LOCAL_CRATE};

use crate::context::CodegenCx;
use crate::llvm;

const CROOT_SECTION: &std::ffi::CStr = c"cheri_croot";
const CROOT_HEADER: &str = "__cheri_croot";
const CROOT_SLOTS: &str = "__cheri_croot_slots";
const CROOT_REGISTER: &str = "cheri_register_root";

fn croot_enabled(tcx: TyCtxt<'_>) -> bool {
    let sess = tcx.sess;
    sess.target.is_like_cheri
        && sess.opts.unstable_opts.cheri_annotate
        && sess.opts.unstable_opts.cheri_croot
}

fn skipped_crate(tcx: TyCtxt<'_>) -> bool {
    matches!(tcx.crate_name(LOCAL_CRATE).as_str(), "core" | "alloc" | "compiler_builtins")
}

fn excluded_item_name(tcx: TyCtxt<'_>, def_id: DefId) -> bool {
    let name = tcx.item_name(def_id);
    let s = name.as_str();
    s == "ARENA"
        || s == "GOT"
        || s.contains("__cap_relocs")
        || s.starts_with("_ZTV")
        || s.contains("vtable")
        || s.contains("VTABLE")
}

fn is_c_exported_static(tcx: TyCtxt<'_>, def_id: DefId) -> bool {
    let attrs = tcx.codegen_fn_attrs(def_id);
    attrs.flags.contains(CodegenFnAttrFlags::NO_MANGLE) || attrs.symbol_name.is_some()
}

fn is_croot_slot(tcx: TyCtxt<'_>, def_id: DefId) -> bool {
    if !tcx.is_static(def_id) || tcx.is_thread_local_static(def_id) {
        return false;
    }
    let DefKind::Static { nested, .. } = tcx.def_kind(def_id) else {
        return false;
    };
    if nested {
        return false;
    }
    if !tcx.is_mutable_static(def_id) {
        return false;
    }
    if excluded_item_name(tcx, def_id) {
        return false;
    }
    let ty = tcx.type_of(def_id).instantiate_identity().skip_norm_wip();
    rustc_middle::ty::ty_is_data_cap(tcx, ty)
}

fn is_write_callee(tcx: TyCtxt<'_>, expr: &Expr<'_>) -> bool {
    let ExprKind::Path(qpath) = expr.kind else {
        return false;
    };
    let QPath::Resolved(_, path) = qpath else {
        return false;
    };
    let Res::Def(_, def_id) = path.res else {
        return false;
    };
    // [LLM-generated] Not the trimmed form: trimmed paths may only be computed when a
    // diagnostic is emitted, and a clean build would ICE at session end.
    let s = rustc_middle::ty::print::with_no_trimmed_paths!(tcx.def_path_str(def_id));
    let name = s.rsplit("::").next().unwrap_or("");
    // Match the last path segment only. `def_path_str` may be
    // `ptr::write_volatile`, `core::ptr::write_volatile`, or a crate-disambiguated
    // form that does not contain the `ptr::` infix.
    matches!(name, "write" | "write_unaligned" | "write_volatile" | "volatile_store")
}

fn static_from_expr(expr: &Expr<'_>) -> Option<DefId> {
    match expr.kind {
        ExprKind::Path(QPath::Resolved(_, path)) => match path.res {
            Res::Def(DefKind::Static { .. }, def_id) => Some(def_id),
            _ => None,
        },
        ExprKind::AddrOf(.., inner) | ExprKind::Unary(UnOp::Deref, inner) => {
            static_from_expr(inner)
        }
        ExprKind::Field(base, _) => static_from_expr(base),
        ExprKind::Cast(inner, _) => static_from_expr(inner),
        _ => None,
    }
}

struct StoreVisitor<'a, 'tcx> {
    tcx: TyCtxt<'tcx>,
    found: &'a mut Vec<DefId>,
}

impl<'a, 'tcx> Visitor<'tcx> for StoreVisitor<'a, 'tcx> {
    fn visit_expr(&mut self, expr: &'tcx Expr<'tcx>) {
        match expr.kind {
            ExprKind::Call(callee, args) if !args.is_empty() && is_write_callee(self.tcx, callee) => {
                if let Some(def_id) = static_from_expr(&args[0]) {
                    self.found.push(def_id);
                }
            }
            ExprKind::Assign(lhs, _, _) => {
                if let Some(def_id) = static_from_expr(lhs) {
                    self.found.push(def_id);
                }
            }
            _ => {}
        }
        intravisit::walk_expr(self, expr);
    }
}

fn fn_is_c_abi<'tcx>(tcx: TyCtxt<'tcx>, local: rustc_span::def_id::LocalDefId) -> bool {
    // Prefer the HIR header: it is already lowered and is C for `extern "C" fn`.
    match tcx.hir_node_by_def_id(local) {
        Node::Item(item) => {
            if let ItemKind::Fn { sig, .. } = item.kind {
                return matches!(sig.header.abi, ExternAbi::C { .. } | ExternAbi::System { .. });
            }
        }
        Node::ImplItem(item) => {
            if let rustc_hir::ImplItemKind::Fn(sig, _) = item.kind {
                return matches!(sig.header.abi, ExternAbi::C { .. } | ExternAbi::System { .. });
            }
        }
        Node::TraitItem(item) => {
            if let rustc_hir::TraitItemKind::Fn(sig, _) = item.kind {
                return matches!(sig.header.abi, ExternAbi::C { .. } | ExternAbi::System { .. });
            }
        }
        _ => {}
    }
    let abi = tcx.fn_sig(local.to_def_id()).skip_binder().abi();
    matches!(abi, ExternAbi::C { .. } | ExternAbi::System { .. })
}

fn collect_croot_statics(tcx: TyCtxt<'_>) -> Vec<DefId> {
    let mut stored = Vec::new();
    let mut exported = Vec::new();
    for local in tcx.iter_local_def_id() {
        let def_id = local.to_def_id();
        match tcx.def_kind(def_id) {
            DefKind::Static { .. } => {
                if is_croot_slot(tcx, def_id) && is_c_exported_static(tcx, def_id) {
                    exported.push(def_id);
                }
            }
            DefKind::Fn | DefKind::AssocFn => {
                if !fn_is_c_abi(tcx, local) {
                    continue;
                }
                let Some(body) = tcx.hir_maybe_body_owned_by(local) else {
                    continue;
                };
                let mut v = StoreVisitor { tcx, found: &mut stored };
                v.visit_body(body);
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    for def_id in stored.into_iter().chain(exported) {
        if is_croot_slot(tcx, def_id) && !out.contains(&def_id) {
            out.push(def_id);
        }
    }
    out.sort_by_cached_key(|d| rustc_middle::ty::print::with_no_trimmed_paths!(tcx.def_path_str(*d)));
    out
}

/// Emit `@__cheri_croot` / `@__cheri_croot_slots` and reserve
/// `cheri_register_root` when this crate has typic C-root entries.
/// Called at the end of a CGU, after items are defined and before
/// `llvm.used` is built, so the table is retained.
pub(crate) fn emit_croot_table<'ll, 'tcx>(cx: &mut CodegenCx<'ll, 'tcx>) {
    let tcx = cx.tcx;
    if !croot_enabled(tcx) || skipped_crate(tcx) {
        return;
    }
    if cx.get_defined_value(CROOT_HEADER).is_some() {
        return;
    }
    let slots = collect_croot_statics(tcx);
    if slots.is_empty() {
        return;
    }
    let mut slot_vals = Vec::with_capacity(slots.len());
    for def_id in &slots {
        slot_vals.push(cx.get_static(*def_id));
    }
    let ptr_ty = cx.type_ptr();
    let n = slot_vals.len() as u64;
    let slots_ty = cx.type_array(ptr_ty, n);
    let slots_init = cx.const_array(ptr_ty, &slot_vals);
    let Some(slots_g) = cx.define_global(CROOT_SLOTS, slots_ty) else {
        return;
    };
    llvm::set_global_constant(slots_g, true);
    llvm::set_linkage(slots_g, llvm::Linkage::LinkOnceODRLinkage);
    llvm::set_section(slots_g, CROOT_SECTION);
    llvm::set_initializer(slots_g, slots_init);
    cx.add_used_global(slots_g);

    let hdr_ty = cx.type_struct(&[cx.type_i64(), ptr_ty], false);
    let hdr_init = cx.const_struct(&[cx.const_u64(n), slots_g], false);
    let Some(hdr) = cx.define_global(CROOT_HEADER, hdr_ty) else {
        return;
    };
    llvm::set_global_constant(hdr, true);
    llvm::set_linkage(hdr, llvm::Linkage::LinkOnceODRLinkage);
    llvm::set_section(hdr, CROOT_SECTION);
    llvm::set_initializer(hdr, hdr_init);
    cx.add_used_global(hdr);

    // Reserve the CR-3 symbol as a declaration only. A defined stub would
    // land in .text; a later CR-3 can supply the body.
    let fn_ty = cx.type_func(&[ptr_ty], cx.type_void());
    let _ = cx.declare_cfn(CROOT_REGISTER, llvm::UnnamedAddr::No, fn_ty);
}
