# Quantifiers on declarations

A `FOR ALL` is written on a declaration and solved where the declaration is
used, and the lattice's order never solves one.

**Problem.** A quantified `FN` is a value whose type binds its group, and that
type reaches wherever a value's type does: a slot typed `:(FN FOR ALL …)`
([examples](../../src/elaborate/tests/examples.rs)), a list's element type, a
union's payload ([families](../../src/elaborate/tests/families.rs)). The order
relates two such types by instantiation
([`admits_function` and `admits_shape`](../../src/type_lattice/sig_relations.rs)),
so over a type holding one its laws hold up to equivalence and not by handle
([concrete types and binders](../../src/type_lattice/README.md#concrete-types-and-binders)).
[Canonical form](../../src/type_lattice/README.md#the-relations) narrows that
gap, and costs a call every variable it drops: under
`EXPR FOR ALL #[Elt] #(KIND x :Elt) -> Type = #(Elt)`, `KIND 1` gives `Any`.
[`sig_subtype`](../../src/type_lattice/sig_relations.rs) solves the same way
inside the order for a signature's keyworded members, so a signature type
holding a quantified member is under the weaker laws too. A signature has no
head parameters; it hides a type through an abstract `TYPE` member, which the
[view door](../../src/knot/module/README.md#the-view-door) binds by the name a
module declares. A name a `USING … SCOPE` surfaces is at most `Any`
([static types](../../src/dispatch/README.md#static-types)), and a keyworded
use of a surfaced member has no candidate, so the load types no use of a module
parameter's member.

**Acceptance criteria.**

- A name bound to a quantified `FN` — by `LET`, as a module member, or surfaced
  by `USING … SCOPE` — is read only at the head of a call. After
  `LET pick = (FN FOR ALL #[Elt] :{x :Elt} -> Elt = #(x))`, `LET keep = [pick]`
  refuses the load, and
  `LET keep = [(FN :{x :Number} -> Number = #(pick {x = x}))]` loads.
- A quantified `FN` written anywhere but a binder's right-hand side or the head
  of a call refuses the load.
- Each call of such a name solves its group from that call's arguments:
  `(pick {x = 1})` is `Number` and `(pick {x = "s"})` is `Str`. A quantified
  function calling itself by name in its own body loads.
- A callable's group keeps every variable it declares, so a call solves each
  one: under `EXPR FOR ALL #[Elt] #(KIND x :Elt) -> Type = #(Elt)`, `KIND 1`
  gives `Number`, by keyword and by name, and the load reads `Elt` in the body
  as a rigid variable. Two definitions that differ only in what they call their
  variables still share one type.
- A type expression `:(FN FOR ALL …)` or `:(EXPR FOR ALL …)` is refused where
  it is elaborated, except as the type of a `VAL` member in a `SIG`.
- `SIG Stack FOR ALL #{Elt: Any} = #[(EXPR #(PUSH _ :Elt) -> :(LIST OF Elt))]`
  elaborates, `Stack WITH {Elt = Number}` is its application, an application
  may leave a head parameter unpinned, and a `TYPE` member in a signature is
  refused where its shape is built.
- A keyworded member of a `SIG` may write its own `FOR ALL`, as
  `SIG Boxes = #[(EXPR FOR ALL #[Elt] #(BOX _ :Elt) -> :(LIST OF Elt))]` does,
  and a `VAL` member may be typed by a quantified function type, as
  `(VAL identity :(FN FOR ALL #[Item] :{x :Item} -> Item))` is.
- The order relates two signature types application by application and never
  solves: `Stack WITH {Elt = Number}` lies under `Stack`, two applications
  whose pins differ are unordered, and two quantified function types are
  ordered only where they are one handle.
- `(Stack WITH {Elt = Number}) & (Stack WITH {Elt = Str})` is a type holding
  both applications. It lies under each, and a module with a `PUSH` at
  `Number` and a `PUSH` at `Str` fits it.
- The lattice's property laws hold by handle over every generated type,
  signature types and quantified function types included, and no law has a
  twin stated up to equivalence.
- A module defining `EXPR FOR ALL #[Elt] #(BOX x :Elt) -> :(LIST OF Elt)` fits
  `Boxes`, and one defining only `EXPR #(BOX x :Number) -> :(LIST OF Number)`
  does not. A module defining only `PUSH` at `Number` fits `Stack` and
  `Stack WITH {Elt = Number}`, and not `Stack WITH {Elt = Str}`; one defining
  `PUSH` at `Number` and at `Str` fits all three.
- The view door checks a module against a signature by *fits*: under
  `SIG Counter FOR ALL #[Carrier] = #[(VAL zero :Carrier)]`, `ints :! Counter`
  binds the view's `Carrier` to what *fits* solves it to, and `ints :| Counter`
  mints a carrier for each parameter the application leaves unpinned.
- With `SIG Crates` declaring `Boxes`'s `BOX` member and a second member, an
  argument at most `Crates` is *always* at a slot `:Boxes`, and of two
  overloads at one key whose slots are `m :Boxes` and `m :Crates`, the second
  strictly outranks the first.
- In a `USING (m :! Boxes) SCOPE` body, each keyworded use of a surfaced member
  is judged at load against that member's head, solving the member's own
  variables afresh: `BOX 1` and `BOX "s"` both load through the one `m`, typed
  `LIST OF Number` and `LIST OF Str`.
- The verdict of each relation that solves is recorded by handle pair, so
  dispatch compares no slot types of its own.

**Directions.**

- *Call-only — decided.* A quantified function is called and never passed or
  stored, so its quantified type enters no other type. The rule is strict: the
  name stands only at the head of a call. To pass one, wrap it in an
  unquantified `FN`; metaprogramming generates such wrappers where a program
  needs many.
- *Every binding — decided.* Call-only covers each binding of a quantified
  `FN`: a `LET`, a module member, a name a `USING` surfaces.
- *Modules carry polymorphism — decided.* A slot that wants a function usable
  at several types, and a payload holding one, is typed by a signature and
  takes a module. The one spelled quantified function type left is a `VAL`
  member's in a `SIG`.
- *One spelling in a signature — decided.* A variable a member needs per use is
  written on the member. A head parameter is one type per module, and an
  application either pins it or leaves it unpinned. Position says which, so a
  per-use variable takes no pin.
- *Two relations — decided.* A construction — interning, a canonical union,
  `join`, `meet`, a cache key — reads the order, which never solves. A
  question — admission, a static verdict, ranking, a settled ascription, the
  return check, the view door — reads *fits*, which solves. *Fits* contains
  the order and is a preorder: two handles may fit each other, and nothing is
  built from it. It is today's `sig_subtype` and `admits_shape`, taken out of
  the order.
- *How* fits *reads a variable — decided.* On the offering side a member's own
  variable is solved afresh for each member, and on the asking side it is held
  rigid. An unpinned head parameter is one rigid unknown for the module on the
  offering side, and one variable, solved and discarded, on the asking side. A
  module's self-signature stands on the offering side, each definition's group
  solved as a call solves it; on the asking side it compares by handle.
- *A signature type — decided.* A set of applications, which may hold several
  of one signature. One set lies under another when each application of the
  upper lies above some application of the lower. The meet is the union of the
  two sets less each application that lies above another, so it is exact.
  Whether one declared signature's members include another's is a question for
  *fits*, never the order.
- *Canonical form — decided.* It goes: the order compares two quantified
  function types by handle, so canonical form has no job left, and dropping a
  variable reads it as its bound. A group is interned with every declared
  variable kept, numbered by first occurrence and then, for a variable no
  position names, in declared order; its digest feeds each bound. A variable no
  argument reaches binds its bound, as the
  [least instance](../../src/type_lattice/README.md#the-unifier-collects-it-does-not-bind)
  of the pair `[Never, bound]`.
- *What a member's variable may solve to — decided.* Anything, impredicatively.
  Instantiation-based containment is undecidable in general, so *fits* is what
  the collector answers: it terminates, and a relation it misses is refused at
  load and at run alike. A predicative solve would refuse `SAME boxing` under
  `EXPR FOR ALL #[Item] #(SAME x :Item) -> Item`, for any module `boxing` with
  a polymorphic member.
- *A head parameter's node — decided.* A named `Parameter { name, bound }`
  replaces `AbstractType`. A `Quantified` is positional within its innermost
  binder, and a head parameter must be read under a member's own `FOR ALL`. A
  parameter is substituted by name within its own signature, and `WITH` pins it
  by name. The `:|` mint is a `Parameter` carrying a nonce until
  [modules](modules.md) keys carriers on their root.
- *How* fits *solves a head parameter — decided.* It pools what the offered
  members contribute, as a call pools its arguments, and takes the least
  instance, per [modules](modules.md)' parameterized signatures. For each asked
  keyworded member one offered overload at its key contributes, each tried in
  turn, since pooling a key's overloads would make *fits* not transitive: a
  module with a `PUSH` at `Number` and a `PUSH` at `Str` fits the meet of
  `Stack`'s two pinned applications, which lies under `Stack`, so it fits
  `Stack` too. An offered member with a group of its own contributes nothing
  and is checked after the solve.
- *Ambiguity under a member head — decided.* *Fits* asks only that some offered
  overload satisfy each keyworded member, and a tie is an ambiguity where a
  call meets it. Refusing a tie would make *fits* not transitive: a module with
  `F` over `Number | Str` and over `Number | Bool` fits a signature asking `F`
  over `Number | Str`, which fits one asking `F` over `Number`, where the two
  overloads tie.
- *An opaque view of a quantified member — decided.* As the coercion walk reads
  any member: a member whose type names no head parameter is the root's own
  function, and one whose type names one is a barrier typed by the member's
  quantified type under the view's substitution. Calling through it is
  [modules](modules.md)'.
- *A keyworded hole filled from a list — decided.* `code USING m` keeps filling
  a keyworded hole from a module's registrations, and `EVAL` keeps offering a
  key's candidates as a list. Neither list is a name, and call-only governs
  names and written `FN`s, so either may hold a quantified registration.
- *What stays with modules — decided.* This item moves the view door onto head
  parameters and types a `USING … SCOPE` body's uses of a signature's member
  heads at load. [Modules](modules.md) evaluates an ascription, keys and
  unbounds carriers, binds a surfaced registration where the body runs, refuses
  `m.f` outside the head of a call, and solves a callee's variable through a
  pin such as `m :(Ordered WITH {Carrier = Elt})`.

## Dependencies

**Requires:** none.

**Unblocks:**

- [Modules](modules.md) — the signature types and the relation its doors read.
