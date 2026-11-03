use rustc_ast::token;
use rustc_ast::tokenstream::{DelimSpacing, DelimSpan, Spacing, TokenStream, TokenTree};
use rustc_errors::ErrorGuaranteed;
use rustc_expand::base::{AttrProcMacro, ExtCtxt};
use rustc_span::Span;
use rustc_span::symbol::{Ident, Symbol, kw, sym};

pub(crate) struct ExpandRequires;

pub(crate) struct ExpandEnsures;

impl AttrProcMacro for ExpandRequires {
    fn expand<'cx>(
        &self,
        ecx: &'cx mut ExtCtxt<'_>,
        span: Span,
        annotation: TokenStream,
        annotated: TokenStream,
    ) -> Result<TokenStream, ErrorGuaranteed> {
        expand_contract_clause_tts(ecx, span, annotation, annotated, kw::ContractRequires)
    }
}

impl AttrProcMacro for ExpandEnsures {
    fn expand<'cx>(
        &self,
        ecx: &'cx mut ExtCtxt<'_>,
        span: Span,
        annotation: TokenStream,
        annotated: TokenStream,
    ) -> Result<TokenStream, ErrorGuaranteed> {
        expand_contract_clause_tts(ecx, span, annotation, annotated, kw::ContractEnsures)
    }
}

/// Expand the function signature to include the contract clause.
///
/// The contracts clause will be injected before the function body and the optional where clause.
/// For that, we search for the body / where token, and invoke the `inject` callback to generate the
/// contract clause in the right place.
///
// FIXME: this kind of manual token tree munging does not have significant precedent among
// rustc builtin macros, probably because most builtin macros use direct AST manipulation to
// accomplish similar goals. But since our attributes need to take arbitrary expressions, and
// our attribute infrastructure does not yet support mixing a token-tree annotation with an AST
// annotated, we end up doing token tree manipulation.
fn expand_contract_clause(
    ecx: &mut ExtCtxt<'_>,
    attr_span: Span,
    annotated: TokenStream,
    inject: impl FnOnce(&mut Vec<TokenTree>) -> Result<(), ErrorGuaranteed>,
) -> Result<TokenStream, ErrorGuaranteed> {
    let mut new_tts = vec![];
    let mut cursor = annotated.iter();

    let is_kw = |tt: &TokenTree, sym: Symbol| {
        if let TokenTree::Token(token, _) = tt { token.is_ident_named(sym) } else { false }
    };

    // Find the `fn` keyword to check if this is a function.
    if cursor
        .find(|tt| {
            new_tts.push((*tt).clone());
            is_kw(tt, kw::Fn)
        })
        .is_none()
    {
        return Err(ecx
            .sess
            .dcx()
            .span_err(attr_span, "contract annotations can only be used on functions"));
    }

    // Contracts are not yet supported on async/gen functions
    if new_tts.iter().any(|tt| is_kw(tt, kw::Async) || is_kw(tt, kw::Gen)) {
        return Err(ecx.sess.dcx().span_err(
            attr_span,
            "contract annotations are not yet supported on async or gen functions",
        ));
    }

    // Found the `fn` keyword, now find either the `where` token or the function body.
    let next_tt = loop {
        let Some(tt) = cursor.next() else {
            return Err(ecx.sess.dcx().span_err(
                attr_span,
                "contract annotations is only supported in functions with bodies",
            ));
        };
        // If `tt` is the last element. Check if it is the function body.
        if cursor.peek().is_none() {
            if let TokenTree::Delimited(_, _, token::Delimiter::Brace, _) = tt {
                break tt;
            } else {
                return Err(ecx.sess.dcx().span_err(
                    attr_span,
                    "contract annotations is only supported in functions with bodies",
                ));
            }
        }

        if is_kw(tt, kw::Where) {
            break tt;
        }
        new_tts.push(tt.clone());
    };

    // At this point, we've transcribed everything from the `fn` through the formal parameter list
    // and return type declaration, (if any), but `tt` itself has *not* been transcribed.
    //
    // Now inject the AST contract form.
    //
    inject(&mut new_tts)?;

    // Above we injected the internal AST requires/ensures construct. Now copy over all the other
    // token trees.
    new_tts.push(next_tt.clone());
    while let Some(tt) = cursor.next() {
        new_tts.push(tt.clone());
        if cursor.peek().is_none()
            && !matches!(tt, TokenTree::Delimited(_, _, token::Delimiter::Brace, _))
        {
            return Err(ecx.sess.dcx().span_err(
                attr_span,
                "contract annotations is only supported in functions with bodies",
            ));
        }
    }

    Ok(TokenStream::new(new_tts))
}

