# bindgraph

A graph of **bind sites** over a description of a program's text. Each site is a
place where names receive their content. The graph records which site owns
each question the text asks. It also records how a call passes names on to a
callee, how an answer or a term over other names gives a name its content, and
the **bound instances** that hold a site's answers under a binding. It is
generic over a **fact domain** and knows nothing of what an answer means.

The crate names no type from its embedders. The dependency direction is
embedder → `bindgraph`, compile-enforced, and its one dependency is
`bumpstorage`, the bump tier it lays what it keeps through.
[koan](../README.md) is its
embedder: koan's reading of the graph — what a question, a binding and an
answer are for its static types — is
[the binding graph](../design/binding-graph.md).

## Three layers

- **Text.** The embedder describes the text through a builder:
  - bodies and how they nest;
  - each body's entry and its statements, in order;
  - the **site names** bound at each site;
  - the **answered names**, which take their content from an answer and have
    no site of their own;
  - the questions at each site, with the names each one reads and the asks
    its answer may make;
  - the arms each question's answer can take;
  - the `PassesName` edges, or **calls**, into a callee's entry: each on an
    arm, or on a site where the text selects the callee outright;
  - the **onward calls**, each on a statement's site into that statement's
    own `BindingNode`, giving its site names terms over names bound before
    it: the content an answer gives a value landing there;
  - the `AnswersName` edges, each giving one answered name its content: on an
    arm, through a call from one of the callee's names, or on a site, by a
    term over other names with no question;
  - the bodies whose callers the text cannot list (`OpenCaller`);
  - the **staged** bodies, which a frame enters only where the program runs
    them later, such as a quote's code.
- **Graph.** Laying the text gives an immutable graph. It computes each name's
  base and orders the sites. It binds each answered name, picks each
  question's and call's owner, checks the description, refuses a recursion
  that grows, and finds where a knot can form. It never reads a binding.
- **Bound.** Bindings are read against a borrowed graph. An **evaluation** is
  one `BindingNode` under one view. It answers the `BindingNode`'s questions,
  gives its answered names their content, and follows its calls, ending in a
  **handle**: the instance's admission, which names no storage. Handles are
  filed by view. The answers behind them rest in the crate, in bumps the
  embedder lends, and may be dropped and built again.

## Order and ownership

**Bases.** A site name's base is the name itself. An answered name's base is
the union, over its `AnswersName` edges, of the bases of what each edge
depends on:
- an edge on an arm depends on its question's reads;
- an edge through a call depends on the call's **inputs**:
  - the names its mapping mentions;
  - its question's reads, for a call on an arm;
  - every name its callee's body, or a body inside it, reads from its lexical
    chain where the call's site sees it;
- an edge on a site depends on the names its term mentions.

Bases are a fixpoint, and only grow.

A body's entry binds before its statements, and its statements bind in order.
An inner body's entry binds after its encloser's entry, and after each
statement of its encloser in a base the inner body, or any body inside it,
reads. Every base a question reads, or a term on a site mentions, is bound on
its body's lexical chain, so those sites form a chain.
- A question's **owner** is the latest of them.
- A call's owner is the latest site in its inputs' bases, or the root's entry
  where it has none. An onward call's inputs are the names its mapping
  mentions and every name its target's evaluation reads that is bound before
  the target. It is owned strictly before its target, and dropped where its
  target is no `BindingNode`.
- Inside a staged body, a question or a call is owned no earlier than the
  body's entry, whatever its reads.
- An answered name is bound at the latest site in its base: the `BindingNode`
  whose answer gives it its content, wherever the statement that names it
  sits. A `LET` is transparent.

Owners and placements depend on the text alone.

Every body's entry is a `BindingNode`. A statement is one only where its site
name is in the base of a name some question reads, some call takes as input,
some edge takes through a call, or some term on a site mentions. That is a name
whose content the run gives, or an onward call gives where its terms arrive. Each node but the root has exactly one
`BindsAfter` edge, to the node that binds just before it, so the nodes form
a tree.

The root's entry binds no site name, and every question reads at least one
name. The root owns the calls with no input. It also binds the answered names
those calls type, and owns the questions whose reads have an empty base.

The edges typing one answered name hang on different arms of one question, its
**deciding question**, or are one edge on a site: through a call there, or a
term. The callee name an edge through a call takes is bound in the callee's
body or visible at its entry.

## Refusing a recursion that grows

For each term a mapping gives a callee, or an `AnswersName` edge gives a name,
the domain reports the names the term mentions. Each mention is **bare**, as
the name itself or a part of it, **built**, inside a type the term builds, or
one **alternative** of a union the term builds. A union adds no depth, so an
alternative counts as bare here. The arcs:
- a call's mapping draws an arc from each mentioned caller name to the callee's
  name;
