# Porosity read off declared members

**Problem.** The static pass reads a value's porosity off its site's static
type ([`porous_site`](../../src/dispatch/statics.rs),
[porosity](../../design/type-tiers.md#porosity)): `Any`, `Value`, every record
type, every signature type and `Module` are porous, so any module or record
handed to a frame at a porous slot is an exit. A body that builds a module
locally and passes it on — `(MODULE m OVER #[link] = (LET held = link))`
handed to a callable taking `:Holder` — is therefore porous and never caches,
though the module captures nothing the body minted and declares no member at a
type that exposes a carrier. Only a value the body received, or a builtin
`ATTR` read of one, is exempt
([two classes of body](../../design/type-tiers.md#two-classes-of-body)).

**Acceptance criteria.**

- A module or record value built in a body is porous only where it captures
  minted content, or where it or one of its members is declared at a type
  whose quantifiers expose its signature, judged structurally over its
  declared members rather than by the static type of the site it is passed at.
- A test passes a module built over a parameter, capturing no view, to a
  frame at a signature-typed slot from a body that views a parameter, and
  finds the body sealed and caching; a second passes a module that captures a
  view the body made, and finds the body porous.
- Every caching test in [the caching tests](../../src/dispatch/tests/caching.rs)
  still holds its tiers flat.

**Directions.**

- *Where the judgement lives — open.* The content tree the load already reads
  for a module-valued expression ([paths](../../src/dispatch/README.md#paths))
  names each capture and member; reading porosity off it rather than off the
  static type keeps the rule at the load. Alternatives: a flag folded per
  module shape at its build.

## Dependencies

**Requires:** none.

**Unblocks:** none.