fn expand_contract_clause_tts(
    ecx: &mut ExtCtxt<'_>,
    attr_span: Span,
    annotation: TokenStream,
    annotated: TokenStream,
    clause_keyword: rustc_span::Symbol,
) -> Result<TokenStream, ErrorGuaranteed> {
    if annotation.is_empty() {
        let (name, example) = if clause_keyword == kw::ContractRequires {
            ("requires", "condition")
        } else {
            ("ensures", "|result: &T| condition")
        };
        ecx.sess.dcx().span_err(
            attr_span,
            format!("`{name}` attribute requires an argument, e.g., `#[{name}({example})]`"),
        );
        return Ok(annotated);
    }

    // CHERI M3.5: block/statement-level `requires` — annotated is not an `fn` item.
    // Inject `#[rustc_mem_contract_site = "..."]` (name-keyed canon) and a
    // `mem_contract_site` intrinsic call so IR carries the same record shape.
    if clause_keyword == kw::ContractRequires && !annotated_is_fn(&annotated) {
        let canon = requires_mem_contract_site(&annotation);
        if canon.is_empty() {
            ecx.sess.dcx().span_err(
                attr_span,
                "block/statement `requires` needs a `valid_*` type-contract predicate",
            );
            return Ok(annotated);
        }
        let joined = canon.join(";");
        return Ok(expand_requires_site(ecx, attr_span, &joined, annotated));
    }

    // CHERI M3.2: function-level type contracts via `#[rustc_mem_contract]`.
    let mem_contracts = if clause_keyword == kw::ContractRequires {
        requires_mem_contract(&annotation, &annotated)
    } else {
        vec![]
    };
    let feature_span = ecx.with_def_site_ctxt(attr_span);
    let expanded = expand_contract_clause(ecx, attr_span, annotated, |new_tts| {
        new_tts.push(TokenTree::Token(
            token::Token::from_ast_ident(Ident::new(clause_keyword, feature_span)),
            Spacing::Joint,
        ));
        new_tts.push(TokenTree::Delimited(
            DelimSpan::from_single(attr_span),
            DelimSpacing::new(Spacing::JointHidden, Spacing::JointHidden),
            token::Delimiter::Brace,
            annotation,
        ));
        Ok(())
    })?;
    Ok(prepend_mem_contract_attr(ecx, attr_span, &mem_contracts.join(";"), expanded))
}

fn annotated_is_fn(annotated: &TokenStream) -> bool {
    annotated.iter().any(|tt| is_ident_tt(tt, kw::Fn))
}

/// Site-level `requires`: free names → `slice(p,n)` (not parameter indices).
fn requires_mem_contract_site(annotation: &TokenStream) -> Vec<String> {
    let mut out = vec![];
    for conj in split_top(annotation, |k| matches!(k, token::AndAnd)) {
        if let Ok(Some(c)) = parse_predicate_site(&conj) {
            out.push(c);
        }
    }
    out
}

fn parse_predicate_site(toks: &[TokenTree]) -> Result<Option<String>, String> {
    // Reuse parse_predicate with a fake params list built from the arg names
    // themselves so indices become names... easier: duplicate lightly.
    let mut i = 0;
    let mut segs = vec![];
    let name = loop {
        let Some(s) = toks.get(i).and_then(tok_ident) else { return Ok(None) };
        segs.push(s);
        i += 1;
        if toks.get(i).and_then(tok_kind) == Some(&token::PathSep)
            && toks.get(i + 1).and_then(tok_ident).is_some()
        {
            i += 1;
            continue;
        }
        break s;
    };
    let Some(&(_, _diag, kind, arity)) = PREDICATES.iter().find(|(n, ..)| *n == name.as_str()) else {
        return Ok(None);
    };
    let path_ok = segs.len() == 1
        || segs.iter().any(|s| matches!(s.as_str(), "contracts" | "mem_contracts"));
    if !path_ok {
        return Err(format!("`{name}`: type-contract predicates must come from `core::contracts`"));
    }
    if toks.get(i).and_then(tok_kind) == Some(&token::PathSep)
        && toks.get(i + 1).and_then(tok_kind) == Some(&token::Lt)
    {
        i += 2;
        let mut depth = 1;
        while depth > 0 {
            match toks.get(i).and_then(tok_kind) {
                Some(token::Lt) => depth += 1,
                Some(token::Gt) => depth -= 1,
                Some(token::Shr) => depth -= 2,
                None if toks.get(i).is_none() => return Ok(None),
                _ => {}
            }
            i += 1;
        }
    }
    let Some(TokenTree::Delimited(_, _, token::Delimiter::Parenthesis, args)) = toks.get(i) else {
        return Ok(None);
    };
    if toks.len() != i + 1 {
        return Ok(None);
    }
    let args = split_top(args, |k| matches!(k, token::Comma));
    if args.len() != arity {
        return Err(format!("`{name}` takes {arity} argument(s)"));
    }
    let mut names = vec![];
    for (k, a) in args.iter().enumerate() {
        let Some(n) = a.first().and_then(tok_ident) else {
            return Err(format!("`{name}`: argument {} must be a local name", k + 1));
        };
        names.push(n.to_string());
    }
    Ok(Some(format!("{kind}({})", names.join(","))))
}

