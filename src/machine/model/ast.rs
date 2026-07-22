//! AST node types shared across the parse module.

use crate::machine::DeliveredCarried;
use std::collections::HashMap;
use std::marker::PhantomData;

use crate::source::{FileId, Span, Spanned};

use crate::machine::model::{
    BinderKey, Carried, Held, KKey, KObject, Parseable, UntypedElement, UntypedKey,
};
use crate::witnessed::reattachable;

#[cfg(test)]
mod tests;

#[derive(Debug, Clone)]
pub enum KLiteral {
    Number(f64),
    String(String),
    Boolean(bool),
    Null,
}

impl KLiteral {
    /// The owned [`KObject`] this literal denotes — region-pure (no borrow), so a construction site can
    /// build it **inside** a [`yoke`](crate::witnessed::Witnessed::yoke) closure. Generic in `'a`
    /// (rather than `'static`) despite [`KObject`] being invariant in its lifetime: every arm owns its
    /// data, so the value is constructible at *any* lifetime — in particular the caller's `yoke` brand.
    pub fn to_kobject<'a>(&self) -> KObject<'a> {
        match self {
            KLiteral::Number(n) => KObject::Number(*n),
            KLiteral::String(s) => KObject::KString(s.clone()),
            KLiteral::Boolean(b) => KObject::Bool(*b),
            KLiteral::Null => KObject::Null,
        }
    }
}

/// A bare type identifier as written in source (`Number`, `Point`, `Mo.Ty`) — a single name
/// token, never compound syntax.
///
/// A thin newtype over the source name: `Deref`s to `str`, derives eq/hash by string. The
/// identifier stays a flat name even when it *denotes* a compound type (a `NEWTYPE` / `UNION`
/// name resolves to a record / tagged type); compound *syntax* (`:(LIST OF …)`, `:(FN … -> …)`)
/// is a `SigiledTypeExpr`, not a `TypeIdentifier`. The position tag rides on the carrier variant
/// (`ExpressionPart::Type`), not on this struct.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypeIdentifier(String);

