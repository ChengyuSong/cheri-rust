use std::{assert_matches, iter};

use rustc_abi::Primitive::Pointer;
use rustc_abi::{Align, BackendRepr, ExternAbi, PointerKind, Scalar, Size};
use rustc_attr_ir::find_attr;
use rustc_attr_ir::lang_items::LangItem;
use rustc_hir as hir;
use rustc_middle::middle::deduced_param_attrs::DeducedParamAttrs;
use rustc_middle::query::Providers;
use rustc_middle::ty::layout::{
    FnAbiError, HasTyCtxt, HasTypingEnv, LayoutCx, LayoutOf, TyAndLayout, fn_can_unwind,
};
use rustc_middle::ty::{self, InstanceKind, ShimKind, Ty, TyCtxt, Unnormalized};
use rustc_span::def_id::DefId;
use rustc_hir::def::DefKind;
use rustc_span::{DUMMY_SP, bug, sym};
use rustc_target::callconv::{
    AbiMap, ArgAbi, ArgAttribute, ArgAttributes, CheriFacts, FnAbi, IndirectMode, PassMode,
};
use tracing::debug;

pub(crate) fn provide(providers: &mut Providers) {
    *providers = Providers {
        fn_abi_of_fn_ptr,
        fn_abi_of_instance_no_deduced_attrs,
        fn_abi_of_instance_raw,
        ..*providers
    };
}