/// Expand `#[requires(...)] { ... }` into a block that carries the site attr
/// and records the contract in IR via `mem_contract_site`.
fn expand_requires_site(
    ecx: &mut ExtCtxt<'_>,
    span: Span,
    canon: &str,
    annotated: TokenStream,
) -> TokenStream {
    let sp = ecx.with_def_site_ctxt(span);
    // #[rustc_mem_contract_site = "canon"]
    let attr_inner = TokenStream::new(vec![
        TokenTree::Token(
            token::Token::from_ast_ident(Ident::new(sym::rustc_mem_contract_site, sp)),
            Spacing::Alone,
        ),
        TokenTree::Token(token::Token::new(token::Eq, sp), Spacing::Alone),
        TokenTree::Token(
            token::Token::new(
                token::TokenKind::lit(token::LitKind::Str, Symbol::intern(canon), None),
                sp,
            ),
            Spacing::Alone,
        ),
    ]);
    let attr = vec![
        TokenTree::Token(token::Token::new(token::Pound, sp), Spacing::Joint),
        TokenTree::Delimited(
            DelimSpan::from_single(sp),
            DelimSpacing::new(Spacing::JointHidden, Spacing::JointHidden),
            token::Delimiter::Bracket,
            attr_inner,
        ),
    ];
    // Intrinsic call: ::core::intrinsics::mem_contract_site("canon");
    // Keep it as a statement at the front of a wrapping block so both
    // `#[requires] { body }` and `#[requires] let x = ...;` work:
    //   #[site] { mem_contract_site("..."); annotated }
    let lit = TokenTree::Token(
        token::Token::new(
            token::TokenKind::lit(token::LitKind::Str, Symbol::intern(canon), None),
            sp,
        ),
        Spacing::Alone,
    );
    let call_args = TokenStream::new(vec![lit]);
    // path: core :: intrinsics :: mem_contract_site ( "canon" ) ;
    let mut call = Vec::new();
    for (name, joint) in [
        ("core", true),
        ("contracts", true),
        ("mem_contracts", true),
        ("mem_contract_site", false),
    ] {
        if name != "core" {
            call.push(TokenTree::Token(token::Token::new(token::PathSep, sp), Spacing::Joint));
        }
        call.push(TokenTree::Token(
            token::Token::from_ast_ident(Ident::new(Symbol::intern(name), sp)),
            if joint { Spacing::Joint } else { Spacing::Alone },
        ));
    }
    call.push(TokenTree::Delimited(
        DelimSpan::from_single(sp),
        DelimSpacing::new(Spacing::JointHidden, Spacing::Alone),
        token::Delimiter::Parenthesis,
        call_args,
    ));
    call.push(TokenTree::Token(token::Token::new(token::Semi, sp), Spacing::Alone));

    let mut body = call;
    body.extend(annotated.iter().cloned());
    let block = TokenTree::Delimited(
        DelimSpan::from_single(span),
        DelimSpacing::new(Spacing::Alone, Spacing::Alone),
        token::Delimiter::Brace,
        TokenStream::new(body),
    );
    let mut out = attr;
    out.push(block);
    TokenStream::new(out)
}

