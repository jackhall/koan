//! Structural classification: the vocabulary a parts run reports itself in, and the cache a node
//! fills from it.
//!
//! A node's parts contribute exactly two things to every structural question: the bucket key they
//! spell and the class of the head part. [`NodeCache`] holds the answers — the key, the dispatch
//! shape, the operator probe, the matched builtin form and the binder plan — computed once at
//! construction, so the dispatch driver reads the cache rather than re-deriving per call.
//!
//! [`KExpression`](super::KExpression) holds raw AST parts and
//! [`WorkingExpression`](crate::machine::model::ast::working::WorkingExpression) holds the
//! scheduler's per-call parts, but both answer these questions the same way, so both carry this
//! one cache.

use smallvec::SmallVec;

use crate::memory::BumpAllocator;
use crate::parse::forms::binder::StoredBinderKey;
use crate::parse::forms::lazy::LazyKinds;
use crate::parse::forms::{Form, form_for};
use crate::parse::labels::KeywordSymbol;

/// One position of a bucket key: a fixed token as its [`KeywordSymbol`], or an argument slot.
/// `Copy` and lifetime-free, so a key run is the same type whether it sits in a `Vec` a caller
/// hands around or in a slice bumped into a region — one type, one derived `Hash`, and equality is
/// a tag plus a `u128` compare with no text to walk.
/// `Ord` is the canonical order a schema's keyworded members sort by — a slot before any keyword,
/// keywords by their symbol's digest bits. Arbitrary as an order over call shapes, stable across
/// runs, and never an interner index, which is all a canonical sort needs.
#[derive(Eq, PartialEq, Clone, Copy, Hash, Debug, PartialOrd, Ord)]
pub enum KeyElement {
    Slot,
    Keyword(KeywordSymbol),
}

/// Bucket key produced by both `ExpressionSignature::untyped_key` and
/// [`KExpression::stored_key`](super::KExpression::stored_key); they MUST agree for any pair that
/// should match. The parser classifies source tokens via
/// [`is_keyword_token`](crate::parse::labels::is_keyword_token) and mints each one's symbol there;
/// `SignatureElement::keyword` uppercases a lowercase Rust-spelled token before minting, so a
/// registration and a call arrive at the same symbol for the same token.
pub type UntypedKey = Vec<KeyElement>;

/// The structural family a part belongs to — the axis shape classification, the bucket key and the
/// operator probe read. A keyword carries the symbol its parse minted, which is what all three
/// readers key by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PartClass {
    Keyword(KeywordSymbol),
    Identifier,
    Type,
    Expression,
    SigiledTypeExpr,
    RecordType,
    ListLiteral,
    DictLiteral,
    RecordLiteral,
    Literal,
    QuotedExpression,
    /// A resolved sub-result the scheduler wrote in. Only a working part reports this.
    Spliced,
    /// A staging hole awaiting its sibling's carrier. Only a working part reports this.
    StagedSlot,
}

/// Pure-structural classification of an expression into the no-keyword fast-lane shapes, the
/// chainable operator shape, and the keyword-bearing shape.
///
/// A function of expression structure only (no scope, no types), so it is computed once when the
/// parts run is complete and cached on the node. The dispatch driver reads the cache rather than
/// re-deriving per call.
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
    /// to a record `KType` (deferring through a dep-finish when a field type sub-dispatches
    /// or forward-references), with no internal type-constructor builtin behind it.
    RecordType,
    /// Single-part literal-shaped expression — `Literal`, `Spliced`, nested
    /// `Expression`, `ListLiteral`, `DictLiteral`, or `RecordLiteral`. Surfaces the
    /// inner value without a bucket lookup.
    LiteralPassThrough,
    /// Chainable operator run: a slot-led key whose keywords alternate with slots,
    /// with two or more keyword positions (`Slot (Keyword Slot)+`, first keyword at
    /// index 1). A refinement of `Keyworded` the classifier carves out as its own track,
    /// which the fold pre-pass folds into nested binary sub-dispatches.
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
    /// A keyword appears anywhere in the parts run (and the chain shape did not match).
    Keyworded,
    /// Head is a non-callable surface — a literal, list, dict, or record — in a
    /// multi-part expression. Heads are always eager and must resolve to something
    /// callable; this shape surfaces a loud `DispatchFailed` from the dispatch entry.
    NonCallableHead,
}

