//! What a code view reads of a stretch of a node's parts, in O(1) however long the node: the
//! [`PartsSummary`] of parts `[from, to)`.
//!
//! A summary counts the parts, the bytes their copy writes, the parts that are no group, the
//! keywords and the **breaks** — two adjacent parts both keywords or both not — and reads the
//! deepest part and whether the first and last are keywords. It is a monoid, so a code view joins
//! its stretches' summaries up its tree, and the operator-run
//! test reads off it: a node is `Slot (Keyword Slot)+` exactly when its summary has no break, a
//! first and last part that are no keyword, and two keywords or more.
//!
//! A node of more than [`BLOCK`] parts lays [`Blocks`] beside itself at construction: running sums
//! every [`BLOCK`] parts, each whole block's deepest part, and a sparse table over those maxima. A
//! stretch is then a difference of two sums, each a block entry and a scan of under [`BLOCK`]
//! parts, and its deepest part two table reads and two partial-block scans. A shorter node is
//! scanned directly.

use crate::memory::{Bump, BumpVec, Writer, collect, resident};
use crate::source::Spanned;

use super::{ALLOCATION_SLACK, ExpressionPart, KeyElement};

/// How many parts one block of a node's [`Blocks`] spans.
pub const BLOCK: usize = 16;

/// What a stretch of parts adds up to: see the module documentation. The default is the empty
/// stretch, the monoid's identity.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct PartsSummary {
    pub len: u32,
    /// The bytes a copy of the parts writes: each part word and key element, and what the part
    /// points at. Saturating.
    pub weight: u32,
    pub non_groups: u32,
    pub keywords: u32,
    pub breaks: u32,
    pub first_keyword: bool,
    pub last_keyword: bool,
    /// The deepest part's depth, as a node over the parts counts it beneath its own levels.
    pub deepest: u32,
}

impl PartsSummary {
    /// One part's summary.
    pub fn of(part: &ExpressionPart<'_>) -> Self {
        let keyword = matches!(part, ExpressionPart::Keyword(_));
        PartsSummary {
            len: 1,
            weight: part_weight(part),
            non_groups: u32::from(!matches!(part, ExpressionPart::Expression(_))),
            keywords: u32::from(keyword),
            breaks: 0,
            first_keyword: keyword,
            last_keyword: keyword,
            deepest: super::super::depth::part_depth(part),
        }
    }

    /// This stretch, then `right`.
    pub fn joined(self, right: Self) -> Self {
        if self.len == 0 {
            return right;
        }
        if right.len == 0 {
            return self;
        }
        PartsSummary {
            len: self.len + right.len,
            weight: self.weight.saturating_add(right.weight),
            non_groups: self.non_groups + right.non_groups,
            keywords: self.keywords + right.keywords,
            breaks: self.breaks
                + right.breaks
                + u32::from(self.last_keyword == right.first_keyword),
            first_keyword: self.first_keyword,
            last_keyword: right.last_keyword,
            deepest: self.deepest.max(right.deepest),
        }
    }

    /// The same parts read in reverse.
    pub fn reversed(self) -> Self {
        PartsSummary {
            first_keyword: self.last_keyword,
            last_keyword: self.first_keyword,
            ..self
        }
    }

    /// Whether a node over these parts is an operator run, `Slot (Keyword Slot)+`.
    pub fn is_operator_run(&self) -> bool {
        self.breaks == 0 && !self.first_keyword && !self.last_keyword && self.keywords >= 2
    }
}

/// What a copy of `part` writes: its word and key element in the node's runs, and what it points
/// at.
fn part_weight(part: &ExpressionPart<'_>) -> u32 {
    let word = size_of::<Spanned<ExpressionPart<'_>>>() + size_of::<KeyElement>();
    (word as u32).saturating_add(part.laid_weight())
}

/// The sums over a node's parts before a block boundary. Weights sum in 64 bits, so a difference
/// of two never saturates; breaks count adjacent pairs ending before the boundary.
#[derive(Clone, Copy, Default, Debug)]
struct Sums {
    weight: u64,
    non_groups: u32,
    keywords: u32,
    breaks: u32,
}

/// A long node's block memo: see the module documentation.
#[derive(Debug)]
pub struct Blocks<'a> {
    /// Entry `b` sums parts `[0, BLOCK·b)`, for `b` in `0..=len / BLOCK`.
    sums: &'a [Sums],
    /// Row `r` of the sparse table, rows laid one after another: entry `j` is the deepest part of
    /// whole blocks `[j, j + 2^r)`. Row 0 holds each whole block's deepest part.
    maxima: &'a [u32],
    /// How many whole blocks the node holds.
    blocks: usize,
}

