//! Label identity: [`Symbol`], the fixed-width handle every syntactic label travels as, and
//! [`LabelInterner`], the run-scoped side table that turns one back into text.
//!
//! A label — a record field name, a struct schema field, an FN parameter name — originates in
//! source text and is fixed at declaration, so its identity is a content digest: the low 128 bits
//! of BLAKE3 over its UTF-8 bytes, the same width and collision footing as a
//! [`TypeDigest`](crate::machine::model::types::TypeDigest). [`Symbol::of`] is a pure function:
//! making a symbol needs no interner, no registry, no execution context, and equal text yields
//! equal symbols in every run.
//!
//! The interner is therefore *not* a lookup authority. Comparisons and probes go straight through
//! symbol bits; the table is written only where a syntactic label is constructed and read only
//! where one is rendered. Its growth is bounded by the run's source text.
//!
//! See [design/label-interning.md](../../../design/label-interning.md).

use std::borrow::Borrow;
use std::cell::RefCell;
use std::collections::HashMap;

use super::types::registry::IdentityBuildHasher;

/// A label's content identity: the low 128 bits of a BLAKE3 hash of its UTF-8 bytes.
///
/// `Copy`, lifetime-free, and compared and hashed without touching text. `Ord` is the numeric
/// order of those bits — the canonical field order for digest feeds and record cell layout.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Default)]
pub struct Symbol(pub u128);

impl Symbol {
    /// The label's digest. Pure — no interner, no allocation, no ambient state.
    pub fn of(text: &str) -> Symbol {
        Symbol::of_hash(blake3::hash(text.as_bytes()))
    }

    /// The low 128 bits of a finished BLAKE3 hash — the single funnel [`of`](Self::of) and
    /// [`KeywordSymbol::of_run`] end in, and so the one site the mint tally counts.
    fn of_hash(hash: blake3::Hash) -> Symbol {
        #[cfg(feature = "alloc-count")]
        MINTED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let low: [u8; 16] = hash.as_bytes()[..16]
            .try_into()
            .expect("BLAKE3 output is 32 bytes");
        Symbol(u128::from_le_bytes(low))
    }
}

/// How many symbols the process has minted, behind the `alloc-count` audit feature. Hashing takes
/// no allocation, so the allocation counter cannot see a mint go away and this is the instrument
/// that can. A process-wide tally, read once by `main` after the run — an audit counter of the
/// same standing as `audit/counting_alloc.rs`'s, compiled out of every build that does not ask
/// for it.
#[cfg(feature = "alloc-count")]
static MINTED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The process's symbol-mint total — every symbol minted since startup.
#[cfg(feature = "alloc-count")]
pub fn symbols_minted() -> u64 {
    MINTED.load(std::sync::atomic::Ordering::Relaxed)
}

/// What a render path prints for a symbol whose text this run never recorded. Rendering is total,
/// so a miss is this placeholder rather than a panic.
const MISSING_LABEL: &str = "<label>";

/// The run's digest → text side table for labels.
///
/// Interior mutability by `RefCell`, matching the type registry beside it: construction sites hold
/// a shared `&RunRegistries` and still need to record text. Never borrowed across a call that can
/// re-enter — [`intern`](Self::intern) and [`resolve`](Self::resolve) each take and release the
/// borrow within one statement.
#[derive(Default)]
pub struct LabelInterner {
    texts: RefCell<HashMap<Symbol, Box<str>, IdentityBuildHasher>>,
}

impl LabelInterner {
    pub fn new() -> Self {
        LabelInterner::default()
    }

    /// Record `text` under its symbol and hand the symbol back. Insert-if-absent: equal text
    /// already recorded costs one lookup.
    pub fn intern(&self, text: &str) -> Symbol {
        let symbol = Symbol::of(text);
        self.record_text(symbol, text);
        symbol
    }

    /// Record `text` under `symbol`, the digest the caller already holds. The one write door the
    /// three public ones funnel through: a caller that classified the text has minted its digest
    /// already, so the recording costs a map lookup and no second hash.
    fn record_text(&self, symbol: Symbol, text: &str) {
        let mut texts = self.texts.borrow_mut();
        texts.entry(symbol).or_insert_with(|| text.into());
    }