impl std::ops::Deref for TypeIdentifier {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

impl TypeIdentifier {
    pub fn leaf(name: String) -> TypeIdentifier {
        TypeIdentifier(name)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Render in surface syntax so the output round-trips through the parser unchanged.
    pub fn render(&self) -> String {
        self.0.clone()
    }
}

/// One element of a parsed expression. `Spliced` is introduced by the scheduler when it
/// splices a completed dep's resolved value into its dependent's parts list.
pub enum ExpressionPart<'a> {
    Keyword(String),
    Identifier(String),
    Type(TypeIdentifier),
    Expression(Box<KExpression<'a>>),
    /// Parse-context marker for a `:(...)` group: the wrapped `KExpression` must dispatch
    /// in type-context, returning a type-side carrier. Shape recognition is the
    /// dispatcher's responsibility — the parser does no folding here. See
    /// [design/typing/type-language-via-dispatch.md](../../../design/typing/type-language-via-dispatch.md).
    SigiledTypeExpr(Box<KExpression<'a>>),
    /// First-class record type `:{x :Number, y :Str}`. The boxed `KExpression` is the
    /// field-list `(x :Number, y :Str)` — the same `<name> :<Type>` pair shape a SIG member
    /// or FN parameter list uses. Unlike `SigiledTypeExpr`, this is matched
    /// structurally (the elaborator folds it straight to `KType::Record`); there is no
    /// internal type-constructor builtin behind it. See
    /// [design/typing/type-language-via-dispatch.md](../../../design/typing/type-language-via-dispatch.md).
    RecordType(Box<KExpression<'a>>),
    ListLiteral(Vec<ExpressionPart<'a>>),
    DictLiteral(Vec<(ExpressionPart<'a>, ExpressionPart<'a>)>),
    /// Anonymous record literal (`{x = 1, y = "a"}`) — identifier-keyed `=` pairs. The
    /// brace frame routes here when the first pair separator is `=`; `:` pairs stay a
    /// `DictLiteral`. Field names are syntactic identifiers (never name-resolved).
    RecordLiteral(Vec<(String, ExpressionPart<'a>)>),
    Literal(KLiteral),
    /// A `#(...)` quote: the parenthesized body captured at parse time as data. The parser folds
    /// the sigil and its group into this part, so quoting is static syntax — there is no runtime
    /// quoting operation and the body never dispatches. Behaves as a literal everywhere: it is a
    /// `Slot` in the untyped key, a single one classifies [`DispatchShape::LiteralPassThrough`],
    /// and it resolves to `KObject::KExpression(<body>)` — the value `$(...)` evaluates. See
    /// [design/expressions-and-parsing.md](../../../design/expressions-and-parsing.md).
    QuotedExpression(Box<KExpression<'a>>),
    /// A resolved sub-result travelling as its producer's [`DeliveredCarried`] envelope — the sealed
    /// carrier (value and reach as one unit) bundled with the retained frame owner that pins its
    /// backing in transit. The lifetime-free `cell` rests on the working expression across steps; the
    /// consuming decide or bind opens it (to classify) or adopts it (to consume) at its own step
    /// brand, reading the value under the envelope's own pin — the producer's retention hold for a
    /// working-copy splice, or the reading scope's own owner for a resident splice, or `None` for a
    /// frameless / run producer whose backing already outlives it.
    Spliced {
        cell: DeliveredCarried,
    },
    /// A positional argument slot whose eager value is being produced by a sibling dispatch,
    /// awaiting its resolved carrier. The keyworded part walk stages an eager part as an owned
    /// [`DepRequest`](crate::machine::core::DepRequest) and leaves this marker in its slot so the
    /// part list keeps its length and index alignment; `install_eager_subs`'s finish overwrites
    /// each marked slot with the resolved [`Spliced`](ExpressionPart::Spliced) cell. It is a
    /// scheduler-internal hole, never a language-level value — it exists only between staging and
    /// splice and is never name-resolved.
    StagedSlot,
}

/// Registry-free rendering of a spliced cell's carried value, for `Debug` and the registry-free
/// [`ExpressionPart::summarize`]. A type name resolves through the registry, which neither signature
/// carries, so the type channel renders its content-digest hex — the value's own identity — and an
/// object renders its type's digest. An unlowered name is already a bare surface string.
fn spliced_summary(carried: Carried<'_>) -> String {
    match carried {
        Carried::Type(kt) => format!("0x{:032x}", kt.digest().0),
        Carried::UnresolvedType(ti) => ti.render(),
        Carried::Object(object) => format!("0x{:032x}", object.ktype().digest().0),
    }
}

impl<'a> std::fmt::Debug for ExpressionPart<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExpressionPart::Keyword(s) => f.debug_tuple("Keyword").field(s).finish(),
            ExpressionPart::Identifier(s) => f.debug_tuple("Identifier").field(s).finish(),
            ExpressionPart::Type(t) => f.debug_tuple("Type").field(t).finish(),
            ExpressionPart::Expression(e) => f.debug_tuple("Expression").field(e).finish(),
            ExpressionPart::SigiledTypeExpr(e) => {
                f.debug_tuple("SigiledTypeExpr").field(e).finish()
            }
            ExpressionPart::RecordType(e) => f.debug_tuple("RecordType").field(e).finish(),
            ExpressionPart::ListLiteral(items) => {
                f.debug_tuple("ListLiteral").field(items).finish()
            }
            ExpressionPart::DictLiteral(pairs) => {
                f.debug_tuple("DictLiteral").field(pairs).finish()
            }
            ExpressionPart::RecordLiteral(pairs) => {
                f.debug_tuple("RecordLiteral").field(pairs).finish()
            }
            ExpressionPart::Literal(l) => f.debug_tuple("Literal").field(l).finish(),
            ExpressionPart::QuotedExpression(e) => {
                f.debug_tuple("QuotedExpression").field(e).finish()
            }
            ExpressionPart::Spliced { cell, .. } => {
                write!(f, "Spliced({})", cell.open(spliced_summary))
            }
            ExpressionPart::StagedSlot => write!(f, "StagedSlot"),
        }
    }
}

impl<'a> ExpressionPart<'a> {
    pub fn expression(parts: Vec<ExpressionPart<'a>>) -> ExpressionPart<'a> {
        ExpressionPart::Expression(Box::new(KExpression::new(
            parts.into_iter().map(Spanned::bare).collect(),
        )))
    }

    /// True when neither this part nor any part nested beneath it is a `Spliced` cell. See
    /// [`KExpression::is_splice_free`].
    fn is_splice_free(&self) -> bool {
        match self {
            // A spliced cell is a scheduler-introduced carrier, not raw syntactic AST.
            ExpressionPart::Spliced { .. } => false,
            // A quote body is parse output, so it holds no `Spliced` cell; the recursion keeps the
            // audit total over hand-built trees rather than trusting the construction site.
            ExpressionPart::Expression(e)
            | ExpressionPart::SigiledTypeExpr(e)
            | ExpressionPart::RecordType(e)
            | ExpressionPart::QuotedExpression(e) => e.is_splice_free(),
            ExpressionPart::ListLiteral(items) => items.iter().all(ExpressionPart::is_splice_free),
            ExpressionPart::DictLiteral(pairs) => pairs
                .iter()
                .all(|(k, v)| k.is_splice_free() && v.is_splice_free()),
            ExpressionPart::RecordLiteral(pairs) => pairs.iter().all(|(_, v)| v.is_splice_free()),
            // A staged slot is a bare marker with nothing nested beneath it — not yet a `Spliced`
            // cell, so it holds no producer reach either.
            ExpressionPart::Keyword(_)
            | ExpressionPart::Identifier(_)
            | ExpressionPart::Type(_)
            | ExpressionPart::Literal(_)
            | ExpressionPart::StagedSlot => true,
        }
    }

    /// Per-part subset of `KExpression::summarize`.
    pub fn summarize(&self) -> String {
        match self {
            ExpressionPart::Keyword(s) => s.clone(),
            ExpressionPart::Identifier(s) => s.clone(),
            ExpressionPart::Type(t) => t.render(),
            ExpressionPart::Expression(e) => e.summarize(),
            ExpressionPart::SigiledTypeExpr(e) => format!(":({})", e.summarize()),
            ExpressionPart::RecordType(e) => format!(":{{{}}}", e.summarize()),
            ExpressionPart::QuotedExpression(e) => format!("#({})", e.summarize()),
            ExpressionPart::ListLiteral(items) => {
                let inner: Vec<String> = items.iter().map(|p| p.summarize()).collect();
                format!("[{}]", inner.join(" "))
            }
            ExpressionPart::DictLiteral(pairs) => {
                let inner: Vec<String> = pairs
                    .iter()
                    .map(|(k, v)| format!("{}: {}", k.summarize(), v.summarize()))
                    .collect();
                format!("{{{}}}", inner.join(", "))
            }
            ExpressionPart::RecordLiteral(pairs) => {
                let inner: Vec<String> = pairs
                    .iter()
                    .map(|(k, v)| format!("{} = {}", k, v.summarize()))
                    .collect();
                format!("{{{}}}", inner.join(", "))
            }
            ExpressionPart::Literal(lit) => match lit {
                KLiteral::Number(n) => n.to_string(),
                KLiteral::String(s) => s.clone(),
                KLiteral::Boolean(b) => b.to_string(),
                KLiteral::Null => "null".to_string(),
            },
            ExpressionPart::Spliced { cell, .. } => cell.open(spliced_summary),
            ExpressionPart::StagedSlot => "<staged>".to_string(),
        }
    }

    /// Slot-aware resolve producing an owned [`Held`] cell, run at [`KFunction::bind_args`] time. A
    /// type rides the `Type` arm; a runtime value rides the `Object` arm. A spliced cell is first
    /// adopted into `scope` (reach folded, value re-anchored at the scope brand) so a cloned type
    /// that still borrows the producer region stays pinned before it is owned-ified; every other arm
    /// owns its value outright. A `Type`-name token in a proper-type slot lowers via
    /// [`KType::from_type_identifier`], falling back to the [`Held::UnresolvedType`] carrier for a
    /// bare user name — no type handle ever denotes an unresolved name, so the surface
    /// [`TypeIdentifier`] rides through verbatim and scope-aware elaboration defers to
    /// [`Scope::resolve_type_identifier`](crate::machine::core::Scope::resolve_type_identifier).
    ///
    /// [`KFunction::bind_args`]: crate::machine::KFunction::bind_args
    pub fn resolve_for(
        &self,
        slot: &crate::machine::model::KType,
        scope: &'a crate::machine::core::Scope<'a>,
        types: &crate::machine::model::types::TypeRegistry,
    ) -> Held<'a> {
        use crate::machine::model::types::KType;
        if let ExpressionPart::Spliced { cell, .. } = self {
            return match scope.adopt_sealed(cell) {
                Carried::Type(kt) => Held::Type(kt),
                Carried::UnresolvedType(ti) => Held::UnresolvedType(ti.clone()),
                Carried::Object(obj) => Held::Object(obj.deep_clone()),
            };
        }
        if let (ExpressionPart::Type(t), KType::PROPER_TYPE | KType::ANY_TYPE) = (self, *slot) {
            return match KType::from_type_identifier(t, types) {
                Ok(kt) => Held::Type(kt),
                Err(_) => Held::UnresolvedType(t.clone()),
            };
        }
        if let (ExpressionPart::SigiledTypeExpr(inner), KType::SIGILED_TYPE_EXPR) = (self, *slot) {
            return Held::Object(KObject::KExpression((**inner).clone()));
        }
        if let (ExpressionPart::RecordType(inner), KType::RECORD_TYPE) = (self, *slot) {
            return Held::Object(KObject::KExpression((**inner).clone()));
        }
        // A `Unary`-mode operator run reduces to `[Keyword, ListLiteral]`; a `:KExpression` slot
        // captures the list literal raw as a one-per-part `KExpression`, so the receiving builtin
        // walks the operand parts itself rather than seeing an eager-evaluated list value.
        if let (ExpressionPart::ListLiteral(items), KType::KEXPRESSION) = (self, *slot) {
            let parts = items.iter().cloned().map(Spanned::bare).collect();
            return Held::Object(KObject::KExpression(KExpression::new(parts)));
        }
        Held::Object(self.resolve(types))
    }

    pub fn resolve(&self, types: &crate::machine::model::types::TypeRegistry) -> KObject<'a> {
        match self {
            ExpressionPart::Keyword(s) => KObject::KString(s.clone()),
            ExpressionPart::Identifier(s) => KObject::KString(s.clone()),
            ExpressionPart::Type(t) => KObject::KString(t.render()),
            ExpressionPart::Literal(KLiteral::Number(n)) => KObject::Number(*n),
            ExpressionPart::Literal(KLiteral::String(s)) => KObject::KString(s.clone()),
            ExpressionPart::Literal(KLiteral::Boolean(b)) => KObject::Bool(*b),
            ExpressionPart::Literal(KLiteral::Null) => KObject::Null,
            ExpressionPart::Expression(e) => KObject::KExpression((**e).clone()),
            // A quote denotes its body as data — the same `KObject` an `Expression` part in a
            // `:KExpression` slot denotes, reached from any slot a literal reaches.
            ExpressionPart::QuotedExpression(e) => KObject::KExpression((**e).clone()),
            // Reaches a value only through the dispatcher's type-context fast lane or sub-Dispatch,
            // both of which unwrap it; hitting `resolve()` means a builtin lost the marker.
            ExpressionPart::SigiledTypeExpr(_) => {
                unreachable!("SigiledTypeExpr only valid in type-context dispatch")
            }
            // Like SigiledTypeExpr: a record type reaches a value through the dispatcher's
            // `RecordType` fast lane or a raw `:RecordType`-slot capture, never `resolve()`.
            ExpressionPart::RecordType(_) => {
                unreachable!("RecordType only valid in type-context dispatch")
            }
            ExpressionPart::ListLiteral(items) => {
                KObject::list(items.iter().map(|p| p.resolve(types)).collect(), types)
            }
            // Non-scalar keys reaching here are a scheduler bug — it must surface them as
            // a structured `ShapeError` before resolve.
            ExpressionPart::DictLiteral(pairs) => {
                let mut map: HashMap<KKey, KObject<'a>> = HashMap::new();
                for (k, v) in pairs {
                    let key_obj = k.resolve(types);
                    let kkey = KKey::try_from_kobject(&key_obj, types).unwrap_or_else(|e| {
                        panic!("DictLiteral::resolve = non-scalar key reached resolve(): {e}")
                    });
                    map.insert(kkey, v.resolve(types));
                }
                KObject::dict(map, types)
            }
            // A record's substrate is born only through the fold door, which `resolve()` has no
            // brand to reach — and it never needs one: eager staging
            // (`eager_shape`/`stage_eager_part`, `dispatch.rs`) routes every `RecordLiteral` part
            // through the scheduled path (`schedule_record_literal`) before any resolve site
            // reaches it, replacing it with a `Spliced` cell first. `resolve_for`'s fallback and
            // the bare-literal fast lane (`exec.rs`) each only ever hand a `RecordLiteral` part to
            // `stage_eager_part`, never to `resolve()`.
            ExpressionPart::RecordLiteral(_) => {
                unreachable!(
                    "a RecordLiteral part is always staged (schedule_record_literal) before any \
                     resolve() site reaches it"
                )
            }
            // A spliced cell is opened / adopted at the consuming scope's brand before resolution, so
            // its value never reaches the region-less `resolve()`. The container arms above recurse
            // safely: a splice is only written at a top-level part slot, never into a literal's elements.
            ExpressionPart::Spliced { .. } => unreachable!(
                "a spliced cell is adopted at the binding scope before resolve(); \
                 resolve() runs only on region-pure parts"
            ),
            // A staged slot is a scheduler-internal hole: `install_eager_subs`'s finish splices
            // every marked slot into a `Spliced` cell before anything binds or resolves it.
            ExpressionPart::StagedSlot => unreachable!(
                "StagedSlot is a transient staging hole; install_eager_subs splices it before resolve() runs"
            ),
        }
    }

    /// The owned [`KObject`] a **region-pure** part denotes, at *any* lifetime — the lifetime-generic
    /// peer of [`resolve`](Self::resolve) for static-cell sites that `yoke`. The borrow-free variants
    /// (keyword, bare identifier, type name, literal) own their data, so the value is constructible at
    /// the caller's `yoke` brand, where [`resolve`]'s invariant `KObject<'a>` cannot go. Borrow-bearing
    /// variants are classified to owned sub-dispatches before any static cell, so they never reach here.
    pub fn resolve_region_pure<'b>(&self) -> KObject<'b> {
        match self {
            ExpressionPart::Keyword(s) | ExpressionPart::Identifier(s) => {
                KObject::KString(s.clone())
            }
            ExpressionPart::Type(t) => KObject::KString(t.render()),
            ExpressionPart::Literal(lit) => lit.to_kobject(),
            // A quote's `KObject::KExpression` is invariant in `'a` with no `'static` rebuild, so it
            // cannot be constructed at the caller's `yoke` brand — the classifier routes a quote to
            // its own sub-dispatch (which seals it through the checked door) before any static cell.
            ExpressionPart::Expression(_)
            | ExpressionPart::SigiledTypeExpr(_)
            | ExpressionPart::RecordType(_)
            | ExpressionPart::QuotedExpression(_)
            | ExpressionPart::ListLiteral(_)
            | ExpressionPart::DictLiteral(_)
            | ExpressionPart::RecordLiteral(_)
            | ExpressionPart::Spliced { .. } => unreachable!(
                "resolve_region_pure is only called on a region-pure static-cell part \
                 (keyword / bare identifier / type name / literal); borrow-bearing parts and \
                 spliced cells are classified to owned sub-dispatches before any static cell"
            ),
            // A staged slot never reaches a static-cell resolve: `install_eager_subs`'s finish
            // splices it into a `Spliced` cell before anything binds or resolves it.
            ExpressionPart::StagedSlot => unreachable!(
                "StagedSlot is a transient staging hole; install_eager_subs splices it before resolve_region_pure() runs"
            ),
        }
    }
}

impl<'a> Clone for ExpressionPart<'a> {
    fn clone(&self) -> Self {
        match self {
            ExpressionPart::Keyword(s) => ExpressionPart::Keyword(s.clone()),
            ExpressionPart::Identifier(s) => ExpressionPart::Identifier(s.clone()),
            ExpressionPart::Type(t) => ExpressionPart::Type(t.clone()),
            ExpressionPart::Expression(e) => ExpressionPart::Expression(e.clone()),
            ExpressionPart::SigiledTypeExpr(e) => ExpressionPart::SigiledTypeExpr(e.clone()),
            ExpressionPart::RecordType(e) => ExpressionPart::RecordType(e.clone()),
            ExpressionPart::QuotedExpression(e) => ExpressionPart::QuotedExpression(e.clone()),
            ExpressionPart::ListLiteral(items) => ExpressionPart::ListLiteral(items.clone()),
            ExpressionPart::DictLiteral(pairs) => ExpressionPart::DictLiteral(pairs.clone()),
            ExpressionPart::RecordLiteral(pairs) => ExpressionPart::RecordLiteral(pairs.clone()),
            ExpressionPart::Literal(l) => ExpressionPart::Literal(l.clone()),
            // `duplicate` copies the erased carrier value and clones the witness (the envelope is not
            // `Copy`); the retained frame owner is a plain `Rc` clone.
            ExpressionPart::Spliced { cell } => ExpressionPart::Spliced {
                cell: cell.duplicate(),
            },
            ExpressionPart::StagedSlot => ExpressionPart::StagedSlot,
        }
    }
}

impl<'a> Clone for KExpression<'a> {
    fn clone(&self) -> Self {
        KExpression {
            parts: self.parts.clone(),
            span: self.span,
            file: self.file,
            untyped_key: self.untyped_key.clone(),
            shape: self.shape,
            operator_probe: self.operator_probe.clone(),
            binder_plan: self.binder_plan.clone(),
            binder_installs: self.binder_installs.clone(),
            _marker: PhantomData,
        }
    }
}

/// The parse-time binder plan for a node that is *itself* a binder: the channel it installs
/// ([`BinderKey`]) and the chain-slot mask marking which of its slots carry nested binders
/// forward. Cached on [`KExpression`] beside [`DispatchShape`]; `None` for a non-binder node.
#[derive(Clone, Debug)]
pub struct BinderPlan {
    pub key: BinderKey,
    pub chain_slot_mask: &'static [bool],
}

/// Pure-structural classification of a `KExpression` into the no-keyword fast-lane
/// shapes, the chainable operator shape, and the keyword-bearing shape.
///
/// A function of expression structure only (no scope, no types), so it is computed
/// once when the parts vector is complete and cached on [`KExpression::shape`]. The
/// dispatch driver reads the cache rather than re-deriving per call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchShape {
    BareIdentifier,
    BareTypeLeaf,
    /// Bare-`Type`-head call: head is a leaf `Type` and `parts[1..]` is non-empty.
    /// Resolves the name synchronously and launches type construction via the shared
    /// apply-a-callable tail.
    TypeCall,
    /// Function-value call: head is a lowercase `Identifier`, followed by ≥1
    /// non-keyword parts.
    FunctionValueCall,
    /// Single-part `:(...)` sigiled type-expression wrapper.
    SigiledTypeExpr,
    /// Single-part `:{…}` record-type sigil. The handler folds the field list straight
    /// to `KType::Record` (deferring through a dep-finish when a field type sub-dispatches
    /// or forward-references), with no internal type-constructor builtin behind it.
    RecordType,
    /// Single-part literal-shaped expression — `Literal`, `Spliced`, nested
    /// `Expression`, `ListLiteral`, `DictLiteral`, or `RecordLiteral`. Surfaces the
    /// inner value without a bucket lookup.
    LiteralPassThrough,
    /// Chainable operator run: a slot-led key whose keywords alternate with slots,
    /// with two or more keyword positions (`Slot (Keyword Slot)+`, first keyword at
    /// index 1). A refinement of `Keyworded`: nothing else produces that shape, so it
    /// carves a track that the fold pre-pass folds into nested binary sub-dispatches.
    OperatorChain,
    /// Head-deferred call: head is a nested `Expression` followed by ≥1 non-keyword
    /// parts. The head is evaluated first; its resulting value (a function or a
    /// constructible type) is then applied to `parts[1..]` via the shared
    /// apply-a-callable tail.
    HeadDeferred,
    /// Type-position head-deferred call: head is a `:(...)` sigiled type expression
    /// followed by ≥1 non-keyword parts. Like `HeadDeferred`, but the resumed value
    /// is admitted only when it is a constructible type; a function or any other value
    /// surfaces a type-shaped `TypeMismatch`.
    TypeHeadDeferred,
    /// A keyword appears anywhere in `expr.parts` (and the chain shape did not match).
    Keyworded,
    /// Head is a non-callable surface — a literal, list, dict, or record — in a
    /// multi-part expression. Heads are always eager and must resolve to something
    /// callable; this shape surfaces a loud `DispatchFailed` from the dispatch entry.
    NonCallableHead,
}

/// Sweeps every part for `Keyword` first so a mixed shape like `(f IF x)` goes to
/// `Keyworded`; only with the no-keyword precondition established does it branch on
/// head shape. A keyword-bearing expression is refined to `OperatorChain` when it
/// matches the `Slot (Keyword Slot)+` shape with ≥2 keyword positions.
pub fn classify_dispatch_shape(expr: &KExpression<'_>) -> DispatchShape {
    if expr
        .parts
        .iter()
        .any(|p| matches!(&p.value, ExpressionPart::Keyword(_)))
    {
        if is_operator_chain_shape(&expr.parts) {
            return DispatchShape::OperatorChain;
        }
        return DispatchShape::Keyworded;
    }
    if let [only] = expr.parts.as_slice() {
        return match &only.value {
            ExpressionPart::Identifier(_) => DispatchShape::BareIdentifier,
            ExpressionPart::Type(_) => DispatchShape::BareTypeLeaf,
            ExpressionPart::SigiledTypeExpr(_) => DispatchShape::SigiledTypeExpr,
            ExpressionPart::RecordType(_) => DispatchShape::RecordType,
            ExpressionPart::Literal(_)
            | ExpressionPart::Spliced { .. }
            | ExpressionPart::Expression(_)
            | ExpressionPart::QuotedExpression(_)
            | ExpressionPart::ListLiteral(_)
            | ExpressionPart::DictLiteral(_)
            | ExpressionPart::RecordLiteral(_) => DispatchShape::LiteralPassThrough,
            // A single staged slot is reachable: `install_eager_subs_track` (the post-pick,
            // no-keyword named-argument tail) calls `KExpression::new` on the freshly staged
            // parts before any dep resolves, and a one-argument reconstructed call stages that
            // sole part when it's eager. The cached shape this fills is never re-derived after
            // the slot splices (`KExpression`'s cache is invariant under splice, per its doc),
            // and this working expression's shape is never re-consulted either way — the finish
            // routes straight to `invoke`/`redispatch`, not back through `classify_dispatch`. A
            // lone hole classifies as a bare identifier — the shape a resolvable single part takes.
            ExpressionPart::StagedSlot => DispatchShape::BareIdentifier,
            ExpressionPart::Keyword(_) => {
                unreachable!("no-keyword precondition: the sweep above caught every Keyword part")
            }
        };
    }
    // `len >= 2` here: the keyword sweep passed and the single-part block did not
    // match, so an empty `parts` falls through as the explicit `NonCallableHead`.
    let Some(head_part) = expr.parts.first() else {
        return DispatchShape::NonCallableHead;
    };
    match &head_part.value {
        ExpressionPart::Type(_) => DispatchShape::TypeCall,
        ExpressionPart::Identifier(_) => DispatchShape::FunctionValueCall,
        ExpressionPart::Expression(_) => DispatchShape::HeadDeferred,
        ExpressionPart::SigiledTypeExpr(_) => DispatchShape::TypeHeadDeferred,
        // A literal / list / dict / record-literal / record-type / quote / future head in a
        // multi-part expression: heads are always eager and must resolve to something
        // callable, so a non-callable head surfaces a loud `DispatchFailed`. A record
        // *type* and a quoted expression are values, not callables, so they join them here.
        ExpressionPart::Literal(_)
        | ExpressionPart::Spliced { .. }
        | ExpressionPart::ListLiteral(_)
        | ExpressionPart::DictLiteral(_)
        | ExpressionPart::RecordLiteral(_)
        | ExpressionPart::RecordType(_)
        | ExpressionPart::QuotedExpression(_) => DispatchShape::NonCallableHead,
        // A staged slot at the head position is reachable the same way as the single-part
        // case above: a reconstructed no-keyword named-argument call whose first part is
        // eager stages it before `KExpression::new` fills this cache, and this working
        // expression's shape is never re-consulted afterward. A hole head classifies as a
        // function-value call — the shape a resolvable identifier head takes.
        ExpressionPart::StagedSlot => DispatchShape::FunctionValueCall,
        ExpressionPart::Keyword(_) => {
            unreachable!("no-keyword precondition: the sweep above caught every Keyword part")
        }
    }
}

/// True iff `parts` is the `Slot (Keyword Slot)+` chainable-operator shape: odd
/// length ≥ 5 (slot, keyword, slot, …), every odd index a `Keyword`, every even
/// index a non-keyword slot, with ≥2 keyword positions. The first keyword sits at
/// index 1, so no keyword-led builtin (`LET …`) collides with it.
fn is_operator_chain_shape(parts: &[Spanned<ExpressionPart<'_>>]) -> bool {
    // Need slot, keyword, slot, keyword, slot — at least 5 parts (2 keywords).
    if parts.len() < 5 || parts.len().is_multiple_of(2) {
        return false;
    }
    parts.iter().enumerate().all(|(i, part)| {
        let is_keyword = matches!(&part.value, ExpressionPart::Keyword(_));
        // Odd indices must be keywords; even indices must be non-keyword slots.
        (i % 2 == 1) == is_keyword
    })
}

/// The unique operator keywords of an `OperatorChain`, sorted and joined into the
/// probe key the per-scope operator registry is looked up by. Returns `None` for any
/// other shape.
fn operator_probe_for(
    parts: &[Spanned<ExpressionPart<'_>>],
    shape: DispatchShape,
) -> Option<String> {
    if shape != DispatchShape::OperatorChain {
        return None;
    }
    let mut ops: Vec<&str> = parts
        .iter()
        .filter_map(|part| match &part.value {
            ExpressionPart::Keyword(s) => Some(s.as_str()),
            _ => None,
        })
        .collect();
    ops.sort_unstable();
    ops.dedup();
    Some(ops.join(" "))
}

/// A parsed Koan expression: an ordered sequence of `ExpressionPart`s.
///
/// `span` and `file` are `None` for hand-built ASTs.
///
/// `untyped_key`, `shape`, and `operator_probe` are a structural cache filled by
/// [`KExpression::fill_cache`] whenever the parts vector is complete. They are invariant under the
/// dispatch-time splice that replaces an eager `Slot` part with a `Spliced` (also a `Slot`), so the
/// dispatch driver reads the cache rather than re-deriving on every call of the enclosing function.
pub struct KExpression<'a> {
    pub parts: Vec<Spanned<ExpressionPart<'a>>>,
    pub span: Option<Span>,
    pub file: Option<FileId>,
    untyped_key: UntypedKey,
    shape: DispatchShape,
    operator_probe: Option<String>,
    /// This node's own binder plan: `Some` iff this node is itself a binder (see [`BinderPlan`]).
    /// Boxed because a binder is a rare node shape, so the common non-binder node pays one pointer
    /// rather than the whole plan inline.
    binder_plan: Option<Box<BinderPlan>>,
    /// Everything this node's subtree installs into the enclosing scope, per the position rule:
    /// this node's own key (when a binder) plus, transitively, the installs of its chain-slot
    /// children and of a redundant single-`Expression` paren wrapper. Read once, at statement
    /// submission, before any splice — a post-splice rebuild recomputes a smaller aggregate that is
    /// never consulted, mirroring the structural cache's invariance-under-splice note above.
    binder_installs: Vec<BinderKey>,
    /// `KExpression` owns every byte of its parts — keywords, identifiers, literals, boxed
    /// sub-expressions, and each splice's lifetime-free `Sealed` carrier cell — so no field
    /// concretely borrows `'a`. The parameter is retained as a phantom so the family stays a
    /// `KExpression<'r>` for the witnessed substrate (its `Reattachable` impl keys on `'r`), and
    /// is held **invariant** in `'a` — the variance of the lifetime-bearing families a `KExpression`
    /// composes (`KObject<'a>` and `KType` are both invariant) — so the phantom grants no laxer
    /// variance than the parts themselves. Zero-size, so the layout is identical across every
    /// `'a` and the [`reattachable!`] retype below stays a no-op transmute.
    _marker: PhantomData<fn(&'a ()) -> &'a ()>,
}

// `KExpression` is fully owned (owned data + lifetime-free `Sealed` splice cells), so the layout is
// identical for every `'a` and this lifetime-retype is a no-op transmute.
reattachable! { KExpression<'static> => KExpression<'r> }

impl<'a> KExpression<'a> {
    /// Spanless constructor; `span`/`file` populated by later phases. Fills the
    /// structural cache from `parts`.
    pub fn new(parts: Vec<Spanned<ExpressionPart<'a>>>) -> Self {
        Self::build(parts, None, None)
    }

    /// Construction chokepoint: takes the full parts vector plus its `span`/`file` and
    /// fills the structural cache. Every literal `KExpression { .. }` site routes here
    /// so no node ships with a stale or unfilled cache.
    pub fn build(
        parts: Vec<Spanned<ExpressionPart<'a>>>,
        span: Option<Span>,
        file: Option<FileId>,
    ) -> Self {
        let mut expr = KExpression {
            parts,
            span,
            file,
            untyped_key: Vec::new(),
            shape: DispatchShape::Keyworded,
            operator_probe: None,
            binder_plan: None,
            binder_installs: Vec::new(),
            _marker: PhantomData,
        };
        expr.fill_cache();
        expr
    }

    /// True when no part anywhere in the tree is a `Spliced` cell — the expression is raw, unevaluated
    /// AST (a quoted expression, an FN body). This is the precondition that lets an AST-embedding object
    /// alloc through the **region-pure** witnessed surface
    /// ([`alloc_object_witnessed`](crate::machine::core::RegionBrand::alloc_object_witnessed)): a
    /// splice-free expression names no producer reach, so the embedding object's reach is the empty set.
    pub fn is_splice_free(&self) -> bool {
        self.parts.iter().all(|p| p.value.is_splice_free())
    }

    /// Recompute the structural cache from the current `parts`. Called by every
    /// constructor and by the parse path once a frame's parts vector is finalized.
    pub fn fill_cache(&mut self) {
        self.untyped_key = self
            .parts
            .iter()
            .map(|part| match &part.value {
                ExpressionPart::Keyword(s) => UntypedElement::Keyword(s.clone()),
                _ => UntypedElement::Slot,
            })
            .collect();
        self.shape = classify_dispatch_shape(self);
        self.operator_probe = operator_probe_for(&self.parts, self.shape);
        self.binder_plan = crate::machine::model::binder::binder_plan_for(self, &self.untyped_key)
            .map(|(key, chain_slot_mask)| {
                Box::new(BinderPlan {
                    key,
                    chain_slot_mask,
                })
            });
        self.binder_installs = self.compute_binder_installs();
    }

    /// Aggregate the binder installs of this node's subtree. Children are always built before
    /// parents (parse and every constructor build bottom-up), so each child's cache is already
    /// filled and this is a plain read. Aggregation never crosses keyword/identifier/type/literal
    /// parts, quotes, sigils, list/dict/record literals, lazy (`:KExpression`) slots, or
    /// block-shaped children.
    fn compute_binder_installs(&self) -> Vec<BinderKey> {
        let mut installs: Vec<BinderKey> = Vec::new();
        if let Some(plan) = &self.binder_plan {
            installs.push(plan.key.clone());
            for (index, part) in self.parts.iter().enumerate() {
                if !plan.chain_slot_mask.get(index).copied().unwrap_or(false) {
                    continue;
                }
                if let ExpressionPart::Expression(child) = &part.value {
                    if !child.is_statement_block() {
                        installs.extend(child.binder_installs.iter().cloned());
                    }
                }
            }
        }
        // Redundant single-`Expression` paren wrapper (`((…))`) passes its child's aggregate
        // straight through. A binder is always keyword-led, so this never co-occurs with the
        // binder-plan branch above.
        if let [only] = self.parts.as_slice() {
            if let ExpressionPart::Expression(child) = &only.value {
                installs = child.binder_installs.clone();
            }
        }
        installs
    }

    /// This node's own binder plan — `Some` iff this node is itself a binder.
    pub fn binder_plan(&self) -> Option<&BinderPlan> {
        self.binder_plan.as_deref()
    }

    /// Everything this node's subtree installs into the enclosing scope (see the field docs).
    pub fn binder_installs(&self) -> &[BinderKey] {
        &self.binder_installs
    }

    /// True when this expression is a statement block: two or more parts, all of them
    /// `Expression`. The single definition the body splitters ([`split_body_statements`] /
    /// [`body_statement_refs`]) and the binder-install aggregation share, so the multi-statement
    /// cutoff is stated once.
    ///
    /// [`split_body_statements`]: crate::machine::split_body_statements
    /// [`body_statement_refs`]: crate::machine::body_statement_refs
    pub fn is_statement_block(&self) -> bool {
        self.parts.len() >= 2
            && self
                .parts
                .iter()
                .all(|part| matches!(part.value, ExpressionPart::Expression(_)))
    }

    /// Cached dispatch shape (see [`classify_dispatch_shape`]).
    pub fn shape(&self) -> DispatchShape {
        self.shape
    }

    /// Cached operator-registry probe key: `Some` only for an `OperatorChain`, holding
    /// the sorted-joined unique operator keywords.
    pub fn operator_probe(&self) -> Option<&str> {
        self.operator_probe.as_deref()
    }

    /// Bucket key: `Keyword` parts contribute `Keyword(s)`; every other variant contributes
    /// `Slot`. Must agree with `ExpressionSignature::untyped_key` for any signature that
    /// should match. Reads the structural cache filled at construction.
    pub fn untyped_key(&self) -> UntypedKey {
        self.untyped_key.clone()
    }

    /// Binder-name extractor for typed-binder builtins (`SIG <Name> = …`, `UNION <Name> = …`):
    /// if `parts[1]` is a single `Type(t)`, returns its bare name; `None` on shape
    /// mismatch. The builtin body surfaces the structured error.
    pub fn binder_name_from_type_part(&self) -> Option<String> {
        match &self.parts.get(1)?.value {
            ExpressionPart::Type(t) => Some(t.render()),
            _ => None,
        }
    }

    /// If every part is `Expression(_)`, return refs to the inner expressions; otherwise
    /// `None`. The returned `Vec` encodes the all-`Expression` shape — callers iterate
    /// `&KExpression` directly without re-matching the variant.
    pub fn borrow_inner_expressions(&self) -> Option<Vec<&KExpression<'a>>> {
        let mut out = Vec::with_capacity(self.parts.len());
        for p in &self.parts {
            match &p.value {
                ExpressionPart::Expression(b) => out.push(b.as_ref()),
                _ => return None,
            }
        }
        Some(out)
    }

    /// Consuming right-fold counterpart of [`Self::borrow_inner_expressions`]: returns
    /// `(preceding, last)` with both unwrapped from `ExpressionPart::Expression`. On any
    /// shape mismatch returns `self` back so the caller can pass through.
    // The `Err` arm hands the whole `KExpression` back for a pass-through, not a diagnostic, so its
    // size is the node's own — expected, like the other owned-`KExpression` returns in the tree.
    #[allow(clippy::result_large_err)]
    pub fn try_take_inner_expressions_split(
        self,
    ) -> Result<(Vec<KExpression<'a>>, KExpression<'a>), Self> {
        let mut iter = self.parts.into_iter();
        let Some(first) = iter.next() else {
            return Err(KExpression::new(Vec::new()));
        };
        let mut last: KExpression<'a> = match first.value {
            ExpressionPart::Expression(b) => *b,
            other => {
                let mut parts = vec![Spanned {
                    value: other,
                    span: first.span,
                }];
                parts.extend(iter);
                return Err(KExpression::new(parts));
            }
        };
        let mut preceding: Vec<KExpression<'a>> = Vec::new();
        for p in iter.by_ref() {
            match p.value {
                ExpressionPart::Expression(b) => {
                    preceding.push(std::mem::replace(&mut last, *b));
                }
                other => {
                    let mut parts: Vec<Spanned<ExpressionPart<'a>>> = preceding
                        .into_iter()
                        .map(|e| Spanned::bare(ExpressionPart::Expression(Box::new(e))))
                        .collect();
                    parts.push(Spanned::bare(ExpressionPart::Expression(Box::new(last))));
                    parts.push(Spanned {
                        value: other,
                        span: p.span,
                    });
                    parts.extend(iter);
                    return Err(KExpression::new(parts));
                }
            }
        }
        Ok((preceding, last))
    }
}

impl<'a> std::fmt::Debug for KExpression<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KExpression")
            .field("parts", &self.parts)
            .finish()
    }
}

impl<'a> Parseable for KExpression<'a> {
    fn ktype(&self) -> crate::machine::model::KType {
        crate::machine::model::KType::KEXPRESSION
    }
}

impl<'a> KExpression<'a> {
    /// Surface rendering of the whole expression — parts only, so no registry is needed.
    pub fn summarize(&self) -> String {
        self.parts
            .iter()
            .map(|p| p.value.summarize())
            .collect::<Vec<_>>()
            .join(" ")
    }
}
