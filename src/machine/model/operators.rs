//! Operator-group registry record. A set of chainable operators is declared
//! together and registered — one region-hosted [`OperatorGroup`] — under
//! every nonempty subset of the group's operators (the per-group powerset,
//! singletons included, so a same-operator run like `a + b + c`, whose deduped probe
//! is just `+`, still resolves). A chain's operator probe (the run digest of the unique
//! operator symbols of a `Slot (Keyword Slot)+` expression) looks the group up in one
//! hashmap hit; a cross-group mix — which nothing registers — simply misses.
//!
//! A group's record is its member set plus one [`ReductionMode`] describing how a
//! recognized run of its operators reduces. It is koan semantic data, so it lives in the
//! declaring scope's region bump: [`OperatorGroup::alloc`] is the one door, the members are a
//! sorted slice of keyword symbols probed by binary search, and the record is `Copy` and
//! `Drop`-free, so region death frees it with the chunks. One allocation backs every one of a
//! group's powerset keys — each key holds a sealed carrier over the same pointee — so sharing is
//! address identity and the install allocates nothing past the probe keys.
//!
//! Registry lookup is innermost-wins
//! ([`Scope::resolve_operator_group_delivered`](crate::machine::core::Scope::resolve_operator_group_delivered)):
//! the builtin comparison / additive / multiplicative groups seeded into the run-global root
//! by `register_builtin_operator_groups` (`src/builtins/arithmetic.rs`) are found last, so they
//! are chaining defaults a declaring scope may override — a registry hit carries no operand
//! types and so cannot type-gate the way a function bucket does. User modules populate the
//! registry through the `OP` / `GROUP` declaration surface
//! ([design/operators.md](../../../design/operators.md), `builtins::op_def` /
//! `builtins::group_def`). This module is the record and lookup keys only — the function-bucket
//! keys [`binary_key`] / [`unary_key`] an operator's overloads live under; the registry's own probe
//! key is [`KeywordSymbol::of_run`].

use crate::machine::core::RegionBrand;

use super::labels::KeywordSymbol;
use super::types::{KeyElement, UntypedKey};

/// Which way a fold nests a run of more than two operands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FoldDirection {
    /// `a ⊙ b ⊙ c` ⇒ `(a ⊙ b) ⊙ c`.
    Left,
    /// `a ⊙ b ⊙ c` ⇒ `a ⊙ (b ⊙ c)`.
    Right,
}

/// How a recognized run of this group's operators reduces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReductionMode {
    /// The whole operand run is handed to one body as a single list operand.
    Unary,
    /// A binary body folds the run left-associated: `a - b - c` ⇒ `(a - b) - c`.
    FoldLeft,
    /// Right-associated: `a ^ b ^ c` ⇒ `a ^ (b ^ c)`.
    FoldRight,
    /// Each adjacent pair dispatches through its own operator's binary body; the pair
    /// results fold through the group's combiner in the declared direction.
    Pairwise {
        /// The **keyword** of the operator the pair results fold through — the builtin comparison
        /// group's `AND`, or a member `OP` declared over the pair-result type. The reducer
        /// synthesizes the infix shape `[left, Keyword(combiner), right]`, so the combiner binds
        /// its two inputs positionally, by signature shape, and imposes no parameter-naming
        /// convention. It is a keyword symbol, not a resolved function: the ordinary scope walk
        /// resolves it at the chain's use site, so a combiner that is missing, non-callable, or of
        /// the wrong arity is an ordinary error there.
        combiner: KeywordSymbol,
        direction: FoldDirection,
    },
}

/// A declared set of mutually chainable operators plus the mode a recognized run of
/// them reduces by. `Copy` and `Drop`-free, hosted in the declaring scope's region bump by
/// [`OperatorGroup::alloc`]: every powerset key the registering module installs holds a sealed
/// carrier over the *same* pointee, so a subset used in one expression resolves to the same group
/// as any other subset by address identity, and region death frees the record with the chunks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperatorGroup<'a> {
    /// The full declared member set (keyword symbols), not the probed subset — sorted by symbol
    /// bits and deduped, the invariant [`Self::covers`]' binary search and
    /// [`Self::declaration_key`]'s rendering read. Member counts are necessarily tiny (the powerset
    /// install is `2^n`), so a sorted run beats a hash table at this size and costs the bump no
    /// `Drop`.
    members: &'a [KeywordSymbol],
    mode: ReductionMode,
}

/// [`Reattachable`](crate::witnessed::Reattachable) family for [`OperatorGroup`] — the carrier
/// family a group record travels under in the `operators` registry, the operator-table twin of
/// [`KFunctionFamily`](crate::machine::core::kfunction::KFunctionFamily).
///
/// A carried group travels as `&'r OperatorGroup<'r>` — a thin reference whose layout does not
/// depend on `'r`; `OperatorGroup<'r>` itself is a reference-to-slice beside a lifetime-free
/// [`ReductionMode`], so every choice of `'r` is one type up to the lifetime and the shared
/// `reattachable!` macro discharges the layout-invariance obligation once.
pub struct OperatorGroupFamily;