// NOTE(eddyb) this is private to avoid using it from outside of
// `fn_abi_of_instance` - any other uses are either too high-level
// for `Instance` (e.g. typeck would use `Ty::fn_sig` instead),
// or should go through `FnAbi` instead, to avoid losing any
// adjustments `fn_abi_of_instance` might be performing.
#[tracing::instrument(level = "debug", skip(tcx, typing_env))]
fn fn_sig_for_fn_abi<'tcx>(
    tcx: TyCtxt<'tcx>,
    instance: ty::Instance<'tcx>,
    typing_env: ty::TypingEnv<'tcx>,
) -> ty::FnSig<'tcx> {
    if let InstanceKind::Shim(ShimKind::ThreadLocal(..)) = instance.def {
        return tcx.mk_fn_sig_safe_rust_abi([], tcx.thread_local_ptr_ty(instance.def_id()));
    }

    let ty = instance.ty(tcx, typing_env);
    match *ty.kind() {
        ty::FnDef(def_id, args) => {
            let mut sig = tcx.instantiate_bound_regions_with_erased(
                tcx.fn_sig(def_id).instantiate(tcx, args.no_bound_vars().unwrap()).skip_norm_wip(),
            );

            // Modify `fn(self, ...)` to `fn(self: *mut Self, ...)`.
            if let ty::InstanceKind::Shim(ty::ShimKind::VTable(..)) = instance.def {
                let mut inputs_and_output = sig.inputs_and_output.to_vec();
                inputs_and_output[0] = Ty::new_mut_ptr(tcx, inputs_and_output[0]);
                sig.inputs_and_output = tcx.mk_type_list(&inputs_and_output);
            }

            sig
        }
        ty::Closure(def_id, args) => {
            let sig = tcx.instantiate_bound_regions_with_erased(args.as_closure().sig());
            let env_ty = tcx.closure_env_ty(
                Ty::new_closure(tcx, def_id, args),
                args.as_closure().kind(),
                tcx.lifetimes.re_erased,
            );

            tcx.mk_fn_sig(
                iter::once(env_ty).chain(sig.inputs().iter().cloned()),
                sig.output(),
                sig.fn_sig_kind,
            )
        }
        ty::CoroutineClosure(def_id, args) => {
            let coroutine_ty = Ty::new_coroutine_closure(tcx, def_id, args);
            let sig = args.as_coroutine_closure().coroutine_closure_sig();

            // When this `CoroutineClosure` comes from a `ConstructCoroutineInClosureShim`,
            // make sure we respect the `target_kind` in that shim.
            // FIXME(async_closures): This shouldn't be needed, and we should be populating
            // a separate def-id for these bodies.
            let mut coroutine_kind = args.as_coroutine_closure().kind();

            let env_ty = if let InstanceKind::Shim(ShimKind::ConstructCoroutineInClosure {
                receiver_by_ref,
                ..
            }) = instance.def
            {
                coroutine_kind = ty::ClosureKind::FnOnce;

                // Implementations of `FnMut` and `Fn` for coroutine-closures
                // still take their receiver by ref.
                if receiver_by_ref {
                    Ty::new_imm_ref(tcx, tcx.lifetimes.re_erased, coroutine_ty)
                } else {
                    coroutine_ty
                }
            } else {
                tcx.closure_env_ty(coroutine_ty, coroutine_kind, tcx.lifetimes.re_erased)
            };

            let sig = tcx.instantiate_bound_regions_with_erased(sig);

            tcx.mk_fn_sig(
                iter::once(env_ty).chain([sig.tupled_inputs_ty]),
                sig.to_coroutine_given_kind_and_upvars(
                    tcx,
                    args.as_coroutine_closure().parent_args(),
                    tcx.coroutine_for_closure(def_id),
                    coroutine_kind,
                    tcx.lifetimes.re_erased,
                    args.as_coroutine_closure().tupled_upvars_ty(),
                    args.as_coroutine_closure().coroutine_captures_by_ref_ty(),
                ),
                sig.fn_sig_kind,
            )
        }
        ty::Coroutine(did, args) => {
            let coroutine_kind = tcx.coroutine_kind(did).unwrap();
            let sig = args.as_coroutine().sig();

            let env_ty = Ty::new_mut_ref(tcx, tcx.lifetimes.re_erased, ty);

            let pin_did = tcx.require_lang_item(LangItem::Pin, DUMMY_SP);
            let pin_adt_ref = tcx.adt_def(pin_did);
            let pin_args = tcx.mk_args(&[env_ty.into()]);
            let env_ty = match coroutine_kind {
                hir::CoroutineKind::Desugared(hir::CoroutineDesugaring::Gen, _) => {
                    // Iterator::next doesn't accept a pinned argument,
                    // unlike for all other coroutine kinds.
                    env_ty
                }
                hir::CoroutineKind::Desugared(hir::CoroutineDesugaring::Async, _)
                | hir::CoroutineKind::Desugared(hir::CoroutineDesugaring::AsyncGen, _)
                | hir::CoroutineKind::Coroutine(_) => Ty::new_adt(tcx, pin_adt_ref, pin_args),
            };

            // The `FnSig` and the `ret_ty` here is for a coroutines main
            // `Coroutine::resume(...) -> CoroutineState` function in case we
            // have an ordinary coroutine, the `Future::poll(...) -> Poll`
            // function in case this is a special coroutine backing an async construct
            // or the `Iterator::next(...) -> Option` function in case this is a
            // special coroutine backing a gen construct.
            let (resume_ty, ret_ty) = match coroutine_kind {
                hir::CoroutineKind::Desugared(hir::CoroutineDesugaring::Async, _) => {
                    // The signature should be `Future::poll(_, &mut Context<'_>) -> Poll<Output>`
                    assert_eq!(sig.yield_ty, tcx.types.unit);

                    let poll_did = tcx.require_lang_item(LangItem::Poll, DUMMY_SP);
                    let poll_adt_ref = tcx.adt_def(poll_did);
                    let poll_args = tcx.mk_args(&[sig.return_ty.into()]);
                    let ret_ty = Ty::new_adt(tcx, poll_adt_ref, poll_args);

                    // We have to replace the `ResumeTy` that is used for type and borrow checking
                    // with `&mut Context<'_>` which is used in codegen.
                    #[cfg(debug_assertions)]
                    {
                        if let ty::Adt(resume_ty_adt, _) = sig.resume_ty.kind() {
                            let expected_adt =
                                tcx.adt_def(tcx.require_lang_item(LangItem::ResumeTy, DUMMY_SP));
                            assert_eq!(*resume_ty_adt, expected_adt);
                        } else {
                            panic!("expected `ResumeTy`, found `{:?}`", sig.resume_ty);
                        };
                    }
                    let context_mut_ref = Ty::new_task_context(tcx);

                    (Some(context_mut_ref), ret_ty)
                }
                hir::CoroutineKind::Desugared(hir::CoroutineDesugaring::Gen, _) => {
                    // The signature should be `Iterator::next(_) -> Option<Yield>`
                    let option_did = tcx.require_lang_item(LangItem::Option, DUMMY_SP);
                    let option_adt_ref = tcx.adt_def(option_did);
                    let option_args = tcx.mk_args(&[sig.yield_ty.into()]);
                    let ret_ty = Ty::new_adt(tcx, option_adt_ref, option_args);

                    assert_eq!(sig.return_ty, tcx.types.unit);
                    assert_eq!(sig.resume_ty, tcx.types.unit);

                    (None, ret_ty)
                }
                hir::CoroutineKind::Desugared(hir::CoroutineDesugaring::AsyncGen, _) => {
                    // The signature should be
                    // `AsyncIterator::poll_next(_, &mut Context<'_>) -> Poll<Option<Output>>`
                    assert_eq!(sig.return_ty, tcx.types.unit);

                    // Yield type is already `Poll<Option<yield_ty>>`
                    let ret_ty = sig.yield_ty;

                    // We have to replace the `ResumeTy` that is used for type and borrow checking
                    // with `&mut Context<'_>` which is used in codegen.
                    #[cfg(debug_assertions)]
                    {
                        if let ty::Adt(resume_ty_adt, _) = sig.resume_ty.kind() {
                            let expected_adt =
                                tcx.adt_def(tcx.require_lang_item(LangItem::ResumeTy, DUMMY_SP));
                            assert_eq!(*resume_ty_adt, expected_adt);
                        } else {
                            panic!("expected `ResumeTy`, found `{:?}`", sig.resume_ty);
                        };
                    }
                    let context_mut_ref = Ty::new_task_context(tcx);

                    (Some(context_mut_ref), ret_ty)
                }
                hir::CoroutineKind::Coroutine(_) => {
                    // The signature should be `Coroutine::resume(_, Resume) -> CoroutineState<Yield, Return>`
                    let state_did = tcx.require_lang_item(LangItem::CoroutineState, DUMMY_SP);
                    let state_adt_ref = tcx.adt_def(state_did);
                    let state_args = tcx.mk_args(&[sig.yield_ty.into(), sig.return_ty.into()]);
                    let ret_ty = Ty::new_adt(tcx, state_adt_ref, state_args);

                    (Some(sig.resume_ty), ret_ty)
                }
            };

            if let Some(resume_ty) = resume_ty {
                tcx.mk_fn_sig_safe_rust_abi([env_ty, resume_ty], ret_ty)
            } else {
                // `Iterator::next` doesn't have a `resume` argument.
                tcx.mk_fn_sig_safe_rust_abi([env_ty], ret_ty)
            }
        }
        _ => bug!("unexpected type {:?} in Instance::fn_sig", ty),
    }
}

