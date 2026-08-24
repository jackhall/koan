//! Expression-signature machinery: the call shape a `KFunction` matches against — an ordered
//! mix of fixed `Keyword` tokens and typed `Argument` slots, plus a `return_type`.
//! `UntypedKey` groups overloads by shape; `Specificity` ranks candidates within a bucket.
//!
//! Not to be confused with the **module-signature** content (`SIG`-declared) at
//! [`crate::machine::model::types::sig_schema::SigSchema`].
//!
//! `return_type` is a [`ReturnType`] rather than a bare [`KType`] so return types that
//! reference a per-call parameter (`-> er`, `-> er.Carrier`) survive FN-definition without
//! sub-dispatching against the outer scope.

use crate::machine::core::RegionBrand;
use crate::machine::model::ast::{
    ExpressionPart, KExpression, KeywordToken, TypeIdentifier, WorkingPart,
};

use super::ktype::KType;
use super::registry::TypeRegistry;
use crate::machine::model::RunRegistries;
use crate::machine::model::labels::{BinderSymbol, KeywordSymbol, LabelInterner};

/// One position of a bucket key: a fixed token as its [`KeywordSymbol`], or an argument slot.
/// `Copy` and lifetime-free, so a key run is the same type whether it sits in a `Vec` a caller
/// hands around or in a slice bumped into a region — one type, one derived `Hash`, and equality is
/// a tag plus a `u128` compare with no text to walk.
#[derive(Eq, PartialEq, Clone, Copy, Hash, Debug)]
pub enum KeyElement {
    Keyword(KeywordSymbol),
    Slot,
}

/// Bucket key produced by both `ExpressionSignature::untyped_key` and
/// `KExpression::untyped_key`; they MUST agree for any pair that should match. The parser
/// classifies source tokens via `is_keyword_token` and mints each one's symbol there;
/// [`ExpressionSignature::mint`] uppercases lowercase registered tokens before minting, so a
/// registration and a call arrive at the same symbol for the same token.
pub type UntypedKey = Vec<KeyElement>;

/// The definition-time identity of a signature for bucket dedupe: element shape plus the
/// per-slot argument type. Two signatures are indistinguishable at dispatch iff their tokens
/// are equal — the pairing invariant [`ExpressionSignature::indistinguishable_from`] documents
/// (reject exactly what [`ExpressionSignature::specificity_vs`] can never split) holds for token
/// equality by construction, because the token is built from the same elements those predicates
/// read.
///
/// A token is owned data with no region lifetime, so a callable's dedupe identity is computed
/// once where the callable is open and travels as plain data to every write path.
#[derive(Clone, PartialEq, Debug)]
pub struct DispatchToken(Vec<DispatchTokenElement>);

/// One position of a [`DispatchToken`]: a fixed token as its [`KeywordSymbol`], or an argument
/// slot carrying its declared type. Both arms are `Copy` handles — a symbol is `u128` bits, a
/// `KType` is interned in the run registry — so an element compares meaningfully across regions
/// and a stored run is a bump of the owned one with nothing copied but the elements themselves.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum DispatchTokenElement {
    Keyword(KeywordSymbol),
    Slot(KType),
}

impl DispatchToken {
    /// Re-home this token into `brand`'s region as the run a bucket entry stores. The entry carries
    /// no drop glue, so dropping a table frees nothing.
    pub fn store_in<'a>(&self, brand: RegionBrand<'a>) -> &'a [DispatchTokenElement] {
        brand.allocator().slice(&self.0)
    }

    /// The elements this token stands for — the duplicate-overload predicate is a slice `==`
    /// against a stored run, deciding without allocating.
    pub fn elements(&self) -> &[DispatchTokenElement] {
        &self.0
    }
}