    /// The text recorded for `symbol`, or `None` if nothing interned it in this run. Render paths
    /// only — a miss is a rendering placeholder, never an error.
    pub fn resolve(&self, symbol: Symbol) -> Option<String> {
        self.texts
            .borrow()
            .get(&symbol)
            .map(|text| text.to_string())
    }

    /// The text for `symbol`, or the standard placeholder when this run never interned it — the
    /// total form of [`resolve`](Self::resolve) every render path uses.
    pub fn render(&self, symbol: Symbol) -> String {
        self.resolve(symbol)
            .unwrap_or_else(|| MISSING_LABEL.to_string())
    }

    /// [`render`](Self::render) as a `Display` view rather than a `String`: the recorded text goes
    /// straight into the caller's formatter. A message that names a label costs the message's own
    /// buffer and nothing else, so a diagnostic built on the path that succeeds is as cheap as one
    /// built from a borrowed name.
    pub fn display(&self, symbol: Symbol) -> LabelDisplay<'_> {
        LabelDisplay {
            labels: self,
            symbol,
        }
    }

    /// Order two symbols by their recorded text, without rendering either into a `String`.
    ///
    /// The sorted rendering arms — a record's fields, a signature's schema — order by the name a
    /// reader sees, not by digest bits, so they need text order. This compares the recorded slices
    /// in place under one borrow; a symbol this run never recorded compares as the same
    /// placeholder [`display`](Self::display) writes for it, so an unrecorded name keeps a fixed
    /// position rather than a hash-dependent one.
    pub fn compare_texts(&self, a: Symbol, b: Symbol) -> std::cmp::Ordering {
        let texts = self.texts.borrow();
        let left = texts.get(&a).map_or(MISSING_LABEL, |text| text);
        let right = texts.get(&b).map_or(MISSING_LABEL, |text| text);
        left.cmp(right)
    }

    /// How many distinct labels this run has recorded.
    pub fn len(&self) -> usize {
        self.texts.borrow().len()
    }

    /// Record a [`StaticName`]'s spelling under its memoized symbol and hand the classified symbol
    /// back — the declaration door for a name fixed in Rust source. The symbol is read off the
    /// memo, so registering the same builtin parameter in a second run costs one map lookup and no
    /// hash.
    pub fn record<S: ClassifiedSymbol>(&self, name: &StaticName<S>) -> S {
        let classified = name.symbol();
        self.record_text(classified.symbol(), name.text());
        classified
    }

    pub fn is_empty(&self) -> bool {
        self.texts.borrow().is_empty()
    }
}

/// A [`LabelInterner::display`] view: one symbol plus the interner that may hold its text.
///
/// Holds the interner borrow only for the length of the write, so a `Display` chain that names
/// several labels never nests the `RefCell` borrow.
pub struct LabelDisplay<'a> {
    labels: &'a LabelInterner,
    symbol: Symbol,
}

impl std::fmt::Display for LabelDisplay<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.labels.texts.borrow().get(&self.symbol) {
            Some(text) => formatter.write_str(text),
            None => formatter.write_str(MISSING_LABEL),
        }
    }
}

/// The lexical Type-token classifier: first char ASCII-uppercase plus at least one
/// ASCII-lowercase elsewhere (`IntOrd`, `Ordered`, `Carrier`). The single canonical
/// predicate for "this name classifies as a Type token" — the parser uses it to tag a
/// `Type` part, the type-language partition (abstract-type members vs value slots in a SIG
/// type table) reuses it, and [`TypeSymbol`] mints against it. See
/// [design/typing/tokens.md](../../../design/typing/tokens.md).
pub fn is_type_name(tok: &str) -> bool {
    let mut chars = tok.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_uppercase() {
        return false;
    }
    chars.any(|c| c.is_ascii_lowercase())
}

/// Suggest a value-classified rewrite of a Type-classified binder name: `IntOrd` → `int_ord`. Each
/// interior uppercase letter opens a new word (see
/// [design/typing/tokens.md](../../../design/typing/tokens.md)). Beside [`is_type_name`] because it
/// is that classifier read backwards — the respelling every diagnostic offers when a value binds
/// under a Type token.
pub fn snake_case_identifier(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 2);
    for (i, ch) in name.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

