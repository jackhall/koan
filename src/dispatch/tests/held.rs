//! Parts held in place: a call or `EVAL` written where its module's member is read is evaluated and
//! bound before the unit holding it runs, as a `LET` holding it would be at a statement of its own.
//! Each test pairs a program with its `LET` spelling — a `LET` immediately before the statement, or
//! inside the arm or body the part is written in — and asks that they print alike, refuse alike,
//! select alike and make as many instances through as many answer runs ([`spelled_alike`]). A
//! law draws such pairs ([`held`](super::generate::held)), each holder of a drawn body kept or
//! written in place.

use proptest::prelude::*;

use super::generate::{Holds, held};
use super::graphs::{BOTHS, SHOW_BOX, laid_alike, spelled_alike};

/// `BOTH` over a `Number` and a `Str`, and `SHOW`, which names its argument's type.
const BOTH_SHOW: &str = r##"EXPR #(BOTH a :Number b :Number) -> Str = #("two numbers")
EXPR #(BOTH a :Number b :Str) -> Str = #("number, string")
EXPR #(BOTH a :Str b :Number) -> Str = #("string, number")
EXPR #(SHOW n :Number) -> Str = #("number")
EXPR #(SHOW s :Str) -> Str = #("string")
"##;

/// `PICK`, a `BOX` behind a call.
const PICK: &str = "EXPR #(PICK x :Any) -> Module = #(\n  BOX x\n)\n";

/// `held` with each `(from, to)` of `written` replaced: the spelling writing its holders in place.
fn in_place(held: &str, written: &[(&str, &str)]) -> String {
    let mut inline = held.to_string();
    for (from, to) in written {
        assert_eq!(inline.matches(from).count(), 1, "{from}");
        inline = inline.replace(from, to);
    }
    inline
}

/// Each `(let, inline)` pair, each after `prelude`, run alike, as [`alike`] asks, but not laid
/// alike: the `LET` spelling of a part held in a `MODULE` body makes its holder a member of the
/// module, so the module's signature, and each question its type reads, reads the holder's
/// carried name too.
fn ran_alike(prelude: &str, pairs: &[(&str, &str)]) {
    for (held, inline) in pairs {
        spelled_alike(&[prelude, held].concat(), &[prelude, inline].concat());
    }
}

/// Each `(let, inline)` pair, each after `prelude`, spelled alike and laying alike graphs
/// ([`laid_alike`]).
fn alike(prelude: &str, pairs: &[(&str, &str)]) {
    for (held, inline) in pairs {
        let (held, inline) = ([prelude, held].concat(), [prelude, inline].concat());
        spelled_alike(&held, &inline);
        laid_alike(&held, &inline);
    }
}

#[test]
fn a_part_held_in_an_earlier_statement_reads_as_its_let_spelling_in_a_later_one() {
    let calls = r##"PRINT (USE #(MODULE made = (LET sample = 1)) #(MODULE made = (LET sample = "hi")))
PRINT (USE #(MODULE made = (LET sample = 2)) #(MODULE made = (LET sample = 3)))
"##;
    alike(
        BOTH_SHOW,
        &[(
            &[
                r##"EXPR #(USE left :Any right :Any) -> Str = #(
  PRINT "start"
  LET built = (EVAL left -> Module)
  LET first = (built.sample)
  LET second = (EVAL right -> Module)
  BOTH first (second.sample)
)
"##,
                calls,
            ]
            .concat(),
            &[
                r##"EXPR #(USE left :Any right :Any) -> Str = #(
  PRINT "start"
  LET first = ((EVAL left -> Module).sample)
  LET second = (EVAL right -> Module)
  BOTH first (second.sample)
)
"##,
                calls,
            ]
            .concat(),
        )],
    );
    // A refusal the later statement meets, after a print between.
    alike(
        BOTHS,
        &[(
            r##"EXPR #(USE left :Any right :Any) -> Str = #(
  PRINT "start"
  LET built = (EVAL left -> Module)
  LET first = (built.sample)
  LET second = (EVAL right -> Module)
  PRINT "between"
  BOTH first (second.sample)
)
PRINT (USE #(MODULE made = (LET sample = "yo")) #(MODULE made = (LET sample = "hi")))
"##,
            r##"EXPR #(USE left :Any right :Any) -> Str = #(
  PRINT "start"
  LET first = ((EVAL left -> Module).sample)
  LET second = (EVAL right -> Module)
  PRINT "between"
  BOTH first (second.sample)
)
PRINT (USE #(MODULE made = (LET sample = "yo")) #(MODULE made = (LET sample = "hi")))
"##,
        )],
    );
}

#[test]
fn a_part_held_beside_later_binders_prints_and_calls_and_catches_reads_as_its_let_spelling() {
    let three = r##"PRINT (USE #(MODULE made = (LET sample = 1)) #(MODULE made = (LET sample = 2)) #(MODULE made = (LET sample = "hi")))
PRINT (USE #(MODULE made = (LET sample = "yo")) #(MODULE made = (LET sample = 2)) #(MODULE made = (LET sample = "hi")))
"##;
    let two = r##"PRINT (USE #(MODULE made = (LET sample = 1)) #(MODULE made = (LET sample = "hi")))
PRINT (USE #(MODULE made = (LET sample = "yo")) #(MODULE made = (LET sample = "hi")))
"##;
    alike(
        BOTH_SHOW,
        &[
            (
                &[
                    r##"EXPR #(USE left :Any mid :Any right :Any) -> Str = #(
  PRINT "start"
  LET built = (EVAL left -> Module)
  LET first = (built.sample)
  LET second = (EVAL mid -> Module)
  PRINT "between"
  LET third = (EVAL right -> Module)
  PRINT "after"
  BOTH first (third.sample)
)
"##,
                    three,
                ]
                .concat(),
                &[
                    r##"EXPR #(USE left :Any mid :Any right :Any) -> Str = #(
  PRINT "start"
  LET first = ((EVAL left -> Module).sample)
  LET second = (EVAL mid -> Module)
  PRINT "between"
  LET third = (EVAL right -> Module)
  PRINT "after"
  BOTH first (third.sample)
)
"##,
                    three,
                ]
                .concat(),
            ),
            (
                &[
                    r##"EXPR #(USE left :Any mid :Any right :Any) -> Str = #(
  PRINT "start"
  LET built = (EVAL left -> Module)
  LET first = (built.sample)
  LET second = (EVAL mid -> Module)
  PRINT (BOTH first (second.sample))
  LET third = (EVAL right -> Module)
  PRINT "after"
  BOTH first (third.sample)
)
"##,
                    three,
                ]
                .concat(),
                &[
                    r##"EXPR #(USE left :Any mid :Any right :Any) -> Str = #(
  PRINT "start"
  LET first = ((EVAL left -> Module).sample)
  LET second = (EVAL mid -> Module)
  PRINT (BOTH first (second.sample))
  LET third = (EVAL right -> Module)
  PRINT "after"
  BOTH first (third.sample)
)
"##,
                    three,
                ]
                .concat(),
            ),
            (
                &[
                    r##"EXPR #(GRAB box :{p :Any}) -> Any = #(box.p.z)
EXPR #(USE left :Any right :Any) -> Str = #(
  LET built = (EVAL left -> Module)
  LET first = (built.sample)
  LET saved = (TRY (GRAB {p = {x = 1}}) WITH #{NoField: (0)})
  LET second = (EVAL right -> Module)
  PRINT "between"
  BOTH first (second.sample)
)
"##,
                    two,
                ]
                .concat(),
                &[
                    r##"EXPR #(GRAB box :{p :Any}) -> Any = #(box.p.z)
EXPR #(USE left :Any right :Any) -> Str = #(
  LET first = ((EVAL left -> Module).sample)
  LET saved = (TRY (GRAB {p = {x = 1}}) WITH #{NoField: (0)})
  LET second = (EVAL right -> Module)
  PRINT "between"
  BOTH first (second.sample)
)
"##,
                    two,
                ]
                .concat(),
            ),
        ],
    );
}

