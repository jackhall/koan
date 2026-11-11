# Digested keyed parameters

**Optional.** An optimization nothing needs yet.

**Problem.** A call that computes its key from the values digests the
argument at each keyed name at every call
([keys](../../src/build/README.md#keys-which-bindings-make-a-settlement)). A
module keeps its content digest, but a view's is hashed again over its source
and its type each time. The load already marks a slot **digested** where a
`:|` views a capture read from it: the slot keeps a memo beside its value,
and a call passing the slot on bare hands the memo to the parameter it binds
([`memos`](../../src/scope/shapes/memos.rs),
[content digests](../../src/values/README.md#content-digests)). A keyed
parameter is no such slot, so a loop calling a keyed body through a site
with no request digests the same argument each hop.

**Acceptance criteria.**

- A parameter passed at a keyed name by a call that computes its key from the
  values is a digested slot: its digest is computed at the first key that
  reaches it and read at each later one, and a call passing the slot on bare
  hands the memo on.
- A test counts the digest demands a loop of such calls opens and counts one
  per argument, not one per call.
- Every program prints as before.

**Directions.**

- *What the memo holds — open.* The argument's digest alone, which leaves the
  tree lookup, the retype and the key's hash at each call; or beside it the
  argument's part of the request. A memo on one slot cannot hold a key that
  joins several arguments.

## Dependencies

**Requires:** none.

**Unblocks:** none.
