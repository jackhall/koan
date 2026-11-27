# An exhaustive check

An opt-in load that also starts from every entry, and keeps only the
refusals.

**Problem.** Over [the binding graph](../../design/binding-graph.md#instances)
the load makes instances from the root alone, so a body no chain of calls
from the top level reaches has no instance until a run cell binds it: a
library's callable, or one called only through a value. A refusal that holds
whatever such a body's names are bound to is then first found as the program
runs. In

```text
EXPR #(SHOW n :Number) -> Str = #("number")
EXPR #(SHOW s :Str) -> Str = #("string")
EXPR FOR ALL #[Elem] #(DESCRIBE v :Elem) -> Str = #(SHOW v)
EXPR FOR ALL #[Item] #(WRAP y :Item) -> Str = #(DESCRIBE [y])
```

nothing calls `WRAP`, and `SHOW v` under `Elem := LIST OF Item` has no
candidate whatever `Item` is, yet the program loads.

**Acceptance criteria.**

- Under an opt-in flag, the load also makes an instance of every entry no
  chain from the root reaches, with its names open, follows its edges and
  arms as it does from the root, and keeps only the refusals. A refusal
  there refuses the load.
- The pass keeps no instance it makes.
- A test: the program above loads without the flag and is refused under it.
- Without the flag, the load is unchanged.

**Directions.**

- *Opt-in — decided.* The default load does no work a run may never use, and
  CI runs the flag.
- *Where the flag is named — open.* A flag of the interpreter, or a
  declaration in the program's text. Recommended: a
  flag, since it prices a load and says nothing about what the program
  means.

## Dependencies

**Requires:** none.

**Unblocks:** none.