#[test]
fn a_part_held_before_an_arm_or_as_its_scrutinee_reads_as_its_let_spelling() {
    alike(
        BOTH_SHOW,
        &[
            (
                r##"EXPR #(USE n :Number left :Any right :Any) -> Str = #(
  LET built = (EVAL left -> Module)
  LET first = (built.sample)
  PRINT "between"
  MATCH n WITH #{
    0: (
      (LET second = (EVAL right -> Module))
      (PRINT "in arm")
      (BOTH first (second.sample))
    ),
    Number: ("other")
  }
)
PRINT (USE 0 #(MODULE made = (LET sample = 1)) #(MODULE made = (LET sample = "hi")))
PRINT (USE 1 #(MODULE made = (LET sample = 1)) #(MODULE made = (LET sample = "hi")))
PRINT (USE 0 #(MODULE made = (LET sample = "yo")) #(MODULE made = (LET sample = "hi")))
"##,
                r##"EXPR #(USE n :Number left :Any right :Any) -> Str = #(
  LET first = ((EVAL left -> Module).sample)
  PRINT "between"
  MATCH n WITH #{
    0: (
      (LET second = (EVAL right -> Module))
      (PRINT "in arm")
      (BOTH first (second.sample))
    ),
    Number: ("other")
  }
)
PRINT (USE 0 #(MODULE made = (LET sample = 1)) #(MODULE made = (LET sample = "hi")))
PRINT (USE 1 #(MODULE made = (LET sample = 1)) #(MODULE made = (LET sample = "hi")))
PRINT (USE 0 #(MODULE made = (LET sample = "yo")) #(MODULE made = (LET sample = "hi")))
"##,
            ),
            (
                r##"EXPR #(USE left :Any right :Any) -> Str = #(
  LET built = (EVAL left -> Module)
  MATCH (built.sample) WITH #{
    Any: (
      (LET second = (EVAL right -> Module))
      (PRINT "between")
      (BOTH it (second.sample))
    )
  }
)
PRINT (USE #(MODULE made = (LET sample = 1)) #(MODULE made = (LET sample = "hi")))
PRINT (USE #(MODULE made = (LET sample = "yo")) #(MODULE made = (LET sample = "hi")))
"##,
                r##"EXPR #(USE left :Any right :Any) -> Str = #(
  MATCH ((EVAL left -> Module).sample) WITH #{
    Any: (
      (LET second = (EVAL right -> Module))
      (PRINT "between")
      (BOTH it (second.sample))
    )
  }
)
PRINT (USE #(MODULE made = (LET sample = 1)) #(MODULE made = (LET sample = "hi")))
PRINT (USE #(MODULE made = (LET sample = "yo")) #(MODULE made = (LET sample = "hi")))
"##,
            ),
        ],
    );
}

#[test]
fn a_part_held_in_a_tail_recursion_reads_as_its_let_spelling_at_every_hop() {
    alike(
        BOTH_SHOW,
        &[(
            r##"EXPR #(TURN n :Number code :Any other :Any) -> Str = #(
  LET built = (EVAL code -> Module)
  LET first = (built.sample)
  LET second = (EVAL other -> Module)
  PRINT (BOTH first (second.sample))
  MATCH n WITH #{0: ("done"), Number: (TURN (n - 1) other code)}
)
PRINT (TURN 3 #(MODULE made = (LET sample = 1)) #(MODULE made = (LET sample = "hi")))
"##,
            r##"EXPR #(TURN n :Number code :Any other :Any) -> Str = #(
  LET first = ((EVAL code -> Module).sample)
  LET second = (EVAL other -> Module)
  PRINT (BOTH first (second.sample))
  MATCH n WITH #{0: ("done"), Number: (TURN (n - 1) other code)}
)
PRINT (TURN 3 #(MODULE made = (LET sample = 1)) #(MODULE made = (LET sample = "hi")))
"##,
        )],
    );
}

#[test]
fn a_call_held_under_a_generic_call_or_a_recursion_reads_as_its_let_spelling() {
    let boxes = [SHOW_BOX, PICK, BOTHS].concat();
    let listed = SHOW_BOX.replace("(LET sample = 1)", "(LET sample = [1])");
    alike(
        &boxes,
        &[
            (
                r##"EXPR #(USE code :Any) -> Str = #(
  LET built = (PICK (EVAL code -> Any))
  SHOW (TRIM (built.sample))
)
PRINT (USE #(5))
PRINT (USE #("a"))
"##,
                r##"EXPR #(USE code :Any) -> Str = #(
  SHOW (TRIM ((PICK (EVAL code -> Any)).sample))
)
PRINT (USE #(5))
PRINT (USE #("a"))
"##,
            ),
            (
                r##"EXPR #(USE code :Number) -> Str = #(
  LET built = (PICK 5)
  LET shown = (TRIM (built.sample))
  SHOW shown
)
PRINT (USE 0)
"##,
                r##"EXPR #(USE code :Number) -> Str = #(
  LET shown = (TRIM ((PICK 5).sample))
  SHOW shown
)
PRINT (USE 0)
"##,
            ),
            (
                r##"EXPR #(USE code :Number) -> Str = #(
  LET built = (PICK 5)
  LET shown = (built.sample)
  SHOW shown
)
PRINT (USE 0)
"##,
                r##"EXPR #(USE code :Number) -> Str = #(
  LET shown = ((PICK 5).sample)
  SHOW shown
)
PRINT (USE 0)
"##,
            ),
        ],
    );
    // A list the run's `SHOW` admits no overload of, refused alike.
    alike(
        &[&listed, PICK, BOTHS].concat(),
        &[(
            r##"EXPR #(USE code :Any) -> Str = #(
  LET built = (PICK (EVAL code -> Any))
  LET shown = (TRIM (built.sample))
  SHOW shown
)
PRINT (USE #(5))
"##,
            r##"EXPR #(USE code :Any) -> Str = #(
  LET shown = (TRIM ((PICK (EVAL code -> Any)).sample))
  SHOW shown
)
PRINT (USE #(5))
"##,
        )],
    );
    let ping = r##"EXPR #(BOTH a :Number b :Number) -> Str = #("two numbers")
EXPR #(BOTH a :Number b :Str) -> Str = #("number, string")
EXPR #(BOTH a :Str b :Number) -> Str = #("string, number")
EXPR FOR ALL #[Elem] #(PING x :Elem n :Number) -> Module = #(
  MATCH n WITH #{0: (BOX x), Number: (PONG x (n - 1))}
)
EXPR FOR ALL #[Elem] #(PONG x :Elem n :Number) -> Module = #(
  MATCH n WITH #{0: (BOX x), Number: (PING x (n - 1))}
)
"##;
    alike(
        &[SHOW_BOX, ping].concat(),
        &[(
            r##"EXPR #(USE code :Any) -> Str = #(
  LET result = (PING (EVAL code -> Any) 3)
  SHOW (result.sample)
)
PRINT (USE #(5))
PRINT (USE #("a"))
"##,
            r##"EXPR #(USE code :Any) -> Str = #(
  SHOW ((PING (EVAL code -> Any) 3).sample)
)
PRINT (USE #(5))
PRINT (USE #("a"))
"##,
        )],
    );
}

#[test]
fn a_part_held_after_a_call_that_prints_or_in_an_operator_run_reads_as_its_let_spelling() {
    alike(
        &[SHOW_BOX, BOTHS].concat(),
        &[(
            r##"EXPR #(USE code :Any) -> Str = #(
  LET built = (EVAL code -> Module)
  BOTH (TRIM 1) (TRIM (built.sample))
)
PRINT (USE #(MODULE made = (LET sample = (PRINT "module"))))
"##,
            r##"EXPR #(USE code :Any) -> Str = #(
  BOTH (TRIM 1) (TRIM ((EVAL code -> Module).sample))
)
PRINT (USE #(MODULE made = (LET sample = (PRINT "module"))))
"##,
        )],
    );
    alike(
        "",
        &[(
            r##"EXPR #(USE code :Any) -> Bool = #(
  PRINT "start"
  LET built = (EVAL code -> Module)
  0 < (built.sample) < 5
)
PRINT (USE #(MODULE made = (LET sample = 1)))
PRINT (USE #(MODULE made = (LET sample = 7)))
"##,
            r##"EXPR #(USE code :Any) -> Bool = #(
  PRINT "start"
  0 < ((EVAL code -> Module).sample) < 5
)
PRINT (USE #(MODULE made = (LET sample = 1)))
PRINT (USE #(MODULE made = (LET sample = 7)))
"##,
        )],
    );
}

#[test]
fn a_part_held_at_the_top_level_reads_as_its_let_spelling_in_later_statements() {
    let modules = |left: &str| {
        format!(
            "LET left = #(MODULE made = (LET sample = {left}))\n\
             LET right = #(MODULE made = (LET sample = \"hi\"))\n"
        )
    };
    let later = (
        "PRINT \"start\"
LET built = (EVAL left -> Module)
LET first = (built.sample)
LET second = (EVAL right -> Module)
PRINT \"between\"
PRINT (BOTH first (second.sample))
",
        "PRINT \"start\"
LET first = ((EVAL left -> Module).sample)
LET second = (EVAL right -> Module)
PRINT \"between\"
PRINT (BOTH first (second.sample))
",
    );
    for left in ["1", "\"yo\""] {
        alike(&[BOTH_SHOW, &modules(left)].concat(), &[later]);
    }
    alike(
        &[BOTH_SHOW, &modules("\"yo\"")].concat(),
        &[(
            "PRINT \"start\"
PRINT \"between\"
LET first = (EVAL left -> Module)
LET second = (EVAL right -> Module)
PRINT (BOTH (first.sample) (second.sample))
",
            "PRINT \"start\"
PRINT \"between\"
PRINT (BOTH ((EVAL left -> Module).sample) ((EVAL right -> Module).sample))
",
        )],
    );
    // A refusal at the top level, met where the holder binds: before `TRIM` prints.
    let listed = SHOW_BOX.replace("(LET sample = 1)", "(LET sample = [1])");
    alike(
        &[&listed, PICK, BOTHS, "LET code = #(5)\n"].concat(),
        &[(
            "LET built = (PICK (EVAL code -> Any))
LET shown = (TRIM (built.sample))
PRINT (SHOW shown)
",
            "LET shown = (TRIM ((PICK (EVAL code -> Any)).sample))
PRINT (SHOW shown)
",
        )],
    );
}

#[test]
fn a_part_held_in_a_module_body_at_the_top_level_reads_as_its_let_spelling() {
    ran_alike(
        BOTH_SHOW,
        &[(
            r##"LET left = #(MODULE made = (LET sample = 1))
LET right = #(MODULE made = (LET sample = "hi"))
MODULE pair = (
  (LET built = (EVAL left -> Module))
  (LET first = (built.sample))
  (PRINT "inside")
  (LET second = (EVAL right -> Module))
  (LET c = (BOTH first (second.sample)))
)
PRINT "between"
PRINT (pair.c)
LET third = (EVAL right -> Module)
LET fourth = (EVAL right -> Module)
PRINT (BOTH (third.sample) (fourth.sample))
"##,
            r##"LET left = #(MODULE made = (LET sample = 1))
LET right = #(MODULE made = (LET sample = "hi"))
MODULE pair = (
  (LET first = ((EVAL left -> Module).sample))
  (PRINT "inside")
  (LET c = (BOTH first ((EVAL right -> Module).sample)))
)
PRINT "between"
PRINT (pair.c)
PRINT (BOTH ((EVAL right -> Module).sample) ((EVAL right -> Module).sample))
"##,
        )],
    );
}

#[test]
fn a_part_held_in_an_arm_beside_one_its_unit_holds_reads_as_its_let_spelling() {
    let calls = r##"PRINT (USE 0 #(MODULE made = (LET sample = 1)) #(MODULE made = (LET sample = "hi")))
PRINT (USE 1 #(MODULE made = (LET sample = 1)) #(MODULE made = (LET sample = "hi")))
PRINT (USE 0 #(MODULE made = (LET sample = "yo")) #(MODULE made = (LET sample = "hi")))
"##;
    let pair = |held: &str, inline: &str| ([held, calls].concat(), [inline, calls].concat());
    let pairs = [
        pair(
            r##"EXPR #(USE n :Number left :Any right :Any) -> Str = #(
  LET first = (EVAL left -> Module)
  BOTH (first.sample) (MATCH n WITH #{
    0: (
      (LET second = (EVAL right -> Module))
      (second.sample)
    ),
    Number: 1
  })
)
"##,
            r##"EXPR #(USE n :Number left :Any right :Any) -> Str = #(
  BOTH ((EVAL left -> Module).sample) (MATCH n WITH #{
    0: ((EVAL right -> Module).sample),
    Number: 1
  })
)
"##,
        ),
        pair(
            r##"EXPR #(USE n :Number left :Any right :Any) -> Str = #(
  PRINT "start"
  LET second = (EVAL right -> Module)
  BOTH (MATCH n WITH #{
    0: (
      (LET built = (EVAL left -> Module))
      (built.sample)
    ),
    Number: 1
  }) (second.sample)
)
"##,
            r##"EXPR #(USE n :Number left :Any right :Any) -> Str = #(
  PRINT "start"
  BOTH (MATCH n WITH #{
    0: ((EVAL left -> Module).sample),
    Number: 1
  }) ((EVAL right -> Module).sample)
)
"##,
        ),
        pair(
            r##"EXPR #(USE n :Number code :Any right :Any) -> Str = #(
  LET shown = (MATCH n WITH #{
    0: (
      (LET built = (EVAL code -> Module))
      (built.sample)
    ),
    Number: (5)
  })
  LET second = (EVAL right -> Module)
  PRINT "between"
  BOTH shown (second.sample)
)
"##,
            r##"EXPR #(USE n :Number code :Any right :Any) -> Str = #(
  LET shown = (MATCH n WITH #{
    0: ((EVAL code -> Module).sample),
    Number: (5)
  })
  LET second = (EVAL right -> Module)
  PRINT "between"
  BOTH shown (second.sample)
)
"##,
        ),
    ];
    for (held, inline) in pairs.iter() {
        alike(BOTH_SHOW, &[(held, inline)]);
    }
}

