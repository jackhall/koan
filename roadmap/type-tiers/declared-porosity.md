# Porosity read off declared members

**Problem.** The static pass reads a value's porosity off its site's static
type ([`porous_site`](../../src/dispatch/statics.rs),
[porosity](../../design/type-tiers.md#porosity)): `Any`, `Value`, every record
type, every signature type and `Module` are porous, so any module or record
handed to a frame at a porous slot is an exit. A body that builds a module
locally and passes it on — `(MODULE m OVER #[link] = (LET held = link))`
handed to a callable taking `:Holder` — is therefore porous and never caches,
though the module captures nothing the body minted. Only a value the body
received, or a builtin `ATTR` read of one, is exempt, and only as a call's
argument: a body returning a value it received is porous all the same
([two classes of body](../../design/type-tiers.md#two-classes-of-body)).

**Acceptance criteria.**

- A value that holds nothing its frame body minted is no exit, as a call's
  argument and at the body's return alike: a value the body received, a
  record literal written in place whose every field holds nothing minted, or
  a module whose content tree reaches no minted content — no view, no
  closure, no module body holding a minting site, and no leaf but a value the
  body received.
- A test passes a module built over a parameter, capturing no view, to a
  frame at a signature-typed slot from a body that views a parameter, and
  finds the body sealed and caching; a second passes a module that captures a
  view the body made, and finds the body porous.
- A module capturing a local binder is sealed where the local's tree reads
  through to received or literal content, and porous where it holds a view or
  content the load cannot see through.
- A body returning a parameter, a record literal of parameters, or a module
  built over a parameter is sealed and caching.
- A loop that views its parameter and hands a callee a module built over its
  link each hop caches and holds its region flat.
- Every caching test in [the caching tests](../../src/dispatch/tests/caching.rs)
  still holds its tiers flat.

**Directions.**

- *Where the judgement lives — decided.* The content tree the load already
  reads for a module-valued expression
  ([paths](../../src/dispatch/README.md#paths)) names each capture; reading
  porosity off it keeps the rule at the load, follows local binders and
  unfolds helper calls. A member's declared type can name minted content only
  through a capture or a minting site in the module body, so no separate
  check of declared types is needed.
- *Records — decided.* A record literal written in place is judged field by
  field; a record bound by `LET` and then passed keeps the static-type rule.

## Dependencies

**Requires:** none.

**Unblocks:** none.
