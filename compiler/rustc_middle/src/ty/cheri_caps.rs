//! M7.2a-w2 [LLM-generated]: monomorphized "might this type contain a capability?"
//! walk, shared by the dealloc-wipe fact emitter and the typic intrinsic used
//! to select RawVec grow/shrink relocate paths.

use rustc_data_structures::fx::FxHashSet;
use rustc_hir::attrs::lang_items::LangItem;
use crate::ty::{self, Ty, TyCtxt};

/// Recursive monomorphized predicate: true if `ty` may contain a CHERI
/// capability (pointer, reference, fn-ptr, or an ADT/array/tuple that holds
/// one). Pointer types are leaves (the pointee is not walked). `PhantomData`
/// is no; `MaybeUninit<T>` follows `T`. Conservative yes for `dyn`, unions
/// are walked field-wise, unsubst generics / infer / aliases that did not
/// normalize.
pub fn ty_may_contain_caps<'tcx>(tcx: TyCtxt<'tcx>, ty: Ty<'tcx>) -> bool {
    fn is_zst<'tcx>(tcx: TyCtxt<'tcx>, ty: Ty<'tcx>) -> bool {
        tcx.layout_of(ty::TypingEnv::fully_monomorphized().as_query_input(ty))
            .map(|l| l.is_zst())
            .unwrap_or(false)
    }
    fn rec<'tcx>(tcx: TyCtxt<'tcx>, ty: Ty<'tcx>, seen: &mut FxHashSet<Ty<'tcx>>) -> bool {
        let ty = tcx
            .try_normalize_erasing_regions(
                ty::TypingEnv::fully_monomorphized(),
                ty::Unnormalized::new_wip(ty),
            )
            .unwrap_or(ty);
        if !seen.insert(ty) {
            return false;
        }
        match *ty.kind() {
            ty::Bool | ty::Char | ty::Int(_) | ty::Uint(_) | ty::Float(_) | ty::Never | ty::Str
            | ty::FnDef(..) => false,
            ty::Ref(..) | ty::RawPtr(..) | ty::FnPtr(..) => true,
            ty::Array(elem, _) | ty::Slice(elem) | ty::Pat(elem, _) => rec(tcx, elem, seen),
            ty::Tuple(ts) => ts.iter().any(|t| rec(tcx, t, seen)),
            ty::Adt(def, args) => {
                if def.is_phantom_data() {
                    return false;
                }
                if tcx.is_lang_item(def.did(), LangItem::MaybeUninit) {
                    return rec(tcx, args.type_at(0), seen);
                }
                def.all_fields().any(|f| {
                    let ft = f.ty(tcx, args).skip_norm_wip();
                    !is_zst(tcx, ft) && rec(tcx, ft, seen)
                })
            }
            ty::Closure(_, args) => rec(tcx, args.as_closure().tupled_upvars_ty(), seen),
            ty::Coroutine(_, args) => {
                let c = args.as_coroutine();
                rec(tcx, c.tupled_upvars_ty(), seen)
                    || rec(tcx, c.resume_ty(), seen)
                    || rec(tcx, c.yield_ty(), seen)
                    || rec(tcx, c.return_ty(), seen)
            }
            ty::CoroutineClosure(_, args) => {
                rec(tcx, args.as_coroutine_closure().tupled_upvars_ty(), seen)
            }
            ty::Dynamic(..)
            | ty::Foreign(_)
            | ty::Param(_)
            | ty::Placeholder(_)
            | ty::Infer(_)
            | ty::Bound(..)
            | ty::Error(_)
            | ty::Alias(..)
            | ty::CoroutineWitness(..)
            | ty::UnsafeBinder(_) => true,
        }
    }
    let mut seen = FxHashSet::default();
    rec(tcx, ty, &mut seen)
}

/// True when type-guided wipe is enabled for this session (CHERI target,
/// `-Zcheri-annotate`, `-Zcheri-wipe-cap-words`).
pub fn wipe_cap_words_enabled(tcx: TyCtxt<'_>) -> bool {
    let sess = tcx.sess;
    sess.target.is_like_cheri
        && sess.opts.unstable_opts.cheri_annotate
        && sess.opts.unstable_opts.cheri_wipe_cap_words
}

/// typic predicate gated on the wipe flags: the value of
/// `core::intrinsics::cheri_ty_may_contain_caps::<T>()`.
pub fn typic_wipe_caps<'tcx>(tcx: TyCtxt<'tcx>, ty: Ty<'tcx>) -> bool {
    wipe_cap_words_enabled(tcx) && ty_may_contain_caps(tcx, ty)
}