/// The universal hole token `_`: a pure-symbol token, so [`is_keyword_token`] classifies it
/// keyword-class and it needs no lexer arm of its own. Declared once for the whole run because two
/// unrelated surfaces read the same token — `MATCH` / `TRY`'s catch-all arm tag, and a `CLOSE OVER`
/// capture pattern's slot position (`(HELPER _)`) — and both recognize it by comparing against this
/// memoized symbol rather than by re-classifying the spelling.
pub static WILDCARD: StaticName<KeywordSymbol> = crate::static_name!(KeywordSymbol, "_");

/// Every classified label newtype below wraps exactly one [`Symbol`] behind a private field and is
/// minted only through the hidden `classify` funnel or [`declared`](ValueSymbol::declared), which
/// run the class predicate on the text. There is no raw-`Symbol` constructor: a `Symbol` alone carries no
/// evidence of what its text looked like, so admitting one would let a caller assert a class the
/// digest cannot witness. A seam holding a bare `Symbol` that needs a class classifies where the
/// text still existed, resolves the text through the run's [`LabelInterner`] and classifies that,
/// or recovers the class from a table already keyed by a classified symbol — the recovery door
/// the `Borrow<Symbol>` impl below opens.
///
/// The classes partition by construction: [`is_keyword_token`] and [`is_type_name`] are disjoint
/// (a Type token has a lowercase letter, a keyword-class alphabetic token has none), and
/// [`ValueSymbol`] is the complement of both. So a [`ValueSymbol`] and a [`TypeSymbol`] can never
/// wrap the same text — which is what makes a value/type binding collision unrepresentable rather
/// than something a write door has to probe for.
///
/// [`is_keyword_token`]: crate::machine::model::is_keyword_token
macro_rules! classified_symbol {
    ($(#[$meta:meta])* $name:ident, $classifies:expr, $class:literal) => {
        $(#[$meta])*
        ///
        /// `Copy` and hashed as the single `u128` its symbol is — every table keyed by it runs the
        /// identity hasher rather than re-mixing digest bits.
        #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
        pub struct $name(Symbol);

        impl $name {
            #[doc = concat!("The symbol for `text` if it classifies as ", $class, ", else `None`.")]
            ///
            /// Pure classification — no interning. The hidden funnel
            /// [`declared`](Self::declared) and [`static_name!`](crate::static_name) share. Only
            /// [`KeywordSymbol`] re-exports it as a surface probe (`of`): the operator and dispatch
            /// tables key by fixed tokens read back out of source. A *name* is minted at the parse
            /// that classifies it, on both sides of the partition, and every later reader carries
            /// that symbol rather than re-classifying a spelling.
            #[doc(hidden)]
            pub fn classify(text: &str) -> Option<Self> {
                let classifies: fn(&str) -> bool = $classifies;
                classifies(text).then(|| $name(Symbol::of(text)))
            }

            #[doc = concat!("Classify `text` as ", $class, " **and** record it for rendering.")]
            ///
            /// The **declaration** constructor: a name that enters a binding table is interned so a
            /// later diagnostic naming it can resolve the text back.
            pub fn declared(text: &str, labels: &LabelInterner) -> Option<Self> {
                let classified = $name::classify(text)?;
                labels.record_text(classified.symbol(), text);
                Some(classified)
            }

            /// The raw digest — for digest feeds, schema scans and
            /// [`render_label`](crate::machine::model::render_label).
            pub fn symbol(self) -> Symbol {
                self.0
            }
        }

        impl sealed::Sealed for $name {}

        impl ClassifiedSymbol for $name {
            fn symbol(self) -> Symbol {
                self.0
            }
        }
    };
}

classified_symbol!(
    /// A **value** binding name: a token that is neither keyword-class nor Type-class (`xs`,
    /// `int_ord`, `it`). The key type of every value-side binding table.
    ValueSymbol,
    |text| !crate::machine::model::is_keyword_token(text) && !is_type_name(text),
    "a value token"
);

classified_symbol!(
    /// A **type** binding name: a Type token per [`is_type_name`] (`IntOrd`, `Carrier`). The key
    /// type of every type-side binding table.
    TypeSymbol,
    is_type_name,
    "a Type token"
);

classified_symbol!(
    /// A **keyword-class** token: fixed syntax, per
    /// [`is_keyword_token`](crate::machine::model::is_keyword_token) — `FN`, `+`, `<=`, and the
    /// run digests built out of them by [`of_run`](KeywordSymbol::of_run), which stand for the
    /// operator sets the chain lane probes by. Nothing *binds* to one; the class exists because the
    /// operator table and the dispatch lane key by fixed tokens.
    KeywordSymbol,
    crate::machine::model::is_keyword_token,
    "a keyword-class token"
);

impl KeywordSymbol {
    /// The probe constructor: classify `text` without interning it. The operator and dispatch
    /// tables key by fixed tokens read back out of source, so the keyword side admits a bare probe.
    pub fn of(text: &str) -> Option<Self> {
        KeywordSymbol::classify(text)
    }

    /// The probe key a *run* of keyword symbols stands for: the members sorted by symbol bits and
    /// deduped, their 16-byte little-endian digests streamed through one hasher. The fragments are
    /// fixed-width, so no separator is needed to keep the feed unambiguous. An operator chain and
    /// the group registration whose powerset keys it must hit
    /// (`crate::machine::core::bindings::ops::powerset_probes`) both mint here, so a registered key
    /// and a live probe agree by construction and no probe path touches text.
    ///
    /// Keyword-class inputs witness the class of the product: a run of keyword-class tokens names
    /// fixed syntax and binds to nothing, exactly what the class stands for.
    pub fn of_run(members: &[KeywordSymbol]) -> Self {
        let sorted = sorted_run(members);
        let mut hasher = blake3::Hasher::new();
        for member in &sorted {
            hasher.update(&member.symbol().0.to_le_bytes());
        }
        KeywordSymbol(Symbol::of_hash(hasher.finalize()))
    }

    /// [`of_run`](Self::of_run) plus a recorded rendering: the members' interned spellings joined
    /// by single spaces in the same sorted, deduped order, recorded under the digest so a
    /// diagnostic naming the probe key renders the run it stands for. Registration-time only — a
    /// live probe mints through [`of_run`](Self::of_run) and renders nothing.
    pub fn declared_run(members: &[KeywordSymbol], labels: &LabelInterner) -> Self {
        let sorted = sorted_run(members);
        let run = KeywordSymbol::of_run(&sorted);
        let mut rendering = String::new();
        for (index, member) in sorted.iter().enumerate() {
            if index > 0 {
                rendering.push(' ');
            }
            // Written straight into the one buffer — `display` borrows the recorded text rather
            // than copying it out, so a run renders in a single allocation.
            let _ = std::fmt::Write::write_fmt(
                &mut rendering,
                format_args!("{}", labels.display(member.symbol())),
            );
        }
        labels.record_text(run.symbol(), &rendering);
        run
    }
}

/// A run of keyword symbols as the set it denotes: sorted by symbol bits and deduped, in a stack
/// buffer.
///
/// Both feeds hand over distinct members, so the buffer is sized by an operator group's member
/// count — and that count is bounded by the group's own powerset install, which writes `2^n`
/// registry entries. A run that spills to the heap is one no declaration would write. The bound
/// matters because the chain probe this feeds mints once per node at parse, where a per-node heap
/// allocation would show up in the recorded baselines.
fn sorted_run(members: &[KeywordSymbol]) -> smallvec::SmallVec<[KeywordSymbol; 8]> {
    let mut sorted: smallvec::SmallVec<[KeywordSymbol; 8]> =
        smallvec::SmallVec::from_slice(members);
    sorted.sort_unstable();
    sorted.dedup();
    sorted
}

/// The **recovery door**: a table keyed by [`TypeSymbol`] admits a probe by bare symbol bits, and a
/// hit hands back the *stored* key — a classified symbol minted where its text existed. Symbol
/// equality is text equality on the shared collision footing, so the probe's originating text is
/// the key's text and the recovered class is witnessed rather than asserted. Nothing is minted on
/// this path and insertion still requires a classified key, so a wrong-class probe misses against a
/// table that could never have held it.
///
/// The `Borrow` contract holds because the derived `Hash` impl hashes the single [`Symbol`] the
/// newtype wraps, byte for byte as `Symbol`'s own does, and equality is that symbol's equality.
///
/// Only `TypeSymbol` carries this: `WITH`'s pin walk and the union-variant probes are the sites
/// where a bare record-field symbol meets a Type-class member table, and nothing probes the other
/// classes by bits. See [design/label-interning.md](../../../design/label-interning.md).
impl Borrow<Symbol> for TypeSymbol {
    fn borrow(&self) -> &Symbol {
        &self.0
    }
}

/// A **bindable** name: the two classes a declaration can actually install under. Keywords are
/// fixed syntax and bind to nothing, so they are not a variant — [`declared`](Self::declared) of
/// keyword text is `None`.
///
/// This is the currency of a seam that accepts either class and routes on the answer: an FN
/// parameter name, a placeholder install, a member probe. The variant *is* the
/// [`BindKind`](crate::machine::model::BindKind), so a site carrying one threads no separate kind
/// tag beside the name.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub enum BinderSymbol {
    Value(ValueSymbol),
    Type(TypeSymbol),
}

impl BinderSymbol {
    /// The hidden classification funnel, mirroring the per-class [`classify`](ValueSymbol::classify)
    /// the macro writes: the two bindable classes are disjoint, so probing Type-side first and
    /// value-side second decides the variant, and keyword-class text answers `None` on both.
    #[doc(hidden)]
    pub fn classify(text: &str) -> Option<Self> {
        match TypeSymbol::classify(text) {
            Some(name) => Some(BinderSymbol::Type(name)),
            None => ValueSymbol::classify(text).map(BinderSymbol::Value),
        }
    }

    /// Classify `text` as bindable **and** record it for rendering — the declaration constructor,
    /// and the only surface way to mint a `BinderSymbol` from text.
    pub fn declared(text: &str, labels: &LabelInterner) -> Option<Self> {
        let classified = BinderSymbol::classify(text)?;
        labels.intern(text);
        Some(classified)
    }

    /// The raw digest.
    pub fn symbol(self) -> Symbol {
        match self {
            BinderSymbol::Value(name) => name.symbol(),
            BinderSymbol::Type(name) => name.symbol(),
        }
    }

    /// Which side of the value/type partition this name binds on.
    pub fn bind_kind(self) -> super::BindKind {
        match self {
            BinderSymbol::Value(_) => super::BindKind::Value,
            BinderSymbol::Type(_) => super::BindKind::Type,
        }
    }
}

/// The diagnostic a declaration raises when its binder name will not classify into the channel it
/// binds into: `wanted` is that channel, `name` the text as written. This is the token-class
/// partition stated **at the text→symbol seam** — past it the classified key types make a crossing
/// unrepresentable, so this is the one place the rule is a runtime disposition rather than a type.
/// See [design/typing/tokens.md](../../../design/typing/tokens.md).
pub fn wrong_binder_class(name: &str, wanted: super::BindKind) -> String {
    match wanted {
        super::BindKind::Type => format!(
            "`{name}` is a value token, so it names a value — a type binds under a Type token \
             (uppercase-leading with at least one lowercase letter)"
        ),
        super::BindKind::Value => format!(
            "`{name}` is a Type token, so it names a type — a value binds under a value token \
             (snake_case)"
        ),
    }
}

/// The classified-symbol types as one bound: what [`StaticName`] is parameterized over and what
/// [`LabelInterner::record`] accepts. Sealed — the three class newtypes plus [`BinderSymbol`] are
/// the vocabulary entire, and a further implementor would be a class the token grammar does not
/// have.
pub trait ClassifiedSymbol: Copy + sealed::Sealed {
    /// The raw digest, so a generic seam can compare and intern without knowing the class.
    fn symbol(self) -> Symbol;
}

mod sealed {
    pub trait Sealed {}
}

impl sealed::Sealed for BinderSymbol {}

impl ClassifiedSymbol for BinderSymbol {
    fn symbol(self) -> Symbol {
        BinderSymbol::symbol(self)
    }
}

/// A label whose spelling is fixed in Rust source — a builtin's parameter name, a tag a builtin
/// raises under — declared once and minted once.
///
/// The text is `&'static`, so its symbol is the same bits for the whole process and there is no
/// reason to re-derive it. The memo is a [`LazyLock`](std::sync::LazyLock) over the class's own
/// `classify`, which makes the first read a mint and every read after it a load: a builtin body that
/// reads a slot on every call hashes nothing, and the class predicate still runs, at first touch,
/// on the same text the spelling would have been classified from.
///
/// A `LazyLock` memo of a pure function is not run state: [`Symbol::of`] answers the same bits in
/// every run and every process, so the cached value cannot carry anything from one run into the
/// next. Build one with [`static_name!`](crate::static_name), which supplies the mint closure and
/// names the class in the panic message.
pub struct StaticName<S: 'static> {
    text: &'static str,
    symbol: std::sync::LazyLock<S>,
}

