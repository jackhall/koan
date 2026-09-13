//! A dict: two aligned runs in the region, keys sorted and cells beside them, so a lookup is a
//! binary search and entry order is key order.

use std::cmp::Ordering;
use std::fmt;

use crate::memory::{BumpAllocator, BumpVec, Writer};
use crate::type_lattice::{KType, TypeRegistry, join};

use super::{Value, Weight, resident};

/// A dict key: a string, a number or a bool. Its representation is private and every door
/// normalises — NaN is refused and `-0` folds to `0` — so the order and equality below agree with
/// IEEE equality on every key that exists.
#[derive(Clone, Copy, Debug)]
pub struct Key<'cell>(Scalar<'cell>);

#[derive(Clone, Copy, Debug)]
enum Scalar<'cell> {
    Str(&'cell str),
    Number(f64),
    Bool(bool),
}

/// Why a value cannot be a key.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum KeyRejected {
    /// Only a string, a number or a bool keys a dict; this is the type that was offered.
    NotAScalar(KType),
    NaN,
}

impl<'cell> Key<'cell> {
    /// The key a value makes, borrowing its bytes where they already live.
    pub fn of(value: &Value<'_, 'cell>) -> Result<Key<'cell>, KeyRejected> {
        match *value {
            Value::Str(text) => Ok(Key::str(text)),
            Value::Number(number) => Key::number(number),
            Value::Bool(flag) => Ok(Key::bool(flag)),
            other => Err(KeyRejected::NotAScalar(other.ktype())),
        }
    }

    pub fn str(text: &'cell str) -> Key<'cell> {
        Key(Scalar::Str(text))
    }

    /// A number key, `-0` folded to `0`; NaN keys nothing.
    pub fn number(number: f64) -> Result<Key<'cell>, KeyRejected> {
        match number {
            number if number.is_nan() => Err(KeyRejected::NaN),
            0.0 => Ok(Key(Scalar::Number(0.0))),
            number => Ok(Key(Scalar::Number(number))),
        }
    }

    pub fn bool(flag: bool) -> Key<'cell> {
        Key(Scalar::Bool(flag))
    }

    /// The key as the value it was made from.
    pub fn value<'graph>(&self) -> Value<'graph, 'cell> {
        match self.0 {
            Scalar::Str(text) => Value::Str(text),
            Scalar::Number(number) => Value::Number(number),
            Scalar::Bool(flag) => Value::Bool(flag),
        }
    }

    pub fn ktype(&self) -> KType {
        match self.0 {
            Scalar::Str(_) => KType::STR,
            Scalar::Number(_) => KType::NUMBER,
            Scalar::Bool(_) => KType::BOOL,
        }
    }

    /// The key laid down in `writer`'s region: a string's bytes are written again, a scalar rides.
    pub(crate) fn rehomed<'dest>(self, writer: Writer<'dest>) -> Key<'dest> {
        Key(match self.0 {
            Scalar::Str(text) => Scalar::Str(writer.text(text)),
            Scalar::Number(number) => Scalar::Number(number),
            Scalar::Bool(flag) => Scalar::Bool(flag),
        })
    }

    fn weight(&self) -> Weight {
        let bytes = match self.0 {
            Scalar::Str(text) => Weight::text(text.len()),
            Scalar::Number(_) | Scalar::Bool(_) => Weight::ZERO,
        };
        Weight::flat::<Key<'static>>().plus(bytes)
    }

    fn rank(&self) -> u8 {
        match self.0 {
            Scalar::Bool(_) => 0,
            Scalar::Number(_) => 1,
            Scalar::Str(_) => 2,
        }
    }
}

/// The total key order: every bool, then every number in numeric order, then every string by bytes.
impl Ord for Key<'_> {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self.0, other.0) {
            (Scalar::Bool(left), Scalar::Bool(right)) => left.cmp(&right),
            (Scalar::Number(left), Scalar::Number(right)) => left.total_cmp(&right),
            (Scalar::Str(left), Scalar::Str(right)) => left.cmp(right),
            _ => self.rank().cmp(&other.rank()),
        }
    }
}

impl PartialOrd for Key<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Key<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Key<'_> {}