#[test]
fn a_part_held_in_a_try_body_or_a_module_body_reads_as_its_let_spelling() {
    let calls = r##"PRINT (USE #(MODULE made = (LET sample = 1)) #(MODULE made = (LET sample = "hi")))
PRINT (USE #(MODULE made = (LET sample = "yo")) #(MODULE made = (LET sample = "hi")))
"##;
    let pair = |held: &str, inline: &str| ([held, calls].concat(), [inline, calls].concat());
    let pairs = [
        pair(
            r##"EXPR #(GRAB box :{p :Any}) -> Any = #(box.p.z)
EXPR #(USE left :Any right :Any) -> Str = #(
  TRY (
    (LET built = (EVAL left -> Module))
    (LET first = (built.sample))
    (LET second = (EVAL right -> Module))
    (PRINT "between")
    (PRINT (BOTH first (second.sample)))
    (GRAB {p = {x = 1}})
    ("unreached")
  ) WITH #{NoField: ("caught")}
)
"##,
            r##"EXPR #(GRAB box :{p :Any}) -> Any = #(box.p.z)
EXPR #(USE left :Any right :Any) -> Str = #(
  TRY (
    (LET first = ((EVAL left -> Module).sample))
    (LET second = (EVAL right -> Module))
    (PRINT "between")
    (PRINT (BOTH first (second.sample)))
    (GRAB {p = {x = 1}})
    ("unreached")
  ) WITH #{NoField: ("caught")}
)
"##,
        ),
        pair(
            r##"EXPR #(USE left :Any right :Any) -> Str = #(
  MODULE pair OVER #[left right] = (
    (LET built = (EVAL left -> Module))
    (LET first = (built.sample))
    (LET second = (EVAL right -> Module))
    (LET c = (BOTH first (second.sample)))
  )
  PRINT "between"
  pair.c
)
"##,
            r##"EXPR #(USE left :Any right :Any) -> Str = #(
  MODULE pair OVER #[left right] = (
    (LET first = ((EVAL left -> Module).sample))
    (LET second = (EVAL right -> Module))
    (LET c = (BOTH first (second.sample)))
  )
  PRINT "between"
  pair.c
)
"##,
        ),
        pair(
            r##"EXPR #(USE left :Any right :Any) -> Str = #(
  MODULE pair OVER #[left right] = (
    (LET first = (EVAL left -> Module))
    (LET second = (EVAL right -> Module))
    (LET c = (BOTH (first.sample) (second.sample)))
  )
  PRINT "between"
  pair.c
)
"##,
            r##"EXPR #(USE left :Any right :Any) -> Str = #(
  MODULE pair OVER #[left right] = (
    (LET c = (BOTH ((EVAL left -> Module).sample) ((EVAL right -> Module).sample)))
  )
  PRINT "between"
  pair.c
)
"##,
        ),
    ];
    for (held, inline) in pairs.iter() {
        ran_alike(BOTH_SHOW, &[(held, inline)]);
    }
}