impl<S: 'static> StaticName<S> {
    /// Declare a name from its spelling and the mint that classifies it. `const`, so a `static`
    /// can hold one; nothing runs until the first [`symbol`](Self::symbol).
    pub const fn new(text: &'static str, mint: fn() -> S) -> Self {
        StaticName {
            text,
            symbol: std::sync::LazyLock::new(mint),
        }
    }

    /// The spelling as written — what a diagnostic naming this slot renders.
    pub fn text(&self) -> &'static str {
        self.text
    }
}

impl<S: Copy> StaticName<S> {
    /// The classified symbol, minted on first read and loaded thereafter.
    pub fn symbol(&self) -> S {
        *self.symbol
    }
}

/// Declare a [`StaticName`] of a given class from a literal spelling:
/// `static NAME: StaticName<ValueSymbol> = static_name!(ValueSymbol, "name");`.
///
/// The class predicate runs at first read, and a spelling that will not classify panics there
/// naming itself and the class it failed — a build-time mistake in programmer-written text, not a
/// runtime disposition. Every builtin slot reaches [`arg`](crate::builtins::arg) at registration
/// and every tag reaches its own registration, so building a prelude forces each declared name
/// once: a spelling that will not classify fails every test that runs a program, not only the one
/// that exercises its builtin.
#[macro_export]
macro_rules! static_name {
    ($class:ty, $text:literal) => {
        $crate::machine::model::StaticName::<$class>::new($text, || {
            <$class>::classify($text).expect(concat!(
                "`",
                $text,
                "` classifies as a ",
                stringify!($class)
            ))
        })
    };
}