- an `AnswersName` edge on an arm or on a site draws one from each name its
  content mentions to the answered name;
- an onward call's mapping draws one from each mentioned name to the site
  name it maps, and those arcs count among a callee's own when a summary is
  drawn;
- an `AnswersName` edge through a call draws a **summary arc** to the answered
  name, from each caller name that reaches the taken name through the callee's
  own arcs. The arc is built if any step on the way builds. Summaries over
  recursive bodies are a fixpoint.

No arc runs from a callee's name back to a caller's, since a result returns
only to the call that asked for it.

The graph is refused where a built arc lies inside a strongly connected
component of those arcs. That is exactly where some closed walk of calls,
composed, gives a name a type built over itself. Checking simple cycles alone
would miss a walk such as a swap followed by a wrap, where each call alone
repeats.

The arcs come from every arm the text lays, whichever arm a binding would
take. So the refusal is decided before anything is bound, and a recursion that
selection alone would stop is refused too. A name a callee inherits unchanged
from its lexical chain draws no arc: it joins no two names and builds nothing.

## The domain's contract

A domain says:

- what a name's content is, its digest, and whether it lasts as long as the
  graph;
- what a `NameQuestion` is, how many answers it has, and the index of each;
- what a `SiteAnswer` is, and which arm an answer takes, if it decides one;
- what a term of a mapping or an `AnswersName` edge is, how it is applied to
  a binding, and which names it mentions;
- the **union** of a list of contents, the union of none being the **empty
  union**;
- the **rigid** of an answered name: its content where no answer gives it one;
- whether a content **arrives**: whether a run binding a site name at its
  site would bind exactly it, never a rigid nor a content standing for
  several.

The domain comes in three parts. `Kept` names what the graph and its
instances keep: a term, a `NameQuestion`, a `NameAnswer` and a `SiteAnswer`,
each `Copy`, `'static` and naming no content, so a graph is keyed by its
`Kept` alone and an embedder may hold one in a covariant cell. `Facts` names
the facts a graph is laid over: what each kept term mentions, each
`NameQuestion`'s answer count and each answer's index. `Domain` names the
content and the judgements over it. A content carries the lifetime of the call that reads it, and a `Domain`
is handed to each call, so nothing the crate keeps can hold one.

Its obligations:

- **Answers are counted.** A `NameQuestion` has a fixed count of answers, and
  every answer the domain gives has an index below it.
- **A digest is its content.** Two contents with one digest are one content,
  and a content that lasts stays readable for the graph's life.
- **Applying never grows unreported.** The result is no deeper than the
  types the term builds plus the deepest content it substitutes. Where the
  domain cannot compute a part of the content, for example over a name left
  open, it leaves the callee's name open instead. A union is no deeper than
  its deepest alternative, and holds each alternative once, a union's
  alternatives among them, so a recursion that adds an alternative already
  there repeats.
- **The empty union is no alternative.** Applying a term with a name it
  mentions only as an alternative bound to the empty union leaves that
  alternative out. Where no alternative is left, it gives the empty union
  itself.
- **Answering is deterministic.** The answer function reads the binding only
  through the doors it is handed, and makes only the asks its question
  declares. The same answers in the same order give the same asks and the
  same `SiteAnswer`.
- **Reads name what an answer depends on.** A question's reads are every name
  its answer depends on. An earlier answer reaches it through an answered
  name, which it reads as any other name, and the crate computes the base.
- **Open names and rigids stay apart.** A name left open inside passed content
  stands for its own unknown, even where the callee binds a name with the same
  id. So does each occurrence of a rigid: no judgement relates two. An ask
  over several names answers as for unrelated unknowns where one of them is
  open or a rigid.

## Instances

An **instance** is one `SiteAnswer` per question its `BindingNode` owns.
Its **admission** is every ask those answers made, with the answer each
took. An instance is a function of its `BindingNode` and its admission alone.

**Handles.** Each ask a `BindingNode`'s questions declare has a fixed field
in that `BindingNode`'s handle, holding the index of the answer taken or a
mark that it was not asked. So a handle is its instance's admission, and two
bindings share one exactly where their admissions are equal. The graph fixes
a handle's width per `BindingNode` when the text is laid, and two questions
of one `BindingNode` declaring the same ask share its field. The caller holds
a handle's words. A handle is `Copy` and names no storage of the crate's, so
it stays good whatever the crate drops.