#[test]
fn a_part_held_in_the_arms_of_a_tail_recursion_reads_as_its_let_spelling() {
    alike(
        BOTH_SHOW,
        &[(
            r##"EXPR #(LOOP n :Number seen :Any code :Any other :Any) -> Str = #(
  PRINT "turn"
  MATCH n WITH #{
    0: (
      (LET last = (EVAL code -> Module))
      (BOTH (seen.sample) (last.sample))
    ),
    Number: (
      (LET next = (EVAL code -> Module))
      (LOOP (n - 1) next other code)
    )
  }
)
LET start = (EVAL #(MODULE made = (LET sample = 1)) -> Module)
PRINT (LOOP 3 start #(MODULE made = (LET sample = 1)) #(MODULE made = (LET sample = "hi")))
"##,
            r##"EXPR #(LOOP n :Number seen :Any code :Any other :Any) -> Str = #(
  PRINT "turn"
  MATCH n WITH #{
    0: (BOTH (seen.sample) ((EVAL code -> Module).sample)),
    Number: (LOOP (n - 1) (EVAL code -> Module) other code)
  }
)
PRINT (LOOP 3 (EVAL #(MODULE made = (LET sample = 1)) -> Module) #(MODULE made = (LET sample = 1)) #(MODULE made = (LET sample = "hi")))
"##,
        )],
    );
}