// ---------------------------------------------------------------------------
// CHERI M3.2 (local research prototype, LLM-generated): type contracts.
//
// `#[core::contracts::requires(... valid_slice::<T>(p, n) ...)]` and the
// fallback `#[core::contracts::mem_contract(requires(...), ret = ..., keep(x),
// ret_borrows(x))]` both inject `#[rustc_mem_contract = "<canon>"]`, a list
// of contracts over Rust parameter indices (`ret` = the return value), e.g.
// `"slice(0,1);ref(ret);ret_borrows(0)"`. Codegen lowers it to IR facts,
// independent of `-Zcontract-checks`. Predicates are recognised by the last
// path segment (`valid_slice`, `valid_slice_mut`, `valid_ref`, `valid_ref_mut`,
// `valid_box`, `valid_cstr`), only as top-level `&&` conjuncts, with plain
// parameter names as arguments (optionally `.cast()`/`.cast_mut()`/
// `.cast_const()`/`as T`). Anything else in a `requires` is left to the
// runtime check only.
// ---------------------------------------------------------------------------

pub(crate) struct ExpandMemContract;

impl AttrProcMacro for ExpandMemContract {
    fn expand<'cx>(
        &self,
        ecx: &'cx mut ExtCtxt<'_>,
        span: Span,
        annotation: TokenStream,
        annotated: TokenStream,
    ) -> Result<TokenStream, ErrorGuaranteed> {
        let Some(params) = fn_param_names(&annotated) else {
            return Err(ecx
                .sess
                .dcx()
                .span_err(span, "`mem_contract` can only be used on functions"));
        };
        let canon = parse_mem_contract(ecx, span, &annotation, &params)?;
        Ok(prepend_mem_contract_attr(ecx, span, &canon, annotated))
    }
}

/// Contracts found in a `requires` expression (silently ignores other terms).
fn requires_mem_contract(annotation: &TokenStream, annotated: &TokenStream) -> Vec<String> {
    let Some(params) = fn_param_names(annotated) else { return vec![] };
    let mut out = vec![];
    for conj in split_top(annotation, |k| matches!(k, token::AndAnd)) {
        if let Ok(Some(c)) = parse_predicate(&conj, &params, false) {
            out.push(c);
        }
    }
    out
}

fn tok_kind(tt: &TokenTree) -> Option<&token::TokenKind> {
    if let TokenTree::Token(t, _) = tt { Some(&t.kind) } else { None }
}

fn tok_ident(tt: &TokenTree) -> Option<Symbol> {
    if let TokenTree::Token(t, _) = tt { t.ident().map(|(i, _)| i.name) } else { None }
}

/// Splits a token stream at top-level separators, tracking `<`/`>` nesting so
/// generic argument lists are not split.
fn split_top(ts: &TokenStream, is_sep: impl Fn(&token::TokenKind) -> bool) -> Vec<Vec<TokenTree>> {
    let mut parts = vec![vec![]];
    let mut angle: i32 = 0;
    for tt in ts.iter() {
        if let Some(k) = tok_kind(tt) {
            match k {
                token::Lt => angle += 1,
                token::Gt => angle = (angle - 1).max(0),
                token::Shr => angle = (angle - 2).max(0),
                _ => {}
            }
            if angle == 0 && is_sep(k) {
                parts.push(vec![]);
                continue;
            }
        }
        parts.last_mut().unwrap().push(tt.clone());
    }
    parts.retain(|p| !p.is_empty());
    parts
}

/// Parameter names of the annotated `fn` item, in order (`None` where the
/// pattern is not a plain identifier). `None` if this is not a fn item.
fn fn_param_names(item: &TokenStream) -> Option<Vec<Option<Symbol>>> {
    let mut it = item.iter();
    it.find(|tt| is_ident_tt(tt, kw::Fn))?;
    let params = it.find_map(|tt| match tt {
        TokenTree::Delimited(_, _, token::Delimiter::Parenthesis, inner) => Some(inner.clone()),
        _ => None,
    })?;
    let mut names = vec![];
    for p in split_top(&params, |k| matches!(k, token::Comma)) {
        let mut name = None;
        for tt in &p {
            match tok_kind(tt) {
                Some(token::Colon) => break,
                Some(token::And) | Some(token::AndAnd) => continue,
                _ => {}
            }
            if matches!(tt, TokenTree::Token(t, _) if t.lifetime().is_some()) {
                continue;
            }
            match tok_ident(tt) {
                Some(s) if s == kw::Mut || s == kw::Ref => continue,
                Some(s) => {
                    name = Some(s);
                    break;
                }
                None => break,
            }
        }
        names.push(name);
    }
    Some(names)
}

fn is_ident_tt(tt: &TokenTree, sym: Symbol) -> bool {
    matches!(tt, TokenTree::Token(t, _) if t.is_ident_named(sym))
}

