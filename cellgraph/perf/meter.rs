//! Per-verb metering: what one call to a public door cost, exclusive of the doors it ran inside
//! it.
//!
//! A benchmark wraps every door call in [`measure`]. Frames nest — an `alloc` runs inside an
//! `enter` — so each frame subtracts what its children spent before adding to its verb's total.
//! `enter` therefore reports the step machinery's own cost and not the value work the step did,
//! which is what makes a row per verb mean anything.
//!
//! The meter itself must not allocate inside a frame, or it would measure itself: the totals are
//! a fixed-size array, and [`reset`] pre-reserves the frame stack past any depth a shape reaches.

use std::cell::RefCell;
use std::time::Instant;

use crate::counting_alloc;

/// Declares [`Verb`] from one table of `Variant => "csv name", Level | Rate` rows, and with it
/// [`Verb::ALL`], [`Verb::name`] and [`Verb::is_level`], so none of the four can say a different
/// thing about a verb. `ALL` is written in the table's order, which is discriminant order, so the
/// tally a row reads at `totals[verb as usize]` belongs to the verb `ALL` zips that row against.
///
/// The kind column says what the row is: a `Rate` is a cost the meter times, a `Level` is one
/// reading of a standing figure.
macro_rules! verbs {
    (@is_level Level) => {
        true
    };
    (@is_level Rate) => {
        false
    };
    ($($(#[$attribute:meta])* $variant:ident => $name:literal, $kind:ident;)*) => {
        /// The public doors a row can be about, plus [`Verb::Harness`] for the benchmark's own
        /// bookkeeping that has to happen inside a step — building an operand list. That work is
        /// reported as its own row rather than folded into a door's, so nothing is hidden and
        /// nothing inflates a real verb.
        ///
        /// [`Verb::Resident`] is no door either: it is the row a shape [`record`]s the bytes it
        /// still holds under, read off the allocator's live balance before the shape tears down.
        #[derive(Clone, Copy)]
        pub enum Verb {
            $($(#[$attribute])* $variant,)*
        }

        impl Verb {
            /// How many rows the table declares.
            pub const COUNT: usize = [$(Verb::$variant,)*].len();

            /// Every verb, in the order rows are printed.
            pub const ALL: [Verb; Verb::COUNT] = [$(Verb::$variant,)*];

            /// The name the row carries. Lower-case and stable: the recorded dataframe keys on it.
            pub fn name(self) -> &'static str {
                match self {
                    $(Verb::$variant => $name,)*
                }
            }

            /// Whether the row is a level rather than a cost. A level is a single reading taken
            /// outside every frame, so it has no block of time for a floor to be about.
            pub fn is_level(self) -> bool {
                match self {
                    $(Verb::$variant => verbs!(@is_level $kind),)*
                }
            }
        }
    };
}

verbs! {
    Create => "create", Rate;
    Enter => "enter", Rate;
    Release => "release", Rate;
    CreateTree => "create_tree", Rate;
    /// The `enter` door taken on a tree cell, rowed apart from the slab case.
    EnterTree => "enter/tree", Rate;
    ReleaseTree => "release_tree", Rate;
    Alloc => "alloc", Rate;
    AllocInto => "alloc_into", Rate;
    Hold => "hold", Rate;
    Keep => "keep", Rate;
    Redeem => "redeem", Rate;
    Read => "read", Rate;
    Register => "register_receipts", Rate;
    Deliver => "deliver_scratch", Rate;
    Receipt => "receipt", Rate;
    Harness => "harness", Rate;
    Resident => "resident", Level;
}

/// One verb's total over a benchmark run. Totals, not means: a per-call figure is a division on
/// read, and keeping the count exact is what lets the record gate on it.
#[derive(Clone, Copy, Default)]
pub struct Tally {
    pub calls: u64,
    pub allocations: u64,
    pub bytes: u64,
    pub nanos: u64,
}

/// One open [`measure`] call: what the tallies read when it opened, and what the frames it has
/// since closed spent.
struct Frame {
    verb: Verb,
    allocations_at: u64,
    bytes_at: u64,
    started: Instant,
    inner_allocations: u64,
    inner_bytes: u64,
    inner_nanos: u64,
}

struct Meter {
    totals: [Tally; Verb::ALL.len()],
    stack: Vec<Frame>,
}

thread_local! {
    static METER: RefCell<Meter> = RefCell::new(Meter {
        totals: [Tally::default(); Verb::ALL.len()],
        stack: Vec::new(),
    });
}

/// Zero the totals and drop any open frame. Called between benchmarks, never inside one, so the
/// reservation it makes is an allocation no row sees.
pub fn reset() {
    METER.with_borrow_mut(|meter| {
        meter.totals = [Tally::default(); Verb::ALL.len()];
        meter.stack.clear();
        meter.stack.reserve(16);
    });
}

/// The totals since the last [`reset`], indexed by [`Verb::ALL`].
pub fn take() -> [Tally; Verb::ALL.len()] {
    METER.with_borrow(|meter| meter.totals)
}

/// Set `verb`'s row to one reading of `bytes`, outside every frame — how a shape reports a figure
/// that is a level rather than a cost. The row's time is a constant `1`: there is nothing timed
/// behind it, and the record's readers divide by a row's time.
pub fn record(verb: Verb, bytes: u64) {
    METER.with_borrow_mut(|meter| {
        debug_assert!(
            meter.stack.is_empty(),
            "a level is recorded outside every frame"
        );
        meter.totals[verb as usize] = Tally {
            calls: 1,
            allocations: 0,
            bytes,
            nanos: 1,
        };
    });
}

/// Run `body` as one call to `verb`, charging it what it spent minus what the verbs inside it did.
///
/// The meter's borrow is never held across `body`: a door called inside another re-enters here.
pub fn measure<R>(verb: Verb, body: impl FnOnce() -> R) -> R {
    METER.with_borrow_mut(|meter| {
        meter.stack.push(Frame {
            verb,
            allocations_at: counting_alloc::thread_allocations(),
            bytes_at: counting_alloc::thread_bytes(),
            started: Instant::now(),
            inner_allocations: 0,
            inner_bytes: 0,
            inner_nanos: 0,
        });
    });

    let result = body();

    METER.with_borrow_mut(|meter| {
        let frame = meter.stack.pop().expect("a frame closes the one it opened");
        let allocations = counting_alloc::thread_allocations() - frame.allocations_at;
        let bytes = counting_alloc::thread_bytes() - frame.bytes_at;
        let nanos = frame.started.elapsed().as_nanos() as u64;

        let total = &mut meter.totals[frame.verb as usize];
        total.calls += 1;
        total.allocations += allocations - frame.inner_allocations;
        total.bytes += bytes - frame.inner_bytes;
        total.nanos += nanos.saturating_sub(frame.inner_nanos);

        if let Some(parent) = meter.stack.last_mut() {
            parent.inner_allocations += allocations;
            parent.inner_bytes += bytes;
            parent.inner_nanos += nanos;
        }
    });

    result
}