/// A holder whose call runs a block binding a name the call reads, and a holder in a block
/// reading an earlier binder of it: each binder binds before the holder's value arrives, in
/// either spelling.
#[test]
fn a_holder_beside_a_block_its_call_runs_reads_as_its_let_spelling() {
    let prelude = [SHOW_BOX, PICK, BOTHS].concat();
    let calls = r##"PRINT (USE 0 #(MODULE made = (LET sample = 1)))
PRINT (USE 1 #(MODULE made = (LET sample = 1)))
"##;
    let pair = |held: &str, inline: &str| ([held, calls].concat(), [inline, calls].concat());
    let pairs = [
        pair(
            r##"EXPR #(USE n :Number code :Any) -> Str = #(
  PRINT "start"
  LET outer = (PICK (MATCH n WITH #{
    0: (
      (LET inner = (EVAL code -> Module))
      (inner.sample)
    ),
    Number: "five"
  }))
  SHOW (TRIM (outer.sample))
)
"##,
            r##"EXPR #(USE n :Number code :Any) -> Str = #(
  PRINT "start"
  SHOW (TRIM ((PICK (MATCH n WITH #{
    0: ((EVAL code -> Module).sample),
    Number: "five"
  })).sample))
)
"##,
        ),
        pair(
            r##"EXPR #(USE n :Number code :Any) -> Str = #(
  PRINT "start"
  SHOW (MATCH n WITH #{
    0: (
      (LET inner = (EVAL code -> Module))
      (LET boxed = (PICK (inner.sample)))
      (boxed.sample)
    ),
    Number: "five"
  })
)
"##,
            r##"EXPR #(USE n :Number code :Any) -> Str = #(
  PRINT "start"
  SHOW (MATCH n WITH #{
    0: ((PICK ((EVAL code -> Module).sample)).sample),
    Number: "five"
  })
)
"##,
        ),
    ];
    for (held, inline) in pairs.iter() {
        alike(&prelude, &[(held, inline)]);
    }
    alike(
        &prelude,
        &[(
            r##"LET code = #(MODULE made = (LET sample = 1))
LET n = 0
PRINT "start"
LET inner = (EVAL code -> Module)
LET outer = (PICK (inner.sample))
PRINT (SHOW (TRIM (outer.sample)))
PRINT "between"
LET picked = (MATCH n WITH #{
  0: (
    (LET built = (EVAL code -> Module))
    (built.sample)
  ),
  Number: "five"
})
LET last = (PICK "s")
PRINT (BOTH picked (last.sample))
"##,
            r##"LET code = #(MODULE made = (LET sample = 1))
LET n = 0
PRINT "start"
PRINT (SHOW (TRIM ((PICK ((EVAL code -> Module).sample)).sample)))
PRINT "between"
LET picked = (MATCH n WITH #{
  0: ((EVAL code -> Module).sample),
  Number: "five"
})
PRINT (BOTH picked ((PICK "s").sample))
"##,
        )],
    );
}

