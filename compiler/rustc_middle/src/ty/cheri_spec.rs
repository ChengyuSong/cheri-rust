//! [LLM-generated] CHERI M7.2e: `#[rustc_cheri_spec = "..."]` memory specs.
//!
//! A spec is trusted (like `unsafe`): it tells rustc what a function, or one `let`
//! statement, does with heap memory, so the CHERI codegen can wipe dead capability
//! words before a block is freed. Nothing here changes the type system. The grammar
//! (one spec per attribute string):
//!
//! ```text
//! spec  := form '(' arg (',' arg)* ')'
//! form  := frees | dealloc_of | relocates | live | retains | borrows | stores
//! arg   := key '=' value | ident                 (the bare ident is for escape specs)
//! expr  := term (('+' | '-') term)*
//! term  := atom ('*' atom)*
//! atom  := INT | size_of::<ty>() | size_of_val(*place) | place | '(' expr ')'
//! place := ident ('.' (ident | INT))*            (pointers are looked through)
//! ty    := Self | generic param | '*' place       (pointee type of a typed place)
//! call  := ident '(' ident (',' ident)* ')'       (a sibling trait method and its args)
//! ```
//!
//! Function forms: `frees(block, size)`, `dealloc_of(ty [, or] [, fields] [, block [, size]])`,
//! `relocates(block, size, keep [, alloc, free])`, `retains(p)`, `borrows(p)`,
//! `stores(p, into)`. Statement form (on a `let` whose initializer is a call to a
//! `relocates` function): `live(block, bytes, held [, ty])`.
//!
//! [LLM-generated] M7.3 additions: the form `vacates(ty, block, from, to)`, the atom
//! `old(expr)` (only inside `vacates`) and the operator `%` (same precedence as `*`).
//! `block` may also be written `&place` there (the address of a field, e.g. a node's
//! key array). See [`CheriSpec::Vacates`].

use rustc_attr_ir::find_attr;
use rustc_hir::def::DefKind;
use rustc_span::def_id::DefId;
use rustc_span::{Span, Symbol, kw};

use crate::ty::{self, Ty, TyCtxt};

/// A place over the function's parameters (or, in a statement spec, its locals):
/// `base.f1.f2`. Each step looks through references, raw pointers, `NonNull`,
/// `Unique`, `Box` and `Option` of those before taking the field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpecPlace {
    pub base: Symbol,
    pub fields: Vec<Symbol>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpecTy {
    /// `Self` or a generic type parameter of the item or its impl / trait.
    Param(Symbol),
    /// `*place`: the pointee type of a pointer-typed place.
    Pointee(SpecPlace),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpecBinOp {
    Add,
    Sub,
    Mul,
    /// [LLM-generated] M7.3: `a % b` (unsigned; codegen uses `max(b, 1)` as the divisor).
    Rem,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpecExpr {
    Lit(u64),
    Place(SpecPlace),
    SizeOf(SpecTy),
    /// `size_of_val(*place)`: the size of the pointee of `place` (sized pointees only).
    SizeOfPointee(SpecPlace),
    Bin(SpecBinOp, Box<SpecExpr>, Box<SpecExpr>),
    /// [LLM-generated] M7.3: `old(expr)`: `expr` read on entry to the function (only in
    /// `vacates`; every other operand of a `vacates` bound is read on return).
    Old(Box<SpecExpr>),
}

/// `name(arg, ...)`: a sibling method of the same trait, with the caller's parameter
/// names as its arguments (used by `relocates(alloc = .., free = ..)`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpecCall {
    pub name: Symbol,
    pub args: Vec<Symbol>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CheriSpec {
    /// Callee contract: calling this function frees `block` (`size` bytes).
    Frees { block: SpecPlace, size: SpecExpr },
    /// Body spec: every block freed by this function held values of `ty`. With
    /// `block` (and optionally `size`), the freed block is that place of this function's
    /// own parameters (evaluated only when this function is the one being compiled).
    /// Without `block`, the block and size come from the `frees` spec of the free.
    /// `fields`: wipe only the capability words of `ty`'s layout (or of `or`, when
    /// the freed size is `size_of::<or>()`), if `ty`'s generic arguments hold no caps.
    DeallocOf {
        ty: SpecTy,
        or: Option<SpecTy>,
        fields: bool,
        block: Option<SpecPlace>,
        size: Option<SpecExpr>,
    },
    /// Callee contract: the call may move the first `keep` bytes of `block` (`size`
    /// bytes) to a new block and free the old one. `alloc` / `free` name the sibling
    /// trait methods that make it equivalent to allocate + copy + free.
    Relocates {
        block: SpecPlace,
        size: SpecExpr,
        keep: SpecExpr,
        alloc: Option<SpecCall>,
        free: Option<SpecCall>,
    },
    /// Statement spec: at this call only the first `bytes` of `block` are live (values
    /// of `ty`, default `held`); the rest held dead values of `held`. When the call
    /// `relocates` with `alloc` / `free` and `ty` may hold capabilities, the call can be
    /// rewritten to allocate + copy + wipe + free (`-Zcheri-spec-relocate-rewrite`).
    Live { block: SpecPlace, bytes: SpecExpr, held: SpecTy, ty: Option<SpecTy> },
    /// Escape specs (parsed and checked; lowering comes with the escape-gap work).
    Retains(Symbol),
    Borrows(Symbol),
    Stores { param: Symbol, into: SpecPlace },
    /// [LLM-generated] M7.3 body spec: on every normal return the bytes `[from, to)` of
    /// `block` hold no live value; they held (moved-out or dropped) values of `ty`.
    /// `block` is read on entry (it must not change); `from` and `to` are read on
    /// return except for their `old(..)` parts, read on entry. `addr`: `block` was
    /// written `&place`, the address of that place, not its pointer value. The
    /// function's own MIR gets a `WipeDead` before each return, so the wipe survives
    /// MIR inlining. The spec is trusted: `from <= to` on every return and the range
    /// lies in the block's allocation whenever it is non-empty.
    Vacates { ty: SpecTy, block: SpecPlace, addr: bool, from: SpecExpr, to: SpecExpr },
}

impl CheriSpec {
    pub fn form_name(&self) -> &'static str {
        match self {
            CheriSpec::Frees { .. } => "frees",
            CheriSpec::DeallocOf { .. } => "dealloc_of",
            CheriSpec::Relocates { .. } => "relocates",
            CheriSpec::Live { .. } => "live",
            CheriSpec::Retains(_) => "retains",
            CheriSpec::Borrows(_) => "borrows",
            CheriSpec::Stores { .. } => "stores",
            CheriSpec::Vacates { .. } => "vacates",
        }
    }
}

// ---------------------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Ident(String),
    Int(u64),
    Punct(char),
    ColCol,
}