// (source name, diagnostic item, IR kind, arity)
const PREDICATES: &[(&str, &str, &str, usize)] = &[
    ("valid_slice", "mem_valid_slice", "slice", 2),
    ("valid_slice_mut", "mem_valid_slice_mut", "slice_mut", 2),
    ("valid_ref", "mem_valid_ref", "ref", 1),
    ("valid_ref_mut", "mem_valid_ref_mut", "ref_mut", 1),
    ("valid_box", "mem_valid_box", "box", 1),
    ("valid_cstr", "mem_valid_cstr", "cstr", 1),
];

/// Parses one conjunct `path::valid_x[::<T>](a, b)`. `Ok(None)`: not a
/// predicate (ignored). `Err`: a predicate with unusable arguments.
/// With `is_ret`, the first argument must be `ret`.
fn parse_predicate(
    toks: &[TokenTree],
    params: &[Option<Symbol>],
    is_ret: bool,
) -> Result<Option<String>, String> {
    // Path: Ident (:: Ident)*, then optional ::<...>, then a (...) group, then end.
    let mut i = 0;
    // Collect the path's identifiers. M3.3 (D-M3.2b): accept a path whose
    // last segment is a `valid_*` predicate and which either is a bare name
    // (brought into scope by `use`) or contains a `contracts` / `mem_contracts`
    // segment. The attribute stores the *diagnostic item* name (`mem_valid_*`),
    // which `cheri_contract_facts` resolves; a same-named user function in a
    // different module is rejected here when its path has no `contracts` segment.
    let mut segs = vec![];
    let name = loop {
        let Some(s) = toks.get(i).and_then(tok_ident) else { return Ok(None) };
        segs.push(s);
        i += 1;
        if toks.get(i).and_then(tok_kind) == Some(&token::PathSep)
            && toks.get(i + 1).and_then(tok_ident).is_some()
        {
            i += 1;
            continue;
        }
        break s;
    };
    let Some(&(_, _diag, kind, arity)) = PREDICATES.iter().find(|(n, ..)| *n == name.as_str()) else {
        return Ok(None);
    };
    let path_ok = segs.len() == 1
        || segs.iter().any(|s| matches!(s.as_str(), "contracts" | "mem_contracts"));
    if !path_ok {
        return Err(format!(
            "`{name}`: type-contract predicates must come from `core::contracts`              (found path without a `contracts` segment)"
        ));
    };
    if toks.get(i).and_then(tok_kind) == Some(&token::PathSep)
        && toks.get(i + 1).and_then(tok_kind) == Some(&token::Lt)
    {
        i += 2;
        let mut depth = 1;
        while depth > 0 {
            match toks.get(i).and_then(tok_kind) {
                Some(token::Lt) => depth += 1,
                Some(token::Gt) => depth -= 1,
                Some(token::Shr) => depth -= 2,
                None if toks.get(i).is_none() => return Ok(None),
                _ => {}
            }
            i += 1;
        }
        if depth < 0 {
            return Ok(None);
        }
    }
    let Some(TokenTree::Delimited(_, _, token::Delimiter::Parenthesis, args)) = toks.get(i) else {
        return Ok(None);
    };
    if toks.len() != i + 1 {
        return Ok(None); // e.g. `valid_slice(p, n) == false`: not a plain conjunct
    }
    let args = split_top(args, |k| matches!(k, token::Comma));
    if args.len() != arity {
        return Err(format!("`{name}` takes {arity} argument(s)"));
    }
    let mut idx = vec![];
    for (k, a) in args.iter().enumerate() {
        let Some(n) = a.first().and_then(tok_ident) else {
            return Err(format!("`{name}`: argument {} must be a parameter name", k + 1));
        };
        // Allowed suffixes that keep the same pointer value.
        let rest = &a[1..];
        let ok_suffix = rest.is_empty()
            || rest.first().is_some_and(|t| is_ident_tt(t, kw::As))
            || (rest.first().and_then(tok_kind) == Some(&token::Dot)
                && rest.get(1).and_then(tok_ident).is_some_and(|m| {
                    matches!(m.as_str(), "cast" | "cast_mut" | "cast_const" | "as_ptr")
                }));
        if !ok_suffix {
            return Err(format!(
                "`{name}`: argument {} must be a parameter (optionally `.cast()` / `as T`)",
                k + 1
            ));
        }
        if k == 0 && is_ret {
            if n.as_str() != "ret" {
                return Err(format!("`ret = {name}(...)`: the first argument must be `ret`"));
            }
            idx.push("ret".to_string());
            continue;
        }
        let Some(pos) = params.iter().position(|p| *p == Some(n)) else {
            return Err(format!("`{name}`: `{n}` is not a parameter of this function"));
        };
        idx.push(pos.to_string());
    }
    Ok(Some(format!("{kind}({})", idx.join(","))))
}