/// Describes a function for determination of its ABI.
struct FnAbiDesc<'tcx> {
    layout_cx: LayoutCx<'tcx>,
    sig: ty::FnSig<'tcx>,

    /// The function's definition, if its body can be used to deduce parameter attributes.
    determined_fn_def_id: Option<DefId>,
    caller_location: Option<Ty<'tcx>>,
    is_virtual_call: bool,
    extra_args: &'tcx [Ty<'tcx>],
}

impl<'tcx> FnAbiDesc<'tcx> {
    fn for_fn_ptr(
        tcx: TyCtxt<'tcx>,
        query: ty::PseudoCanonicalInput<'tcx, (ty::PolyFnSig<'tcx>, &'tcx ty::List<Ty<'tcx>>)>,
    ) -> Self {
        let ty::PseudoCanonicalInput { typing_env, value: (sig, extra_args) } = query;
        Self {
            layout_cx: LayoutCx::new(tcx, typing_env),
            sig: tcx.normalize_erasing_regions(
                typing_env,
                Unnormalized::new_wip(tcx.instantiate_bound_regions_with_erased(sig)),
            ),
            // Parameter attributes can never be deduced for indirect calls, as there is no
            // function body available to use.
            determined_fn_def_id: None,
            caller_location: None,
            is_virtual_call: false,
            extra_args,
        }
    }

    fn for_instance(
        tcx: TyCtxt<'tcx>,
        query: ty::PseudoCanonicalInput<'tcx, (ty::Instance<'tcx>, &'tcx ty::List<Ty<'tcx>>)>,
    ) -> Self {
        let ty::PseudoCanonicalInput { typing_env, value: (instance, extra_args) } = query;
        let is_virtual_call = matches!(instance.def, ty::InstanceKind::Virtual(..));
        let is_tls_shim_call =
            matches!(instance.def, ty::InstanceKind::Shim(ty::ShimKind::ThreadLocal(_)));
        Self {
            layout_cx: LayoutCx::new(tcx, typing_env),
            sig: tcx.normalize_erasing_regions(
                typing_env,
                Unnormalized::new_wip(fn_sig_for_fn_abi(tcx, instance, typing_env)),
            ),
            // Parameter attributes can be deduced from the bodies of neither:
            // - virtual calls, as they might call other functions from the vtable; nor
            // - TLS shims, as they would refer to the underlying static.
            determined_fn_def_id: (!is_virtual_call && !is_tls_shim_call)
                .then(|| instance.def_id()),
            caller_location: instance
                .def
                .requires_caller_location(tcx)
                .then(|| tcx.caller_location_ty()),
            is_virtual_call,
            extra_args,
        }
    }
}

fn fn_abi_of_fn_ptr<'tcx>(
    tcx: TyCtxt<'tcx>,
    query: ty::PseudoCanonicalInput<'tcx, (ty::PolyFnSig<'tcx>, &'tcx ty::List<Ty<'tcx>>)>,
) -> Result<&'tcx FnAbi<'tcx, Ty<'tcx>>, &'tcx FnAbiError<'tcx>> {
    let desc = FnAbiDesc::for_fn_ptr(tcx, query);
    fn_abi_new_uncached(desc)
}

fn fn_abi_of_instance_no_deduced_attrs<'tcx>(
    tcx: TyCtxt<'tcx>,
    query: ty::PseudoCanonicalInput<'tcx, (ty::Instance<'tcx>, &'tcx ty::List<Ty<'tcx>>)>,
) -> Result<&'tcx FnAbi<'tcx, Ty<'tcx>>, &'tcx FnAbiError<'tcx>> {
    let desc = FnAbiDesc::for_instance(tcx, query);
    fn_abi_new_uncached(desc)
}

fn fn_abi_of_instance_raw<'tcx>(
    tcx: TyCtxt<'tcx>,
    query: ty::PseudoCanonicalInput<'tcx, (ty::Instance<'tcx>, &'tcx ty::List<Ty<'tcx>>)>,
) -> Result<&'tcx FnAbi<'tcx, Ty<'tcx>>, &'tcx FnAbiError<'tcx>> {
    // The `fn_abi_of_instance_no_deduced_attrs` query may have been called during CTFE, so we
    // delegate to it here in order to reuse (and, if necessary, augment) its result.
    tcx.fn_abi_of_instance_no_deduced_attrs(query).map(|fn_abi| {
        let params = FnAbiDesc::for_instance(tcx, query);
        // If the function's body can be used to deduce parameter attributes, then adjust such
        // "no deduced attrs" ABI; otherwise, return that ABI unadjusted.
        params.determined_fn_def_id.map_or(fn_abi, |fn_def_id| {
            fn_abi_adjust_for_deduced_attrs(&params.layout_cx, fn_abi, params.sig.abi(), fn_def_id)
        })
    })
}