/// True iff `s` classifies as a keyword (fixed token). See
/// [tokens.md](../../../../design/typing/tokens.md): pure-symbol tokens (no ASCII letters)
/// are always keywords; alphabetic tokens are keywords iff they have ≥2 ASCII-uppercase
/// letters and no ASCII-lowercase letters.
pub fn is_keyword_token(s: &str) -> bool {
    let has_letter = s.chars().any(|c| c.is_ascii_alphabetic());
    if !has_letter {
        return true;
    }
    let upper_count = s.chars().filter(|c| c.is_ascii_uppercase()).count();
    let has_lower = s.chars().any(|c| c.is_ascii_lowercase());
    upper_count >= 2 && !has_lower
}

/// The one-slot case of [`ExpressionSignature::most_specific`], over the slot types alone: returns
/// `Some(i)` iff `candidates[i]` is strictly more specific than every peer. A tournament whose
/// candidates differ in exactly one type — MATCH's typed arms — needs no signature to run through,
/// since with a single slot [`ExpressionSignature::specificity_vs`] reduces to the pairwise
/// [`KType::is_more_specific_than`] probe this reads directly.
pub fn most_specific_ktype(candidates: &[KType], registries: &RunRegistries) -> Option<usize> {
    candidates
        .iter()
        .enumerate()
        .find(|(i, a)| {
            candidates
                .iter()
                .enumerate()
                .all(|(j, b)| *i == j || a.is_more_specific_than(*b, registries))
        })
        .map(|(i, _)| i)
}

/// The normalized spelling of a fixed token: uppercased iff it contains a lowercase ASCII letter.
/// [`ExpressionSignature::mint`] applies it on the way into the region and
/// [`SignatureDraft::untyped_key`] applies it to answer what bucket a draft *will* key, so the two
/// never disagree about a token's spelling.
fn normalized_keyword(token: &str) -> std::borrow::Cow<'_, str> {
    if token.chars().any(|c| c.is_ascii_lowercase()) {
        std::borrow::Cow::Owned(token.to_ascii_uppercase())
    } else {
        std::borrow::Cow::Borrowed(token)
    }
}

impl SignatureDraft<'_> {
    /// The bucket key this draft will key once minted — [`ExpressionSignature::untyped_key`] read off
    /// the pre-mint buffer. A draft keyword already carries the symbol of its **normalized**
    /// spelling ([`SignatureElement::keyword`]), so the key is read straight off the elements.
    pub fn untyped_key(&self) -> UntypedKey {
        self.elements
            .iter()
            .map(|el| match el {
                SignatureElement::Keyword(kw) => KeyElement::Keyword(kw.symbol()),
                SignatureElement::Argument(_) => KeyElement::Slot,
            })
            .collect()
    }
}

/// `Incomparable` means neither dominates — e.g. `<Number> <Any>` vs `<Any> <Number>` against
/// an input that matches both.
#[derive(Debug, Eq, PartialEq, Clone, Copy)]
pub enum Specificity {
    StrictlyMore,
    StrictlyLess,
    Equal,
    Incomparable,
}

/// A callable's call shape at rest: a bumped run of elements plus a `return_type`. Every field is
/// `Copy` and `Drop`-free — the keyword and parameter-name text is `&'a str` bumped into the
/// signature's own region — which is what lets a `KFunction` live in the region bump rather than a
/// lifetime-typed cell ([value-substrates.md § Untyped arenas](../../../../design/value-substrates.md#untyped-arenas-the-drop-free-end-state)).
///
/// `'a` names both the elements run and `return_type`'s `Deferred` arm, which captures a live
/// [`KExpression`] for per-call re-elaboration.
///
/// The fields are private because [`Self::mint`] is the only door: it is what normalizes the fixed
/// tokens and re-homes every name at the destination brand, so a signature that skipped either is
/// unconstructible. Build one through a [`SignatureDraft`].
#[derive(Clone, Copy)]
pub struct ExpressionSignature<'a> {
    return_type: ReturnType<'a>,
    elements: &'a [SignatureElement<'a>],
    /// The parameter schema: `(symbol, declared type)` in declaration order, bumped once at
    /// [`Self::mint`]. This is the argument currency's key half — a call carries only a values
    /// slice aligned to it, so no name-keyed container is built per call. Shared with the function
    /// type's parameter record, which is built from this same slice.
    params: &'a [(BinderSymbol, KType)],
    /// For each parameter, its index into [`Self::elements`] — and therefore into a committed
    /// call's `parts`, which `validate_call_args` pins 1:1 with the elements. What lets a call fill
    /// the values slice positionally.
    part_slots: &'a [u16],
}

