# The binding graph

What a program's text asks, grouped by where each question's inputs arrive.
The build stores one graph of bind sites, the questions each owns and the
edges between them, and every later reader — the keys, the trace, the image,
the run — reads that graph instead of deriving its own piece of it. The story
across [the build](../src/build/README.md), which answers each question,
[build storage](build-storage.md), which says where an answer rests, and
[gradual typing](gradual-typing.md), which says what an answer may claim.

## Vocabulary

| Name | Is |
|---|---|
| **bind site** | a place in the text where names receive their content: a body's entry, or a statement ([frontiers and seams](../src/build/README.md#frontiers-and-seams)) |
| **`BindingNode`** | one bind site in the graph: the names bound there and the `SiteQuestion`s it owns |
| **`SiteQuestion`** | what the text asks at one site: which candidate runs, which arm is taken, what type a retype stamps, whether a check holds |
| **`SiteAnswer`** | one `SiteQuestion` answered under a binding: the candidate, the arm, the type, settled, or a refusal |
| **`NameQuestion`** | one judgement about one bound name, asked while answering a `SiteQuestion`: does `Elem` fit `Number`? |
| **`BindsAfter`** edge | this bind site binds later than that one |
| **`PassesName`** edge | a call site gives a callee's bound name its content: the caller's name itself, a type built over the caller's names, or content the text fixes |
| **`AnswerBranch`** edge | an answer chooses which `BindingNode`s follow |
| **`OpenCaller`** | a mark on a `BindingNode` whose callers the text cannot list: a callable that escapes, one offered to code an `EVAL` runs, a callee reached through a value |
| **`BoundInstance`** | a `BindingNode` with its names given content, holding a `SiteAnswer` for each `SiteQuestion` the node owns |

The graph is two things kept apart. **Nodes, questions and edges are a
function of the text**: which `BindingNode` owns a `SiteQuestion` never
depends on a binding. **Instances and answers are made under bindings**: a
`BindingNode` has one `BoundInstance` per distinct binding that changes an
answer.

## Which bind site owns a question

A `SiteQuestion` reads names, and every name it reads is bound on the lexical
chain of the body it sits in. Its owner is the latest bind site among them. A
question that reads no bound name is owned by the root `BindingNode` of its
base and answered once.

Along one chain the order is `BindsAfter`'s: an outer body's entry before an
inner body's, a body's entry before its statements in order, and an inner
body's entry after each statement whose name it captures. A statement is a
bind site only where a question waits on a name it binds.

So a `BindingNode` is not a body. An inner body's question that waits only on
an outer body's name is owned by the outer body's entry, and one body's
questions may be split across its entry and several of its statements. Every
question still names its site and body, so the text's structure is kept
beside the grouping.

## What a bind site binds

A `BindingNode` binds names and never holds a value. A `NameQuestion` reads
one aspect of what a name is bound to:

- the solution of a `FOR ALL` name;
- the type a type value denotes;
- a module's content;
- which member of its declared union a named value carries.

A `BoundInstance` is found by those aspects alone, so two bindings that
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

**`BindsAfter`** orders the bind sites of one lexical chain. A
`BoundInstance` rests on the instance of each bind site before it.

**`PassesName`** is how an answer is reached through a call. It moves no
ownership: the callee's own bind site owns its questions. It says what the
callee's names are bound to at that call, so the build makes the callee's
`BoundInstance` when the caller's is made, without waiting for a frame. Under

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

**`AnswerBranch`** records that an answer changes which questions exist
after it: which callee a call keeps, and so which body's bind sites follow;
whether a binder holds a module, and so whether its content is a name later
questions wait on; which member of a union a name carries. The graph's
control flow is over bind sites and answers, never over statements.

**`OpenCaller`** marks where the graph ends. What arrives there is not in the
text, so its `BoundInstance`s are made as the program runs.

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

**A recursion that grows its type.** A recursive group is a set of bodies
that call each other, over the calls the build keeps
([recursion](../src/build/README.md#recursion)). Around a cycle of
`PassesName` edges inside one, the mappings compose; where the composition
gives a `FOR ALL` name a type built over itself, each turn binds the name one
level deeper and the cycle has no last `BoundInstance`:

```text
EXPR FOR ALL #[Item] #(DEEPEN y :Item) -> Any = #(DEEPEN [y])
```

The build refuses it and asks for a declared recursive type; a parameter of
that type is one type at every depth, and one instance serves the whole
recursion. A cycle that only passes names on, renames them, or passes fixed
content repeats after finitely many turns and is built. A cycle through an
`OpenCaller` is one the text does not show, and is answered as the program
runs.

## The crate

The graph is its own crate, `bindgraph`, beside
[cellgraph](../cellgraph/README.md) and [lattice](../lattice/README.md), and
names nothing of koan. It is generic over a **fact domain**: what a
`SiteAnswer` and a `NameQuestion` are, when a binding keeps a
`NameQuestion`'s answer, how a `PassesName` mapping is applied to a binding
and whether a composed mapping grows, and the order under which an answer
only narrows. The embedder also supplies the function that answers one
`SiteQuestion` under a binding and reports the `NameQuestion`s it asked.
Koan's domain is static types and verdicts over the
[type lattice](../lattice/src/types/README.md), and its function is
[the one walk](../src/build/README.md#the-one-walk)'s per unit. The crate
owns which bind site owns a question, the edges, and how a `BoundInstance`
is found, made and reused.

Its laws are property tests over generated graphs and bindings. The oracle
is the same graph answered from scratch with every name bound at once.

- **One owner.** Every `SiteQuestion` is owned by exactly one `BindingNode`,
  whatever is bound.
- **Bound equals declared.** A `BoundInstance`'s answers equal the oracle's.
- **Order does not matter.** Binding the sites of a chain one at a time, or
  all at once, reaches the same answers.
- **A passed name binds alike.** An instance made through a `PassesName`
  edge equals the callee's own under the mapped binding.
- **Reuse is exact.** A `BoundInstance` found for a binding equals one made
  from scratch for it.
- **Finite.** A graph in which every name is bound through the text — no
  `OpenCaller`, no name the run binds — has finitely many `BoundInstance`s,
  or is refused.

## In the literature

| Koan | Known as | Source |
|---|---|---|
| A question owned by the bind site where its last input arrives | binding-time analysis | Jones, Gomard and Sestoft, *Partial evaluation and automatic program generation* (1993) |
| Bind sites in a partial order, not two stages | multi-level binding-time analysis | Glück and Jørgensen, *Efficient multi-level generating extensions for program specialization* (1995) |
| A recursion that grows its type, refused | polymorphic recursion, whose inference is undecidable and whose instantiation is unbounded | Henglein, *Type inference with polymorphic recursion* (1993) |

## Open work

- [The binding graph crate](../roadmap/one-build/binding-graph-crate.md) —
  the crate and its laws, beside the build.
- [The build over the binding graph](../roadmap/one-build/build-over-the-binding-graph.md)
  — koan's build storing and reading it.
- [A small union split where it is bound](../roadmap/one-build/small-unions-split.md)
- [The run reads site answers](../roadmap/one-build/run-reads-site-answers.md)
  — the run performing a `SiteAnswer` where it now reads a node's syntax.
- [A recursion that grows its type is refused](../roadmap/types/growing-recursion-refused.md)
- [Partial evaluation of the top level](../roadmap/one-build/partial-evaluation.md)
  — values decided at build, over this graph.