/// A string key renders quoted, so `{"1": x}` and `{1: x}` read apart.
impl fmt::Display for Key<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Scalar::Str(text) => write!(f, "\"{text}\""),
            Scalar::Number(number) => write!(f, "{number}"),
            Scalar::Bool(flag) => write!(f, "{flag}"),
        }
    }
}

/// A dict value, resident in the region its keys and cells live in.
#[derive(Clone, Copy, Debug)]
pub struct Dict<'graph, 'cell> {
    keys: &'cell [Key<'cell>],
    cells: &'cell [Value<'graph, 'cell>],
    ktype: KType,
    weight: Weight,
}

impl<'graph, 'cell> Dict<'graph, 'cell> {
    /// Lay down `entries` sorted by key; where a key repeats, its last occurrence wins. Keys are
    /// written into `writer`'s region wherever they borrowed from, and the key and value types are
    /// the joins over what stays. The sort is staged over `scratch`.
    pub fn new(
        writer: Writer<'cell>,
        entries: &[(Key<'_>, Value<'graph, 'cell>)],
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
    ) -> &'cell Dict<'graph, 'cell> {
        let mut order: BumpVec<'_, usize> = BumpVec::with_capacity_in(entries.len(), scratch);
        order.extend(0..entries.len());
        order.sort_unstable_by(|left, right| {
            entries[*left]
                .0
                .cmp(&entries[*right].0)
                .then(left.cmp(right))
        });
        let mut kept: BumpVec<'_, usize> = BumpVec::with_capacity_in(entries.len(), scratch);
        for (position, index) in order.iter().enumerate() {
            let superseded = order
                .get(position + 1)
                .is_some_and(|next| entries[*next].0 == entries[*index].0);
            if !superseded {
                kept.push(*index);
            }
        }
        let (mut key_type, mut value_type) = (KType::NEVER, KType::NEVER);
        let mut weight = Weight::flat::<Self>();
        let keys = writer.fill(kept.len(), |at| {
            let key = entries[kept[at]].0;
            key_type = join(types, scratch, key_type, key.ktype());
            weight = weight.plus(key.weight());
            key.rehomed(writer)
        });
        let cells = writer.fill(kept.len(), |at| {
            let cell = entries[kept[at]].1;
            value_type = join(types, scratch, value_type, cell.ktype());
            weight = weight.plus(cell.weight());
            cell
        });
        Self::from_runs(
            writer,
            keys,
            cells,
            types.dict(key_type, value_type),
            weight,
        )
    }

    /// A dict over sorted keys and aligned cells already resident in `writer`'s region, under a type
    /// and weight the caller already knows — the deep copy's arm.
    pub(crate) fn from_runs(
        writer: Writer<'cell>,
        keys: &'cell [Key<'cell>],
        cells: &'cell [Value<'graph, 'cell>],
        ktype: KType,
        weight: Weight,
    ) -> &'cell Dict<'graph, 'cell> {
        resident(
            writer,
            Dict {
                keys,
                cells,
                ktype,
                weight,
            },
        )
    }

    /// The same entries under `ktype` — an ascription's retype, sharing both runs.
    pub fn with_type(&self, writer: Writer<'cell>, ktype: KType) -> &'cell Dict<'graph, 'cell> {
        Self::from_runs(writer, self.keys, self.cells, ktype, self.weight)
    }

    /// The cell under `key`, found by binary search; `key` may borrow anywhere.
    pub fn get(&self, key: &Key<'_>) -> Option<&'cell Value<'graph, 'cell>> {
        let cells = self.cells;
        self.keys
            .binary_search_by(|probe| probe.cmp(key))
            .ok()
            .map(|at| &cells[at])
    }

    /// The entries in key order.
    pub fn entries(
        &self,
    ) -> impl ExactSizeIterator<Item = (&'cell Key<'cell>, &'cell Value<'graph, 'cell>)>
    + use<'graph, 'cell> {
        self.keys.iter().zip(self.cells.iter())
    }

    pub fn keys(&self) -> &'cell [Key<'cell>] {
        self.keys
    }

    pub fn cells(&self) -> &'cell [Value<'graph, 'cell>] {
        self.cells
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    pub fn ktype(&self) -> KType {
        self.ktype
    }

    pub fn weight(&self) -> Weight {
        self.weight
    }
}
