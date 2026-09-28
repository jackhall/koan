# Slicing and splicing

Lists and strings edited by slicing and splicing, each result a view that
shares its sources until it crosses.

**Problem.** No koan operation slices, concatenates or splices a list or a
string. A [`List`](../../src/values/list.rs) is one run of cells in its region
and a string is borrowed where its bytes live
([values](../../src/values/README.md#what-a-value-is)), and data is immutable,
so an edit built over flat runs copies every element it keeps: k edits over n
elements cost O(kn).

**Acceptance criteria.**

- A slice, a concatenation or a splice of a list or a string is a view, which
  lays down none of its sources' cells or bytes.
- A slice carries its source's type, and a concatenation or a splice the join
  of its sources' types, whether it is a view or written flat: a slice `[1]` of
  `[1 "a"]` carries `(LIST OF Any)` and misses a `(LIST OF Number)` slot.
- A view equals and renders as the flat value it stands for.
- A view built from views is one view: reading an element of a view built by n
  successive edits takes O(log n) steps.
- A copy at a crossing writes a view as flat storage holding only the elements
  it shows, and retains nothing of its sources.
- A view weighs what its copy writes, so a crossing prices a slice of k
  elements by k, whatever its source's length.
- No transformation that runs koan code is a view, and no crossing runs koan
  code.

**Directions.**

- *Editing is construction — decided.* Data is immutable, so an edit builds a
  new value from slices of its source and the parts spliced between them, and
  slicing and splicing are the whole interface. Swift's
  `RangeReplaceableCollection`, which derives every edit of a `String` or an
  `Array` from `replaceSubrange`, is the nearest precedent.
- *A slice is typed by its source — decided.* Typing a slice by the join of its
  own elements would cost every slice of a heterogeneous list O(k), or a
  memoized join in every node of a view. Typed by its source, a slice's type
  costs nothing, and a list's `rest` fits wherever the list did, as an ML
  list's tail does. A program that wants the narrower type asks for it.
- *Views are invisible — decided.* A view is typed as the flat value it stands
  for, and a program tells the two apart only by cost. Swift's `Substring` is a
  type of its own so a reader sees what it retains; here the crossing drops the
  retention, so no type needs to show it.
- *A crossing resolves a view — decided.* A copy already rebuilds every region
  part ([crossing](../../src/values/README.md#crossing)), so it writes a view
  flat at no extra cost, and the [verdict](../../src/values/README.md#crossing)
  copies a small view of a large source rather than pinning the source whole.
  That is the retention Java's old `String.substring` leaked, and Haskell's
  `ByteString.copy` leaves to the programmer.
- *Only structural transformations are views — decided.* A transformation that
  runs koan code, as `map` and `filter` do, is lazy only as a
  [yielding iterator](yielding-iterators.md), a value of its own type, and a
  copy of one copies its pending call and never forces it. Forcing inside a
  copy could fail, never finish, perform an effect wherever the value happens
  to cross, and cost what no weight measures.
- *Which structural transformations — open.* Slice and concatenation, and so
  splice. Reverse, interleave and zip also remap indices and run no code, and
  each widens the algebra views compose in: reverse needs a direction per run,
  interleave a stride, and zip builds each pair when it is read, so its reads
  allocate. Reversing or interleaving a string needs a unit: bytes break UTF-8,
  code points split combining marks, and graphemes cost a segmentation.
- *A view's representation and memos — open.* A view's type comes from its
  sources in O(1), so its weight is the one summary it needs. A slice of one
  run can be the run, an offset and a length, weighed in O(1) by the run's
  prefix sums. A concatenation or a splice of many runs can recompute its weight
  when it is built, O(k) per view; or it can be a balanced tree of runs — a
  rope, or a finger tree as Haskell's `Data.Sequence` is, or Clojure's
  `core.rrb-vector` — whose nodes memoize their subtree's weight, O(log n) per
  view at the price of chunking flat runs.
- *Narrowing a slice's type — open.* Koan has no cast. A narrowing checks each
  of the slice's k elements against the narrower type, and could be a builtin
  of its own or the list counterpart of a record's
  [projection](../../tutorial/07-records.md).
- *Spreading with `..` — open.* `..xs` spreads a list's elements one level
  deep, and a quote's `..$xs` spreads when its code runs
  ([code splicing](code-splicing.md)). A spread into a list literal, or into the arguments of a function called by name, needs nothing
  more. A spread into a keyworded use leaves its bucket key's slot count
  unknown where the use is written, and dispatch keys on the full bucket key
  ([keyworded uses](../../src/scope/README.md#keyworded-uses)): either such a spread is refused, or the use is
  dispatched once the count is known.
- *A list pattern with a rest — open.* `MATCH` has no pattern for a list's
  shape. A `[]` arm and an `[x, ..rest]` arm, which together cover every list,
  would bind `rest` as a slice, as Rust's `[first, rest @ ..]` and Python's
  `[x, *rest]` do, and would mirror spreading with `..`. Each `rest` is typed as
  the list it came from, so a recursion over a list of one run pays O(1) per
  step.
- *String templates — open.* JavaScript's tagged templates and Python's
  t-strings keep a template's literal parts and its interpolated values apart
  until a consumer, such as an SQL escaper, splices them. That is the shape of a
  quote with its bindings
  ([quotes](../../src/scope/README.md#quotes)), and it would give
  strings a splicing surface for construction as well as editing.

## Dependencies

Splicing into code is [code splicing](code-splicing.md)'s.

**Requires:**


**Unblocks:**

- [Code splicing](code-splicing.md) — slice as a builtin, views, and `..` outside a quote.