crate::witnessed::reattachable! {
    OperatorGroupFamily => &'r OperatorGroup<'r>,
}

impl<'a> OperatorGroup<'a> {
    /// The single allocation door: copy `members` (sorted by symbol bits and deduped here) into
    /// `brand`'s region and bump the record beside them. The mode is `Copy` and lifetime-free, so
    /// it rides through untouched. The one record every powerset key of this declaration shares, so
    /// the install is allocation-free past its probe keys and the cheap identity arm of the
    /// registry upsert is an address compare.
    pub fn alloc(
        brand: RegionBrand<'a>,
        members: &[KeywordSymbol],
        mode: ReductionMode,
    ) -> &'a OperatorGroup<'a> {
        let mut sorted: Vec<KeywordSymbol> = members.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        brand.allocator().value(OperatorGroup {
            members: brand.allocator().slice(&sorted),
            mode,
        })
    }

    /// The mode a recognized run of this group's operators reduces by.
    pub fn mode(&self) -> ReductionMode {
        self.mode
    }

    /// Every member operator symbol, in sorted order.
    pub fn member_symbols(&self) -> impl Iterator<Item = KeywordSymbol> + use<'a> {
        self.members.iter().copied()
    }

    /// True iff every operator in `probe_operators` is a member of this group — the
    /// admission gate for a chain whose probe hit this group's registry slot. A probe
    /// subset that names a non-member is a cross-group mix that must miss.
    pub fn covers(&self, probe_operators: &[KeywordSymbol]) -> bool {
        probe_operators
            .iter()
            .all(|op| self.members.binary_search(op).is_ok())
    }

    /// The stored form of the registry upsert's **structural** identity rule — mode and member set
    /// rendered into one owned, lifetime-free string. Two `OP` statements over one symbol and
    /// distinct operand types are two bucket overloads but one declaration, and each allocates its
    /// own record, so the upsert has to answer identity by content rather than by address.
    ///
    /// Rendered here, where the record is open, so the registry write verb compares plain data and
    /// never opens a carrier ([`OverloadSeal`](crate::machine::OverloadSeal) is the same move for
    /// the `functions` table). The comparison is exact, not a digest of the rendering: each member
    /// is written as the hex of its own digest, which *is* its text's content identity, in the
    /// already sorted and deduped stored order; the mode segment is fenced off by a control byte no
    /// rendering below can produce, so equal renderings mean equal declarations.
    pub fn declaration_key(&self) -> String {
        let mode = match self.mode {
            ReductionMode::Unary => "unary".to_string(),
            ReductionMode::FoldLeft => "fold-left".to_string(),
            ReductionMode::FoldRight => "fold-right".to_string(),
            ReductionMode::Pairwise {
                combiner,
                direction,
            } => {
                let direction = match direction {
                    FoldDirection::Left => "left",
                    FoldDirection::Right => "right",
                };
                format!("pairwise-{direction} {}", symbol_key(combiner))
            }
        };
        let members = self
            .members
            .iter()
            .map(|member| symbol_key(*member))
            .collect::<Vec<_>>()
            .join(" ");
        format!("{mode}\u{1}{members}")
    }
}

/// One operator's contribution to a [`declaration_key`](OperatorGroup::declaration_key): its digest
/// in fixed-width hex. A symbol *is* the content identity of its glyph, so rendering the bits
/// preserves the exact content comparison the key promises without resolving any text.
fn symbol_key(symbol: KeywordSymbol) -> String {
    format!("{:032x}", symbol.symbol().0)
}

/// The function-bucket key a binary use of `sym` computes — `[Slot, Keyword(sym), Slot]` — and the
/// key every declaration of `sym` registers its binary overload under. The two must agree on the
/// symbol: an overload registered under any other key sits in a bucket no koan expression ever
/// computes, so the operator silently never dispatches.
pub fn binary_key(sym: KeywordSymbol) -> UntypedKey {
    vec![KeyElement::Slot, KeyElement::Keyword(sym), KeyElement::Slot]
}

/// The function-bucket key a reduced unary run of `sym` computes — `[Keyword(sym), Slot]`, the same
/// shape as the prefix form `sym [a b c]` — and the key every declaration of `sym` registers its
/// list-form overload under. Same symbol-agreement contract as [`binary_key`].
pub fn unary_key(sym: KeywordSymbol) -> UntypedKey {
    vec![KeyElement::Keyword(sym), KeyElement::Slot]
}
