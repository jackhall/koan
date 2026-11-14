# The binding graph

The build's intermediate program: what a program's text asks, grouped by where
each question's inputs arrive. A `BindingNode` is a block of that program. Its
parameters are the names bound at one place in the text, and its body is the
questions waiting on them. A `PassesName` edge is its call, an `AnswerBranch`
its branch, and a `BoundInstance` one block's result. The build executes the
program from its root as far as the text supplies content. A run cell
continues it wherever content arrives as the program runs, over the same
questions and the same instances. So a block boundary sits exactly where
content can arrive separately, and follows neither a body nor an expression.
The story across [the build](../src/build/README.md), which types the text,
[build storage](build-storage.md), which says what each tier holds, and
[gradual typing](gradual-typing.md), which says what an answer may claim.

## Vocabulary

| Name | Is |
|---|---|
| **bind site** | a place in the text where names receive their content: a body's entry, or a place inside a body where a name's type arrives as the program runs |
| **`BindingNode`** | one bind site in the graph: the names bound there and the `SiteQuestion`s it owns |
| **answered name** | a name whose type an answer gives, such as a dispatched call's result; it is bound at the `BindingNode` whose answer types it, and has no bind site of its own |
| **`SiteQuestion`** | what the text asks at one site and cannot answer alone: which candidate runs, which arm is taken, what type a retype stamps, whether a check holds |
| **`SiteAnswer`** | one `SiteQuestion` answered under a binding: the candidate, the arm, the type, settled, or a refusal |
| **`NameQuestion`** | one judgement about a bound name, or relating a few, with finitely many answers, asked while answering a `SiteQuestion`: does `Elem` fit `Number`? |
| **`BindsAfter`** edge | this bind site binds later than that one |
| **`PassesName`** edge | a call gives a callee's bound name its content: the caller's name itself, a type built over the caller's names, or content the text fixes |
| **`AnswersName`** edge | an answer gives an answered name its type: on the arm of the question that decides it, through a call, from the callee's result, or on a site, as a `MATCH` on a value gives the union of its arms |
| **`AnswerBranch`** edge | an answer chooses which `BindingNode`s follow |
| **`OpenCaller`** | a mark on a `BindingNode` whose callers the text cannot list: a callable that escapes, one offered to code an `EVAL` runs, a callee reached through a value |
| **`BoundInstance`** | a `BindingNode` with its names given content, holding a `SiteAnswer` for each `SiteQuestion` the node owns |
| **rigid** | the type of an answered name no answer types: a lexical variable bounded by its declared type, standing for whatever the run binds |
| **knot** | answers that wait on each other around a recursion; every name in one is its own rigid, unless the knot is a tail loop typed by the arms that end it |

The graph is two things kept apart. **Nodes, questions and edges are a
function of the text**: which `BindingNode` owns a `SiteQuestion` never
depends on a binding. **Instances and answers are made under bindings**: a
`BindingNode` has one `BoundInstance` per distinct binding that changes an
answer.

## One program

```text
EXPR #(SHOW n :Number) -> Str = #("number")
EXPR #(SHOW s :Str) -> Str = #("string")
EXPR FOR ALL #[Elem] #(REPORT x :Elem code :Any) -> Str = #(
  LET first = (SHOW x)
  LET built = (EVAL code -> Module)
  SHOW (built.sample)
)
PRINT (REPORT 5 #(MODULE made = (LET sample = "hi")))
```

- `REPORT 5 …` is selected by the text alone, so it is no question. It
  carries a `PassesName` edge giving `Elem` the content `Number`.
- `SHOW x` waits on `Elem`, so `REPORT`'s entry owns it. The build makes that
  instance under `Elem := Number` before anything runs.
- `built` receives its type from the `EVAL`, as the program runs. So
  `LET built = …` is a bind site, and it owns `SHOW (built.sample)`. The run
  cell that reaches it finds or makes its instance, and every module whose
  `sample` is a `Str` shares that instance.
- `first` is a `Str` by `SHOW`'s declaration, so `LET first = …` is no bind
  site.

