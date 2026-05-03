# Roadmap

Open structural items that don't fit in a single PR. Each section names the problem, why it
matters, and possible directions — not a fixed design.

The order matters. Sequencing is purely about technical and design dependencies — Koan has
no users yet, so backward-compatibility costs play no role. The cost being optimized is
engineering rework: doing one item before another it depends on means doing the dependent
item twice.

Shipped items live in [DECISIONS.md](DECISIONS.md). What's shipped so far: user-defined
functions, the dispatch-as-node scheduler refactor, first-cut tail-call optimization, the
leak fix (with lexical closures + per-call arenas), structured error propagation, and the
user-defined-types substrate (return-type enforcement at runtime). The next signature
revision after error handling lands monadic side-effect capture; user-declarable types and
traits unlock the items downstream (group-based operators, the IF-THEN→MATCH deprecation's
Bool design call), so they sit in the middle of the sequence rather than last.

## Transient-node reclamation

**Problem.** TCO's slot reuse covers only the outermost user-fn frame.
[`Scheduler`](src/execute/scheduler.rs)'s `nodes`/`results` vecs still grow per iteration
whenever a body-internal sub-expression spawns a sub-`Dispatch`/`Bind`. Realistic recursion
(the predicate computation in an `IF`-guarded base case, or a recursive call's argument
expressions) accumulates entries. The `frame_holding_slots` sidecar added during the leak
fix is one piece of the substrate, but full transient-node reclamation — detecting that a
`Bind`/`Aggregate` and all its sub-`Dispatch`es are no longer reachable and reclaiming
their vec slots — is unbuilt.

**Impact.** This gates true O(1) tail-recursive memory. Factorial, list walk, and similar
patterns run in O(n) scheduler memory until it lands. It's the load-bearing remaining
problem from the leak fix.

**Sequencing.** Deserves its own roadmap entry once the surrounding `BuiltinFn` signature
settles — the next pass for monadic effects revisits that signature, and folding
reclamation into the same pass keeps the rewrite cheap.

## Open issues from the leak-fix audit

Most leak-fix follow-ups landed (see [DECISIONS.md](DECISIONS.md)). Two remain:

- **Miri hasn't run.** `CallArena::new`'s heap-pin + lifetime-erasure transmutes match the
  existing `RuntimeArena::alloc_*` pattern, but neither has been validated under Miri. The
  closure-escape paths in particular cross several lifetime-erased boundaries; Miri is the
  cheapest way to prove the unsafe blocks are settled.