fn lex(s: &str) -> Result<Vec<Tok>, String> {
    let mut out = Vec::new();
    let b: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c.is_whitespace() {
            i += 1;
        } else if c.is_ascii_alphabetic() || c == '_' {
            let st = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == '_') {
                i += 1;
            }
            out.push(Tok::Ident(b[st..i].iter().collect()));
        } else if c.is_ascii_digit() {
            let st = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == '_') {
                i += 1;
            }
            let t: String = b[st..i].iter().filter(|c| **c != '_').collect();
            let v = if let Some(h) = t.strip_prefix("0x") {
                u64::from_str_radix(h, 16)
            } else {
                t.parse::<u64>()
            };
            out.push(Tok::Int(v.map_err(|_| format!("bad integer `{t}`"))?));
        } else if c == ':' && i + 1 < b.len() && b[i + 1] == ':' {
            out.push(Tok::ColCol);
            i += 2;
        } else if "(),=.*+-<>%&".contains(c) {
            out.push(Tok::Punct(c));
            i += 1;
        } else {
            return Err(format!("unexpected character `{c}`"));
        }
    }
    Ok(out)
}

struct P {
    t: Vec<Tok>,
    i: usize,
}

impl P {
    fn peek(&self) -> Option<&Tok> {
        self.t.get(self.i)
    }
    fn eat_punct(&mut self, c: char) -> bool {
        if self.peek() == Some(&Tok::Punct(c)) {
            self.i += 1;
            true
        } else {
            false
        }
    }
    fn expect_punct(&mut self, c: char) -> Result<(), String> {
        if self.eat_punct(c) { Ok(()) } else { Err(format!("expected `{c}`")) }
    }
    fn ident(&mut self) -> Result<String, String> {
        match self.peek().cloned() {
            Some(Tok::Ident(s)) => {
                self.i += 1;
                Ok(s)
            }
            _ => Err("expected an identifier".to_string()),
        }
    }
    fn place(&mut self) -> Result<SpecPlace, String> {
        let base = Symbol::intern(&self.ident()?);
        let mut fields = Vec::new();
        while self.eat_punct('.') {
            match self.peek().cloned() {
                Some(Tok::Ident(s)) => {
                    self.i += 1;
                    fields.push(Symbol::intern(&s));
                }
                Some(Tok::Int(n)) => {
                    self.i += 1;
                    fields.push(Symbol::intern(&n.to_string()));
                }
                _ => return Err("expected a field name after `.`".to_string()),
            }
        }
        Ok(SpecPlace { base, fields })
    }
    fn ty(&mut self) -> Result<SpecTy, String> {
        if self.eat_punct('*') {
            return Ok(SpecTy::Pointee(self.place()?));
        }
        let s = self.ident()?;
        if self.peek() == Some(&Tok::ColCol) || self.peek() == Some(&Tok::Punct('<')) {
            return Err(format!(
                "type paths are not supported in specs (`{s}`); use a generic parameter, `Self` or `*place`"
            ));
        }
        Ok(SpecTy::Param(Symbol::intern(&s)))
    }
    fn atom(&mut self) -> Result<SpecExpr, String> {
        match self.peek().cloned() {
            Some(Tok::Int(n)) => {
                self.i += 1;
                Ok(SpecExpr::Lit(n))
            }
            Some(Tok::Punct('(')) => {
                self.i += 1;
                let e = self.expr()?;
                self.expect_punct(')')?;
                Ok(e)
            }
            Some(Tok::Ident(s)) if s == "size_of" => {
                self.i += 1;
                if self.peek() != Some(&Tok::ColCol) {
                    return Err("expected `size_of::<ty>()`".to_string());
                }
                self.i += 1;
                self.expect_punct('<')?;
                let t = self.ty()?;
                self.expect_punct('>')?;
                self.expect_punct('(')?;
                self.expect_punct(')')?;
                Ok(SpecExpr::SizeOf(t))
            }
            Some(Tok::Ident(s)) if s == "old" && self.t.get(self.i + 1) == Some(&Tok::Punct('(')) => {
                self.i += 2;
                let e = self.expr()?;
                self.expect_punct(')')?;
                Ok(SpecExpr::Old(Box::new(e)))
            }
            Some(Tok::Ident(s)) if s == "size_of_val" => {
                self.i += 1;
                self.expect_punct('(')?;
                self.expect_punct('*')?;
                let p = self.place()?;
                self.expect_punct(')')?;
                Ok(SpecExpr::SizeOfPointee(p))
            }
            Some(Tok::Ident(_)) => Ok(SpecExpr::Place(self.place()?)),
            _ => Err("expected an expression".to_string()),
        }
    }
    fn term(&mut self) -> Result<SpecExpr, String> {
        let mut l = self.atom()?;
        loop {
            let op = if self.eat_punct('*') {
                SpecBinOp::Mul
            } else if self.eat_punct('%') {
                SpecBinOp::Rem
            } else {
                return Ok(l);
            };
            let r = self.atom()?;
            l = SpecExpr::Bin(op, Box::new(l), Box::new(r));
        }
    }
    fn expr(&mut self) -> Result<SpecExpr, String> {
        let mut l = self.term()?;
        loop {
            let op = if self.eat_punct('+') {
                SpecBinOp::Add
            } else if self.eat_punct('-') {
                SpecBinOp::Sub
            } else {
                return Ok(l);
            };
            let r = self.term()?;
            l = SpecExpr::Bin(op, Box::new(l), Box::new(r));
        }
    }
    fn call(&mut self) -> Result<SpecCall, String> {
        let name = Symbol::intern(&self.ident()?);
        self.expect_punct('(')?;
        let mut args = Vec::new();
        if !self.eat_punct(')') {
            loop {
                args.push(Symbol::intern(&self.ident()?));
                if self.eat_punct(')') {
                    break;
                }
                self.expect_punct(',')?;
            }
        }
        Ok(SpecCall { name, args })
    }
}

