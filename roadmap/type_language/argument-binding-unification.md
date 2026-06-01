# Argument-binding unification

Make the record literal `{x = 1}` the single named-argument surface *and* runtime
carrier: every construction and call site reads one record literal, and a call's
resolved arguments install into the callee scope as one record block.

**Problem.** Named arguments have two interchangeable surface forms and a
per-entry runtime install, neither sharing the record substrate.

- *Surface.* A named-argument list parses as either the paren `=`-triple form
  `(x = 3)` (walked by
  [`parse_keyword_triple_list`](../../src/parse/triple_list.rs)) or the dict-form
  `{x: 3}`, both funneled through
  [`parse_named_value_pairs` / `NamedPairs`](../../src/machine/model/values/named_pairs.rs)
  at three callsites: struct construction
  ([`struct_value.rs`](../../src/machine/execute/dispatch/constructors/struct_value.rs)),
  function-value calls ([`kfunction.rs`](../../src/machine/core/kfunction.rs)), and the
  dispatch-side kwarg parse ([`signature.rs`](../../src/machine/model/types/signature.rs),
  which also covers functor application since a functor is a `KFunction` carrier). The
  `(x = 3)` form overloads paren grouping and the `{x: 3}` form overloads the dict
  literal, so each bracket carries two meanings disambiguated only by position —
  exactly the ambiguity the record value's `{x = 1}` surface (`=` pairs) was designed
  to resolve, but the call/construction sites still emit the legacy forms.
- *Runtime.* Installing a call's arguments rebuilds a map entry-by-entry across three
  representations. Dispatch resolves arguments into
  [`ArgumentBundle { args: HashMap<String, Rc<KObject>> }`](../../src/machine/core/kfunction/argument_bundle.rs),
  then the invoke path installs them into
  [`Bindings.data: HashMap<String, (&KObject, BindingIndex)>`](../../src/machine/core/bindings.rs),
  tagging every entry with a `BindingIndex`. But `BindingIndex` is the *lexical
  position of the installing statement* — all of a call's parameters install at the
  same position, so the per-entry `(value, index)` pairing stores one index
  redundantly across the whole block.

**Impact.**

- The record literal `{x = 1}` is the one named-argument surface across construction
  and calls — `Point {x = 1, y = 2}`, `f {x = 1}`, `:(MyFunctor {T = IntOrd})` — so the
  paren and brace surfaces shed their double duty and a reader learns one form.
- A call's resolved arguments *are* the `Record<KObject>` value the surface literal
  produces, so the argument bundle, the surface literal, and the scope's value map
  share one shape; binding becomes an extend/move of the argument record rather than an
  entry-by-entry copy into a differently-shaped container.
- A call's arguments install under a single frame-level binding index, so the per-entry
  index tagging disappears; a field's binding-index lookup, where still needed, derives
  from its position in the ordered record.

**Directions.**

- *Named-argument surface — decided.* `{x = 1}` (record literal) becomes the sole
  named-argument form at all three callsites; the `(name = value)` paren-kwarg form and
  the `{name: value}` dict-form retire. `{x: 3}` struct construction necessarily goes —
  once `:` means dict, a struct can't be constructed from a dict. Positional
  construction (`MyStruct 1 2 3`, the `ConstructorCall` lane) is unaffected.
- *Dispatch classifier — decided.* The `FunctionValueCall` shape and the sigiled
  functor-application shape carry a `RecordLiteral` argument part where they carried a
  parenthesized kwarg `Expression`;
  [`classify_dispatch_shape`](../../src/machine/execute/dispatch.rs) and the `fn_value`
  path admit the record-literal body, and the kwarg extractors above read field names
  off the `RecordLiteral` directly instead of re-walking a paren group.
- *Zero-named-argument call surface — decided.* An empty `{}` is an empty record (`:{}`,
  the top of the record lattice), so a nullary call writes `f {}`: the bundle is always
  a record, empty or not, and a bare `f` stays a name *reference*, not a call. An empty
  record is well-typed on its own, so it sidesteps the empty-container error an empty
  dict trips. This flips slice-1's empty-`{}`-is-a-dict default — a one-line `BraceMode`
  change (`Unknown` resolves to record) that can land as a small first pass ahead of the
  surface cutover, retargeting the `empty_braces_stay_dict` parser test.
- *Empty-dict spelling — deferred.* With `{}` taken by the empty record, an empty dict
  (already annotation-only under the empty-container rule, so a rare surface) needs a
  non-`{}` form — likely an `EMPTY MAP` builtin. Sequenced after the flip.
- *Frame-level index — decided.* Carry one `BindingIndex` per parameter block at the
  frame, not one per entry. The visibility predicate (`Bindings::visible`) reads the
  frame index.
- *Shared carrier — decided.* The three carriers share the `Record<V>` *shape* with a
  per-carrier value type, rather than collapsing to one Rust container: the argument
  bundle is `Record<Rc<KObject>>`, the scope value map `Record<&'a KObject>` (plus the
  one frame-level `BindingIndex` above), and the struct/record value `Record<KObject>`.
  Keeping ownership distinct preserves the per-call-arena model — `Bindings.data`'s
  `&'a KObject` is arena-allocated and freed en masse when the frame retires
  ([per-call-arena-protocol.md](../../design/per-call-arena-protocol.md)), so forcing it
  onto `Rc` for a zero-copy single-carrier collapse would add per-binding refcount
  traffic and break arena reclamation, and `Bindings.data` carries
  visibility/shadowing metadata a bare record doesn't model. Binding is then a uniform
  `.map`/extend over one shape (a `Record<Rc>` → `Record<&'a>` arena transform), not a
  `HashMap`→`IndexMap` rebuild.
- *Invoke-path rewrite — deferred.* The exact rewrite of `KFunction::bind` / invoke to
  emit a record block is sequenced after the surface cutover lands.
- *Corpus migration — decided.* Every `(x = 1)` / `(T = IntOrd)` / `{x: 3}` named-arg
  site in tests, examples ([TUTORIAL.md](../../TUTORIAL.md)), and design docs rewrites
  to the record form in the same change; the retired forms become parse errors so a
  missed callsite fails loudly rather than silently.

## Dependencies

**Requires:**

None — the anonymous record value this surface and install path bind has shipped (see
[type-language-via-dispatch.md § Record-type sigil](../../design/typing/type-language-via-dispatch.md#record-type-sigil)),
along with the [record substrate](../../design/typing/ktype.md#record-fields-and-ktype-hashing)
the runtime carriers consolidate onto.

**Unblocks:**

None — this is the surface-and-runtime payoff of the record substrate. (Soft, not a
dependency edge: per-call type-parameter binding's invoke-path wiring rebases onto this
block-install path if this lands first, but isn't blocked by it.)