/// Declare a builtin's parameter slots as one group:
/// `slots! { SLOTS { left, right } }`, read as `&SLOTS.left`.
///
/// Each slot is written once, as the ident that names it: the spelling the signature registers and
/// the body reads back is [`stringify!`]-ed out of that ident, so the two cannot disagree. Grouping
/// is a matter of where the declarations sit and nothing else — every field is its own
/// [`StaticName`], forced independently at its first read, so a group of *n* slots mints exactly
/// the *n* symbols the same slots declared one at a time would.
///
/// Value class is the whole vocabulary here: a parameter slot binds a value name, so the group is
/// [`StaticName<ValueSymbol>`] throughout and a spelling that will not classify panics at its first
/// read. A name the machine fixes in Rust source that is *not* a slot — a type or a variant tag —
/// declares through [`static_name!`](crate::static_name) instead, which names its class.
#[macro_export]
macro_rules! slots {
    ($group:ident { $($slot:ident),+ $(,)? }) => {
        /// One builtin's parameter slots, each a name fixed in Rust source.
        struct SlotNames {
            $(
                $slot: $crate::machine::model::StaticName<$crate::machine::model::ValueSymbol>,
            )+
        }

        static $group: SlotNames = SlotNames {
            $(
                $slot: $crate::machine::model::StaticName::new(stringify!($slot), || {
                    <$crate::machine::model::ValueSymbol>::classify(stringify!($slot)).expect(concat!(
                        "`",
                        stringify!($slot),
                        "` classifies as a value-class parameter slot"
                    ))
                }),
            )+
        };
    };
}

#[cfg(test)]
mod tests;