/// Parse one spec string.
pub fn parse_spec(s: &str) -> Result<CheriSpec, String> {
    let mut p = P { t: lex(s)?, i: 0 };
    let form = p.ident()?;
    p.expect_punct('(')?;
    // Keyed arguments (`key = value`), bare flags (`fields`) and bare names (escapes).
    let mut keys: Vec<(String, usize)> = Vec::new();
    let mut bare: Vec<String> = Vec::new();
    let known_keys: &[&str] = match form.as_str() {
        "frees" => &["block", "size"],
        "dealloc_of" => &["ty", "or", "block", "size"],
        "relocates" => &["block", "size", "keep", "alloc", "free"],
        "live" => &["block", "bytes", "held", "ty"],
        "retains" | "borrows" => &[],
        "stores" => &["into"],
        "vacates" => &["ty", "block", "from", "to"],
        _ => {
            return Err(format!(
                "unknown spec `{form}`; expected one of `frees`, `dealloc_of`, `relocates`, `live`, `retains`, `borrows`, `stores`, `vacates`"
            ));
        }
    };
    let mut values: Vec<(String, Value)> = Vec::new();
    let mut block_addr = false;
    enum Value {
        E(SpecExpr),
        T(SpecTy),
        P(SpecPlace),
        C(SpecCall),
    }
    if !p.eat_punct(')') {
        loop {
            let k = p.ident()?;
            if p.eat_punct('=') {
                if !known_keys.contains(&k.as_str()) {
                    return Err(format!("unknown key `{k}` in `{form}`"));
                }
                if keys.iter().any(|(x, _)| *x == k) {
                    return Err(format!("duplicate key `{k}` in `{form}`"));
                }
                keys.push((k.clone(), values.len()));
                let v = match k.as_str() {
                    "ty" | "or" | "held" => Value::T(p.ty()?),
                    "block" if form == "vacates" && p.eat_punct('&') => {
                        block_addr = true;
                        Value::P(p.place()?)
                    }
                    "block" | "into" => Value::P(p.place()?),
                    "alloc" | "free" => Value::C(p.call()?),
                    _ => Value::E(p.expr()?),
                };
                values.push((k, v));
            } else {
                bare.push(k);
            }
            if p.eat_punct(')') {
                break;
            }
            p.expect_punct(',')?;
        }
    }
    if p.peek().is_some() {
        return Err("unexpected tokens after the spec".to_string());
    }
    let mut take = |k: &str| -> Option<Value> {
        let pos = values.iter().position(|(x, _)| x == k)?;
        Some(values.remove(pos).1)
    };
    let need = |v: Option<Value>, k: &str| v.ok_or_else(|| format!("`{form}` needs `{k} = ...`"));
    let as_e = |v: Value| match v {
        Value::E(e) => e,
        _ => unreachable!(),
    };
    let as_t = |v: Value| match v {
        Value::T(t) => t,
        _ => unreachable!(),
    };
    let as_p = |v: Value| match v {
        Value::P(p) => p,
        _ => unreachable!(),
    };
    let as_c = |v: Value| match v {
        Value::C(c) => c,
        _ => unreachable!(),
    };
    let only_bare = |allowed: &[&str]| -> Result<(), String> {
        for b in &bare {
            if !allowed.contains(&b.as_str()) {
                return Err(format!("unexpected `{b}` in `{form}`"));
            }
        }
        Ok(())
    };
    let spec = match form.as_str() {
        "frees" => {
            only_bare(&[])?;
            CheriSpec::Frees {
                block: as_p(need(take("block"), "block")?),
                size: as_e(need(take("size"), "size")?),
            }
        }
        "dealloc_of" => {
            only_bare(&["fields"])?;
            let ty = as_t(need(take("ty"), "ty")?);
            let or = take("or").map(as_t);
            let fields = bare.iter().any(|b| b == "fields");
            let block = take("block").map(as_p);
            let size = take("size").map(as_e);
            if or.is_some() && !fields {
                return Err("`or = ...` needs `fields`".to_string());
            }
            if size.is_some() && block.is_none() {
                return Err("`size = ...` needs `block = ...`".to_string());
            }
            CheriSpec::DeallocOf { ty, or, fields, block, size }
        }
        "relocates" => {
            only_bare(&[])?;
            let block = as_p(need(take("block"), "block")?);
            let size = as_e(need(take("size"), "size")?);
            let keep = as_e(need(take("keep"), "keep")?);
            let alloc = take("alloc").map(as_c);
            let free = take("free").map(as_c);
            if alloc.is_some() != free.is_some() {
                return Err("`relocates` needs both `alloc = ...` and `free = ...`, or neither".to_string());
            }
            CheriSpec::Relocates { block, size, keep, alloc, free }
        }
        "live" => {
            only_bare(&[])?;
            CheriSpec::Live {
                block: as_p(need(take("block"), "block")?),
                bytes: as_e(need(take("bytes"), "bytes")?),
                held: as_t(need(take("held"), "held")?),
                ty: take("ty").map(as_t),
            }
        }
        "retains" | "borrows" => {
            if bare.len() != 1 {
                return Err(format!("`{form}` takes exactly one parameter name"));
            }
            let s = Symbol::intern(&bare[0]);
            if form == "retains" { CheriSpec::Retains(s) } else { CheriSpec::Borrows(s) }
        }
        "stores" => {
            if bare.len() != 1 {
                return Err("`stores` takes one parameter name and `into = place`".to_string());
            }
            CheriSpec::Stores {
                param: Symbol::intern(&bare[0]),
                into: as_p(need(take("into"), "into")?),
            }
        }
        "vacates" => {
            only_bare(&[])?;
            CheriSpec::Vacates {
                ty: as_t(need(take("ty"), "ty")?),
                block: as_p(need(take("block"), "block")?),
                addr: block_addr,
                from: as_e(need(take("from"), "from")?),
                to: as_e(need(take("to"), "to")?),
            }
        }
        _ => unreachable!(),
    };
    if !matches!(spec, CheriSpec::Vacates { .. }) && spec_has_old(&spec) {
        return Err(format!("`old(..)` is only allowed in `vacates`, not in `{form}`"));
    }
    Ok(spec)
}