/// `#[mem_contract(requires(P && Q), ret = P, keep(x), ret_borrows(x))]`.
fn parse_mem_contract(
    ecx: &mut ExtCtxt<'_>,
    span: Span,
    annotation: &TokenStream,
    params: &[Option<Symbol>],
) -> Result<String, ErrorGuaranteed> {
    let err = |ecx: &mut ExtCtxt<'_>, m: String| {
        Err(ecx.sess.dcx().span_err(span, format!("invalid `mem_contract`: {m}")))
    };
    let mut out: Vec<String> = vec![];
    for item in split_top(annotation, |k| matches!(k, token::Comma)) {
        let head = item.first().and_then(tok_ident).map(|s| s.as_str().to_string());
        match (head.as_deref(), item.get(1)) {
            (Some("requires"), Some(TokenTree::Delimited(_, _, token::Delimiter::Parenthesis, e)))
                if item.len() == 2 =>
            {
                let mut n = 0;
                for conj in split_top(e, |k| matches!(k, token::AndAnd)) {
                    match parse_predicate(&conj, params, false) {
                        Ok(Some(c)) => {
                            out.push(c);
                            n += 1;
                        }
                        Ok(None) => {
                            return err(ecx, "`requires` must be a `&&` of `valid_*` predicates".into());
                        }
                        Err(m) => return err(ecx, m),
                    }
                }
                if n == 0 {
                    return err(ecx, "empty `requires`".into());
                }
            }
            (Some("ret"), Some(tt)) if tok_kind(tt) == Some(&token::Eq) => {
                match parse_predicate(&item[2..], params, true) {
                    Ok(Some(c)) => out.push(c),
                    Ok(None) => return err(ecx, "`ret = ` must be a `valid_*` predicate".into()),
                    Err(m) => return err(ecx, m),
                }
            }
            (Some(m @ ("keep" | "ret_borrows")), Some(TokenTree::Delimited(_, _, token::Delimiter::Parenthesis, a)))
                if item.len() == 2 =>
            {
                let a: Vec<_> = a.iter().cloned().collect();
                let Some(n) = (a.len() == 1).then(|| tok_ident(&a[0])).flatten() else {
                    return err(ecx, format!("`{m}(x)` takes one parameter name"));
                };
                let Some(pos) = params.iter().position(|p| *p == Some(n)) else {
                    return err(ecx, format!("`{n}` is not a parameter of this function"));
                };
                out.push(format!("{m}({pos})"));
            }
            _ => {
                return err(
                    ecx,
                    "expected `requires(...)`, `ret = ...`, `keep(x)` or `ret_borrows(x)`".into(),
                );
            }
        }
    }
    Ok(out.join(";"))
}

/// Prepends `#[rustc_mem_contract = "<canon>"]` (def-site span, so the
/// `rustc_attrs` gate is satisfied by the macro's `allow_internal_unstable`).
fn prepend_mem_contract_attr(
    ecx: &mut ExtCtxt<'_>,
    span: Span,
    canon: &str,
    annotated: TokenStream,
) -> TokenStream {
    if canon.is_empty() {
        return annotated;
    }
    let sp = ecx.with_def_site_ctxt(span);
    let inner = TokenStream::new(vec![
        TokenTree::Token(
            token::Token::from_ast_ident(Ident::new(sym::rustc_mem_contract, sp)),
            Spacing::Alone,
        ),
        TokenTree::Token(token::Token::new(token::Eq, sp), Spacing::Alone),
        TokenTree::Token(
            token::Token::new(
                token::TokenKind::lit(token::LitKind::Str, Symbol::intern(canon), None),
                sp,
            ),
            Spacing::Alone,
        ),
    ]);
    let mut tts = vec![
        TokenTree::Token(token::Token::new(token::Pound, sp), Spacing::Joint),
        TokenTree::Delimited(
            DelimSpan::from_single(sp),
            DelimSpacing::new(Spacing::JointHidden, Spacing::JointHidden),
            token::Delimiter::Bracket,
            inner,
        ),
    ];
    tts.extend(annotated.iter().cloned());
    TokenStream::new(tts)
}
