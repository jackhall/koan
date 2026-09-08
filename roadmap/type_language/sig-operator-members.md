# SIG operator members

A signature declares an operator — and a group of operators that chain together — the way it
declares any other keyworded member, and an ascription view carries enough for
`USING <view> SCOPE` to reduce runs by it.

**Problem.** An `OP` declaration writes two channels: dispatch-bucket overloads (a binary
operator under its `[Slot, <sym>, Slot]` key, a unary operator's list-form and bridge pair) and a
group record in the per-scope operator registry — the member set and chaining mode the run
reducer walks ([design/operators.md](../../design/operators.md)). The bucket half already reaches
the keyworded surface: `SigSchema::raw_self_sig`
([sig_schema.rs](../../src/machine/model/types/sig_schema.rs)) projects every bucket the body
registered, and a bodyless FN head spelling the machine-fixed operand names —
`(FN (left :Carrier + right :Carrier) -> Carrier)` — declares the bucket and is satisfied by a
module's `OP #(+) OVER Str`. But that spelling makes the user restate the operand binders the
`OP` surface fixes, and it claims nothing about chaining. The registry half is not schema content
at all: satisfaction never examines a group record, and an ascription view's operator registry is
empty, so a three-operand run of a non-builtin symbol inside `USING <view> SCOPE` misses with
"no operator group declares all of …" even where the source module reduces it (a builtin symbol
such as `+` masks the gap: the root's group reduces the run and the view's wrapper is dispatched).
Two further defects sit on the path: the keyworded replay into a view applies the builtin-shadow
guard meant for user `FN` definitions, so ascribing a module with a `+` member fails with
`name '(_ + _)' is already bound`; and a `GROUP`'s mixed runs — the motivation for groups — have
no signature spelling, so no view can carry them.

**Acceptance criteria.**

- A SIG body declares an operator member with a bodyless `OP` head carrying the quoted symbol —
  `(OP #(+) OVER Carrier)` — and the member projects into the schema's keyworded channel under
  the same key, parameter names and types the definition registers, derived from the definition's
  own element builder.
- A SIG body declares a group with the bodyless `GROUP` form — the definition minus its name:
  `(GROUP FOLD LEFT = ((OP #(+) OVER Carrier) (OP #(-) OVER Carrier)))` and
  `(GROUP PAIRWISE FOLD #(BOTH) LEFT = (…))` — whose members are read by the definition's own
  member scan. A pairwise group's combiner must be a keyworded member of the signature, checked
  at the SIG finish.
- The schema carries an operator channel: a set of group records (sorted member set, chaining
  mode). It feeds the content digest, renders in a signature's name, rides `TYPE OF`, survives
  `WITH`, and intersects in a signature join. A bare head declares a singleton `FoldLeft` record;
  a unary head a singleton `Unary` one.
- Satisfaction admits a module for a declared keyworded operator member through the same
  most-specific selection every keyworded member takes, and the three keyworded failure
  diagnostics render the operator head (`OP #(+) OVER Carrier`). For each declared group record
  the module's registry must hold a record with an **equal** mode whose member set **includes**
  the declared one; the two new failures name the members and the modes. Two modules whose
  operators differ only in chaining mode are therefore distinguished, pinned by a test and
  documented in [design/typing/modules.md](../../design/typing/modules.md) and
  [design/operators.md](../../design/operators.md).
- An ascription view installs both halves: the selected overload per declared member (coerced
  across an opaque barrier exactly as any keyworded member is, and installed without the
  builtin-shadow guard) and one fresh registry record per declared group over exactly the
  declared members, so a run — a mixed run of declared group members included — inside
  `USING <view> SCOPE` reduces by the declared mode.
- The unary head `(UNARY OP #(~) OVER Carrier -> Result)` declares the whole triple — list-form
  and bridge overloads plus the `Unary` record — and the result-less spelling is a reserved key
  with the pointed "must declare its result type" diagnostic.
- The pairwise heterogeneous head `(OP #(<) OVER Carrier -> Bool)` is admissible exactly where
  the definition admits it — inside a SIG `PAIRWISE` group — and refused elsewhere with a pointed
  diagnostic. A definition of any operator surface inside a SIG body, and a head or bodyless
  group outside one, are each refused naming the other spelling.
- The FN-head spelling of an operator key stays legal and means "bucket only, no chaining
  claimed"; the docs say so.

**Directions.**

- *Declarator spelling — decided.* Bodyless `OP` heads and the bodyless `GROUP` form (spelling
  (a): keep the `=` and the head list, drop the name — a SIG binds no value). Both reuse the
  definition's own mode overloads, element builder and member scan.
- *Chaining mode as signature content — decided.* Group records are schema content; satisfaction
  is equal mode plus member inclusion (width applies to the member set as to every other channel).
- *The unary triple — decided.* One head names the triple.
- *Where "inside a SIG group" lives — decided.* A `ScopeKind::SigGroup { mode }` body scope,
  transparent to the SIG-body gate so heads still record into the SIG collectors, read by the head
  to skip its singleton record and to admit `->` under a pairwise mode.

Plan: `scratch/sig-operator-members-plan.md`.

## Dependencies

**Requires:** none — extends the shipped keyworded surface.

**Unblocks:**

- [Expression shapes are their own kind of function](expression-shapes.md) — the shape
  representation must cover operator members, so their surface is settled first.