/// [LLM-generated] M7.3: does `e` contain `old(..)`?
pub fn expr_has_old(e: &SpecExpr) -> bool {
    match e {
        SpecExpr::Old(_) => true,
        SpecExpr::Bin(_, a, b) => expr_has_old(a) || expr_has_old(b),
        SpecExpr::Lit(_) | SpecExpr::Place(_) | SpecExpr::SizeOf(_) | SpecExpr::SizeOfPointee(_) => {
            false
        }
    }
}

fn spec_has_old(s: &CheriSpec) -> bool {
    match s {
        CheriSpec::Frees { size, .. } => expr_has_old(size),
        CheriSpec::DeallocOf { size, .. } => size.as_ref().is_some_and(expr_has_old),
        CheriSpec::Relocates { size, keep, .. } => expr_has_old(size) || expr_has_old(keep),
        CheriSpec::Live { bytes, .. } => expr_has_old(bytes),
        CheriSpec::Vacates { from, to, .. } => expr_has_old(from) || expr_has_old(to),
        CheriSpec::Retains(_) | CheriSpec::Borrows(_) | CheriSpec::Stores { .. } => false,
    }
}

// ---------------------------------------------------------------------------------------
// Lookup
// ---------------------------------------------------------------------------------------

/// The raw spec strings written directly on `def_id`.
pub fn own_spec_strs(tcx: TyCtxt<'_>, def_id: DefId) -> Vec<(Symbol, Span)> {
    find_attr!(tcx, def_id, RustcCheriSpec(v) => v.iter().copied().collect::<Vec<_>>())
        .unwrap_or_default()
}