/// Returns argument attributes for a scalar argument.
fn arg_attrs_for_rust_scalar<'tcx>(
    cx: LayoutCx<'tcx>,
    scalar: Scalar,
    layout: TyAndLayout<'tcx>,
    offset: Size,
    is_return: bool,
    determined_fn_def_id: Option<DefId>,
    rustic_abi: bool,
) -> ArgAttributes {
    let mut attrs = ArgAttributes::new();

    if !scalar.is_uninit_valid() {
        attrs.set(ArgAttribute::NoUndef);
    }

    // Only pointer types handled below.
    let Scalar::Initialized { value: Pointer(_), valid_range } = scalar else { return attrs };

    // Set `nonnull` if the validity range excludes zero.
    if !valid_range.contains(0) {
        attrs.set(ArgAttribute::NonNull);
    }

    let tcx = cx.tcx();

    if let Some(pointee) = layout.pointee_info_at(&cx, offset) {
        if pointee.align > Align::ONE {
            attrs.pointee_align =
                Some(pointee.align.min(cx.tcx().sess.target.max_reliable_alignment()));
        }

        attrs.pointee_size = pointee.size;

        if let Some(kind) = pointee.safe {
            // The aliasing rules for `Box<T>` are still not decided, but currently we emit
            // `noalias` for it. This can be turned off using an unstable flag.
            // See https://github.com/rust-lang/unsafe-code-guidelines/issues/326
            let noalias_for_box = tcx.sess.opts.unstable_opts.box_noalias;

            // `&T` where `T` contains no `UnsafeCell<U>` is immutable, and can be marked as both
            // `readonly` and `noalias`, as LLVM's definition of `noalias` is based solely on memory
            // dependencies rather than pointer equality. However this only applies to arguments,
            // not return values.
            //
            // `&mut T` and `Box<T>` where `T: Unpin` are unique and hence `noalias`.
            let no_alias = match kind {
                PointerKind::SharedRef { frozen } => frozen,
                PointerKind::MutableRef { unpin } => unpin,
                PointerKind::Box { unpin, global } => unpin && global && noalias_for_box,
            };
            // We can never add `noalias` in return position; that LLVM attribute has some very surprising semantics
            // (see <https://github.com/rust-lang/unsafe-code-guidelines/issues/385#issuecomment-1368055745>).
            if no_alias && !is_return {
                attrs.set(ArgAttribute::NoAlias);
            }

            // Set writable if no_alias is set, it's a mutable reference and the feature is enabled.
            if tcx.sess.opts.unstable_opts.llvm_writable
                && matches!(kind, PointerKind::MutableRef { unpin: true })
                && !is_return
            {
                let rustc_no_writable = match determined_fn_def_id {
                    Some(def_id) => find_attr!(tcx, def_id, RustcNoWritable),
                    None => true, // If no def_id exists, we make the conservative choice and disable the feature.
                };

                if !rustc_no_writable {
                    attrs.set(ArgAttribute::Writable);
                }
            }

            // NoFree is not valid on return values. If it were, it would mean something like
            // "will not be freed until the end of the program", which is generally not valid for
            // references.
            let no_free = !is_return
                && match kind {
                    // Non-frozen shared references are not necessarily dereferenceable for the
                    // entire duration of the function
                    // (see <https://github.com/rust-lang/rust/pull/98017>).
                    PointerKind::SharedRef { frozen } => frozen,
                    // Mutable references to potentially self-referential types are not necessarily
                    // dereferenceable for the entire duration of the function
                    // (see <https://github.com/rust-lang/unsafe-code-guidelines/issues/381>).
                    PointerKind::MutableRef { unpin } => unpin,
                    // Box may be deallocated during execution of the function.
                    PointerKind::Box { .. } => false,
                };
            if no_free {
                attrs.set(ArgAttribute::NoFree);
            }

            if matches!(kind, PointerKind::SharedRef { frozen: true }) && !is_return {
                attrs.set(ArgAttribute::ReadOnly);
                attrs.set(ArgAttribute::CapturesReadOnly);
            }

            // CHERI M2 (local prototype): safe pointers are allocation-bounded under M1r.
            // M3.0: not for parameters of non-Rust-ABI definitions, whose callers may be C
            // passing arbitrary capabilities; such definitions narrow these parameters at
            // entry instead (`-Zcheri-export-entry-bounds`, see `cheri_bounds.rs`).
            // M3.2: the type contract of a typed signature is also an IR fact on
            // - returns (the callee produced a valid `&T` / `Box<T>`; for a C callee this is
            //   C's trusted promise, as for the C mirror's `RS_RET_*`), and
            // - parameters of foreign declarations (Rust calling C: Rust passes the typed
            //   value, so the fact holds; Q-M3.1b).
            let callee_is_foreign =
                determined_fn_def_id.is_some_and(|def_id| tcx.is_foreign_item(def_id));
            if tcx.sess.target.is_like_cheri
                && (is_return
                    || rustic_abi
                    || callee_is_foreign
                    || !tcx.sess.opts.unstable_opts.cheri_export_entry_bounds)
            {
                attrs.set(ArgAttribute::AllocInbounds);
            }
            // M3.2: `Box<T>` (Global) records its allocator family, the trusted part of
            // `valid_box`.
            if tcx.sess.target.is_like_cheri
                && matches!(kind, PointerKind::Box { global: true, .. })
            {
                attrs.set(ArgAttribute::BoxFamily);
            }
        }
    }

    attrs
}

