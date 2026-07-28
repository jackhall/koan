//! The `WriteGate` mint is `pub(in crate::machine)`. Every binding-table write verb takes a
//! `&mut WriteGate`, so a caller outside the machine — a builtin body, an embedder — cannot
//! name a write verb at all: it has no way to produce the capability they all require.

fn main() {
    let _gate = koan::machine::WriteGate::for_unpublished_scope();
}