/// The specs of a function: its own, or (for a trait impl method without its own)
/// those of the trait method it implements. Malformed strings are skipped here; they
/// are reported by `check_attr` in the defining crate.
pub fn fn_specs(tcx: TyCtxt<'_>, def_id: DefId) -> (DefId, Vec<CheriSpec>) {
    let own = own_spec_strs(tcx, def_id);
    if !own.is_empty() {
        return (def_id, own.iter().filter_map(|(s, _)| parse_spec(s.as_str()).ok()).collect());
    }
    if matches!(tcx.def_kind(def_id), DefKind::AssocFn)
        && let Some(tr) = tcx.trait_item_of(def_id)
    {
        let inh = own_spec_strs(tcx, tr);
        if !inh.is_empty() {
            return (tr, inh.iter().filter_map(|(s, _)| parse_spec(s.as_str()).ok()).collect());
        }
    }
    (def_id, Vec::new())
}

// ---------------------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------------------

/// The pointee of a pointer-like type: `&T`, `*T`, `Box<T>`, `NonNull<T>`, `Unique<T>`
/// (a struct whose only non-`PhantomData` field is pointer-like), a pattern type over a
/// pointer, or `Option` of one of these.
pub fn pointee_of<'tcx>(tcx: TyCtxt<'tcx>, ty: Ty<'tcx>) -> Option<Ty<'tcx>> {
    fn rec<'tcx>(tcx: TyCtxt<'tcx>, ty: Ty<'tcx>, depth: u32) -> Option<Ty<'tcx>> {
        if depth == 0 {
            return None;
        }
        match *ty.kind() {
            ty::Ref(_, t, _) | ty::RawPtr(t, _) => Some(t),
            ty::Pat(t, _) => rec(tcx, t, depth - 1),
            ty::Adt(def, args) => {
                if def.is_box() {
                    return Some(args.type_at(0));
                }
                if tcx.is_lang_item(def.did(), rustc_hir::attrs::lang_items::LangItem::Option) {
                    return rec(tcx, args.type_at(0), depth - 1);
                }
                if !def.is_struct() {
                    return None;
                }
                let mut cand = def
                    .non_enum_variant()
                    .fields
                    .iter()
                    .map(|f| f.ty(tcx, args).skip_norm_wip())
                    .filter(|t| !matches!(t.kind(), ty::Adt(d, _) if d.is_phantom_data()));
                let one = cand.next()?;
                if cand.next().is_some() {
                    return None;
                }
                rec(tcx, one, depth - 1)
            }
            _ => None,
        }
    }
    rec(tcx, ty, 6)
}

pub fn is_pointer_like<'tcx>(tcx: TyCtxt<'tcx>, ty: Ty<'tcx>) -> bool {
    pointee_of(tcx, ty).is_some()
}

/// Index of the struct field `name` (by name, or a tuple index) in `adt`.
pub fn field_index<'tcx>(adt: ty::AdtDef<'tcx>, name: Symbol) -> Option<usize> {
    if !adt.is_struct() {
        return None;
    }
    adt.non_enum_variant().fields.iter().position(|f| f.name == name)
}