/// Ensure that the ABI makes basic sense.
fn fn_abi_sanity_check<'tcx>(
    cx: &LayoutCx<'tcx>,
    fn_abi: &FnAbi<'tcx, Ty<'tcx>>,
    spec_abi: ExternAbi,
) {
    fn fn_arg_attrs_sanity_check(attrs: &ArgAttributes, is_ret: bool) {
        if attrs.regular.contains(ArgAttribute::NoFree) {
            assert!(!is_ret, "NoFree not valid on return values");
        }
    }

    fn fn_arg_sanity_check<'tcx>(
        cx: &LayoutCx<'tcx>,
        spec_abi: ExternAbi,
        arg: &ArgAbi<'tcx, Ty<'tcx>>,
        is_ret: bool,
    ) {
        let tcx = cx.tcx();

        if spec_abi.is_rustic_abi() {
            if arg.layout.is_zst() {
                // Casting closures to function pointers depends on ZST closure types being
                // omitted entirely in the calling convention.
                assert!(arg.is_ignore());
            }
            if let PassMode::Indirect { mode, .. } = arg.mode
                && spec_abi != ExternAbi::RustTail
            {
                assert!(mode == IndirectMode::Pointer, "rust abi must use plain pointer mode");
            }
        } else if arg.layout.pass_indirectly_in_non_rustic_abis(cx) {
            assert_matches!(
                arg.mode,
                PassMode::Indirect { mode: IndirectMode::Pointer, .. },
                "the {spec_abi} ABI does not implement `#[rustc_pass_indirectly_in_non_rustic_abis]`"
            );
        }

        match &arg.mode {
            PassMode::Ignore => {
                assert!(arg.layout.is_zst());
            }
            PassMode::Direct(attrs) => {
                // Here the Rust type is used to determine the actual ABI, so we have to be very
                // careful. Scalar/Vector is fine, since backends will generally use
                // `layout.backend_repr` and ignore everything else.
                match arg.layout.backend_repr {
                    BackendRepr::Scalar(_)
                    | BackendRepr::SimdVector { .. }
                    | BackendRepr::SimdScalableVector { .. } => {}
                    BackendRepr::ScalarPair { .. } => {
                        panic!("`PassMode::Direct` used for ScalarPair type {}", arg.layout.ty)
                    }
                    BackendRepr::Memory { .. } => {
                        panic!(
                            "`PassMode::Direct` for aggregates not allowed\n\
                             Problematic type: {:#?}",
                            arg.layout,
                        );
                    }
                }
                fn_arg_attrs_sanity_check(attrs, is_ret);
            }
            PassMode::Pair(attrs1, attrs2) => {
                // Similar to `Direct`, we need to make sure that backends use `layout.backend_repr`
                // and ignore the rest of the layout.
                assert!(
                    matches!(arg.layout.backend_repr, BackendRepr::ScalarPair { .. }),
                    "PassMode::Pair for type {}",
                    arg.layout.ty
                );
                fn_arg_attrs_sanity_check(attrs1, is_ret);
                fn_arg_attrs_sanity_check(attrs2, is_ret);
            }
            PassMode::Cast { .. } => {
                // `Cast` means "transmute to `CastType`"; that only makes sense for sized types.
                assert!(arg.layout.is_sized());
            }
            PassMode::Indirect { meta_attrs: None, attrs, .. } => {
                // No metadata, must be sized.
                // Conceptually, unsized arguments must be copied around, which requires dynamically
                // determining their size, which we cannot do without metadata. Consult
                // t-opsem before removing this check.
                assert!(arg.layout.is_sized());
                // Indirect returns are arguments from an ABI perspective.
                fn_arg_attrs_sanity_check(attrs, false);
            }
            PassMode::Indirect { meta_attrs: Some(meta_attrs), attrs, address_space: _, mode } => {
                // With metadata. Must be unsized and not on the stack.
                assert!(arg.layout.is_unsized() && *mode == IndirectMode::Pointer);
                // Also, must not be `extern` type.
                let tail = tcx.struct_tail_for_codegen(arg.layout.ty, cx.typing_env);
                if matches!(tail.kind(), ty::Foreign(..)) {
                    // These types do not have metadata, so having `meta_attrs` is bogus.
                    // Conceptually, unsized arguments must be copied around, which requires dynamically
                    // determining their size. Therefore, we cannot allow `extern` types here. Consult
                    // t-opsem before removing this check.
                    panic!("unsized arguments must not be `extern` types");
                }
                // Indirect returns are arguments from an ABI perspective.
                fn_arg_attrs_sanity_check(attrs, false);
                fn_arg_attrs_sanity_check(meta_attrs, false);
            }
        }
    }

    for arg in fn_abi.args.iter() {
        fn_arg_sanity_check(cx, spec_abi, arg, false);
    }
    fn_arg_sanity_check(cx, spec_abi, &fn_abi.ret, true);
}

