# Builtin shapes

The builtin form table restated as typed expression shapes, with its untyped key
and its raw slots derived rather than hand-kept, under names that say which
shape is meant.

**Problem.** A builtin's syntax is described three times. The
[form table](../../src/parse/builtin_shapes.rs) spells each `Form`'s untyped bucket key —
keywords and bare slots — and, beside it, `lazy_slots`, the part kinds each slot
keeps raw. The lattice's
[`ExpressionShape`](../../src/type_lattice/README.md#the-node-vocabulary) is the same
keyword-and-slot sequence with a type at each slot and a return, and a form's
key is that shape with the types erased: the first runtime's
[`newtype_def`](../../src/builtins/newtype_def.rs) registers three typed
overloads under the one `NEWTYPE _ = _` key, and its `untyped_key` must agree
with the table's by convention alone. `lazy_slots` repeats a third time what a
slot typed `KExpression`, `SigiledTypeExpr` or `RecordType` already says.
[`roles`](../../src/scope/roles.rs) is a fourth column, keyed by `FormId` beside
the table rather than in it. The names hide the relation: `Form` and
[`Shape`](../../src/scope/README.md#three-tiers) are both bare words beside
`ExpressionShape`, and the `Schema` role — the part of a type declaration after
`=` — shares its word with a `:{…}` record, `SigSchema` and `NodeSchema`, none
of which it is.

**Acceptance criteria.**

- A `BuiltinShape` is one `static` entry per builtin bucket: its keyword run,
  at each slot a role and one slot type per overload, one return per overload,
  and its binder facts. No entry spells an untyped key or a raw-slot list.
- A builtin's `ExpressionKey` is the erasure of its entry, read off the entry's
  own elements, so every overload of a bucket shares one key; an entry whose
  slots disagree on how many overloads the bucket has does not build.
- The part kinds a slot keeps raw are derived from the raw-leaf slot types of
  the bucket's overloads, and every reader of `lazy_kinds_at` reads the same
  answer it reads today.
- An entry whose `Body`, `Branches`, `Quantifiers` or `Data` slot is typed
  anything but `KExpression` in some overload, or whose `Rhs` slot keeps a part
  raw, does not build.
- A node still resolves its `BuiltinShape` once at construction, by a probe over
  `static` data that touches no registry, and caches a `&'static` reference.
- One door in [`elaborate`](../../src/elaborate/README.md) interns a
  `BuiltinShape`'s overloads as `ExpressionShape` handles, one per overload and
  none for a reserved bucket, and each handle's erased elements equal the
  entry's `ExpressionKey`.
- `Form` is `BuiltinShape` in a module named `builtin_shapes`, the untyped
  bucket key is `ExpressionKey`, scope's `Shape` is `BodyShape`, and the
  `Schema` role is `Definition`, in source and in every module README.

**Directions.**

- *Static specs, erased at build time — decided.* The typed shapes rest in
  `static` data and the key and raw slots are erasures of them. A table
  built per run and keyed by handles is rejected: a node knows only its untyped
  key when it probes, the parser probes before any registry exists, and every
  run would pay the interning up front for no lookup it could use.
- *Overloads in columns — decided.* An entry holds one keyword run, and each
  slot holds its overloads' types side by side, because that is how a
  user-defined bucket works too: one key, several typed overloads under it.
  Overloads that erase to different keys cannot be written. One full element run
  per overload is rejected: agreement would be a `const` comparison of keywords,
  and `const` evaluation cannot read through
  [`KEYWORDS`](../../src/parse/builtin_shapes.rs), whose names each hold a lazy memo.
- *Binder facts stay out of the type — decided.* A user cannot define a binder,
  so `BinderFacts` is a fact of the `BuiltinShape` and never of an
  `ExpressionShape`.
- *How a static spec names a slot type — decided.* By its `KType`: a leaf handle
  is a `const` content digest
  ([`handle.rs`](../../src/type_lattice/handle.rs)), so a slot typed by a leaf
  rests in a `static` as the handle itself and the raw slots are the ones whose
  handle is a raw leaf. [`parse`](../../src/parse/README.md) gains an edge to
  the lattice for those constants alone, which closes a cycle with the
  lattice's import of `parse`'s labels, and `KType` gains a `const` equality
  the derivation compares handles with.
- *A slot typed by a compound — decided.* A small static spec beside the leaf
  handles — a union of leaves, the empty record — interned by the `elaborate`
  door. Those two are every compound a builtin slot uses.
- *A reserved bucket — decided.* Nothing registers under it, yet its slots keep
  parts raw so its miss stays a miss; it carries one overload returning `Never`
  to derive them from, and the door interns nothing for it.
- *`CATCH`'s return — decided.* `Any` for now. The first runtime declared
  `Result {Ok = Any, Error = KError}`, an application over two declared types no
  `static` can hold; how the table says it is settled once more
  [module](modules.md) and [dispatch](dispatch.md) code exists.
- *Whether role belongs in `ExpressionShape` — open.* Every builtin bucket
  already gives each slot one role across its overloads, because `roles` is
  keyed by `FormId` and a `FormId` is one key. Carrying role in the lattice node
  makes that a rule user `EXPR`s obey too — an `EXPR` joins a bucket only when
  its roles agree with the bucket's — which keeps one spelling from meaning two
  things and lets a body shape classify the parts of a user-defined call from
  its key alone. Against it: two shapes accepting the same calls become two
  types, the order needs a rule for comparing roles, and a body shape is built
  before a later or imported `EXPR`'s roles are known. The binding roles —
  `Name`, `Quantifiers`, a `Body` that opens a body shape — stay builtin-only
  either way. *Recommended:* settle it under [dispatch](dispatch.md), where
  bucket admission is decided; this item keeps role a `BuiltinShape` fact.

## Dependencies

**Requires:** none — the form table, the role table and `ExpressionShape` all
ship.

**Unblocks:**

- [Type declarations](type-declarations.md) — the door reads a declaration's
  definition part through its builtin shape's roles.