/// A callable born beside a binder typed over a held part, in an arm or a nested arm around it,
/// inside another callable, at each hop of a recursion, or written before it.
#[test]
fn a_callable_capturing_a_binder_over_a_held_part_reads_as_its_let_spelling() {
    let pairs: [(&str, &[(&str, &str)]); 8] = [
        (
            r##"EXPR #(USE left :Any right :Any) -> Str = #(
  LET built = (EVAL left -> Module)
  LET first = (built.sample)
  LET second = (EVAL right -> Module)
  LET go = (FN :{n :Number} -> Str = #(BOTH first (second.sample)))
  PRINT "between"
  go {n = 1}
)
PRINT (USE #(MODULE made = (LET sample = 1)) #(MODULE made = (LET sample = "hi")))
PRINT (USE #(MODULE made = (LET sample = "yo")) #(MODULE made = (LET sample = "hi")))
"##,
            &[(
                r##"  LET built = (EVAL left -> Module)
  LET first = (built.sample)
"##,
                r##"  LET first = ((EVAL left -> Module).sample)
"##,
            )],
        ),
        (
            r##"EXPR #(USE left :Any right :Any) -> Str = #(
  LET built = (EVAL left -> Module)
  LET first = (built.sample)
  LET go = (FN :{code :Any} -> Str = #(
    PRINT "called"
    LET second = (EVAL code -> Module)
    PRINT "between"
    BOTH first (second.sample)
  ))
  go {code = right}
)
PRINT (USE #(MODULE made = (LET sample = 1)) #(MODULE made = (LET sample = "hi")))
PRINT (USE #(MODULE made = (LET sample = "yo")) #(MODULE made = (LET sample = "hi")))
"##,
            &[(
                r##"  LET built = (EVAL left -> Module)
  LET first = (built.sample)
"##,
                r##"  LET first = ((EVAL left -> Module).sample)
"##,
            )],
        ),
        (
            r##"EXPR #(USE left :Any right :Any) -> Str = #(
  LET built = (EVAL left -> Module)
  LET first = (built.sample)
  LET second = (EVAL right -> Module)
  LET go = (FN :{n :Number} -> Str = #(
    LET inner = (FN :{m :Number} -> Str = #(BOTH first (second.sample)))
    PRINT "outer"
    inner {m = n}
  ))
  PRINT "between"
  go {n = 1}
)
PRINT (USE #(MODULE made = (LET sample = 1)) #(MODULE made = (LET sample = "hi")))
PRINT (USE #(MODULE made = (LET sample = "yo")) #(MODULE made = (LET sample = "hi")))
"##,
            &[(
                r##"  LET built = (EVAL left -> Module)
  LET first = (built.sample)
"##,
                r##"  LET first = ((EVAL left -> Module).sample)
"##,
            )],
        ),
        (
            r##"EXPR #(TURN n :Number code :Any other :Any) -> Str = #(
  LET built = (EVAL code -> Module)
  LET first = (built.sample)
  LET second = (EVAL other -> Module)
  LET go = (FN :{k :Number} -> Str = #(BOTH first (second.sample)))
  PRINT (go {k = n})
  MATCH n WITH #{0: ("done"), Number: (TURN (n - 1) other code)}
)
PRINT (TURN 3 #(MODULE made = (LET sample = 1)) #(MODULE made = (LET sample = "hi")))
"##,
            &[(
                r##"  LET built = (EVAL code -> Module)
  LET first = (built.sample)
"##,
                r##"  LET first = ((EVAL code -> Module).sample)
"##,
            )],
        ),
        (
            r##"EXPR #(USE left :Any right :Any) -> Str = #(
  LET go = (FN :{k :Number} -> Str = #(BOTH first (second.sample)))
  LET second = (EVAL right -> Module)
  LET built = (EVAL left -> Module)
  LET first = (built.sample)
  PRINT "between"
  go {k = 1}
)
PRINT (USE #(MODULE made = (LET sample = 1)) #(MODULE made = (LET sample = "hi")))
PRINT (USE #(MODULE made = (LET sample = "yo")) #(MODULE made = (LET sample = "hi")))
"##,
            &[(
                r##"  LET built = (EVAL left -> Module)
  LET first = (built.sample)
"##,
                r##"  LET first = ((EVAL left -> Module).sample)
"##,
            )],
        ),
        (
            r##"EXPR #(USE n :Number left :Any right :Any) -> Str = #(
  LET built = (EVAL left -> Module)
  LET first = (built.sample)
  MATCH n WITH #{
    0: (
      (LET second = (EVAL right -> Module))
      (LET go = (FN :{k :Number} -> Str = #(BOTH first (second.sample))))
      (PRINT "in arm")
      (go {k = 1})
    ),
    Number: ("other")
  }
)
PRINT (USE 0 #(MODULE made = (LET sample = 1)) #(MODULE made = (LET sample = "hi")))
PRINT (USE 1 #(MODULE made = (LET sample = 1)) #(MODULE made = (LET sample = "hi")))
PRINT (USE 0 #(MODULE made = (LET sample = "yo")) #(MODULE made = (LET sample = "hi")))
"##,
            &[(
                r##"  LET built = (EVAL left -> Module)
  LET first = (built.sample)
"##,
                r##"  LET first = ((EVAL left -> Module).sample)
"##,
            )],
        ),
        (
            r##"EXPR #(USE n :Number left :Any right :Any) -> Str = #(
  MATCH n WITH #{
    0: (
      (LET built = (EVAL left -> Module))
      (LET first = (built.sample))
      (MATCH n WITH #{
        0: (
          (LET second = (EVAL right -> Module))
          (LET go = (FN :{k :Number} -> Str = #(BOTH first (second.sample))))
          (go {k = 1})
        ),
        Number: ("inner other")
      })
    ),
    Number: ("other")
  }
)
PRINT (USE 0 #(MODULE made = (LET sample = 1)) #(MODULE made = (LET sample = "hi")))
PRINT (USE 1 #(MODULE made = (LET sample = 1)) #(MODULE made = (LET sample = "hi")))
PRINT (USE 0 #(MODULE made = (LET sample = "yo")) #(MODULE made = (LET sample = "hi")))
"##,
            &[(
                r##"      (LET built = (EVAL left -> Module))
      (LET first = (built.sample))
"##,
                r##"      (LET first = ((EVAL left -> Module).sample))
"##,
            )],
        ),
        (
            r##"EXPR #(USE n :Number left :Any right :Any) -> Str = #(
  LET built = (EVAL left -> Module)
  LET first = (built.sample)
  MATCH n WITH #{
    0: (
      (LET other = (EVAL right -> Module))
      (LET second = (other.sample))
      (LET go = (FN :{k :Number} -> Str = #(BOTH first second)))
      (PRINT "in arm")
      (go {k = 1})
    ),
    Number: ("other")
  }
)
PRINT (USE 0 #(MODULE made = (LET sample = 1)) #(MODULE made = (LET sample = "hi")))
PRINT (USE 1 #(MODULE made = (LET sample = 1)) #(MODULE made = (LET sample = "hi")))
PRINT (USE 0 #(MODULE made = (LET sample = "yo")) #(MODULE made = (LET sample = "hi")))
"##,
            &[
                (
                    r##"  LET built = (EVAL left -> Module)
  LET first = (built.sample)
"##,
                    r##"  LET first = ((EVAL left -> Module).sample)
"##,
                ),
                (
                    r##"      (LET other = (EVAL right -> Module))
      (LET second = (other.sample))
"##,
                    r##"      (LET second = ((EVAL right -> Module).sample))
"##,
                ),
            ],
        ),
    ];
    for (held, written) in pairs {
        alike(BOTH_SHOW, &[(held, &in_place(held, written))]);
    }
}

/// A `MODULE` body reading a binder typed over a held part, and a callable born in a `MODULE` body
/// or a `TRY` body holding the part, in a frame and at the top level.
#[test]
fn a_module_or_try_body_capturing_a_binder_over_a_held_part_reads_as_its_let_spelling() {
    let pairs: [(&str, &[(&str, &str)]); 4] = [
        (
            r##"EXPR #(USE left :Any right :Any) -> Str = #(
  LET built = (EVAL left -> Module)
  LET first = (built.sample)
  LET second = (EVAL right -> Module)
  MODULE pair OVER #[first second] = (
    (LET a = first)
    (LET b = (second.sample))
    (LET c = (BOTH first (second.sample)))
  )
  PRINT "between"
  pair.c
)
PRINT (USE #(MODULE made = (LET sample = 1)) #(MODULE made = (LET sample = "hi")))
PRINT (USE #(MODULE made = (LET sample = "yo")) #(MODULE made = (LET sample = "hi")))
"##,
            &[(
                r##"  LET built = (EVAL left -> Module)
  LET first = (built.sample)
"##,
                r##"  LET first = ((EVAL left -> Module).sample)
"##,
            )],
        ),
        (
            r##"EXPR #(USE left :Any right :Any) -> Str = #(
  MODULE pair OVER #[left right] = (
    (LET built = (EVAL left -> Module))
    (LET first = (built.sample))
    (LET second = (EVAL right -> Module))
    (LET go = (FN :{k :Number} -> Str = #(BOTH first (second.sample))))
    (LET c = (go {k = 1}))
  )
  PRINT "between"
  pair.c
)
PRINT (USE #(MODULE made = (LET sample = 1)) #(MODULE made = (LET sample = "hi")))
PRINT (USE #(MODULE made = (LET sample = "yo")) #(MODULE made = (LET sample = "hi")))
"##,
            &[(
                r##"    (LET built = (EVAL left -> Module))
    (LET first = (built.sample))
"##,
                r##"    (LET first = ((EVAL left -> Module).sample))
"##,
            )],
        ),
        (
            r##"LET left = #(MODULE made = (LET sample = 1))
LET right = #(MODULE made = (LET sample = "hi"))
MODULE pair = (
  (LET built = (EVAL left -> Module))
  (LET first = (built.sample))
  (LET second = (EVAL right -> Module))
  (LET go = (FN :{k :Number} -> Str = #(BOTH first (second.sample))))
  (LET c = (go {k = 1}))
)
PRINT "between"
PRINT (pair.c)
"##,
            &[(
                r##"  (LET built = (EVAL left -> Module))
  (LET first = (built.sample))
"##,
                r##"  (LET first = ((EVAL left -> Module).sample))
"##,
            )],
        ),
        (
            r##"EXPR #(GRAB box :{p :Any}) -> Any = #(box.p.z)
EXPR #(USE left :Any right :Any) -> Str = #(
  TRY (
    (LET built = (EVAL left -> Module))
    (LET first = (built.sample))
    (LET second = (EVAL right -> Module))
    (LET go = (FN :{k :Number} -> Str = #(BOTH first (second.sample))))
    (PRINT (go {k = 1}))
    (GRAB {p = {x = 1}})
    ("unreached")
  ) WITH #{NoField: ("caught")}
)
PRINT (USE #(MODULE made = (LET sample = 1)) #(MODULE made = (LET sample = "hi")))
PRINT (USE #(MODULE made = (LET sample = "yo")) #(MODULE made = (LET sample = "hi")))
"##,
            &[(
                r##"    (LET built = (EVAL left -> Module))
    (LET first = (built.sample))
"##,
                r##"    (LET first = ((EVAL left -> Module).sample))
"##,
            )],
        ),
    ];
    for (held, written) in pairs {
        ran_alike(BOTH_SHOW, &[(held, &in_place(held, written))]);
    }
}

/// At the top level, a callable written after a binder typed over a held part or before it, a
/// tie member capturing it, and two held parts among a tie's eager parts.
#[test]
fn a_callable_at_the_top_level_capturing_a_binder_over_a_held_part_reads_as_its_let_spelling() {
    let pairs: [(&str, &[(&str, &str)]); 4] = [
        (
            r##"LET left = #(MODULE made = (LET sample = "yo"))
LET right = #(MODULE made = (LET sample = "hi"))
LET built = (EVAL left -> Module)
LET first = (built.sample)
EXPR #(GO code :Any) -> Str = #(
  PRINT "called"
  LET second = (EVAL code -> Module)
  PRINT "between"
  BOTH first (second.sample)
)
PRINT (GO right)
"##,
            &[(
                r##"LET built = (EVAL left -> Module)
LET first = (built.sample)
"##,
                r##"LET first = ((EVAL left -> Module).sample)
"##,
            )],
        ),
        (
            r##"EXPR #(GO code :Any) -> Str = #(
  PRINT "called"
  LET second = (EVAL code -> Module)
  PRINT "between"
  BOTH first (second.sample)
)
LET left = #(MODULE made = (LET sample = "yo"))
LET right = #(MODULE made = (LET sample = "hi"))
LET built = (EVAL left -> Module)
LET first = (built.sample)
PRINT (GO right)
"##,
            &[(
                r##"LET built = (EVAL left -> Module)
LET first = (built.sample)
"##,
                r##"LET first = ((EVAL left -> Module).sample)
"##,
            )],
        ),
        (
            r##"LET left = #(MODULE made = (LET sample = "yo"))
LET right = #(MODULE made = (LET sample = "hi"))
LET built = (EVAL left -> Module)
LET first = (built.sample)
LET a = [first f]
LET f = (FN :{code :Any} -> Str = #(
  LET second = (EVAL code -> Module)
  PRINT "in f"
  BOTH first (second.sample)
))
PRINT (f {code = right})
"##,
            &[(
                r##"LET built = (EVAL left -> Module)
LET first = (built.sample)
"##,
                r##"LET first = ((EVAL left -> Module).sample)
"##,
            )],
        ),
        (
            r##"EXPR #(USE left :Any right :Any) -> Str = #(
  LET one = (EVAL left -> Module)
  LET two = (EVAL right -> Module)
  LET a = {x = (one.sample), y = (two.sample), g = f}
  LET f = (FN :{k :Number} -> Str = #(BOTH (a.x) (a.y)))
  PRINT "between"
  f {k = 1}
)
PRINT (USE #(MODULE made = (LET sample = 1)) #(MODULE made = (LET sample = "hi")))
PRINT (USE #(MODULE made = (LET sample = "yo")) #(MODULE made = (LET sample = "hi")))
"##,
            &[(
                r##"  LET one = (EVAL left -> Module)
  LET two = (EVAL right -> Module)
  LET a = {x = (one.sample), y = (two.sample), g = f}
"##,
                r##"  LET a = {x = ((EVAL left -> Module).sample), y = ((EVAL right -> Module).sample), g = f}
"##,
            )],
        ),
    ];
    for (held, written) in pairs {
        alike(BOTH_SHOW, &[(held, &in_place(held, written))]);
    }
}

/// A part held among a tie's eager parts is evaluated once, before the tie.
#[test]
fn a_part_held_among_a_ties_eager_parts_is_evaluated_once() {
    alike(
        "LET code = #(MODULE made = (LET sample = (PRINT \"module\")))\n",
        &[(
            r##"LET built = (EVAL code -> Module)
LET a = [built f]
LET f = (FN :{x :Number} -> Any = #(a))
PRINT "done"
"##,
            r##"LET a = [(EVAL code -> Module) f]
LET f = (FN :{x :Number} -> Any = #(a))
PRINT "done"
"##,
        )],
    );
}

/// `Ord`, a signature over its `Carrier`, a module fitting it, and `SAME`, which asks its two
/// arguments to be of one type.
const ORD_SAME: &str = r##"SIG Ord FOR ALL #[Carrier] = #[(VAL zero :Carrier)]
MODULE ints = (LET zero = 0)
LET fixed = (ints :| Ord)
EXPR FOR ALL #[Elem] #(SAME a :Elem b :Elem) -> Str = #("same")
"##;

/// A view written over a part held in place reads as a view over its `LET` binder: its carrier is
/// named through the holder, so `SAME` is selected where the load can name both carriers, and
/// refused at the run where they differ.
#[test]
fn a_view_over_a_part_held_in_place_reads_as_its_let_spelling() {
    let one = r##"EXPR #(CHECK code :Any) -> Str = #(
  LET built = (EVAL code -> Module)
  SAME ((built :| Ord).zero) (fixed.zero)
)
PRINT (CHECK #(MODULE made = (LET zero = 0)))
"##;
    let two = r##"EXPR #(CHECK left :Any right :Any) -> Str = #(
  LET first = (EVAL left -> Module)
  LET second = (EVAL right -> Module)
  SAME ((first :| Ord).zero) ((second :| Ord).zero)
)
PRINT (CHECK #(MODULE made = (LET zero = 0)) #(MODULE made = (LET zero = 0)))
"##;
    let bound = r##"EXPR #(CHECK left :Any right :Any) -> Str = #(
  PRINT "start"
  LET first = (EVAL left -> Module)
  LET v = (first :| Ord)
  LET second = (EVAL right -> Module)
  PRINT "between"
  SAME (v.zero) ((second :| Ord).zero)
)
PRINT (CHECK #(MODULE made = (LET zero = 0)) #(MODULE made = (LET zero = 0)))
PRINT (CHECK #(MODULE made = (LET zero = 0)) #(MODULE made = (LET zero = 1)))
"##;
    let pairs: [(&str, &[(&str, &str)]); 3] = [
        (
            one,
            &[
                ("  LET built = (EVAL code -> Module)\n", ""),
                ("(built :| Ord)", "((EVAL code -> Module) :| Ord)"),
            ],
        ),
        (
            two,
            &[
                ("  LET first = (EVAL left -> Module)\n", ""),
                ("  LET second = (EVAL right -> Module)\n", ""),
                ("(first :| Ord)", "((EVAL left -> Module) :| Ord)"),
                ("(second :| Ord)", "((EVAL right -> Module) :| Ord)"),
            ],
        ),
        (
            bound,
            &[
                ("  LET first = (EVAL left -> Module)\n", ""),
                ("(first :| Ord)", "((EVAL left -> Module) :| Ord)"),
            ],
        ),
    ];
    for (held, written) in pairs {
        alike(ORD_SAME, &[(held, &in_place(held, written))]);
    }
}

/// A quantified member read off a part held in place is instanced as one read off its `LET`
/// binder: where its module's head parameter is pinned at a path through the holder.
#[test]
fn a_quantified_member_read_off_a_part_held_in_place_reads_as_its_let_spelling() {
    let prelude = r##"SIG Mapper FOR ALL #[Carrier] = #[(VAL seed :Carrier) (VAL apply :(FN FOR ALL #[Elt] :{x :Carrier, y :Elt} -> Elt))]
MODULE nums = ((LET seed = 1) (LET apply = (FN FOR ALL #[Elt] :{x :Number, y :Elt} -> Elt = #(y))))
EXPR #(LOOSE m :Mapper) -> Mapper = #(m)
"##;
    let held = r##"EXPR #(VIA m :Mapper) -> Any = #(
  LET held = (LOOSE m)
  LET go :(FN :{x :Number, y :Number} -> Number) = held.apply
  go {x = 1, y = 2}
)
PRINT (VIA nums)
"##;
    let written = [
        ("  LET held = (LOOSE m)\n", ""),
        ("held.apply", "(LOOSE m).apply"),
    ];
    alike(prelude, &[(held, &in_place(held, &written))]);
}

#[test]
fn a_call_whose_overload_a_holder_decides_reads_as_its_let_spelling() {
    let boxes = [
        BOTH_SHOW,
        "EXPR #(BOX n :Number) -> Module = #(MODULE made = (LET sample = 1))\n",
        "EXPR #(BOX s :Str) -> Module = #(MODULE made = (LET sample = \"hi\"))\n",
    ]
    .concat();
    let calls = r##"PRINT (USE #(MODULE made = (LET sample = 1)))
PRINT (USE #(MODULE made = (LET sample = "hi")))
"##;
    let framed = r##"EXPR #(USE left :Any) -> Str = #(
  LET built = (EVAL left -> Module)
  LET boxed = (BOX (built.sample))
  SHOW (boxed.sample)
)
"##;
    let top = r##"LET left = #(MODULE made = (LET sample = 1))
LET built = (EVAL left -> Module)
LET boxed = (BOX (built.sample))
PRINT (SHOW (boxed.sample))
"##;
    let read = ("(boxed.sample)", "((BOX (built.sample)).sample)");
    let framed = [framed, calls].concat();
    alike(
        &boxes,
        &[
            (
                &framed,
                &in_place(
                    &framed,
                    &[("  LET boxed = (BOX (built.sample))\n", ""), read],
                ),
            ),
            (
                top,
                &in_place(top, &[("LET boxed = (BOX (built.sample))\n", ""), read]),
            ),
        ],
    );
}

/// Under Miri: each part held in place is evaluated into its activation's held slot before its unit
/// and read back from it — at the top level among a tie's eager parts, in an arm across tail hops,
/// in a frame's tail statement whose value the frame hands on, and through a lambda's capture of a
/// binder over one.
#[test]
fn a_part_held_in_place_is_read_from_its_slot_wherever_its_frame_hands_it() {
    let source = r##"EXPR #(GO n :Number code :Any) -> Any = #(
  MATCH n WITH #{0: ((EVAL code -> Module).sample), Number: (GO (n - 1) code)}
)
EXPR #(USE left :Any) -> Any = #(
  LET first = ((EVAL left -> Module).sample)
  LET go = (FN :{n :Number} -> Any = #(first))
  PRINT (go {n = 1})
  ((EVAL left -> Module).sample)
)
LET code = #(MODULE made = (LET sample = 1))
LET a = [(EVAL code -> Module) f]
LET f = (FN :{x :Number} -> Any = #(a))
PRINT (GO 1 code)
PRINT (USE #(MODULE made = (LET sample = "hi")))
"##;
    assert_eq!(super::run(source), "1\nhi\nhi");
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: crate::tests::case_share(1, 4),
        ..ProptestConfig::default()
    })]

    /// A body writing any of its holders in place runs as its `LET` spelling, as [`alike`] asks: at the top level, in a called frame, an arm, a `TRY` body, a
    /// `MODULE` body and a nested callable. It lays alike graphs too, but in a `MODULE` body, where
    /// the `LET` spelling makes each holder a member ([`ran_alike`]).
    #[test]
    fn a_body_holding_any_of_its_holders_in_place_reads_as_its_let_spelling(
        program in held(),
        written in any::<u8>(),
    ) {
        let (held, inline) = program.render(|holder| written >> (holder % 8) & 1 == 1);
        spelled_alike(&held, &inline);
        if program.holds != Holds::Module {
            laid_alike(&held, &inline);
        }
    }
}

/// A spliced `EVAL` held in place, whose holder no question names, is a seam as its `LET` spelling
/// is: the frame binds the part once it is held, so the call reading a path through it closes
/// under what the part holds.
#[test]
fn a_spliced_eval_no_question_names_reads_as_its_let_spelling() {
    let prelude = "\
SIG Counter FOR ALL #[Carrier] = #[(VAL zero :Carrier)]
EXPR FOR ALL #[Cr] #(ECHO elem :(Counter WITH {Carrier = Cr})) -> Cr = #(elem.zero)
MODULE ints = ((LET zero = 0))
";
    let held = "\
EXPR #(VIA g :Any) -> Any = #(
  PRINT \"in\"
  LET m = (EVAL #($g) -> Counter)
  ECHO m
)
PRINT (VIA ints)
";
    let inline = in_place(
        held,
        &[
            ("  LET m = (EVAL #($g) -> Counter)\n", ""),
            ("ECHO m", "ECHO (EVAL #($g) -> Counter)"),
        ],
    );
    assert_eq!(super::run(&[prelude, &inline].concat()), "in\n0");
    alike(prelude, &[(held, &inline)]);
}
