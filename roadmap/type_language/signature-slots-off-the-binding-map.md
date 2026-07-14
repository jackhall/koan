# Signature value slots off the binding map

**Problem.** A name's token class picks its universe — a Type token names a type, a value token
names a value — and [`Bindings`](../../src/machine/core/bindings.rs) enforces that partition on
every write ([design/typing/tokens.md](../../design/typing/tokens.md)). A SIG decl scope is the
one exception. `VAL compare :Number` records the slot as `bindings.types["compare"] = Number`, so
a value token lands in the type map, and `Bindings` carries a `slot_table` bit whose only purpose
is to let `partition_guard` admit that write; `Scope::child_under_sig` is its only setter.

The write is not really a binding — nothing can name `compare` as a type, and no lookup reaches
it through the type channel. The map is being used as a *schema*: member name → declared type.
The declaration's type members (`TYPE Carrier`, `LET Carrier = Number`) genuinely do bind in the
type universe, since a later body statement resolves `:Carrier` against them; only the VAL slots
are schema entries wearing a binding's clothes.

The consequence is that the split has to be recovered on every read. `SigSchema::of_sig`
([sig_schema.rs](../../src/machine/model/types/sig_schema.rs)) walks the flat map and re-runs
`is_type_name` on each key to sort members from slots, and the ascription and `WITH` paths
([ascribe.rs](../../src/builtins/ascribe.rs), [with.rs](../../src/builtins/type_ops/with.rs))
each re-derive the same classification. A `SigSchema` — the normalized `{abstract_members,
manifest_members, value_slots}` carrier the subtyping relation is already defined over — is
rebuilt from the flat map at each projection rather than stored.

**Acceptance criteria.**

- `Bindings` has no `slot_table` field, and `partition_guard` admits no exception: every
  value-token write to `types` and every Type-token write to `data` is an error.
- `Scope::child_under_sig` mints an ordinary `Bindings`.
- A signature's value slots are stored on [`ModuleSignature`](../../src/machine/model/values/module.rs)
  separately from its decl scope's type bindings, and its `SigSchema` is built once at SIG finish
  rather than re-projected per read.
- No consumer recovers the member/slot split by re-running `is_type_name` over a signature's type
  map.
- A SIG body statement still resolves a type name against the declaration's own type members, so
  a SIG-local `TYPE`/`LET` member shadows the builtin table as it does today.
- The full test suite and the Miri audit slate are green across the change.

**Directions.**

- *Where the slots live — open.* Either distinct fields on `ModuleSignature` (`value_slots`,
  filled as the body's VAL statements finish) or a whole `SigSchema` stored on the signature at
  finish, with `of_sig` reducing to a clone-plus-pin. Recommended: the stored `SigSchema` — it is
  already the carrier every consumer wants, and it retires the per-read reprojection along with
  the flat map.
- *How a VAL statement reaches that storage — open.* `VAL` currently registers through the decl
  scope, which is what puts it in the binding map. It needs a channel to the signature under
  construction — a slot table hung off the decl scope, a `RefCell` collector the SIG finish
  drains, or a body-local accumulator threaded through `await_body_in_scope`.
- *Module self-sigs — decided.* `SigSchema::raw_self_sig` is unaffected. A module's child scope is
  a genuine binding scope where the partition already holds (type members in `types`, value members
  in `data`), so its split needs no `is_type_name` filter to recover — that filter becomes
  redundant and can go.

## Dependencies

Surfaced while shipping the token-class partition, which had to carve out the SIG slot table to
land; this item removes the carve-out.

**Requires:** none — the substrate is shipped.

**Unblocks:** none.