/// The pre-mint form of an [`ExpressionSignature`]: the same elements in a growable buffer, before
/// [`ExpressionSignature::mint`] normalizes them and re-homes their text.
///
/// A draft's names may borrow from anywhere at `'a` — a `&'static` builtin literal, a program-storage
/// AST part, a string already bumped elsewhere — because the mint door re-bumps every one of them at
/// the destination. That is what keeps this a *buffer* of the stored element type rather than a
/// parallel owned representation.
pub struct SignatureDraft<'a> {
    pub return_type: ReturnType<'a>,
    pub elements: Vec<SignatureElement<'a>>,
}

/// Carrier for an FN's declared return type. The surface admits parameter-name references
/// in return-type position (`FN (LIFT er: Ordered) -> er = ...`); `Deferred` holds the
/// captured surface form for per-call re-elaboration against the per-call scope where the
/// parameter's type-language identity is registered. See
/// [functors.md](../../../../design/typing/functors.md).
///
/// `'a` threads only through the `Deferred` arm's captured [`KExpression`] — `Resolved`'s
/// `KType` is owned and carries no lifetime of its own.
#[derive(Clone, Copy)]
pub enum ReturnType<'a> {
    Resolved(KType),
    Deferred(DeferredReturn<'a>),
}

/// Surface form preserved for per-call re-elaboration. Two carriers mirror the two FN
/// return-type slot kinds:
///
/// - `Type` — parser-preserved structured form (`er`, `List<er>`). Re-elaborated per
///   call via `elaborate_type_identifier`. Owns its strings, so no region lifetime.
/// - `Expression` — captured `:(…)` / dotted return expression (`er.Carrier`,
///   `Set WITH {…}`). Re-runs as a sub-Dispatch under the per-call scope; the resulting
///   `Carried::Type`'s inner `KType` is the per-call return type.
#[derive(Clone, Copy)]
pub enum DeferredReturn<'a> {
    Type(TypeIdentifier<'a>),
    Expression(KExpression<'a>),
}

/// Hashable type-language shadow of a [`DeferredReturn`], stored inside
/// `KType::DeferredReturn`. Both carriers hold owned surface text: the `Type` carrier the bare
/// name (a [`TypeIdentifier`] borrows the storage that parsed it, and a `KType` outlives every
/// region), the `Expression` carrier the canonical `summarize()` render — NOT the live
/// `KExpression`, which impls neither `Eq` nor `Hash`. Identity is syntactic — `Type` by name,
/// `Expression` by canonical render — so a synthesized `KType::DeferredReturn` ret slot compares,
/// hashes, and ranks by surface form.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum DeferredReturnSurface {
    Type(String),
    Expression(String),
}

impl DeferredReturnSurface {
    pub fn from_deferred(d: &DeferredReturn<'_>, labels: &LabelInterner) -> Self {
        match d {
            DeferredReturn::Type(t) => Self::Type(t.render()),
            DeferredReturn::Expression(e) => Self::Expression(e.summarize(labels)),
        }
    }

    pub fn render(&self) -> String {
        match self {
            Self::Type(s) | Self::Expression(s) => s.clone(),
        }
    }
}

impl<'a> std::fmt::Debug for ReturnType<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReturnType::Resolved(kt) => f.debug_tuple("Resolved").field(kt).finish(),
            ReturnType::Deferred(d) => f.debug_tuple("Deferred").field(d).finish(),
        }
    }
}

