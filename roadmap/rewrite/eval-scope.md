# Quote binding

How a quote's names and keyworded uses are bound, and where its code is built
and run, as [quotes](../../src/scope/README.md#quotes) lays out.

**Problem.** The shape builder never enters a quote: its code is data until an
`EVAL` builds a block shape for it when the `EVAL` runs, and every free name
then resolves by name at the `EVAL`'s own position
([`Shape::for_eval`](../../src/scope/shape.rs)). So `TWICE #(PRINT x)`, where
`TWICE` runs its code argument, reads `x` in `TWICE`'s scope rather than the
caller's, and a name `TWICE`'s scope happens to declare captures the caller's.
A quote has no way to say which of its names it takes from where it is
written, and `$(…)` is the only spelling that runs code. At run time the search
reaches through only one frame: [`through_chain`](../../src/scope/activation.rs)
follows a block activation's pointer to its enclosing one, and a callable's or
module's activation reaches only its own slots and the captures its shape fixed
where it was built. And a statement containing `EVAL` at any depth waits on
every unit declared before it, so a binder before it that waits on that
statement refuses the body with `EvalCycle`. A comparison that reaches a
function is `Incomparable`
([equality and rendering](../../src/knot/README.md#equality-and-rendering)).

**Acceptance criteria.**

- A name written unmarked in a quote is a hole: it binds only to a builtin, a
  binder in the quote's own code, or a name `code USING src` supplies, and never
  to a binding where the quote is written or run.
- With `twice` a function whose body evaluates its code parameter,
  `twice #($x MINUS 1)` evaluates with the caller's `x` under the drain, and
  `twice #(x MINUS 1)` is an unbound-name error when its `EVAL` runs, whatever
  `twice`'s scope, parameters or body declare.
- `$x` resolves where the quote is written and `\x` where its code is built,
  for value and type names alike, and `$x` never binds to a binder in the code.
- `$(…)` and `\(…)` each wrap exactly one keyworded use; one holding no
  keyworded use, or a closed builtin expression shape such as `LET`, is refused,
  while `$(PRINT x)` and `$(a == b)` are not.
- A mark belongs to the innermost quote that is a value, and a mark no quote
  value holds — in a body written in place outside one included — is refused.
- No line is led by `$`: inside a quote, `$x` alone on a line is the bound name
  `x`. `$(…)` evaluates nothing, and `EVAL` runs code.
- A quote's carried type is its code kind and the `\` names no binder in its own
  code fills, spelled `:(Expression NEEDING #[y])`. A parameter of that type
  admits a quote whose needed names the list covers, `:Code` admits every quote,
  and an `EVAL` of the parameter reads each listed name where the `EVAL` is
  written, refused where the shape is built when one is not visible there.
- Each written quote's code is shaped where the program loads and never refused
  there: an error in it is reported when an `EVAL` runs it, and an `EVAL` of a
  written quote builds no shape.
- Code an `EVAL` runs chains its operator runs under the builtin groups and the
  groups it holds itself, whatever groups surround the quote or the `EVAL`.
- An `EVAL` takes no hold on a frame and waits on no binder declared before it;
  its operand is an eager mention like any other.
- `LET echo = #(PRINT $echo)` is a one-node knot, and it equals its copy.
- Code compares as a bisimulation that follows bound names: two quotes of the
  same text whose `$` names bind different values are unequal.
- A function equals its copy and is unequal to the same text written at another
  site, and a module or a barrier is `Incomparable`.
- A function built from code has one shape per `FN` written in the quote, and
  the bindings its code carries reach it as captures: with
  `make = FN :{v :Number} -> Any = #(EVAL #(FN :{} -> Number = #($v)))`, two
  results of `make 1` are equal and a result of `make 2` is not.
- Printing code never follows a binding, and prints each mark as written.
- `code USING src` binds each hole of `code` a field of `src` names and returns
  code with the others still holes; a field naming no hole is ignored.

**Directions.**

- *Code fills holes, frames never do — decided.* A binder anywhere in composed
  code binds a hole in it, in either direction, while a callable's parameters,
  locals and scope never reach code it receives, and a name bound with `$` is
  never re-resolved. Composition only fills holes, so it cannot capture a name its
  writer resolved; the one silent case is a forgotten `$`. A builtin belongs to
  no scope, so a hole naming one resolves to it.
- *`\` resolves where code is built — decided.* The explicit escape from
  hygiene, one name or one keyworded use at a time. Bawden and Rees's syntactic
  closures name the same free-name list, and Racket's `datum->syntax` is the
  escape its macro writer applies.
- *`$` resolves, it never evaluates — decided.* A computed value enters a quote
  as a bound name: `LET v = (…)` then `#(… $v …)`. No builtin turns a computed
  value into code, since a value rendered as syntax does not in general read
  back as that value. Neither `$(y)` nor `\(y)` is `$y` or `\y`, since a group
  mark covers only its keyworded use, and `$` leads no line: it prefixes one
  atom or glues to one group. On a compound atom it marks the leading name, so
  `$a.b` is `ATTR $a b`.
- *Which quote a mark belongs to — decided.* The innermost quote that is a
  value. A quote a builtin reads as written — a callable's body, a head, an arm
  — is syntax of the code around it, so a `$y` in an `FN` body inside `#(…)` is
  that quote's, and resolves where it is written.
- *Keyworded uses in a quote — decided.* Resolving them is
  [dispatch](dispatch.md)'s: a hole's candidates, `$(…)` and `\(…)`, and the
  bucket keys a `NEEDING` list names. This item parses the group marks and
  refuses one that wraps no keyworded use or a closed builtin expression shape,
  whose bucket resolves the same everywhere. An open bucket such as `PRINT`'s or
  `==`'s can be overloaded, so a mark around its use is not refused.
- *Running code is `EVAL` — decided.* A builtin, so every quote sees it.
- *A code parameter's type names what the code needs — decided.* Spelled from the
  code's side, `:(Expression NEEDING #[y])`: when code is composed its holes are
  its inputs and its binders its outputs. The parameter's side consents by
  offering the names, as `\` is the consent of the side that writes, so neither
  side is captured without saying so. The names' bindings resolve statically
  where the `EVAL` is written, so no frame is searched at run time. Haskell's
  implicit parameters are the precedent: `?x` in a type, supplied by name where
  the value is used. The spelling names nothing else: a quote's carried type is
  its syntax, which says neither the value its code returns nor its names'
  types, and dispatch trusts a carried type, so a return type there could fail
  only when the code is built rather than fall through dispatch.
- *A quote's carried type — decided.* Its code kind and the `\` names no binder
  in its own code fills, which takes the code's scope, so each written quote's
  code is shaped where the program loads. The shape depends on the code alone,
  so the one built at load is the one every `EVAL` of that quote runs. A quote
  is still checked only where its code runs: an error in its shape is kept for
  the `EVAL` that runs it. Composition recomputes the carried type.
- *A callable body built from code — decided.* A body written in place resolves
  its names where it is written, which is where it is built. Code that arrives
  in a body — a code name as the body, or code spliced into a written one — has
  its holes filled only by binders in the composed body, and the callable's
  parameters and scope fill only its `\` marks.
- *A function built from code — decided.* An `FN` written in a quote has one
  shape, built with the quote's code, and whatever differs between two code
  values of that quote — its `$` bindings, its `USING` supplies, the names an
  `EVAL` offers — reaches the function as captures, so equality counts it.
  Code composed at run time builds a shape per composed value
  ([code splicing](code-splicing.md)).
- *A free name at `EVAL` — decided.* An unbound-name error. Resolving it where the
  `EVAL` is written would need a hold on that frame and would order the `EVAL`
  after every binder declared before it.
- *Operator groups in code an `EVAL` runs — decided.* It chains under the
  builtin groups and the groups the code holds itself. A group frame around the
  quote or the `EVAL` is a frame, and frames fill nothing in code; a hole could
  not see a user's operator overloads anyway.
- *Code equality — decided.* Code compares by structure and follows its bound
  names as a bisimulation, as circular data does
  ([equality](../../src/values/README.md#equality-and-rendering)). A hole
  compares by its symbol, and a `\` mark by its symbol and its mark.
- *Function equality — decided.* A function compares by its shape handle, one
  per written `FN`, `EXPR` or `OP` so a copy keeps it, and by its captures
  compared as a bisimulation. Bindings are immutable and no shape retains a
  defining scope, so the shape and captures fix what a function does. Comparing
  syntax instead would compare every keyworded binding a body reaches. Erlang
  funs, C# delegates and Julia closures compare the same way. A builtin's
  equality is [dispatch](dispatch.md)'s, which makes builtins function values.
- *Knot membership — decided.* A `$` name naming a fellow knot member is a
  deferred mention, so a quote holding one is born in its knot as a function
  node is. A hole or a `\` mark names no binding and adds no edge.
- *A `USING` field that names no hole — decided.* Ignored, as record width
  subtyping ignores a field a slot does not name, so several templates can share
  one context record. A hole an earlier `USING` filled is no hole, so a later one
  never rebinds it.

## Dependencies

**Requires:**


**Unblocks:**

- [Dispatch](dispatch.md) — a quote's keyworded uses are holes unless marked.
- [Code names](code-names.md) — a sigiled name inside a quote resolves beside the quote's bindings.
- [Code splicing](code-splicing.md) — a quote's bindings and knot edges.