`REPORT`'s body is split across two `BindingNode`s, and the build reaches the
first through a call without waiting for the second.

## Which bind site owns a question

A `SiteQuestion` reads names, and every name it reads is bound on the lexical
chain of the body it sits in. Its owner is the latest bind site among them,
reading an answered name as the bind sites its type depends on.

**What the text alone decides is no question.** A call whose candidate the
text selects, a check the text settles and a type the text fixes are laid
with the graph and held in no instance. A `SiteQuestion` exists only where
answering it asks a `NameQuestion`. The root `BindingNode` of a base binds no
name of a site. It owns only the questions that read nothing but what the
text's own calls return, such as `SHOW (packed.sample)` after a top-level
`LET packed = (PACK 5)`.

**Which binders are names.** A body's entry binds its parameters, its
`FOR ALL` names and its `:Type` parameters. Inside a body, a binder is a bind
site only where its type arrives as the program runs: an `EVAL`'s result, a
`USING`'s surfaced names, a module the run builds. A binder whose type an
answer decides, such as a call whose result an answer decides, is an
answered name, with a `LET` or without one. It is bound at the `BindingNode`
whose answer gives it its type, wherever its statement sits, so a `LET` is
transparent. A `LET` whose type the text writes over earlier names is no name
at all, and a question reading it reads the names behind it.

Along one chain the order is `BindsAfter`'s: an outer body's entry before an
inner body's, a body's entry before the bind sites inside it in order, and an
inner body's entry after each bind site whose name it captures.

So a `BindingNode` is not a body. An inner body's question that waits only on
an outer body's name is owned by the outer body's entry, and one body's
questions may be split across its entry and several bind sites inside it.
Every question still names its site and body, so the text's structure is kept
beside the grouping.

## What a bind site binds

A `BindingNode` binds names and never holds a value. A `NameQuestion` reads
one aspect of what a name is bound to:

- the solution of a `FOR ALL` name;
- the type a type value denotes;
- a module's content;
- which member of its declared union a named value carries.

A judgement relating two bound names, such as whether one fits the other, is
one `NameQuestion` reading both.

**Every `NameQuestion` has finitely many answers**: yes or no, an arm, a
kind, a member of a union. No answer is a type. A `SiteAnswer` that names a
type holds it written over its `BindingNode`'s names, and a frame writes its
own content in as it reads. So an instance names no type of any binding, and
a `BindingNode` has at most one instance per combination of answers, a number
the text fixes whatever the run binds.

A `BoundInstance` is found by those answers alone, so two bindings that
answer every `NameQuestion` alike share one instance, and the value itself
stays in its slot. A `BoundInstance` is not a frame: two calls with one
binding share an instance, and a frame reads the instances of every
`BindingNode` that owns a question in its body. Nor is it a copy of a body
per type: a body whose questions read nothing of a name has one instance
whatever the name is bound to.

**A small union is split where it is bound.** A name declared at a union
asks which member it carries, a `NameQuestion` with one answer per member, so
its bind site has an `AnswerBranch` per member and the frame picks its
instance by one test as it binds. A body is split while its combinations,
across all of its bind sites, number at most twelve; past that the members are told apart as the program
runs.

**`Any` marks what the run decides.** A name declared `Any` is bound to
nothing the build reads: no `NameQuestion` is asked of it, and a
`SiteQuestion` that waits on it is answered as the program runs. It is the
one place the text says so.

Stepping a statement at build time to keep its value is a separate
abstraction over this one
([partial evaluation](../roadmap/one-build/partial-evaluation.md)).

## Edges

**`BindsAfter`** orders the bind sites of one lexical chain.

**`PassesName`** is how an answer is reached through a call. It moves no
ownership: the callee's own bind site owns its questions. It says what the
callee's names are bound to at that call, so the build makes the callee's
`BoundInstance` when the caller's is made, without waiting for a frame. It
sits on the arm of the `SiteQuestion` that selects its call, or on the call's
site where the text selects the callee. It is followed where the last of its
inputs arrives: the names its mapping mentions, the reads of the question
that selects it, and the names its callee reads from its lexical chain. So a
closure called with fixed arguments still receives its encloser's names.
Under