impl<'a> std::fmt::Debug for DeferredReturn<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeferredReturn::Type(t) => f.debug_tuple("Type").field(t).finish(),
            DeferredReturn::Expression(e) => f.debug_tuple("Expression").field(e).finish(),
        }
    }
}

impl<'a> ReturnType<'a> {
    /// Surface name for diagnostics.
    pub fn name(&self, registries: &RunRegistries) -> String {
        match self {
            ReturnType::Resolved(kt) => kt.name(registries),
            ReturnType::Deferred(DeferredReturn::Type(t)) => t.render(),
            ReturnType::Deferred(DeferredReturn::Expression(e)) => e.summarize(&registries.labels),
        }
    }

    /// Lift-time return-type check. `Deferred` returns `true` — the real slot check
    /// runs in the per-call elaboration's dep-finish, where the resolved `KType`
    /// is available.
    pub fn matches_value(
        &self,
        obj: &crate::machine::model::values::KObject<'a>,
        registries: &RunRegistries,
    ) -> bool {
        match self {
            ReturnType::Resolved(kt) => kt.matches_value(obj, registries),
            ReturnType::Deferred(_) => true,
        }
    }

    pub fn is_resolved(&self) -> bool {
        matches!(self, ReturnType::Resolved(_))
    }
}

impl<'a> ExpressionSignature<'a> {
    /// **Build a signature into `brand`'s region** — the sole door, and the reason a signature's text
    /// always lives where the signature does.
    ///
    /// Two things happen here and nowhere else. Fixed tokens are **normalized**: a lowercase
    /// registered token is uppercased so the bucket key matches what dispatch computes from incoming
    /// expressions. TODO(monadic-effects): emit a warning instead of silently rewriting once effects
    /// exist — rejecting would lose the "drop in a builtin without thinking about caps" affordance.
    /// And every name is **re-homed** through [`RegionBrand::allocator`], including a `&'static`
    /// builtin literal, so "a signature's text lives in the signature's own region" holds with no
    /// exceptions and a draft is free to name text borrowed from anywhere at `'a`.
    pub fn mint(brand: RegionBrand<'a>, draft: SignatureDraft<'a>, labels: &LabelInterner) -> Self {
        let elements: &'a [SignatureElement<'a>] =
            brand
                .allocator()
                .slice_from_iter(draft.elements.into_iter().map(|element| match element {
                    // Registration is a declaration site: the normalized spelling is interned here
                    // so a diagnostic naming this shape's bucket resolves the keyword back out of a
                    // symbol-only key.
                    SignatureElement::Keyword(kw) => {
                        let text = brand.allocator().text(&normalized_keyword(kw.text()));
                        SignatureElement::Keyword(
                            KeywordToken::declared(text, labels).expect(
                                "a signature keyword classifies keyword-class once normalized",
                            ),
                        )
                    }
                    // An argument's name is already its symbol — fixed-width, region-free — so
                    // only the keyword text above needs re-homing.
                    SignatureElement::Argument(argument) => SignatureElement::Argument(argument),
                }));
        // The parameter schema, built once here: every later call reads it rather than re-keying
        // its own. Both slices ride the signature's own region, so they live exactly as long as it.
        let params = brand.allocator().slice_from_iter(
            elements
                .iter()
                .filter_map(|element| match element {
                    SignatureElement::Argument(argument) => Some((argument.name, argument.ktype)),
                    SignatureElement::Keyword(_) => None,
                })
                .collect::<Vec<_>>(),
        );
        let part_slots = brand.allocator().slice_from_iter(
            elements
                .iter()
                .enumerate()
                .filter_map(|(slot, element)| match element {
                    SignatureElement::Argument(_) => Some(slot as u16),
                    SignatureElement::Keyword(_) => None,
                })
                .collect::<Vec<_>>(),
        );
        ExpressionSignature {
            return_type: draft.return_type,
            elements,
            params,
            part_slots,
        }
    }

    /// The parameter schema — `(symbol, declared type)` in declaration order. The key half of the
    /// argument currency; a call pairs it with a values slice on the step scratch.
    pub fn params(&self) -> &'a [(BinderSymbol, KType)] {
        self.params
    }

    /// Each parameter's index into [`Self::elements`], parallel to [`Self::params`].
    pub fn part_slots(&self) -> &'a [u16] {
        self.part_slots
    }

    /// This signature's declared return contract.
    pub fn return_type(&self) -> ReturnType<'a> {
        self.return_type
    }

    /// The call shape, as the bumped run it rests in.
    pub fn elements(&self) -> &'a [SignatureElement<'a>] {
        self.elements
    }

    pub fn matches<'e>(&self, expr: &KExpression<'e>, types: &TypeRegistry) -> bool {
        if self.elements().len() != expr.parts.len() {
            return false;
        }
        self.elements()
            .iter()
            .zip(expr.parts)
            .all(|(el, part)| match (el, &part.value) {
                (SignatureElement::Keyword(s), ExpressionPart::Keyword(t)) => {
                    s.symbol() == t.symbol()
                }
                (SignatureElement::Keyword(_), _) => false,
                (SignatureElement::Argument(arg), part_value) => arg.matches(part_value, types),
            })
    }

    /// Slot types are erased — same shape with different types lives in the same bucket and
    /// competes on specificity at dispatch time.
    pub fn untyped_key(&self) -> UntypedKey {
        self.elements()
            .iter()
            .map(|el| match el {
                SignatureElement::Keyword(kw) => KeyElement::Keyword(kw.symbol()),
                SignatureElement::Argument(_) => KeyElement::Slot,
            })
            .collect()
    }

    /// The stored form of [`Self::indistinguishable_from`]: slot types are kept, so two
    /// signatures compare equal here exactly when that predicate holds. The write path keys its
    /// dedupe on this rather than on a live signature comparison, so a bucket write never has to
    /// re-anchor the callables already stored there.
    pub fn dispatch_token(&self) -> DispatchToken {
        DispatchToken(
            self.elements()
                .iter()
                .map(|el| match el {
                    SignatureElement::Keyword(kw) => DispatchTokenElement::Keyword(kw.symbol()),
                    SignatureElement::Argument(a) => DispatchTokenElement::Slot(a.ktype),
                })
                .collect(),
        )
    }

    /// Assumes `self` and `other` share an `UntypedKey` — only argument slots contribute,
    /// since fixed-token positions are equal by construction.
    pub fn specificity_vs(
        &self,
        other: &ExpressionSignature<'a>,
        registries: &RunRegistries,
    ) -> Specificity {
        let mut any_more = false;
        let mut any_less = false;
        for (a, b) in self.elements().iter().zip(other.elements().iter()) {
            if let (SignatureElement::Argument(aa), SignatureElement::Argument(bb)) = (a, b) {
                if aa.ktype.is_more_specific_than(bb.ktype, registries) {
                    any_more = true;
                } else if bb.ktype.is_more_specific_than(aa.ktype, registries) {
                    any_less = true;
                }
            }
        }
        match (any_more, any_less) {
            (true, false) => Specificity::StrictlyMore,
            (false, true) => Specificity::StrictlyLess,
            (false, false) => Specificity::Equal,
            (true, true) => Specificity::Incomparable,
        }
    }

    /// Pairwise specificity tournament across co-bucket signatures. Returns `Some(i)` iff
    /// `candidates[i]` is `StrictlyMore` than every peer — `Equal` against any peer means a
    /// same-arg-type duplicate, which must surface as ambiguity rather than silently win.
    pub fn most_specific(
        candidates: &[&ExpressionSignature<'a>],
        registries: &RunRegistries,
    ) -> Option<usize> {
        candidates
            .iter()
            .enumerate()
            .find(|(i, a)| {
                candidates.iter().enumerate().all(|(j, b)| {
                    *i == j || matches!(a.specificity_vs(b, registries), Specificity::StrictlyMore)
                })
            })
            .map(|(i, _)| i)
    }

    /// Definition-time duplicate gate: true iff `other` has the same element shape and a
    /// type-equal `Argument` slot at every position. Pairing invariant with the tournament
    /// above: this must reject exactly the signatures `specificity_vs` can never split —
    /// per-slot type equality makes every mutual `is_more_specific_than` probe false, so the
    /// two would tie as `Equal` on every call and poison the bucket with unresolvable
    /// ambiguity. Return types are deliberately excluded: dispatch never selects on them, so
    /// they distinguish nothing. Independent of `Argument::name`.
    /// `other` is free in its own lifetime: the comparison reads `elements` alone, which carries
    /// none, so a bucket's dormant overload opened at some other brand compares against a live one
    /// without the two regions having to be the same. [`DispatchToken`] equality is the stored
    /// form of this same predicate.
    pub fn indistinguishable_from(&self, other: &ExpressionSignature<'_>) -> bool {
        if self.elements().len() != other.elements().len() {
            return false;
        }
        self.elements()
            .iter()
            .zip(other.elements().iter())
            .all(|(x, y)| match (x, y) {
                (SignatureElement::Keyword(s), SignatureElement::Keyword(t)) => {
                    s.symbol() == t.symbol()
                }
                (SignatureElement::Argument(ax), SignatureElement::Argument(ay)) => {
                    ax.ktype == ay.ktype
                }
                _ => false,
            })
    }
}

