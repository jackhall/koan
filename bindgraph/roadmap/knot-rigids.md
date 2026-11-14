# Rigids that name their knot

A knot that carries one type and takes nothing from outside gives its names one rigid that an ask
may relate.

**Optional.** A knot that reaches an alternative from outside is typed by their union
([a knot that carries one type](../README.md#instances)), so the knots this item reaches are
recursions with no alternative from outside: no base case the text writes. No program is known
where relating their rigids changes an answer.

**Problem.** A name the answers leave untyped is a rigid ([knots](../README.md#instances)). A rigid
is one term per name, the same in every view and at every level of a recursion
([domain.rs](../src/domain.rs)): `LOOP` under `Elem := Number` and under `Elem := Str` both give
`rigid(rest)`. So the domain's contract keeps rigids apart: each occurrence stands for its own
unknown, and no ask relates two.

A knot made only of takes, such as two results that take each other through calls, carries one
type through every level. Yet each of its names is its own rigid. An ask over a type that holds
two of them cannot see that they are equal. An example is a candidate `FOR ALL #[Elem]` matched
against a pair whose two halves are both `Elem`, offered a pair whose halves are two names of
the knot.

One term per name is also what keeps the views of some texts finite. Body `G`, with entry name
`y`, has a result `r` that takes `r` through a self-call passing `y := y`: a knot of takes. Body
`F`, with entry name `x`, has a name `a` that takes `G`'s `r` through a call passing `y := x`,
then calls itself passing `x := a`. `a` is `rigid(r)` at every level, so `F` under
`x := rigid(r)` repeats and the walk stops. The growth check refuses nothing in this text, since
no arc in it is built.

**Acceptance criteria.**

- The names of a knot whose every step is a take or a union alternative, and that reaches no
  alternative from outside, share one rigid that names the knot.
- The domain makes a knot's rigid through a door of its own, and its obligation that rigids stay
  apart excepts two occurrences of one knot's rigid. Over the tests' domain, an ask whether two
  names of such a knot are the same answers that they are.
- Rigids of different knots stay apart, and so do rigids of one knot reached under different
  views.
- A knot of any other kind keeps each name its own rigid.
- A knot's rigid is the same from every start, in the crate and in its reference, and after the
  embedder resets the bump.
- The set of views a text reaches stays finite. The Finite law holds over generated texts whose
  views hold knot rigids, and over the text of `F` and `G` above.
- The laws hold over generated texts with such knots, and the crate's reference shares the rigids
  the same way.

**Directions.**

- *How the views stay finite — open.* A rigid that names its knot by view makes `F` above reach a
  new view at every level. The options:
  - the growth check treats a knot's rigid as built over the names of its view, drawn from the
    text's components. `F` is refused when it is laid, and so is every text that passes one
    recursion's inferred result back into another, whether or not a binding would knot it;
  - a knot whose view already holds a rigid of its own text component keeps each name its own
    rigid. Nothing more is refused. The rigid then carries which components it came through, and
    two components that feed each other are not yet shown finite.
- *What names a knot — open.* The options:
  - an id the crate interns per knot, keyed by the knot's lowest name and that name's view. The
    table is the crate's to keep, an id depends on the order knots are met, and a bump reset
    changes it;
  - the lowest name and its view written into the rigid itself, so views nest;
  - the domain makes the rigid from the lowest name and the digests of that name's view. Nothing
    is kept, and the rigid is the same from every start.

  Recommended: the domain makes it from the digests. Under each option, a knot of several
  evaluations over one name needs an order on views to pick the lowest.
- *Which knots share — decided.* Only a knot whose every step is a take or a union alternative. A
  question or a callee's binding on a knot changes the type from level to level
  ([knots](../README.md#instances)). The kind is read where an evaluation closes the knot, per
  binding ([a knot's kind](../README.md#instances)).

## Dependencies

**Requires:** none.

**Unblocks:** none.