#[tracing::instrument(
    level = "debug",
    skip(cx, caller_location, determined_fn_def_id, is_virtual_call)
)]
fn fn_abi_new_uncached<'tcx>(
    FnAbiDesc {
        layout_cx: ref cx,
        sig,
        determined_fn_def_id,
        caller_location,
        is_virtual_call,
        extra_args,
    }: FnAbiDesc<'tcx>,
) -> Result<&'tcx FnAbi<'tcx, Ty<'tcx>>, &'tcx FnAbiError<'tcx>> {
    let tcx = cx.tcx();

    let abi_map = AbiMap::from_target(&tcx.sess.target);
    let conv = abi_map.canonize_abi(sig.abi(), sig.c_variadic()).unwrap();

    let mut inputs = sig.inputs();
    let extra_args = if sig.abi() == ExternAbi::RustCall {
        assert!(!sig.c_variadic() && extra_args.is_empty());

        if let Some(input) = sig.inputs().last()
            && let ty::Tuple(tupled_arguments) = input.kind()
        {
            inputs = &sig.inputs()[0..sig.inputs().len() - 1];
            tupled_arguments
        } else {
            bug!(
                "argument to function with \"rust-call\" ABI \
                    is not a tuple"
            );
        }
    } else {
        assert!(sig.c_variadic() || extra_args.is_empty());
        extra_args
    };

    let arg_of = |ty: Ty<'tcx>, arg_idx: Option<usize>| -> Result<_, &'tcx FnAbiError<'tcx>> {
        let span = tracing::debug_span!("arg_of");
        let _entered = span.enter();
        let is_return = arg_idx.is_none();

        let layout = cx.layout_of(ty).map_err(|err| &*tcx.arena.alloc(FnAbiError::Layout(*err)))?;
        let layout = if is_virtual_call && arg_idx == Some(0) {
            // Don't pass the vtable, it's not an argument of the virtual fn.
            // Instead, pass just the data pointer, but give it the type `*const/mut dyn Trait`
            // or `&/&mut dyn Trait` because this is special-cased elsewhere in codegen
            make_thin_self_ptr(cx, layout)
        } else {
            layout
        };

        Ok(ArgAbi::new(layout, |scalar, offset| {
            arg_attrs_for_rust_scalar(
                *cx,
                scalar,
                layout,
                offset,
                is_return,
                determined_fn_def_id,
                sig.abi().is_rustic_abi(),
            )
        }))
    };

    let mut fn_abi = FnAbi {
        ret: arg_of(sig.output(), None)?,
        args: inputs
            .iter()
            .copied()
            .chain(extra_args.iter().copied())
            .chain(caller_location)
            .enumerate()
            .map(|(i, ty)| arg_of(ty, Some(i)))
            .collect::<Result<_, _>>()?,
        c_variadic: sig.c_variadic(),
        fixed_count: inputs.len() as u32,
        conv,
        // FIXME return false for tls shim
        can_unwind: fn_can_unwind(
            tcx,
            // Since `#[rustc_nounwind]` can change unwinding, we cannot infer unwinding by `fn_def_id` for a virtual call.
            determined_fn_def_id,
            sig.abi(),
        ),
    };
    fn_abi_adjust_for_abi(cx, &mut fn_abi, sig.abi());
    if !is_virtual_call && let Some(def_id) = determined_fn_def_id {
        cheri_contract_facts(cx, &mut fn_abi, def_id, inputs.len());
    }
    debug!("fn_abi_new_uncached = {:?}", fn_abi);
    fn_abi_sanity_check(cx, &fn_abi, sig.abi());
    Ok(tcx.arena.alloc(fn_abi))
}

#[tracing::instrument(level = "trace", skip(cx))]
fn fn_abi_adjust_for_abi<'tcx>(
    cx: &LayoutCx<'tcx>,
    fn_abi: &mut FnAbi<'tcx, Ty<'tcx>>,
    abi: ExternAbi,
) {
    assert_ne!(
        abi,
        ExternAbi::LlvmIntrinsic,
        "fn_abi_of_instance should not be called on LLVM intrinsics"
    );

    if abi.is_rustic_abi() {
        fn_abi.adjust_for_rust_abi(cx);
    } else {
        fn_abi.adjust_for_foreign_abi(cx, abi);
    }
}

#[tracing::instrument(level = "trace", skip(cx))]
fn fn_abi_adjust_for_deduced_attrs<'tcx>(
    cx: &LayoutCx<'tcx>,
    fn_abi: &'tcx FnAbi<'tcx, Ty<'tcx>>,
    abi: ExternAbi,
    fn_def_id: DefId,
) -> &'tcx FnAbi<'tcx, Ty<'tcx>> {
    let tcx = cx.tcx();
    // Look up the deduced parameter attributes for this function, if we have its def ID.
    // We'll tag its parameters with those attributes as appropriate.
    let deduced = if abi.is_rustic_abi() { tcx.deduced_param_attrs(fn_def_id) } else { &[] };
    if deduced.is_empty() {
        fn_abi
    } else {
        let mut fn_abi = fn_abi.clone();
        apply_deduced_attributes(cx, deduced, 0, &mut fn_abi.ret);
        for (arg_idx, arg) in fn_abi.args.iter_mut().enumerate() {
            apply_deduced_attributes(cx, deduced, arg_idx + 1, arg);
        }
        debug!("fn_abi_adjust_for_deduced_attrs = {:?}", fn_abi);
        fn_abi_sanity_check(cx, &fn_abi, abi);
        tcx.arena.alloc(fn_abi)
    }
}

/// Apply deduced optimization attributes to a parameter using an indirect pass mode.
///
/// `deduced` is a possibly truncated list of deduced attributes for a return place and arguments.
/// `idx` the index of the parameter on the list (0 for a return place, and 1.. for arguments).
fn apply_deduced_attributes<'tcx>(
    cx: &LayoutCx<'tcx>,
    deduced: &[DeducedParamAttrs],
    idx: usize,
    arg: &mut ArgAbi<'tcx, Ty<'tcx>>,
) {
    // Deduction is performed under the assumption of the indirection pass mode.
    let PassMode::Indirect { ref mut attrs, .. } = arg.mode else {
        return;
    };
    // The default values at the tail of the list are not encoded.
    let Some(deduced) = deduced.get(idx) else {
        return;
    };
    if deduced.read_only(cx.tcx(), cx.typing_env, arg.layout.ty) {
        debug!("added deduced ReadOnly attribute");
        attrs.regular.insert(ArgAttribute::ReadOnly);
    }
    if deduced.captures_none(cx.tcx(), cx.typing_env, arg.layout.ty) {
        debug!("added deduced CapturesNone attribute");
        attrs.regular.insert(ArgAttribute::CapturesNone);
    }
}