/// One position of a call shape: a fixed token, or a parameter slot. A keyword's text is an `&'a
/// str` borrow — bumped into the signature's own region once [`ExpressionSignature::mint`] has run,
/// and free to borrow from anywhere at `'a` before that — beside the [`KeywordSymbol`] the shape's
/// bucket key and every keyword comparison read.
#[derive(Clone, Copy, Debug)]
pub enum SignatureElement<'a> {
    Keyword(KeywordToken<'a>),
    Argument(Argument),
}

impl<'a> SignatureElement<'a> {
    /// A draft keyword from bare text: the symbol is minted from the **normalized** spelling
    /// [`ExpressionSignature::mint`] will store, so a draft that spells a token lowercase keys the
    /// same bucket as the uppercased form the mint re-homes. The text rides through as written.
    ///
    /// Panics on text that cannot classify post-normalization — unreachable from source (a
    /// lowercase token parses as an Identifier, never a Keyword), and a builtin draft spelling one
    /// could never dispatch.
    pub fn keyword(text: &'a str) -> SignatureElement<'a> {
        SignatureElement::Keyword(
            KeywordToken::drafted(text, &normalized_keyword(text))
                .expect("a signature keyword classifies keyword-class once normalized"),
        )
    }
}

/// `name` keys the slot in the signature's parameter schema; `ktype` gates what `ExpressionPart`s
/// it accepts. The name is classified at signature build — where the parameter's source text is
/// still in hand — so a frame bind reads the binding class straight off the schema instead of
/// re-deriving it from text. Lifetime-free: a [`BinderSymbol`] is a fixed-width content digest, so
/// an argument borrows nothing and a signature's parameter names need no region of their own.
#[derive(Clone, Copy, Debug)]
pub struct Argument {
    pub name: BinderSymbol,
    pub ktype: KType,
}

impl Argument {
    pub fn matches<'e>(&self, part: &ExpressionPart<'e>, types: &TypeRegistry) -> bool {
        self.ktype.accepts_part(part, types)
    }

    /// The dispatch-path peer of [`Self::matches`], over the scheduler's own part form.
    pub fn matches_working_part<'e>(
        &self,
        part: &WorkingPart<'e>,
        registries: &RunRegistries,
    ) -> bool {
        self.ktype.accepts_working_part(part, registries)
    }
}

#[cfg(test)]
mod tests;
