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

    /// The digest of `fragments` joined by single spaces, taken without materializing the join:
    /// one hasher streams each fragment and the separator between them, so the bytes fed are
    /// exactly the bytes [`of`](Self::of) sees over the joined string and the two agree bit for
    /// bit. The joined form is what a caller renders; nothing here builds it.
    pub fn of_parts(fragments: &[&str]) -> Symbol {
        let mut hasher = blake3::Hasher::new();
        for (index, fragment) in fragments.iter().enumerate() {
            if index > 0 {
                hasher.update(b" ");
            }
            hasher.update(fragment.as_bytes());
        }
        Symbol::of_hash(hasher.finalize())
    }

    /// The low 128 bits of a finished BLAKE3 hash — the single funnel both [`of`](Self::of) and
    /// [`of_parts`](Self::of_parts) end in, and so the one site the mint tally counts.
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

/// The process's symbol-mint total — every [`Symbol::of`] and [`Symbol::of_parts`] since startup.
#[cfg(feature = "alloc-count")]
pub fn symbols_minted() -> u64 {
    MINTED.load(std::sync::atomic::Ordering::Relaxed)
}

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
        let mut texts = self.texts.borrow_mut();
        texts.entry(symbol).or_insert_with(|| text.into());
        symbol
    }

    /// The text recorded for `symbol`, or `None` if nothing interned it in this run. Render paths
    /// only — a miss is a rendering placeholder, never an error.
    pub fn resolve(&self, symbol: Symbol) -> Option<String> {
        self.texts
            .borrow()
            .get(&symbol)
            .map(|text| text.to_string())
    }

    /// How many distinct labels this run has recorded.
    pub fn len(&self) -> usize {
        self.texts.borrow().len()
    }

    pub fn is_empty(&self) -> bool {
        self.texts.borrow().is_empty()
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

/// Every classified label newtype below wraps exactly one [`Symbol`] behind a private field and is
/// minted only through [`of`](ValueSymbol::of) / [`declared`](ValueSymbol::declared), which run the
/// class predicate on the text. There is no raw-`Symbol` constructor: a `Symbol` alone carries no
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
            /// Pure classification — no interning. This is the **probe** constructor: a lookup that
            /// arrives as source text converts once here and compares symbol bits below, and a
            /// wrong-class name misses the table by returning `None` at the seam.
            pub fn of(text: &str) -> Option<Self> {
                let classifies: fn(&str) -> bool = $classifies;
                classifies(text).then(|| $name(Symbol::of(text)))
            }

            #[doc = concat!("Classify `text` as ", $class, " **and** record it for rendering.")]
            ///
            /// The **declaration** constructor: a name that enters a binding table is interned so a
            /// later diagnostic naming it can resolve the text back.
            pub fn declared(text: &str, labels: &LabelInterner) -> Option<Self> {
                let classified = $name::of(text)?;
                labels.intern(text);
                Some(classified)
            }

            /// The raw digest — for digest feeds, schema scans and
            /// [`render_label`](crate::machine::model::render_label).
            pub fn symbol(self) -> Symbol {
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
    /// space-joined operator probe keys built out of them (`"+ *"`), which stay keyword-class
    /// because they gain no lowercase letter. Nothing *binds* to one; the class exists because the
    /// operator table and the dispatch lane key by fixed tokens.
    KeywordSymbol,
    crate::machine::model::is_keyword_token,
    "a keyword-class token"
);

impl KeywordSymbol {
    /// The probe key `fragments` joined by single spaces stands for, minted without building the
    /// join — how an operator chain reaches its group registration, whose powerset keys are minted
    /// from the joined spelling (`crate::machine::core::bindings::ops::powerset_probes`).
    ///
    /// Every fragment must classify keyword-class on its own, and the joined run then does too: a
    /// separator adds no lowercase letter, and a fragment carrying letters already clears the
    /// two-uppercase bar for the whole.
    pub fn of_parts(fragments: &[&str]) -> Option<Self> {
        fragments
            .iter()
            .all(|fragment| crate::machine::model::is_keyword_token(fragment))
            .then(|| KeywordSymbol(Symbol::of_parts(fragments)))
    }
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
/// fixed syntax and bind to nothing, so they are not a variant — `BinderSymbol::of` of keyword text
/// is `None`.
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
    /// The bindable symbol for `text`, or `None` if it is keyword-class. Pure — no interning.
    pub fn of(text: &str) -> Option<Self> {
        if let Some(name) = TypeSymbol::of(text) {
            return Some(BinderSymbol::Type(name));
        }
        ValueSymbol::of(text).map(BinderSymbol::Value)
    }

    /// Classify `text` as bindable **and** record it for rendering — the declaration constructor.
    pub fn declared(text: &str, labels: &LabelInterner) -> Option<Self> {
        let classified = BinderSymbol::of(text)?;
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

#[cfg(test)]
mod tests;