- **KFuture conservative anchoring leaves room for tightening.**
  [`lift_kobject`](src/execute/lift.rs)'s KFuture arm attaches the dying-frame Rc
  unconditionally because we don't track which arena each of `KFuture.bundle.args` and
  `KFuture.parsed.parts` came from. With per-descendant arena provenance this could become
  a `needs_lift`-style targeted attach. Non-issue today (KFutures don't escape as values),
  but worth revisiting alongside the async-features work that will make KFutures escape.

## Generalize `Scope::out` into monadic side-effect capture

**Problem.** [`Scope::out`](src/dispatch/scope.rs) is a `Box<dyn Write>` sink that exists
solely so [`PRINT`](src/dispatch/builtins/print.rs) has somewhere to send bytes and tests
can swap stdout for a buffer. It is the only side-effect channel the runtime has, and it is
hard-coded to one channel and one shape (write bytes). Every additional effect Koan
eventually wants to support — file IO, time, randomness, network, environment access, even
error reporting — would either grow `Scope` by another ad-hoc `Box<dyn ...>` field or get
baked into `std::io` calls inside individual builtins.

Meanwhile the [`Monadic`](src/dispatch/ktraits.rs) trait already exists, with `pure` +
`bind` over a `Wrap<T>` GAT, and its doc comment says it is "intended as the abstraction
Koan's deferred-task and error-handling combinators will share once they're fleshed out."
Today it is implemented only for `Option` and threaded through nothing in the runtime. It
is scaffolding without a building.

**Impact.**

- *No effect inspection.* Tests can capture `PRINT` output by swapping the writer, but
  there is no equivalent for any other effect a builtin might want to perform. Each new
  effect requires its own bespoke testing seam.
- *No mocking or replay.* A program's behavior is whatever the host system decides at the
  moment of the call. Deterministic replay of a Koan program (feed it a recorded effect
  trace, get the same output) is impossible without a uniform effect channel.
- *No pure/effectful boundary.* The language has no way to know whether an expression is
  referentially transparent. Optimizations the scheduler could make (memoization,
  reordering, parallelism) are unsafe by default because any builtin might secretly write
  to a file or read the clock.
- *Effect ordering is implicit.* Today, effects happen in whatever order the scheduler
  runs builtins. There is no declarative "this expression's effect is X, sequenced after
  Y" — it is all operational.

**Directions.** None of these are decided.

- *Effect type.* Probably an enum: `Effect::Output(Vec<u8>)`, `Effect::Read(handle)`,
  `Effect::Now`, `Effect::Random`, plus a catch-all for builtins to declare custom
  effects. Open question: enumerated (closed set, easy to handle exhaustively) vs
  trait-object (`Box<dyn Effect>`, extensible by user code if/when user-defined functions
  can declare their own effects).
- *Carrier shape.* `BuiltinFn` returns not a bare `&'a KObject<'a>` (or `Result<...>`
  after the error-handling item) but an `Effectful<T>` carrier — a value paired with a
  list of pending effects. `Effectful` implements `Monadic`: `pure(v)` is `(v, [])`,
  `bind` concatenates effect lists. This is the long-promised second `Monadic` impl the
  trait's doc comment is waiting for.
- *Handler in `Scope`.* `Scope::out` becomes `Scope::handler: Box<dyn EffectHandler>`. The
  handler decides what to do with each `Effect` as the interpreter drains them: a default
  handler actually performs them (write to stdout, read the clock); a test handler
  captures them into a vec; a replay handler feeds results from a pre-recorded trace.
- *Drainage points.* Effects can either be performed eagerly (handler runs them as each
  builtin returns) or lazily (collected up the tree and run in batches at top-level
  expression boundaries). Eager is simpler and matches today's behavior; lazy unlocks
  reordering and is closer to the "monad transformer stack" shape this is converging on.
  Pick one explicitly rather than letting it emerge.

**Sequencing.** `BodyResult` already absorbed one revision (`Value | Tail` for TCO); the
error item added a second (`Err` arm) and this one adds a third (`Effectful<...>`). Three
churning passes over every builtin in [builtins/](src/dispatch/builtins/) is meaningfully
worse than one. Unless the effect story sharpens enough to fold into the same pass as
ownership and errors, this should land last and accept that the prior two items are
stepping stones rather than end states.

## User-declarable types and traits

The substrate landed (see [DECISIONS.md](DECISIONS.md)). What's still open is the
user-facing surface.

**Problem.** [`KType`](src/dispatch/kfunction.rs) remains a *closed* enum — users still
can't declare a record, a tagged union, or a trait. Its doc comment already flags the
limitation: *"In the future this should not assume all types can be enumerated; the user
should be able to define duck types."* [`KObject::UserDefined`](src/dispatch/kobject.rs)
is still a unit-variant placeholder. Argument types in user-fn signatures are also still
uniformly `Any` — per-param annotations are the natural next surface extension and reuse
the parser's new `Type` token class.

**Impact.**

- *User functions can only operate on built-in types.* Now that user-defined functions
  exist, the language can express a function over `Number` but not over a `Point` —
  `Point` has no surface syntax because user types don't exist. The function feature is
  operationally usable but stuck at scalars and the built-in `List`/`Dict`. There is no
  path from "the language has a function abstraction" to "the language has a record
  abstraction the function can operate on."
- *No abstraction over types.* Writing a function over "anything that can be iterated" or
  "anything that can be compared" requires a trait or contract — Koan has no way to
  express either. The host-side [`ktraits.rs`](src/dispatch/ktraits.rs) (`Parseable`,
  `Iterable`, `Monadic`, etc.) gives the runtime its own vocabulary; user code is denied
  the analog and has to write per-concrete-type variants of every function.
- *Dispatch priority is built on the wrong model if types land later.* With seven host
  types, signature specificity is a tiny finite-set comparison. With user types,
  specificity becomes a partial order over a lattice that grows as user code grows —
  subtyping, trait satisfaction, and structural matching each want different specificity
  rules. A priority comparator designed for the closed-enum case is not the same
  comparator needed for the open-lattice case.

**Directions.** None decided.

- *Type representation.* Move `KType` from a closed enum to an extensible structure.
  Either add a `KType::User(TypeId)` variant alongside the existing host types and keep a
  `Scope`-level registry of definitions, or replace the enum entirely with a trait-object
  that host types and user types both implement uniformly. The first is incremental; the
  second is cleaner but a bigger refactor.
- *Surface syntax.* Type definitions and trait definitions are themselves builtins —
  likely `TYPE Point = STRUCT x:Number y:Number` and `TRAIT Iterable = ...` shapes.
  Mechanically these are `KFunction`s with fixed signatures, so the surface design echoes
  (and likely shares machinery with) the FN signature work in the user-functions item.
- *Traits.* A trait is a named bag of operation signatures that a type can claim to
  implement. Functions accept a trait-typed parameter and dispatch over any concrete type
  satisfying it. The dispatch machinery sees a trait the same way it sees a parent type
  in a subtyping hierarchy — a less-specific match that concrete types beat. The priority
  rules need a "concrete > trait > `Any`" hierarchy reserved in their design even if
  traits don't ship in the first cut.
- *Wiring up `KObject::UserDefined`.* The placeholder variant becomes something like
  `UserDefined(TypeId, HashMap<String, KObject>)` — a tagged record carrying a type
  identifier and field values. Other `KObject` variants stay as-is; user types are an
  addition, not a replacement.

## Deprecate IF-THEN in favor of MATCH

**Problem.** [MATCH](src/dispatch/builtins/match_case.rs) is strictly more expressive than
[IF-THEN](src/dispatch/builtins/if_then.rs): `IF cond THEN value` is equivalent to
`MATCH cond CASE true: value`. Keeping both gives the user two equivalent constructs to
learn, keeps `if_then.rs` alive as the lone consumer of the parser's lazy-slot machinery
(`lazy_candidate` is invoked nowhere else), and forces every future branching feature
(pattern bindings, exhaustive-case checks) to be specified twice.

**Directions.** The load-bearing design call is the runtime representation of `Bool`.

- *Special-case MATCH on Bool.* Keep `KObject::Bool(bool)` as a primitive and teach MATCH
  that `true`/`false` are valid case labels for it. IF-THEN desugars to `MATCH cond CASE
  true: value` either at parse time or as a thin shim builtin. Smallest change.
- *Promote Bool to a tagged union.* `true` and `false` become the two variants of a
  built-in tagged union; MATCH dispatches over them via the same machinery as user tagged
  unions. Cleaner uniformly but changes Bool's representation
  (`KObject::Bool(bool)` → `KObject::Tagged { tag: "true"|"false", value: Null }`),
  affects every type-checking call site, and costs one `Rc` per Bool value. Worth doing
  only if other primitives are heading the same way (a bigger language-design question).
- *Hybrid.* Keep `KObject::Bool(bool)` in storage; project to a synthetic tagged union
  when MATCH consumes one. Compromise — keeps the cheap representation while letting
  MATCH treat Bool uniformly with user tagged unions.

**Sequencing.** Lands cleanest after user-defined types/tagged-unions hardens, when "is
Bool a tagged union" is answerable in context. Mechanically the deprecation itself
(delete `if_then.rs`, register the desugaring, remove the lazy-slot path if nothing else
needs it) is a one-PR cleanup once the Bool question is settled.

## Group-based operators

**Problem.** Operators like `+`/`-` (additive group over Number), `*`/`/` (multiplicative
group over Rationals), and `/`/`..` (path-join + parent-dir over filesystem paths) form
*mathematical groups* — paired binary ops with an identity and an inverse. Today each
operator is a flat builtin registered independently; the language has no concept that
`+` and `-` come as a pair, that `Path` could declare its own group under different
operators, or that a function over "anything that forms a group" could be written
generically. Every new operator-bearing type duplicates registration and re-derives
dispatch correctness in the user's head.

**Directions.**

- *Group as a trait.* On top of the user-defined-traits substrate, a `Group<T>` trait
  declares the binary op, its inverse, and an identity. Registering `Number` as
  `Group<Number>` under `+`/`-` is one trait impl; registering `Path` as `Group<Path>`
  under `/`/`..` is another. Operator dispatch consults the trait when no concrete
  overload matches. Most expressive option.
- *Group as a syntax-level shorthand.* `GROUP + - OVER Number` (or similar) registers
  both operators and links them in one declaration, without depending on the trait
  machinery. Less powerful — no generic-over-groups functions — but unblocks "this type
  wants a paired operator" without traits.
- *Group laws.* Math groups have axioms (associativity, identity, inverse). The language
  can either trust the declaration (cheap, possibly wrong) or sample-test it (expensive,
  partial). Trusting is fine if violations only produce wrong answers, not crashes —
  which is the case for a dispatch-only mechanism.
- *Parser surface.* [operators.rs](src/parse/operators.rs)'s registry is flat today.
  Group declarations would either feed it at runtime (slot allocation deferred to
  dispatch) or extend a compile-time table (structural, rigid). User-definable groups
  force the runtime path.

**Sequencing.** Depends on user-defined types and traits. Without traits, the syntax-level
shorthand still works but doesn't unlock the generic-function-over-groups payoff. Land
alongside or after the trait machinery.

## Other deferred surface items

Smaller pieces called out in passing as the larger items shipped:

- **Errors as first-class values.** `KObject::Err` would let errors bind via `LET` and pass
  as args. Needs the dispatcher to either short-circuit through error-typed slots or
  splice errors into them.
- **Catch-builtins** (`MATCH`, `OR_ELSE`-style). Likely require either a `KType::Result`
  extension or an `Argument.catches_errors` flag, which intersects with the user-defined-
  types work above.
- **`RAISE "msg"` builtin** to produce `KError::User` from in-language code.
- **Source spans on `KExpression`** so error frames can name `file:line` instead of
  textual summaries.
- **Continue-on-error after the first top-level failure** (useful for a future REPL).
- **Variadic argument signatures** — original "function body is a sequence of expressions"
  sketch; the comparator's tiebreak rule for variadic-vs-fixed signatures is the
  load-bearing question.
- **Per-parameter type annotations** on user-fn signatures, which today are uniformly
  `Any`. Reuses the parser's existing `Type` token class.

## Refactor for cleaner abstractions

**Standing item, exploratory.** The other roadmap entries add features; this one's job is
to *remove* — places where the abstraction grew accidentally and a generalization has
become visible. Examples worth a look when surrounding code next changes for unrelated
reasons:

- *Builtin registration patterns.* The `register_builtin` + signature-construction
  skeleton repeats across [builtins/](src/dispatch/builtins/). Whether the duplication is
  noise to factor or "deliberate so each builtin reads top-to-bottom on its own" is an
  open call — the answer depends on how builtins evolve under monadic effects and
  user-defined types.
- *Parser pass boundaries.* [parse/](src/parse/)'s passes pipe strings between each
  other (`quotes.rs` → `whitespace.rs` → `expression_tree.rs`). Typed outputs would
  compose more cleanly. Low priority — current pipeline works.

**When to act.** Refactor each only when the next feature would multiply the existing
duplication. Don't refactor preemptively; the cost of churn outweighs the cost of
carrying a small duplication that hasn't grown teeth yet.