/// One step of a place: look through pointers (repeatedly), then take field `name`.
/// Returns the field's type plus how many pointer levels were looked through.
pub fn field_step<'tcx>(
    tcx: TyCtxt<'tcx>,
    mut ty: Ty<'tcx>,
    name: Symbol,
) -> Result<(Ty<'tcx>, u32, usize), String> {
    let mut derefs = 0;
    loop {
        if let ty::Adt(def, args) = *ty.kind()
            && let Some(i) = field_index(def, name)
        {
            let f = &def.non_enum_variant().fields[rustc_abi::FieldIdx::from_usize(i)];
            return Ok((f.ty(tcx, args).skip_norm_wip(), derefs, i));
        }
        match pointee_of(tcx, ty) {
            Some(p) if derefs < 4 => {
                ty = p;
                derefs += 1;
            }
            _ => return Err(format!("no field `{name}` on type `{ty}`")),
        }
    }
}

/// The (identity) types of `def_id`'s parameters, and their names.
pub fn fn_params<'tcx>(tcx: TyCtxt<'tcx>, def_id: DefId) -> Vec<(Option<Symbol>, Ty<'tcx>)> {
    let sig = tcx.fn_sig(def_id).instantiate_identity().skip_norm_wip();
    let sig = tcx.instantiate_bound_regions_with_erased(sig);
    let idents = tcx.fn_arg_idents(def_id);
    sig.inputs()
        .iter()
        .enumerate()
        .map(|(i, t)| (idents.get(i).copied().flatten().map(|id| id.name), *t))
        .collect()
}

pub fn param_index(params: &[(Option<Symbol>, Ty<'_>)], name: Symbol) -> Option<usize> {
    params.iter().position(|(n, _)| *n == Some(name))
}

/// Type of a place whose base has type `base_ty`.
pub fn place_ty_from<'tcx>(
    tcx: TyCtxt<'tcx>,
    base_ty: Ty<'tcx>,
    p: &SpecPlace,
) -> Result<Ty<'tcx>, String> {
    let mut t = base_ty;
    for f in &p.fields {
        t = field_step(tcx, t, *f)?.0;
    }
    Ok(t)
}

/// Type of a place over `def_id`'s parameters (identity generics).
pub fn param_place_ty<'tcx>(
    tcx: TyCtxt<'tcx>,
    params: &[(Option<Symbol>, Ty<'tcx>)],
    p: &SpecPlace,
) -> Result<Ty<'tcx>, String> {
    let Some(i) = param_index(params, p.base) else {
        return Err(format!("no parameter named `{}`", p.base));
    };
    place_ty_from(tcx, params[i].1, p)
}

/// A generic type parameter (or `Self`) of `def_id` or its parents, by name.
pub fn generic_ty<'tcx>(tcx: TyCtxt<'tcx>, def_id: DefId, name: Symbol) -> Option<Ty<'tcx>> {
    if name == kw::SelfUpper
        && let Some(impl_did) = tcx.impl_of_assoc(def_id)
    {
        return Some(tcx.type_of(impl_did).instantiate_identity().skip_norm_wip());
    }
    let mut g = Some(tcx.generics_of(def_id));
    while let Some(gen_) = g {
        for p in &gen_.own_params {
            if p.name == name && matches!(p.kind, ty::GenericParamDefKind::Type { .. }) {
                return Some(Ty::new_param(tcx, p.index, p.name));
            }
        }
        g = gen_.parent.map(|pd| tcx.generics_of(pd));
    }
    None
}

/// Resolve a spec type over `def_id`'s generics and parameter types (identity args).
/// `local_ty` resolves a place base that is not a parameter (statement specs).
pub fn spec_ty<'tcx>(
    tcx: TyCtxt<'tcx>,
    def_id: DefId,
    params: &[(Option<Symbol>, Ty<'tcx>)],
    t: &SpecTy,
    local_ty: &dyn Fn(Symbol) -> Option<Ty<'tcx>>,
) -> Result<Ty<'tcx>, String> {
    match t {
        SpecTy::Param(n) => generic_ty(tcx, def_id, *n)
            .ok_or_else(|| format!("`{n}` is not a generic type parameter or `Self` here")),
        SpecTy::Pointee(p) => {
            let base = match param_index(params, p.base) {
                Some(i) => params[i].1,
                None => local_ty(p.base).ok_or_else(|| format!("no parameter named `{}`", p.base))?,
            };
            let pt = place_ty_from(tcx, base, p)?;
            pointee_of(tcx, pt).ok_or_else(|| format!("`*{}`: `{pt}` is not a pointer", show_place(p)))
        }
    }
}

pub fn show_place(p: &SpecPlace) -> String {
    let mut s = p.base.to_string();
    for f in &p.fields {
        s.push('.');
        s.push_str(f.as_str());
    }
    s
}