```text
EXPR FOR ALL #[Elem] #(KEEP x :Elem) -> Elem = #(x)
EXPR FOR ALL #[Item] #(PASS y :Item) -> Item = #(KEEP y)
EXPR FOR ALL #[Item] #(WRAP y :Item) -> :(LIST OF Item) = #(KEEP [y])
PRINT (WRAP 5)
```

`PASS` gives `Elem` the name `Item` itself, `WRAP` gives it `LIST OF Item`,
and the last line gives `WRAP`'s `Item` the fixed content `Number`. An
instance may be made at a type still open over the caller's names: `KEEP`'s
instance at `LIST OF Item` answers whatever holds for every `Item`, and a
refusal that holds for every `Item` is found there.

**`AnswersName`** is how a type reaches an answered name. A caller reads its
callee's result through the name its statement binds, and never through the
callee's instance:

- **The type a call returns** is the callee's declared return type with the
  call's mapping written in: `-> Elem` under `Elem := Number` is `Number`.
- **The module a functor returns** is its last statement read as a
  description with the parameters left open, the call's arguments written in.
- **A result that waits on a question inside the callee**, such as a last
  statement selecting between two functors by `Elem`, is an answered name of
  the callee, typed on that question's arms. An `AnswersName` edge through the
  call gives the caller's name the callee's result when the callee's instance
  is made. Results chain this way across calls, one edge per call.
- **The type a `MATCH` on a value gives** is the union of its arms' types. An
  `AnswersName` edge on the `MATCH`'s site, with no question, gives an
  answered name that union, written over the names the arms' types are.

The text fixes the first two, so the caller's `LET` is no name. A callee the
text cannot name leaves the name to the run. A result that waits on the run,
or on itself around a recursion, is a **rigid**: a type the run binds,
bounded by what its declaration says. In a knot, where answers wait on each
other around a recursion, every name is its own rigid, unless every step of
the knot is a result taken through a call or one arm of a `MATCH`. Such a tail
loop carries one type through every level, and every name in it is the union
of the arms that end it. Under

```text
EXPR #(BOX n :Number) -> Module = #(MODULE made = (LET sample = 1))
EXPR #(BOX s :Str) -> Module = #(MODULE made = (LET sample = "hi"))
EXPR FOR ALL #[Elem] #(SPIN x :Elem n :Number) -> Module = #(
  MATCH n WITH #{0: (BOX x), Number: (SPIN x (n - 1))}
)
PRINT ((SPIN 5 3).sample)
```

