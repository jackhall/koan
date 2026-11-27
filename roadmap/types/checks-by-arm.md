# A check settled arm by arm

A check over a `FOR ALL` name that holds on every arm of the selection it
reads, settled where the program loads.

**Problem.** Under
[a check over a `FOR ALL` name](checks-at-every-binding.md), a check loads
only where the load settles it with the name read through its bound. A check
on what a selection returns can hold at every binding and still not be
settled so:

```koan
EXPR #(DOUBLE n :Number) -> Number = #(n * 2)
EXPR #(DOUBLE s :Str) -> Str = #(s)
EXPR FOR ALL #{Elt: :(Number | Str)} #(TWICE x :Elt) -> Elt = #(DOUBLE x)
PRINT (TWICE 4)
PRINT (TWICE "ab")
```

`TWICE`'s return holds at every binding under `Number | Str`. On the arm
where `DOUBLE n` runs, its `Number` lies under `Elt` only because that arm is
taken only where `Elt` is `Number`, which the bound does not say. So `TWICE`
is refused at load, and the program writes `TWICE` twice, once per type, or
returns `:(Number | Str)`.

**Acceptance criteria.**

- The program above loads and prints `8`, then `ab`.
- The load judges a check that reads a selection's result once per arm, each
  `FOR ALL` name narrowed by what taking the arm says of it.
- A check that fails on an arm some binding takes is refused at load, and the
  message names the arm: with `EXPR #(DOUBLE s :Str) -> Number = #(1)` in
  place of the second `DOUBLE`, `TWICE` is refused, since the `Str` arm
  returns a `Number`.
- An arm whose slot a wider type lies under settles nothing by itself: with
  `DOUBLE r :{size :Number}` returning `:{size :Number}` and
  `Elt: :{size :Number}`, `TWICE` is refused, since a binding
  `:{size :Number, width :Number}` takes that arm and the result lacks
  `width`.

**Directions.**

- *What taking an arm says of a name — open.* Taking an arm puts the name
  under the arm's slot. The check needs the arm's return *above* the name
  too, which follows only where nothing but `Never` lies under the slot, as
  under `Number` and not under a record type. (1) Settle only arms whose slot
  is such a type. (2) Settle an arm whose return is written over its own
  argument's type as well, as a head reading its parameters writes it.

## Dependencies

**Requires:**

- [A check over a `FOR ALL` name holds at every binding](checks-at-every-binding.md)
  — the refusal this item lifts.

**Unblocks:** none — a leaf.