#[tracing::instrument(level = "debug", skip(cx))]
fn make_thin_self_ptr<'tcx>(
    cx: &(impl HasTyCtxt<'tcx> + HasTypingEnv<'tcx>),
    layout: TyAndLayout<'tcx>,
) -> TyAndLayout<'tcx> {
    let tcx = cx.tcx();
    let wide_pointer_ty = if layout.is_unsized() {
        // unsized `self` is passed as a pointer to `self`
        // FIXME (mikeyhew) change this to use &own if it is ever added to the language
        Ty::new_mut_ptr(tcx, layout.ty)
    } else {
        match layout.backend_repr {
            BackendRepr::ScalarPair { .. } | BackendRepr::Scalar(..) => (),
            _ => bug!("receiver type has unsupported layout: {:?}", layout),
        }

        // In the case of Rc<Self>, we need to explicitly pass a *mut RcInner<Self>
        // with a Scalar (not ScalarPair) ABI. This is a hack that is understood
        // elsewhere in the compiler as a method on a `dyn Trait`.
        // To get the type `*mut RcInner<Self>`, we just keep unwrapping newtypes until we
        // get a built-in pointer type
        let mut wide_pointer_layout = layout;
        while !wide_pointer_layout.ty.is_raw_ptr() && !wide_pointer_layout.ty.is_ref() {
            wide_pointer_layout = wide_pointer_layout
                .non_1zst_field(cx)
                .expect("not exactly one non-1-ZST field in a `DispatchFromDyn` type")
                .1
        }

        wide_pointer_layout.ty
    };

    // we now have a type like `*mut RcInner<dyn Trait>`
    // change its layout to that of `*mut ()`, a thin pointer, but keep the same type
    // this is understood as a special case elsewhere in the compiler
    let unit_ptr_ty = Ty::new_mut_ptr(tcx, tcx.types.unit);

    TyAndLayout {
        ty: wide_pointer_ty,

        // NOTE(eddyb) using an empty `ParamEnv`, and `unwrap`-ing the `Result`
        // should always work because the type is always `*mut ()`.
        ..tcx.layout_of(ty::TypingEnv::fully_monomorphized().as_query_input(unit_ptr_ty)).unwrap()
    }
}

// ---------------------------------------------------------------------------
// CHERI M3.2 (local research prototype, LLM-generated): type contracts as IR
// facts. See /workspace/cheri/m3/CONTRACTS.md. Complements the per-scalar
// facts of `arg_attrs_for_rust_scalar` with the ones that need the whole
// signature:
// - typed signatures: lifetime-derived captures for `&T` / `&mut T` /
//   `Option<&T>` / `&[T]` parameters, and `"mem.range"` on Rust-ABI `&[T]`;
// - raw-pointer type contracts (`#[rustc_mem_contract]`, injected by
//   `core::contracts::{requires, mem_contract}`), mapped exactly as the C
//   mirror (`mem_contract.h` + LLVM `mem-contract-c`).
// ---------------------------------------------------------------------------

/// The region of a top-level `&'r T` or `Option<&'r T>` parameter.
fn cheri_top_ref_region<'tcx>(tcx: TyCtxt<'tcx>, ty: Ty<'tcx>) -> Option<ty::Region<'tcx>> {
    match *ty.kind() {
        ty::Ref(r, ..) => Some(r),
        ty::Adt(adt, args) if tcx.is_diagnostic_item(sym::Option, adt.did()) => {
            match *args.type_at(0).kind() {
                ty::Ref(r, ..) => Some(r),
                _ => None,
            }
        }
        _ => None,
    }
}

fn cheri_count_region<'tcx>(ty: Ty<'tcx>, r: ty::Region<'tcx>) -> usize {
    ty.walk()
        .filter(|arg| matches!(arg.kind(), ty::GenericArgKind::Lifetime(re) if re == r))
        .count()
}

fn cheri_set_captures(attrs: &mut ArgAttributes, claim: ArgAttribute) {
    attrs.regular -= ArgAttribute::CapturesNone; // clears all three capture levels
    attrs.regular -= ArgAttribute::CapturesRetBorrow;
    attrs.regular |= claim;
}