`SPIN`'s result is `BOX x`'s module at every level, so `sample` is `Number`
at the build. A tail loop with no arm that ends it keeps its rigids
([the crate's knots](../bindgraph/README.md#instances)). The same edge carries
an earlier answer of one body to a later question of it: an arm that decides
a type binds the name later questions read, and no question reads another's
answer. Only a type crosses the edge, never a value.

**`AnswerBranch`** records that an answer changes which questions exist
after it: which callee a call keeps, and so which body's bind sites follow;
whether a binder holds a module, and so whether its content is a name later
questions wait on; which member of a union a name carries. Every branch the
text allows is laid with the graph, each candidate a call could select among
them, and an answer takes one, or none while it is undecided. The graph's
control flow is over bind sites and answers, never over statements.

**`OpenCaller`** marks where the graph ends. What arrives there is not in the
text, so its `BoundInstance`s are made as the program runs.

## Instances

**Made by evaluating stored questions.** The load types the text once and
stores each `SiteQuestion` with what answering it needs: the types it judges,
written over names, the `NameQuestion`s it asks, and its arms. An instance is
made by asking those `NameQuestion`s of a binding. No statement is typed
again under a binding, at build or as the program runs.

**Made from the root.** The load makes the instances its text reaches from
the root: it follows each edge of a call the text selects, then each arm an
answer takes, and gives each answered name its type as its answer is made.
An answer left undecided follows no arm. A `BindingNode` no such chain
reaches has no instance until a run cell binds it.

**Found by view.** An instance's **admission** is every `NameQuestion` its
answers asked, with the answer each took. The crate files each instance under
its `BindingNode` and its **view**: the digests of the content the names that
`BindingNode` reads are bound to. A run cell reaching a bind site reads the
binding off its slots, and takes the instance by one lookup where its view is
filed. Otherwise it makes the instance by the same evaluation the load runs.
A view whose every content the load names is kept for the agent's life. Any
other rests in a small table of its `BindingNode`'s own, where a newer view
may take its place, so the views a program adds as it runs stay bounded.

**Held by handle.** A frame holds one handle per `BindingNode` that owns a
question in its body, taken as it reaches that bind site, and reaches its
enclosers' handles through its closure. A handle is its instance's admission:
one small field per `NameQuestion` its `BindingNode`'s questions can ask,
holding the answer taken or that it was not asked. The load sizes it per
`BindingNode`. It names no storage, so nothing a frame holds can dangle.

**Dropped and rebuilt.** Instances rest inside the crate. Those the load
makes are frozen with the load, shared by every agent and never freed. Those
a run cell makes rest in one bump its agent lends, under a budget. Past the
budget nothing more rests until the agent resets the bump, dropping them all
at once ([where answers rest](../bindgraph/README.md#instances)). An instance is a function of its
`BindingNode` and its admission, so a read through a handle whose answers
were dropped evaluates the stored questions again, served the handle's own
answers, and no reader can tell. Nothing of the build rests in a run region
but a frame's handles, and the crate keeps no type of a binding: a view holds
digests, and an answered name's type lives for one evaluation. A type with a
frame's content written in is a lattice type, and rests in
[the lattice's tiers](type-tiers.md).

**One graph per base.** The program has one graph, laid at the load. Code
composed as the program runs is a base of its own
([where facts are decided](../src/build/README.md#where-facts-are-decided)),
with a graph laid when it is composed, resting in the composition and dropped
with it.

## Refusal

A refusal is a `SiteAnswer`, found in the `BoundInstance` of the bind site
that owns its question: where the last of its inputs arrives, and no later.
A refusal in an instance the build makes refuses the build.

**The build refuses what it cannot bound.** Where the build can see that a
piece of text asks for instances without limit, it refuses the text and names
the declaration that would bound them; it does not leave them to the run.
Nothing else limits how many instances a body is given. What the text cannot
show — an `OpenCaller`'s callers, a name bound by a value the run builds, a
name declared `Any` — is answered as the program runs.

**A recursion that grows its type.** Around a closed walk of `PassesName`
and `AnswersName` edges the mappings compose. A result returns only to the
call that asked for it, so a callee called from two places joins no two
callers. Where the composition gives a
`FOR ALL` name a type built over itself, each turn binds the name one level
deeper, and the walk has no last `BoundInstance`:

```text
EXPR FOR ALL #[Item] #(DEEPEN y :Item) -> Any = #(DEEPEN [y])
```

The walk need not be a simple cycle. Under

```text
EXPR FOR ALL #[Left Right] #(MIX a :Left b :Right) -> Any = #(
  LET swapped = (MIX b a)
  MIX a [a]
)
```

each call alone repeats, the first after two turns and the second after one,
but the swap followed by the wrap binds `Right` to `LIST OF Right`. The walks
run over every call the text lays, whichever candidate a binding would select,
so a growing walk is found before anything is bound. A recursion that dispatch
alone would stop at a deeper type is refused too.

The build refuses such a walk, names the `FOR ALL` name, and asks for a
declared recursive type. A parameter of that type is one type at every depth,
and one instance serves the whole recursion. A walk that only passes names on,
renames them, or passes fixed content somewhere around it repeats after
finitely many turns, and is built. A call through a value lays no edge, so a
cycle that closes through an `OpenCaller`'s callers is not in the graph, and
is answered as the program runs.

## The crate

The graph is its own crate, [`bindgraph`](../bindgraph/README.md), beside
[cellgraph](../cellgraph/README.md) and [lattice](../lattice/README.md). It
names nothing of koan. It is generic over a **fact domain**, which says:

- what a `SiteAnswer` and a `NameQuestion` are, how many answers a
  `NameQuestion` has, and which one a binding gives;
- a content's digest, and whether the load names it;
- how the term a `PassesName` mapping or an `AnswersName` edge carries is
  applied to a binding;
- which names a term mentions, each bare or inside a type it builds;
- the rigid type of an answered name no answer types.

The embedder also supplies the function that answers one `SiteQuestion` under
a binding, asking only the `NameQuestion`s the text declares for it. Koan's
domain is static types and verdicts over the
[type lattice](../lattice/src/types/README.md), and its function evaluates
one stored question.

The crate owns which bind site owns a question and binds an answered name,
the edges, the knots, the refusal of a walk that grows, and how a
`BoundInstance` is found, made, dropped and rebuilt. It lays what it keeps
through `bumpstorage`, the bump tier the lattice's tables rest on too, and
depends on nothing else. That an answer only narrows as more is bound is
koan's law ([properties](../src/build/README.md#properties)), held by its
rules over the lattice. The crate does not hold it.

Its laws are property tests over generated graphs and bindings. The oracle
is a reference that evaluates the same graph from scratch.

- **One owner.** Every `SiteQuestion` is owned by exactly one `BindingNode`,
  whatever is bound.
- **Bound equals declared.** A `BoundInstance`'s answers equal the oracle's.
- **Order does not matter.** Binding the sites of a chain one at a time, or
  all at once, reaches the same answers.
- **A passed name binds alike.** An instance made through a `PassesName` or
  an `AnswersName` edge equals its `BindingNode`'s own under the binding the
  edge gives.
- **Reuse is exact.** Two bindings share a handle exactly where their
  admissions are equal, and the instance a handle reads equals one made from
  scratch for either.
- **Rebuilt is exact.** A read through a handle equals the read with nothing
  dropped, whatever the budget.
- **Finite.** A graph in which every name is bound through the text — no
  `OpenCaller`, no name the run binds — reaches finitely many bindings, or is
  refused. A `BindingNode` has finitely many `BoundInstance`s whatever is
  bound.
- **Start anywhere.** An evaluation started at any `BindingNode` and binding
  that another start reached gives the same answers, knots included.
- **A repeated call walks nothing.** Asking again for an instance whose view
  is filed walks no edge.

## In the literature

| Koan | Known as | Source |
|---|---|---|
| A question owned by the bind site where its last input arrives | binding-time analysis | Jones, Gomard and Sestoft, *Partial evaluation and automatic program generation* (1993) |
| Bind sites in a partial order, not two stages | multi-level binding-time analysis | Glück and Jørgensen, *Efficient multi-level generating extensions for program specialization* (1995) |
| A recursion that grows its type, refused | polymorphic recursion, whose inference is undecidable and whose instantiation is unbounded | Henglein, *Type inference with polymorphic recursion* (1993) |

## Open work

- [Every typing rule states its question](../roadmap/one-build/typing-rules-state-their-questions.md)
  — the questions the load stores.
- [The build over the binding graph](../roadmap/one-build/build-over-the-binding-graph.md)
  — koan's load laying it, and the build and the run cells executing it.
- [An exhaustive check](../roadmap/one-build/exhaustive-check.md) — an
  opt-in load that also starts from every entry.
- [A small union split where it is bound](../roadmap/one-build/small-unions-split.md)
- [The run reads site answers](../roadmap/one-build/run-reads-site-answers.md)
  — the run performing a `SiteAnswer` where it now reads a node's syntax.
- [A recursion that grows its type is refused](../roadmap/types/growing-recursion-refused.md)
- [Partial evaluation of the top level](../roadmap/one-build/partial-evaluation.md)
  — values decided at build, over this graph.
- [Rigids that name their knot](../bindgraph/roadmap/knot-rigids.md) — the
  names a knot of takes makes equal, judged equal.