**Evaluation.** `Bound::instance` evaluates a `BindingNode`
under its **view**: the binding restricted to the names it can read. A name
the binding types is taken as given, answered names included. The evaluation
computes the others, demand by demand. A demand is a question's answer, or an
answered name's content.
- **Earlier questions.** A question owned by an earlier node that the
  evaluation needs, deciding one of its names or selecting one of its calls, is
  answered inside it, under its binding, and left out of its admission.
- **Calls.** Each taken call is followed, and the callee's evaluation is found
  or made under the binding the call passes. That binding holds the mapped
  names and the names the callee reads from its chain.
- **Onward calls.** Each onward call the node owns is followed where every
  name its target binds and reads is given a content that arrives. The
  target's evaluation is found or made under the owner's binding and those
  names, as a callee's is.
- **The handle.** A finished evaluation's handle is read off the asks its
  answers made.
- **Content.** An answered name's content lives for one `instance` call. A
  caller that takes a name from a callee whose view is filed derives it
  again: from the arm its deciding question's answer takes, or from its term
  on a site, the term applied to the binding the call passes.

**Reading an instance.** Two doors read what an instance holds after its
`instance` call returns, each through the same rest-or-replay path:
- `Bound::answer` gives one question's `SiteAnswer` through a handle of the
  node that owns it, from where the answers rest, or by replaying the answer
  function with the doors serving the handle's fields. `Bound::answers`
  gives every answer of the node at once.
- `Bound::content` gives the content of an answered name a node binds, under
  a binding of that node. It derives the content as a caller derives a name
  it takes: from the node's filed evaluation, found or made, through the arm
  the name's deciding question takes, its term on a site, or the callee's
  filed evaluation it is taken from. So an embedder that holds a handle and
  the binding behind it reads any answer or answered name again, and keeps
  neither.

**Knots.** Demands that need each other around a cycle form a **knot**,
typically an evaluation reaching itself again through calls.
- Every name in a knot is its own rigid, unless the knot carries one type
  (below).
- A name whose deciding question lies in a knot not yet closed takes no arm,
  and needs nothing more. So, too, a name typed through a call that passes a
  name of a knot not yet closed takes nothing from the callee, and joins that
  knot.
- In a filed evaluation, a question its node owns is read through the handle,
  final at once. A name it decides reads the question's reads itself, and
  takes no arm where one of them lies in a knot not yet closed, so the name
  lies in the knot the question would have.
- Every other demand reads all its needs, each fixed by the text and by final
  values. So the knots are the strongly connected components of one graph of
  needs, found as the demands run on an explicit stack, and they depend on no
  order of starts or of mentions.

A name whose deciding question takes no arm, or whose taken name waits on the
run, is its own rigid too. A name bound at a callee's statement waits on the
run unless onward calls reach that statement's node from the callee's entry
or from before it; where they do, it is taken from that node's evaluation.

**Where a knot can form.** Each need a demand can read is an arc between two
answered names or questions of the text, whatever the view. An evaluation
waits only on the answered names its own `BindingNode` binds, and on a
callee's name only where the callee's entry binds it or onward calls reach
the node that does; everything else comes with its view. So the arcs run:
- from a question to each answered name it reads that its owner binds;
- from an answered name to its deciding question;
- from a name typed on an arm to each answered name its deciding question
  reads that the name's node binds;
- from a name typed on a site to each answered name its term mentions that
  the name's node binds;
- from a name typed through a call to each answered name the call passes that
  the name's node binds, to the callee name it takes where the callee's entry
  binds it or onward calls reach its node, and then to each answered name an
  onward call on the way passes that its owner binds.

Laying draws these arcs from every arm the text lays and takes their strongly
connected components. A **text component** is one that holds a cycle: more
than one member, or one with an arc to itself. Every knot, under every
binding, lies in one, and `Graph` answers which text component each answered
name and question lies in, if any. A demand outside every text component is
kept out of the search for knots, and is final when it completes.

A text component bounds where a knot can form, never what it is. The arm an
answer takes can cut a cycle the text lays, a call that passes its callee a
different view makes a recursion with no knot, and a call that swaps two
names makes one knot of two evaluations over one name. So no answer, handle
or rigid is read off a text component.

**A knot's kind.** A member of a knot **takes** where it is a name whose
content waits on a callee name of the same knot, whatever selected its call.
It is a **union alternative** where it is a name typed on a site whose term
mentions each name of its own knot only as one alternative of a union. A knot
**carries one type** where every member takes or is a union alternative. The
kind is read where an evaluation closes the knot, from the step each member
took, never from the text component it lies over.