/// Integer-like: an integer, a pattern type over one, or a struct whose only non-ZST
/// field is integer-like (e.g. `UsizeNoHighBit`).
pub fn is_integer_like<'tcx>(tcx: TyCtxt<'tcx>, ty: Ty<'tcx>) -> bool {
    fn rec<'tcx>(tcx: TyCtxt<'tcx>, ty: Ty<'tcx>, d: u32) -> bool {
        if d == 0 {
            return false;
        }
        match *ty.kind() {
            ty::Int(_) | ty::Uint(_) => true,
            ty::Pat(t, _) => rec(tcx, t, d - 1),
            ty::Adt(def, args) if def.is_struct() => {
                let mut fs = def
                    .non_enum_variant()
                    .fields
                    .iter()
                    .map(|f| f.ty(tcx, args).skip_norm_wip())
                    .filter(|t| !matches!(t.kind(), ty::Adt(d, _) if d.is_phantom_data()));
                match (fs.next(), fs.next()) {
                    (Some(t), None) => rec(tcx, t, d - 1),
                    _ => false,
                }
            }
            _ => false,
        }
    }
    rec(tcx, ty, 4)
}

/// Check a spec expression over `def_id`'s parameters (or locals, via `local_ty`).
pub fn check_expr<'tcx>(
    tcx: TyCtxt<'tcx>,
    def_id: DefId,
    params: &[(Option<Symbol>, Ty<'tcx>)],
    e: &SpecExpr,
    local_ty: &dyn Fn(Symbol) -> Option<Ty<'tcx>>,
) -> Result<(), String> {
    match e {
        SpecExpr::Lit(_) => Ok(()),
        SpecExpr::Place(p) => {
            let t = place_ty_any(tcx, params, p, local_ty)?;
            if is_integer_like(tcx, t) {
                Ok(())
            } else {
                Err(format!("`{}` has type `{t}`, not an integer", show_place(p)))
            }
        }
        SpecExpr::SizeOf(t) => spec_ty(tcx, def_id, params, t, local_ty).map(|_| ()),
        SpecExpr::SizeOfPointee(p) => {
            let t = place_ty_any(tcx, params, p, local_ty)?;
            if is_pointer_like(tcx, t) {
                Ok(())
            } else {
                Err(format!("`size_of_val(*{})`: `{t}` is not a pointer", show_place(p)))
            }
        }
        SpecExpr::Bin(_, a, b) => {
            check_expr(tcx, def_id, params, a, local_ty)?;
            check_expr(tcx, def_id, params, b, local_ty)
        }
        SpecExpr::Old(a) => check_expr(tcx, def_id, params, a, local_ty),
    }
}

pub fn place_ty_any<'tcx>(
    tcx: TyCtxt<'tcx>,
    params: &[(Option<Symbol>, Ty<'tcx>)],
    p: &SpecPlace,
    local_ty: &dyn Fn(Symbol) -> Option<Ty<'tcx>>,
) -> Result<Ty<'tcx>, String> {
    let base = match param_index(params, p.base) {
        Some(i) => params[i].1,
        None => local_ty(p.base).ok_or_else(|| format!("no parameter or local named `{}`", p.base))?,
    };
    place_ty_from(tcx, base, p)
}

/// Check a pointer-valued place (`block = ...`).
pub fn check_block<'tcx>(
    tcx: TyCtxt<'tcx>,
    params: &[(Option<Symbol>, Ty<'tcx>)],
    p: &SpecPlace,
    local_ty: &dyn Fn(Symbol) -> Option<Ty<'tcx>>,
) -> Result<(), String> {
    let t = place_ty_any(tcx, params, p, local_ty)?;
    if is_pointer_like(tcx, t) {
        Ok(())
    } else {
        Err(format!("`block = {}`: `{t}` is not a pointer", show_place(p)))
    }
}

/// Well-formedness of the specs on a function (`check_attr`). Returns one message per
/// problem. Statement specs are checked when MIR is built (they name locals).
pub fn check_fn_spec<'tcx>(tcx: TyCtxt<'tcx>, def_id: DefId, spec: &CheriSpec) -> Vec<String> {
    let params = fn_params(tcx, def_id);
    let no_local = |_: Symbol| None;
    let mut errs = Vec::new();
    let mut r = |x: Result<(), String>| {
        if let Err(e) = x {
            errs.push(e)
        }
    };
    match spec {
        CheriSpec::Frees { block, size } => {
            r(check_block(tcx, &params, block, &no_local));
            r(check_expr(tcx, def_id, &params, size, &no_local));
        }
        CheriSpec::DeallocOf { ty, or, fields: _, block, size } => {
            r(spec_ty(tcx, def_id, &params, ty, &no_local).map(|_| ()));
            if let Some(o) = or {
                r(spec_ty(tcx, def_id, &params, o, &no_local).map(|_| ()));
            }
            if let Some(b) = block {
                r(check_block(tcx, &params, b, &no_local));
            }
            if let Some(s) = size {
                r(check_expr(tcx, def_id, &params, s, &no_local));
            }
        }
        CheriSpec::Relocates { block, size, keep, alloc, free } => {
            r(check_block(tcx, &params, block, &no_local));
            r(check_expr(tcx, def_id, &params, size, &no_local));
            r(check_expr(tcx, def_id, &params, keep, &no_local));
            for c in [alloc, free].into_iter().flatten() {
                r(check_sibling_call(tcx, def_id, &params, c));
            }
        }
        CheriSpec::Live { .. } => {
            r(Err("`live` is a statement spec; put it on the `let` of the call".to_string()))
        }
        CheriSpec::Retains(p) | CheriSpec::Borrows(p) => {
            if param_index(&params, *p).is_none() {
                r(Err(format!("no parameter named `{p}`")));
            } else if !ty_may_hold_ref(tcx, params[param_index(&params, *p).unwrap()].1) {
                r(Err(format!("`{}({p})`: the parameter holds no pointer", spec.form_name())));
            }
        }
        CheriSpec::Stores { param, into } => {
            if param_index(&params, *param).is_none() {
                r(Err(format!("no parameter named `{param}`")));
            }
            r(check_block(tcx, &params, into, &no_local));
        }
        CheriSpec::Vacates { ty, block, addr, from, to } => {
            r(spec_ty(tcx, def_id, &params, ty, &no_local).map(|_| ()));
            if *addr {
                r(place_ty_any(tcx, &params, block, &no_local).map(|_| ()));
            } else {
                r(check_block(tcx, &params, block, &no_local));
            }
            r(check_expr(tcx, def_id, &params, from, &no_local));
            r(check_expr(tcx, def_id, &params, to, &no_local));
        }
    }
    // `alloc` and `free` can fail the same way; report each message once.
    errs.dedup();
    errs
}

/// Escape specs need a parameter that can carry a pointer (generic parameters may).
fn ty_may_hold_ref<'tcx>(tcx: TyCtxt<'tcx>, ty: Ty<'tcx>) -> bool {
    match *ty.kind() {
        ty::Bool | ty::Char | ty::Int(_) | ty::Uint(_) | ty::Float(_) | ty::Str | ty::Never => false,
        _ => {
            let _ = tcx;
            true
        }
    }
}