/// The (first) pointer attributes of an argument passed as a scalar or pair.
fn cheri_ptr_attrs<'a, 'tcx>(arg: &'a mut ArgAbi<'tcx, Ty<'tcx>>) -> Option<&'a mut ArgAttributes> {
    match &mut arg.mode {
        PassMode::Direct(attrs) | PassMode::Pair(attrs, _) => Some(attrs),
        _ => None,
    }
}

fn cheri_contract_facts<'tcx>(
    cx: &LayoutCx<'tcx>,
    fn_abi: &mut FnAbi<'tcx, Ty<'tcx>>,
    def_id: DefId,
    n_inputs: usize,
) {
    let tcx = cx.tcx();
    if !tcx.sess.target.is_like_cheri
        || !matches!(tcx.def_kind(def_id), DefKind::Fn | DefKind::AssocFn)
    {
        return;
    }
    let poly = tcx.fn_sig(def_id).instantiate_identity().skip_norm_wip();
    let usig = poly.skip_binder();
    if usig.inputs().len() != n_inputs || fn_abi.args.len() < n_inputs {
        return;
    }
    let is_decl = tcx.is_foreign_item(def_id);

    // 1. Typed signatures: "not retained" from lifetimes (borrowck-checked).
    //    A late-bound region that occurs exactly once in the inputs (in this
    //    parameter) cannot outlive the call except through the return value.
    for (i, &ty) in usig.inputs().iter().enumerate() {
        let Some(r) = cheri_top_ref_region(tcx, ty) else { continue };
        if !matches!(r.kind(), ty::ReBound(..)) {
            continue; // 'static, or early-bound (may appear in where-clauses)
        }
        let n_in: usize = usig.inputs().iter().map(|&t| cheri_count_region(t, r)).sum();
        if n_in != 1 {
            continue;
        }
        let n_out = cheri_count_region(usig.output(), r);
        let Some(attrs) = cheri_ptr_attrs(&mut fn_abi.args[i]) else { continue };
        cheri_set_captures(
            attrs,
            if n_out == 0 { ArgAttribute::CapturesAddress } else { ArgAttribute::CapturesRetBorrow },
        );
    }
    // 2. Typed signatures: a `&[T]` / `&str` passed as (ptr, len) carries its extent.
    for arg in fn_abi.args.iter_mut() {
        let elem = match *arg.layout.ty.kind() {
            ty::Ref(_, p, _) => match *p.kind() {
                ty::Slice(e) => cx.layout_of(e).map(|l| l.size).ok(),
                ty::Str => Some(Size::from_bytes(1)),
                _ => None,
            },
            _ => None,
        };
        if let Some(sz) = elem
            && sz != Size::ZERO
            && let PassMode::Pair(a, _) = &mut arg.mode
        {
            a.cheri = CheriFacts::range_pair(sz);
        }
    }

    // 3. Raw-pointer type contracts.
    let Some(canons) = find_attr!(tcx, def_id, RustcMemContract(v) => v.clone()) else { return };
    let full = is_decl || tcx.sess.opts.unstable_opts.cheri_contract_def_facts;
    let mut keep = vec![];
    let mut ret_borrows = vec![];
    let mut items = vec![];
    for canon in canons.iter() {
        for item in canon.as_str().split(';').filter(|s| !s.is_empty()) {
            let Some((kind, rest)) = item.split_once('(') else { continue };
            let args: Vec<&str> = rest.trim_end_matches(')').split(',').collect();
            match kind {
                "keep" => keep.extend(args.first().and_then(|a| a.parse::<usize>().ok())),
                "ret_borrows" => {
                    ret_borrows.extend(args.first().and_then(|a| a.parse::<usize>().ok()))
                }
                _ => items.push((kind.to_string(), args.iter().map(|a| a.to_string()).collect::<Vec<_>>())),
            }
        }
    }
    for (kind, args) in items {
        let is_slice = kind == "slice" || kind == "slice_mut";
        let is_mut = kind.ends_with("_mut");
        let is_box = kind == "box";
        let is_cstr = kind == "cstr";
        let shared = !is_mut && !is_box;
        let is_ret = args.first().map(String::as_str) == Some("ret");
        let p = if is_ret { None } else { args.first().and_then(|a| a.parse::<usize>().ok()) };
        if !is_ret && p.is_none_or(|p| p >= n_inputs) {
            continue;
        }
        let len = if is_slice { args.get(1).and_then(|a| a.parse::<usize>().ok()) } else { None };
        if is_slice && len.is_none_or(|l| l >= n_inputs) {
            continue;
        }
        let arg = match p {
            Some(p) => &mut fn_abi.args[p],
            None => &mut fn_abi.ret,
        };
        let Some(pointee) = arg.layout.ty.builtin_deref(true) else { continue };
        let pl = cx.layout_of(pointee).ok().filter(|l| l.is_sized());
        let (size, align) = match (is_cstr, pl) {
            (true, _) => (Size::ZERO, None),
            (false, Some(l)) => (l.size, Some(l.align.abi)),
            (false, None) => continue,
        };
        let PassMode::Direct(attrs) = &mut arg.mode else { continue };
        if is_slice && let Some(l) = len {
            attrs.cheri = CheriFacts::range_arg(l);
        }
        if !full {
            continue; // definitions: extent record only (M3.3 enforces the rest)
        }
        // Spatial part.
        attrs.regular |= ArgAttribute::AllocInbounds | ArgAttribute::NonNull;
        if let Some(a) = align
            && a > Align::ONE
        {
            attrs.pointee_align = Some(a);
        }
        if (kind == "ref" || kind == "ref_mut") && !is_ret {
            attrs.pointee_size = size;
        }
        if is_cstr {
            attrs.regular |= ArgAttribute::CStr;
        }
        // Trusted part.
        attrs.regular |= ArgAttribute::NoUndef;
        if is_box {
            attrs.regular |= ArgAttribute::BoxFamily;
        }
        if let Some(p) = p {
            attrs.regular |= ArgAttribute::NoAlias;
            if shared {
                attrs.regular |= ArgAttribute::ReadOnly;
            }
            if !is_box {
                attrs.regular |= ArgAttribute::NoFree;
                let claim = if ret_borrows.contains(&p) {
                    ArgAttribute::CapturesRetBorrow
                } else if keep.contains(&p) {
                    if shared { ArgAttribute::CapturesReadOnly } else { ArgAttribute::empty() }
                } else {
                    ArgAttribute::CapturesAddress // borrowed for the call (Q-M3.1a)
                };
                cheri_set_captures(attrs, claim);
            }
        }
        if is_slice
            && let Some(l) = len
            && size != Size::ZERO
            && let PassMode::Direct(la) = &mut fn_abi.args[l].mode
        {
            la.regular |= ArgAttribute::NoUndef;
            la.cheri = CheriFacts::len_elem(size);
        }
    }
}
