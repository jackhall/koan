# The fact domain over the binding graph

Koan's **fact domain** over [the binding graph](../../../design/binding-graph.md):
what a name is bound to, how a question is asked of names bound to content,
how a term over names is applied to a binding, and which names a term
mentions. It is the embedder's half of [the crate's contract](../../../bindgraph/README.md#the-domains-contract);
[the build](../../build/README.md#the-binding-graphs-text) folds the load's
records into the graph's text, and this module answers over it
([`graph.rs`](../graph.rs)).

## Names and content

A name of the graph is one of the variables the types a question or a term
names are written over: a site name's lexical level, or a derived name — an
answered name, or a module binder's carried name. A module binder's
**content name** is no variable: a question reads it only through a view's
carrier or a tree's identity over the binder.

A binding gives a name a `Content`: a type, the type a module carries, or a
module's digest beside its bare signature and sealing. A type may still be
written over a caller's names left open, each named by its address in its
base, never by a lexical level, since two bodies number their names alike. An
answered name no answer gives content is its rigid, named by its address too.

## The domain

- **Ask.** `ask` writes each content into the question's positions and asks
  the closed door, or [the rigid answer](../../scope/rigid.rs) where a
  position stays open. Each position whose content holds an open variable or
  a rigid is freshened apart from every other, so no judgement relates two.
- **Apply.** `apply` writes a binding into a term, each name it leaves open
  written as its open variable, and gives nothing where a derived name's rule
  cannot compute over the content.
- **Arrives.** A content arrives at a holder only where a run binding the
  holder at its site would bind exactly it, by the name's kind: a type name a
  concrete type, a carried name the type a module carries, a content name a
  module; never a rigid or a type still open.
- **Union and rigid** are the crate's union of contents and an answered
  name's rigid.

Two closed functions read a module binder's content under a binding, each over
the laid facts and the binding's content alone, the walk one caller of each:
**module substitution** ([`classes.rs`](classes.rs)), what a binding makes a
path class's variable and what a path reads off a module's content tree; and
**tree application** ([`trees.rs`](trees.rs)), a content tree with a call or
a binding written into its leaves and the views, retypes and member reads over
it built again. [`modules.rs`](modules.rs) hands both what a question asked
under a binding reads: the class recipes the fold laid and the contents the
binding gives the names a recipe reads. Every function here is closed over
its inputs and types no statement.

## The run's door and the load's refusals

A frame makes an instance through one door: the names its slots bind,
converted to the graph's contents, handed to the base's instances, which find
it filed by view or make it. The load makes the instances its text reaches
from the root and reports the first refusal one of them answers, worded under
that instance's binding and placed at the call that entered it
([refusal](../../build/README.md#refusal)).

## Laws

[The rigid answer is every binding's](tests.rs): a rigid answer holds under
every binding within the names' ends. The instance laws over the graph are the
dispatch suite's ([properties](../../build/README.md#properties)).
