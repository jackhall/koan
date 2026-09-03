# Uniform forward type references in sigiled type expressions

Every sigiled type surface answers a forward type reference the same way, with
one diagnostic that names the position rule.

**Problem.** A type name declared lexically later than its consumer is invisible
to the elaborator — a position error rather than a park, per
[elaboration.md § Type names obey strict source order](../../design/typing/elaboration.md).
Every type-expression surface enforces that rule, but each renders the refusal
its own way. Over one program shape — a consumer naming a type declared on the
next line — the surfaces answer:

| Surface | Diagnostic |
| --- | --- |
| bare leaf annotation (`:Ordered`) | `unbound name 'Ordered'` |
| `:(LIST OF Ordered)`, `:(MAP Str -> Ordered)` | `unbound name 'Ordered'` |
| record type field (`:{Ty :Ordered}`) | shape error: unknown type name Ordered in record fields for Ty |
| `:(FN :{Ty :Ordered} -> Module)` | the record field's message, since the parameter list is a record type |
| NEWTYPE record repr (`NEWTYPE Box = :{v :Later}`) | shape error: unknown type name Later in NEWTYPE record repr for v |
| NEWTYPE bare-leaf repr (`NEWTYPE Box = Later`) | shape error: NEWTYPE repr \`Later\` is not a known type |

Four renders of one rule, and none of them says that the name exists but is
declared too late — the writer is told the name is unknown, so the fix the
message suggests (declare it) is the fix they already applied. The message
shapes come from three raisers: the field-list surfaces build theirs through
`unknown_type_name`
([`resolver.rs`](../../src/machine/model/types/resolver.rs)) with the field-list
context noun attached; the bare-leaf annotation and type-constructor operand
paths surface the resolver's `Unbound` arm as an ordinary unbound-name error
([`resolve_type_identifier.rs`](../../src/machine/execute/decide/resolve_type_identifier.rs));
and a slot carrying an
[`Argument::role`](../../src/machine/model/types/signature.rs) label is raised by
the dispatch lane instead
([`keyworded.rs`](../../src/machine/execute/decide/keyworded.rs)), which renders
the role noun and drops the "unknown type name" wording.
The `NameLookup::Parked` arm — the one path that does wait — is reachable only
for an *earlier* still-finalizing binder, so no surface parks on a later
sibling.

The bare-leaf `NEWTYPE` repr additionally speaks with **two** voices for one
slot. Its `Type`-token operand is exempt from the dispatch-time park, so a name
naming a co-declared sibling reaches the body raw and misses there as
`NEWTYPE repr: unknown type name \`Aa\``
([`newtype_def.rs`](../../src/builtins/newtype_def.rs)), while every other miss
at the same slot is the lane's role-labeled render above. Both are pointed and
neither is wrong; they are two wordings for one position, which is the split
this item collapses.

**Acceptance criteria.**

- A forward type reference reports one diagnostic across every type-expression
  surface: a bare leaf annotation, a `:(LIST OF …)` element, a `:(MAP … -> …)`
  key or value, a record-type field, an `:(FN :{…} -> …)` parameter list, and a
  NEWTYPE repr in both its record and bare-leaf spellings — the bare-leaf one
  through a single raiser, not one wording from the dispatch lane and another
  from the body.
- That diagnostic distinguishes a name that is declared later from a name that
  is never declared, and names the declaration whose position is the problem.
- A test walks one forward-reference program shape across each surface above and
  asserts the same error kind from each.
- Whether a forward reference parks is a single rule stated in
  [elaboration.md](../../design/typing/elaboration.md) and observed identically
  by every surface above.

**Directions.**

- *Whether any surface may park on a later sibling — decided.* No surface
  parks: strict source order stands as
  [elaboration.md](../../design/typing/elaboration.md) states it, and the work
  is purely diagnostic — one message, raised from one place, that says
  "declared later". Forward parks exist nowhere in the language: the claim
  store's exclusive visibility cutoff makes a later sibling's placeholder
  invisible ([name-placeholders.md](../../design/execution/name-placeholders.md)),
  and a non-binder form's `Type`-token operands park at dispatch only on
  *backward* in-flight producers
  ([`resolve_dispatch.rs`](../../src/machine/execute/decide/resolve_dispatch.rs)),
  so parking here would make forward references legal in type position while
  they stay illegal in value position. (The retired
  `sigil_fn_forward_reference_defers_via_combine` test pinned the parking
  reading for the `:(FN …)` parameter list alone, in
  `tests/type_language_sigil.rs` before commit 473ba96d: FN's field-list walk
  built its elaborator without a lexical chain, so index gating never applied.
  That reached exactly one surface and contradicted the invariant.)
- *Where the uniform diagnostic is raised — decided.* One raiser function
  (`type_name_miss`, beside `unknown_type_name` in
  [`resolver.rs`](../../src/machine/model/types/resolver.rs)) that every
  surface's miss arm calls; the field-list context noun (*record fields for
  `Ty`*) survives as optional framing on it. Never-declared misses unify into
  the same wording family — `` unknown type name `X` `` plus the context
  framing (the dispatch lane's role wording folds in); a context-free bare-leaf
  miss keeps `UnboundName` for value-channel symmetry.
- *Telling "declared later" from "never declared" — decided.* A distinct
  `KErrorKind::ForwardReference { name, context, hint }` — named without a
  type-specific noun so a future recursive-value-definition feature can reuse
  it — classified by one unfiltered re-probe on the error path. Its display-only
  `hint` (supplied by the type channel) suggests the `MODULE`-body
  co-declaration for mutual recursion.

## Dependencies

**Requires:** none — a diagnostic-and-rule cleanup over the shipped resolver.

**Unblocks:** none.