impl<'a> Blocks<'a> {
    /// The memo of `parts`, laid through `writer`, where they are more than [`BLOCK`].
    pub(super) fn of(
        writer: Writer<'a, '_>,
        parts: &[Spanned<ExpressionPart<'_>>],
    ) -> Option<&'a Blocks<'a>> {
        if parts.len() <= BLOCK {
            return None;
        }
        let blocks = parts.len() / BLOCK;
        let mut sums = Sums::default();
        let sums = writer.fill(blocks + 1, |block| {
            let at = sums;
            if block < blocks {
                for index in block * BLOCK..(block + 1) * BLOCK {
                    sums = added(sums, parts, index);
                }
            }
            at
        });
        let deepest = |block: usize| {
            (parts[block * BLOCK..(block + 1) * BLOCK].iter())
                .map(|part| PartsSummary::of(&part.value).deepest)
                .max()
                .unwrap_or(0)
        };
        // Row `r` spans `2^r` blocks, so it holds `blocks + 1 - 2^r` entries, while `2^r ≤ blocks`.
        let scratch = Bump::new();
        let mut table = BumpVec::new_in(&scratch);
        table.extend((0..blocks).map(deepest));
        let (mut below, mut span) = (0, 1);
        while 2 * span <= blocks {
            let start = table.len();
            for at in 0..blocks + 1 - 2 * span {
                let deeper = table[below + at].max(table[below + at + span]);
                table.push(deeper);
            }
            (below, span) = (start, 2 * span);
        }
        let maxima = collect(writer, table.iter().copied());
        Some(resident(
            writer,
            Blocks {
                sums,
                maxima,
                blocks,
            },
        ))
    }

    /// The bytes the memo of a node of `parts` parts writes, its allocations' slack included:
    /// nothing at most [`BLOCK`] parts. A function of the count alone, so a code view prices a node
    /// it has not laid.
    pub(super) fn bound(parts: usize) -> usize {
        if parts <= BLOCK {
            return 0;
        }
        let blocks = parts / BLOCK;
        let rows = (0..=blocks.ilog2()).map(|row| blocks + 1 - (1 << row));
        size_of::<Blocks<'_>>()
            + (blocks + 1) * size_of::<Sums>()
            + rows.sum::<usize>() * size_of::<u32>()
            + 3 * ALLOCATION_SLACK
    }

    /// The bytes a copy of this memo writes, its allocations' slack included.
    pub(super) fn laid_bytes(&self) -> usize {
        size_of::<Blocks<'_>>()
            + size_of_val(self.sums)
            + size_of_val(self.maxima)
            + 3 * ALLOCATION_SLACK
    }

    /// The sums over parts `[0, at)`: a block entry and a scan of the parts past it.
    fn sums_to(&self, parts: &[Spanned<ExpressionPart<'_>>], at: usize) -> Sums {
        let block = at / BLOCK;
        (block * BLOCK..at).fold(self.sums[block], |sums, index| added(sums, parts, index))
    }

    /// The deepest part of whole blocks `[from, to)`, `from < to`: two sparse-table reads.
    fn deepest_blocks(&self, from: usize, to: usize) -> u32 {
        let row = (to - from).ilog2() as usize;
        // Row `r` starts past every shorter row, each `2^k - 1` entries shorter than the blocks.
        let start = (0..row).map(|k| self.blocks + 1 - (1 << k)).sum::<usize>();
        self.maxima[start + from].max(self.maxima[start + to - (1 << row)])
    }
}

/// `sums` past part `index`, the break between it and the part before counted.
fn added(sums: Sums, parts: &[Spanned<ExpressionPart<'_>>], index: usize) -> Sums {
    let part = PartsSummary::of(&parts[index].value);
    let alike = index > 0
        && matches!(parts[index - 1].value, ExpressionPart::Keyword(_)) == part.first_keyword;
    Sums {
        weight: sums.weight + u64::from(part.weight),
        non_groups: sums.non_groups + part.non_groups,
        keywords: sums.keywords + part.keywords,
        breaks: sums.breaks + u32::from(alike),
    }
}

/// The summary of parts `[from, to)` of `parts`, read through `blocks` where the node laid them.
pub(super) fn stretch(
    parts: &[Spanned<ExpressionPart<'_>>],
    blocks: Option<&Blocks<'_>>,
    from: usize,
    to: usize,
) -> PartsSummary {
    debug_assert!(from <= to && to <= parts.len());
    let scanned = |from: usize, to: usize| {
        (parts[from..to].iter())
            .map(|part| PartsSummary::of(&part.value))
            .fold(PartsSummary::default(), PartsSummary::joined)
    };
    let Some(blocks) = blocks.filter(|_| to - from > 2 * BLOCK) else {
        return scanned(from, to);
    };
    let (low, high) = (blocks.sums_to(parts, from), blocks.sums_to(parts, to));
    // The break before `from` lies outside the stretch.
    let outside = from > 0
        && matches!(parts[from - 1].value, ExpressionPart::Keyword(_))
            == matches!(parts[from].value, ExpressionPart::Keyword(_));
    let (first, last) = (from.div_ceil(BLOCK), to / BLOCK);
    let mut deepest = scanned(from, first * BLOCK)
        .deepest
        .max(scanned(last * BLOCK, to).deepest);
    if first < last {
        deepest = deepest.max(blocks.deepest_blocks(first, last));
    }
    PartsSummary {
        len: (to - from) as u32,
        weight: u32::try_from(high.weight - low.weight).unwrap_or(u32::MAX),
        non_groups: high.non_groups - low.non_groups,
        keywords: high.keywords - low.keywords,
        breaks: high.breaks - low.breaks - u32::from(outside),
        first_keyword: matches!(parts[from].value, ExpressionPart::Keyword(_)),
        last_keyword: matches!(parts[to - 1].value, ExpressionPart::Keyword(_)),
        deepest,
    }
}
