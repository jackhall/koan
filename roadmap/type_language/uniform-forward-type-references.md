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

Three renders of one rule, and none of them says that the name exists but is
declared too late — the writer is told the name is unknown, so the fix the
message suggests (declare it) is the fix they already applied. The two message
shapes come from different raisers: the field-list surfaces build theirs through
`unknown_type_name`
([`resolver.rs`](../../src/machine/model/types/resolver.rs)) with the field-list
context noun attached, while the bare-leaf and type-constructor operand paths
surface the resolver's `Unbound` arm as an ordinary unbound-name error
([`resolve_type_identifier.rs`](../../src/machine/execute/decide/resolve_type_identifier.rs)).
The `NameLookup::Parked` arm — the one path that does wait — is reachable only
for an *earlier* still-finalizing binder, so no surface parks on a later
sibling.

**Acceptance criteria.**

- A forward type reference reports one diagnostic across every type-expression
  surface: a bare leaf annotation, a `:(LIST OF …)` element, a `:(MAP … -> …)`
  key or value, a record-type field, an `:(FN :{…} -> …)` parameter list, and a
  NEWTYPE record repr.
- That diagnostic distinguishes a name that is declared later from a name that
  is never declared, and names the declaration whose position is the problem.
- A test walks one forward-reference program shape across each surface above and
  asserts the same error kind from each.
- Whether a forward reference parks is a single rule stated in
  [elaboration.md](../../design/typing/elaboration.md) and observed identically
  by every surface above.

**Directions.**

- *Whether any surface may park on a later sibling — open.* Two readings.
  (a) Strict source order stands as
  [elaboration.md](../../design/typing/elaboration.md) states it, and the work is
  purely diagnostic: one message, raised from one place, that says "declared
  later". (b) A sigiled type expression may park on a later top-level sibling's
  placeholder, and the work extends deferral to every surface uniformly. Reading
  (b) is what the retired `sigil_fn_forward_reference_defers_via_combine` test
  pinned for the `:(FN …)` parameter list alone (in `tests/type_language_sigil.rs`
  before commit 473ba96d): FN's own field-list walk built its elaborator without
  a lexical chain, so index gating never applied and the later sibling's
  placeholder was visible to park on. That behaviour reached exactly one surface
  and contradicts the invariant the design doc pins, which is why it is a fork to
  rule rather than a regression to restore. Recommended: (a) — the strict-order
  rule is stated as a language invariant, and (b) makes forward references legal
  in type position while they stay illegal in value position.
- *Where the uniform diagnostic is raised — open.* Either every surface routes
  its miss through `Scope::resolve_type_identifier`'s `Unbound` arm and the
  field-list contexts become extra framing on one error, or `unknown_type_name`
  becomes the single raiser and the bare-leaf paths adopt it. The choice decides
  whether the field-list context noun (*record fields for `Ty`*) survives.
- *Telling "declared later" from "never declared" — open.* The resolver gates
  candidates by `idx < cutoff`, so it already knows the difference at the point
  of the miss; whether it reports that as a distinct `KErrorKind` or as extra
  wording on the existing one is unruled.

## Dependencies

**Requires:** none — a diagnostic-and-rule cleanup over the shipped resolver.

**Unblocks:** none.
