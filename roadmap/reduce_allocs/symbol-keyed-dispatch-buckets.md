# Symbol-keyed dispatch buckets

**Problem.** The `functions` table and its claim store key by element slices that embed
keyword *text*. A bucket key is `&'a [StoredElement]` with
`StoredElement::Keyword(&'a str)` ([src/machine/model/types/signature.rs](../../src/machine/model/types/signature.rs)),
its owned twin is `UntypedElement::Keyword(String)`, and the cross-form probe rests on the
hand-written `hash_key_element!` macro (tag byte + string hash) plus two
`hashbrown::Equivalent` impls (`UntypedKeyProbe`, `StoredKeyProbe`). So every registration
bumps each keyword's bytes into the scope region (`store_untyped_key`, and again for the
dedupe `DispatchToken`), every probe hashes and byte-compares keyword text, and
`owned_untyped_key` rebuilds owned keyword `String`s wherever a bucket key is read back
out. The dispatch lane is the last key surface whose currency is text rather than a
[`Symbol`](../../src/machine/model/labels.rs)-family digest.

**Ruling (pinned).** Nothing binds to a keyword. A keyword token
(`is_keyword_token` — at least two uppercase letters and no lowercase, or no letters at
all) is fixed dispatch syntax, meaningful only in combination with a full signature; it is
not an identifier, and no value or type ever binds under one. The value name a combined
`LET name = FN …` form binds is an ordinary value token with no relation to the
signature's keywords. Keyword identity is therefore its own vocabulary: `KeywordSymbol`,
disjoint from the value/type symbol vocabularies, minted only from keyword-class text and
interned at declaration so diagnostics can render it. The newtype ships with
[Symbol-keyed scope binding tables](symbol-keyed-scope-tables.md), where the operator
table already keys by it; this item extends the vocabulary to the dispatch bucket keys.

**Acceptance criteria.**

- `StoredElement` and `UntypedElement` **collapse into one** `Copy`, lifetime-free
  element type (`Keyword(KeywordSymbol)` / `Slot`) — the pair exists only because a
  keyword's text needs an owner, and a symbol doesn't. No keyword text in any bucket
  key, bucket claim key, or `DispatchToken`; registration bumps no keyword bytes
  (re-homing a key is a plain `Copy`-slice bump).
- The single-scheme machinery the split forced is deleted, not ported: the
  `hash_key_element!` macro becomes a derived `Hash`, and both probe wrappers
  (`UntypedKeyProbe`, `StoredKeyProbe`) with their `Equivalent` impls go away — a
  `&'a [Element]` key is probed by a plain `&[Element]` through the standard `Borrow`
  blanket, and key equality is a tag plus `u128` compare per element.
- `DispatchTokenElement` / `StoredDispatchTokenElement` collapse the same way
  (`Slot(KType)` is already `Copy`), taking `DispatchToken::store_in`'s text bumps
  with them.
- FN / OP registration interns each signature keyword in the run's label interner;
  everywhere a bucket key's text is rendered (`iter_functions`, `DuplicateOverload` and
  other diagnostics naming a bucket) resolves through the interner, with a resolve miss
  rendering the standard stable placeholder.
- A call's dispatch probe does not hash keyword text per call: the keyword symbols a probe
  compares are computed once at parse and carried by the expression, the way the cached
  operator probe already is.
- The recorded allocation baselines do not regress: the dispatch-heavy audit shapes
  (`operator_chain`, `builtin_call` differencing) hold their bounds, and the
  registration-side drop (keyword byte bumps per bucket key) is visible in a measured
  figure.

**Directions.**

- *Where keyword symbols are computed — open. Recommended: the parse boundary.*
  `ExpressionPart::Keyword` carries the symbol beside its text (text stays for rendering
  and error paths), so neither registration nor dispatch ever re-hashes. The lookup-seam
  alternative the scope-tables item chose is wrong here: `Symbol::of` is a BLAKE3 hash,
  and paying it per keyword per dispatch on the hot probe path is a regression risk the
  parse-time cache removes outright.
- *Operator keys share the keyword vocabulary — decided per
  [Symbol-keyed scope binding tables](symbol-keyed-scope-tables.md).* A letterless token
  (`+`, `:!`) satisfies `is_keyword_token`, and the operator table already keys by the
  `KeywordSymbol` of its space-joined probe; a keyword element inside a dispatch key
  carries the same newtype, so an operator glyph names one symbol on both surfaces.
- *Entry text — deferred.* `FunctionBucketEntry::summary` and
  `OperatorEntry::declaration` are entry payloads, not keys — the summary's fate belongs
  to [deferred-signature-summaries.md](deferred-signature-summaries.md).

## Dependencies

**Requires:**

- [Symbol-keyed scope binding tables](symbol-keyed-scope-tables.md) — the classified
  symbol vocabulary (`ValueSymbol` / `TypeSymbol` / `KeywordSymbol`), the
  declaration-seam interning conventions, and the identity-hashed table plumbing all
  land there.

**Unblocks:** none.