**A knot that carries one type.** Every member of a knot reaches every other,
so such a knot gives all its names one type: the union of the alternatives it
reaches from outside. Each union alternative's term is applied with every name
of the knot it mentions bound to the empty union. What is left, unless it is
the empty union itself, is an alternative from outside. The domain builds
their union, each once and in digest order, as the knot closes, so no edge of
the knot is walked again. A knot that reaches no alternative from outside, or
one the domain cannot compute, leaves each name its own rigid. So does a knot
of any other kind: a question or a built type on a knot changes the type from
level to level.

**Filed by view.** `Bound` files each handle under its `BindingNode` and the
digests of its view's content, and keeps no content. A repeated `instance`
call for a filed view walks nothing, and a view that is not filed is
evaluated again. A view whose every content lasts as long as the graph is
kept for the life of the bump the embedder lends for it. Every other view
rests in a table of its
`BindingNode`'s own, of a slot count the embedder sets, where a newer view
overwrites an older one. A slot holds its digests in full, so a hit is exact.
The load sets no slot count, and keeps every view it files.

**Where answers rest.** What `Bound` makes before the embedder freezes it is
its **frozen part**: read-only, `Sync`, never freed, and shared by every
`Bound` laid over it. Answers made after the freeze rest in one bump the
embedder lends each `Bound`, under a byte budget it sets: a `Resting` store
laid in that bump and handed to each call, which counts every byte it lays,
alignment slack included, so a bump made with a capacity of the budget never
grows. Past the budget nothing more rests until the embedder resets the bump
whole, which it may do between any two calls. A read through a handle whose
answers were dropped replays the answer function, the doors serving the
handle's fields: it reads no binding and asks the domain nothing, and its
answers rest again where the budget has room. Everything `Bound` keeps is `Copy`,
carries no drop glue, and is laid through `bumpstorage`, in tables that hash
by a content word.

**What the embedder is told.** The answer function's doors say which
evaluation of the `instance` call is answering (`Doors::own_evaluation`) and
which call first entered it (`Doors::entered`), and `Answers::entering` hears
of each evaluation the call enters, beside the call that entered it and the
evaluation that made that call. They place an answer among those one call
makes, for the embedder's report, and change no answer.

**Measured.** Under the `perf` feature, off by default, `Bound` counts the
domain's asks and the answer function's runs, the demands it puts in the
search for knots, and the knots it closes, those that carry one type apart
from the others.

## Laws

Each is a property test over generated texts and bindings in a small domain
the tests supply, but the last, a loop the tests run over new content. The oracle is a reference that evaluates from scratch,
sharing no code with the crate: it finds knots by taking strongly connected
components, over and over, of what is still unknown.

- **One owner.** Every question is owned by exactly one `BindingNode`, every
  call has one owner, and every answered name is bound at one `BindingNode`.
  A question's answer under a binding of the sites up to its owner equals the
  oracle's.
- **Bound equals declared.** Every instance's answers equal the oracle's,
  under bindings that may type answered names as well as site names.
- **Order does not matter.** Binding a chain's sites one at a time, or all at
  once, reaches the same answers.
- **A passed name binds alike.** An instance made through a `PassesName`
  edge, an onward call among them, or an `AnswersName` edge equals its node's
  own under the binding the edge gives.
- **Reuse is exact.** Two bindings of a `BindingNode` get equal handles
  exactly where their admissions are equal, and the answers a handle reads
  equal those made from scratch for either.
- **Rebuilt is exact.** Every read through a handle equals the read with
  nothing dropped, under any budget and any view-table size, zero among them.
- **One answer is the node's.** `Bound::answer` for a question equals that
  question's element of `Bound::answers`, under any budget.
- **Content read again is the evaluation's.** `Bound::content` for an
  answered name equals the content the oracle gives it under the same
  binding. For a filed view whose answers all rest, the read runs the answer
  function only for the questions an earlier node owns, which rest nowhere,
  and rests nothing.
- **Finite.** A text whose every name is bound through the text has finitely
  many instances, or is refused. A brute-force composition of terms, matching
  each result to its call, refuses exactly the same texts.
- **Start anywhere.** An evaluation started at any node and view that another
  start reached gives the same answers.
- **Knots stay in the text's components.** The names and questions of every
  knot the oracle resolves lie in one text component.
- **A repeated call walks nothing.** Repeating an `instance` call whose view
  is filed walks no edge and runs no answer function.
- **Kept bytes are bounded.** After the freeze, what a `Bound` keeps stays
  within its budget, its view tables and its lasting views, however many
  contents it is shown.

## Open work

The crate's own open work is on [its roadmap](roadmap/README.md). Koan's work
over it:

- [The build image](../roadmap/one-build/build-image.md) — the graph written
  out and read back without laying.