/// `alloc = name(args)` / `free = name(args)` on a trait method: `name` must be a
/// method of the same trait and every argument a parameter of this method.
fn check_sibling_call<'tcx>(
    tcx: TyCtxt<'tcx>,
    def_id: DefId,
    params: &[(Option<Symbol>, Ty<'tcx>)],
    c: &SpecCall,
) -> Result<(), String> {
    let Some(tr) = tcx.trait_of_assoc(def_id) else {
        return Err("`alloc = ...` / `free = ...` are only allowed on a trait method".to_string());
    };
    let Some(item) = tcx.associated_items(tr).filter_by_name_unhygienic(c.name).next() else {
        return Err(format!("the trait has no method `{}`", c.name));
    };
    if !matches!(tcx.def_kind(item.def_id), DefKind::AssocFn) {
        return Err(format!("`{}` is not a method", c.name));
    }
    let n = tcx.fn_sig(item.def_id).skip_binder().skip_binder().inputs().len();
    if n != c.args.len() {
        return Err(format!("`{}` takes {n} arguments, the spec passes {}", c.name, c.args.len()));
    }
    for a in &c.args {
        if param_index(params, *a).is_none() {
            return Err(format!("no parameter named `{a}`"));
        }
    }
    Ok(())
}

/// The sibling trait method named by a `relocates` spec.
pub fn sibling_method(tcx: TyCtxt<'_>, def_id: DefId, name: Symbol) -> Option<DefId> {
    let tr = tcx.trait_of_assoc(def_id)?;
    tcx.associated_items(tr).filter_by_name_unhygienic(name).next().map(|i| i.def_id)
}

/// [LLM-generated] CHERI M7.2h: one callee store mark of a non-generic function, as
/// kept in crate metadata (query `cheri_store_marks`). The body stores the pointer given
/// by Rust argument `src` (0-based) into the 16-byte slot at byte offset `off` from
/// argument `dst`; `src_deref` is `Some(d)` when the stored value is the pointer read
/// at byte offset `d` through the shared reference `src`. Computed by
/// `rustc_codegen_ssa::mir::cheri_store_marks` in the defining crate and only read by
/// the CHERI codegen of downstream crates.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[derive(rustc_macros::Decodable, rustc_macros::Encodable, rustc_macros::StableHash)]
pub struct CheriStoreMark {
    pub src: u32,
    pub dst: u32,
    pub off: u64,
    pub src_deref: Option<u64>,
}