/// Sweeps the key for `Keyword` first so a mixed shape like `(f IF x)` goes to `Keyworded`; only
/// with the no-keyword precondition established does it branch on the head part's class. A
/// keyword-bearing key is refined to `OperatorChain` when it matches the `Slot (Keyword Slot)+`
/// shape with ≥2 keyword positions.
///
/// A function of the stored key and the head's class — the two facts a parts run contributes — so
/// the rule is written once and both expression families reach it with what they already carry.
pub fn classify_dispatch_shape(key: &[KeyElement], head: Option<PartClass>) -> DispatchShape {
    if key.iter().any(|e| matches!(e, KeyElement::Keyword(_))) {
        if is_operator_chain_shape(key) {
            return DispatchShape::OperatorChain;
        }
        return DispatchShape::Keyworded;
    }
    // `key` and the parts run are element-for-element, so a one-element key means a single part and
    // an empty one means an empty run — which falls through as the explicit `NonCallableHead`.
    let Some(head) = head else {
        return DispatchShape::NonCallableHead;
    };
    if key.len() == 1 {
        return match head {
            PartClass::Identifier => DispatchShape::BareIdentifier,
            PartClass::Type => DispatchShape::BareTypeLeaf,
            PartClass::SigiledTypeExpr => DispatchShape::SigiledTypeExpr,
            PartClass::RecordType => DispatchShape::RecordType,
            PartClass::Literal
            | PartClass::Spliced
            | PartClass::Expression
            | PartClass::QuotedExpression
            | PartClass::ListLiteral
            | PartClass::DictLiteral
            | PartClass::RecordLiteral => DispatchShape::LiteralPassThrough,
            // A lone hole classifies as a bare identifier — the shape a resolvable single part
            // takes.
            PartClass::StagedSlot => DispatchShape::BareIdentifier,
            PartClass::Keyword(_) => {
                unreachable!(
                    "no-keyword precondition: the sweep above caught every Keyword element"
                )
            }
        };
    }
    match head {
        PartClass::Type => DispatchShape::TypeCall,
        PartClass::Identifier => DispatchShape::FunctionValueCall,
        PartClass::Expression => DispatchShape::HeadDeferred,
        PartClass::SigiledTypeExpr => DispatchShape::TypeHeadDeferred,
        // A literal / list / dict / record-literal / record-type / quote / resolved head in a
        // multi-part expression: heads are always eager and must resolve to something callable, so
        // a non-callable head surfaces a loud `DispatchFailed`. A record *type* and a quoted
        // expression are values, not callables, so they join them here.
        PartClass::Literal
        | PartClass::Spliced
        | PartClass::ListLiteral
        | PartClass::DictLiteral
        | PartClass::RecordLiteral
        | PartClass::RecordType
        | PartClass::QuotedExpression => DispatchShape::NonCallableHead,
        // A staged slot at the head position is reachable the same way as the single-part case
        // above. A hole head classifies as a function-value call — the shape a resolvable
        // identifier head takes.
        PartClass::StagedSlot => DispatchShape::FunctionValueCall,
        PartClass::Keyword(_) => {
            unreachable!("no-keyword precondition: the sweep above caught every Keyword element")
        }
    }
}

/// True iff `key` is the `Slot (Keyword Slot)+` chainable-operator shape: odd length ≥ 5 (slot,
/// keyword, slot, …), every odd index a `Keyword`, every even index a `Slot`, with ≥2 keyword
/// positions. The first keyword sits at index 1, so no keyword-led builtin (`LET …`) collides
/// with it.
fn is_operator_chain_shape(key: &[KeyElement]) -> bool {
    // Need slot, keyword, slot, keyword, slot — at least 5 elements (2 keywords).
    if key.len() < 5 || key.len().is_multiple_of(2) {
        return false;
    }
    key.iter().enumerate().all(|(index, element)| {
        // Odd indices must be keywords; even indices must be slots.
        (index % 2 == 1) == matches!(element, KeyElement::Keyword(_))
    })
}

/// The probe key an `OperatorChain` looks the per-scope operator registry up by: the digest of the
/// run of its operator keywords, minted by [`KeywordSymbol::of_run`]. `None` for any other shape.
///
/// The group registration mints its powerset keys through the same constructor, so a registered key
/// and this probe agree by construction and neither side touches text. The node carries `u128`
/// bits, and a registry probe compares them.
pub fn operator_probe_for(key: &[KeyElement], shape: DispatchShape) -> Option<KeywordSymbol> {
    if shape != DispatchShape::OperatorChain {
        return None;
    }
    // Distinct operators, in a stack buffer. `of_run` reads its members as a set, so dropping a
    // repeat here mints the same digest — what it buys is the bound. A chain holds one entry per
    // operator it names, not one per term, so the buffer is sized by the member count of the group
    // the chain must resolve against rather than by the length of an arbitrarily long run.
    let mut operators: SmallVec<[KeywordSymbol; 8]> = SmallVec::new();
    for element in key {
        if let KeyElement::Keyword(symbol) = element
            && !operators.contains(symbol)
        {
            operators.push(*symbol);
        }
    }
    Some(KeywordSymbol::of_run(&operators))
}

