# Reject the unquoted operator symbol

**Problem.** An operator symbol is
[documented](../../design/operators.md) as quoted — `OP #(+) OVER Number = (…)`
— but the unquoted `OP (+) OVER Number = (…)` also registers, and registers
*worse*. The two spellings are read by two different extractors that disagree.

The slot-side read takes the symbol from the declaration's evaluated
`:KExpression` argument, where a `QuotedExpression` part and a plain
`Expression` part are indistinguishable — both resolve to a
`KObject::KExpression` — so the unquoted form passes and the operator's bodies
and registry entry land. The statement-side read,
[`op_def::symbol_from_parts`](../../src/builtins/op_def.rs), scans the
*unevaluated* parts and matches `QuotedExpression` only. Two callers depend on
it, and each degrades differently:

- the **binder hook** discards its diagnostic, so an unquoted declaration
  silently installs no pending-overload park edges — a use site that should
  park until the declaration finalizes instead misses dispatch, making the
  operator's visibility depend on scheduler order;
- **`GROUP`'s member scan** propagates it, so an unquoted `OP` inside a `GROUP`
  body hard-errors rather than being read as a member.

So the unquoted form is neither accepted nor rejected: it half-works at the top
level, and fails loudly one context over. The quote is what makes the symbol an
ordinary slot rather than a keyword that would key a distinct dispatch bucket
per operator, and nothing else in the surface is positioned to carry that
meaning — so the unquoted spelling has no coherent reading to be given.

**Acceptance criteria.**

- `OP (+) OVER Number = (…)` — and every other unquoted spelling of the symbol
  slot — is refused at the declaration with the respelling diagnostic that names
  the quoted form, in every context: bare, inside a `GROUP` body, and under
  `UNARY`.
- One extractor decides what a valid operator symbol is; the binder hook, the
  `GROUP` member scan, and the declaration body all read through it and agree.
- No declaration can register bodies or a registry entry while failing to
  install its park edges.

**Directions.**

- *Where the rejection lands — open.* (a) Tighten the slot-side read to require
  a `QuotedExpression` part, so the body's own extraction refuses the unquoted
  form and the two extractors converge on one rule; (b) type the symbol slot so
  a plain `Expression` cannot inhabit it, refusing the form at dispatch rather
  than in the body. Recommended: (a) — `:KExpression` admits both parts by
  design, and narrowing the *type* to exclude a parenthesized expression would
  reach past this surface into the type language.
- *Whether the binder hook may ever discard a diagnostic — decided.* It may not.
  The hook's silent discard is what turns a bad symbol into a scheduler-order
  bug instead of an error; whichever option lands, the hook and the body must
  reach the same verdict.

## Dependencies

The surface this tightens shipped with the operator declaration surface
([design/operators.md](../../design/operators.md)).

**Requires:** none — leaf cleanup.

**Unblocks:** none tracked.