/// The stored bucket key: a run of the key elements the parts spell, bumped once at construction,
/// so reading it is a slice borrow and nothing is hashed — the parse already minted every symbol in
/// the run.
pub fn stored_untyped_key<'a>(
    brand: BumpAllocator<'a>,
    elements: impl ExactSizeIterator<Item = KeyElement>,
) -> &'a [KeyElement] {
    brand.alloc_slice_fill_iter(elements)
}

/// The structural facts a node caches at construction: a function of its parts run and the form
/// table, computed once, shared by the AST node and the scheduler's working node.
///
/// Every field but the binder plan is settled the moment the key is: a splice substitutes slots one
/// for one and writes no keyword position, so the key, the probe and the form entry are invariant
/// under it. The plan is filled by the AST node's seal alone — a binder is always parsed AST — and
/// rides a working copy unchanged.
#[derive(Clone, Copy)]
pub struct NodeCache<'a> {
    key: &'a [KeyElement],
    shape: DispatchShape,
    operator_probe: Option<KeywordSymbol>,
    form: Option<&'static Form>,
    /// The form this node declares under — `Some` only once the AST seal has run, so the binder
    /// facts of a coincidental key match on a synthesized run are never read. See [`declaring`].
    ///
    /// [`declaring`]: Self::declaring
    declared: Option<&'static Form>,
    binder_plan: Option<&'a StoredBinderKey<'a>>,
}

impl<'a> NodeCache<'a> {
    /// The cache of a run whose key is `key` and whose head part reports `head` — every field but
    /// the binder plan, which the AST node's seal adds.
    pub fn build(key: &'a [KeyElement], head: Option<PartClass>) -> Self {
        let shape = classify_dispatch_shape(key, head);
        NodeCache {
            key,
            shape,
            operator_probe: operator_probe_for(key, shape),
            form: form_for(key.iter().copied()),
            declared: None,
            binder_plan: None,
        }
    }

    /// This cache read as a declaration — the second half of the AST node's seal, once the node the
    /// extractors read is standing: the matched form's binder facts and the plan they produced.
    ///
    /// Only a parsed node passes through here, which is what keeps a *synthesized* run from
    /// declaring anything. A synthesis writes its own keyword spine — a unary chain reduction emits
    /// `<operator> <operands>`, the shape `TYPE _` and `NEWTYPE _` also spell — so its key can match
    /// a binder form by coincidence. Such a node is not that declaration, so it reports no declared
    /// name and installs nothing, while still reading the form for its lazy slots, which are a fact
    /// about the key alone.
    pub fn declaring(self, binder_plan: Option<&'a StoredBinderKey<'a>>) -> Self {
        NodeCache {
            declared: self.form,
            binder_plan,
            ..self
        }
    }

    /// This cache re-classified for a new head class, the key and every table fact riding through —
    /// the splice path, where a resolved cell replaces the part that reported the old class.
    pub fn resplice(self, head: Option<PartClass>) -> Self {
        NodeCache {
            shape: classify_dispatch_shape(self.key, head),
            ..self
        }
    }

    /// The stored bucket key, as a borrow of the run bumped at construction.
    pub fn stored_key(&self) -> &'a [KeyElement] {
        self.key
    }

    /// Cached dispatch shape (see [`classify_dispatch_shape`]).
    pub fn shape(&self) -> DispatchShape {
        self.shape
    }

    /// Cached operator-registry probe key: `Some` only for an `OperatorChain`, holding the symbol
    /// of its distinct operator keywords.
    pub fn operator_probe(&self) -> Option<KeywordSymbol> {
        self.operator_probe
    }

    /// The [`FORMS`](crate::parse::forms::FORMS) entry this node's bucket key matches,
    /// `None` for every user-defined bucket.
    pub fn form(&self) -> Option<&'static Form> {
        self.form
    }

    /// This node's own binder plan — `Some` iff this node is itself a binder.
    pub fn binder_plan(&self) -> Option<StoredBinderKey<'a>> {
        self.binder_plan.copied()
    }

    /// The plan as the bumped borrow it is stored as, for the working copy that carries it through
    /// a splice unchanged.
    pub fn binder_plan_ref(&self) -> Option<&'a StoredBinderKey<'a>> {
        self.binder_plan
    }

    /// The declared-name position of the binder form this node's bucket key matches
    /// ([`BinderFacts::name_slot`](crate::parse::forms::binder::BinderFacts::name_slot)); `None`
    /// when the node matches no form, the form installs no binder, or its spine carries no declared
    /// name (`FN`, `OP`).
    pub fn binder_name_slot(&self) -> Option<usize> {
        self.declared?.binder?.name_slot
    }

    /// The kinds of part that stay raw at slot `index`, empty when the slot evaluates.
    pub fn lazy_kinds_at(&self, index: usize) -> LazyKinds {
        self.form
            .map_or(LazyKinds::EMPTY, |form| form.lazy_kinds_at(index))
    }
}
