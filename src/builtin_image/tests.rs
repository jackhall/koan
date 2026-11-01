//! The builtin image, pinned: the checked-in file is what the recipes generate over the seed, and
//! every digest and text in it is its static content's.

use std::fmt::Write;

use crate::dispatch::{Koan, overload_types};
use crate::elaborate::{builtin_error, builtin_result, builtin_shape_types, variant_named};
use crate::memory::Bump;
use crate::parse::{BUILTIN_SHAPES, ShapeElement};
use crate::program::{CellSubstrate, Output};
use crate::scope::FaultKind;
use crate::symbols::{SymbolInterner, TypeSymbol};
use crate::type_lattice::image::{check, check_texts, compare_or_update, emit, static_name};
use crate::type_lattice::{Halves, KType, RootBuilder, SEED};
use crate::values::{BuildBudget, Half, Types};

use super::{ERROR, ERROR_VARIANTS, IMAGE, OVERLOAD_TYPES, RESULT, SHAPE_TYPES, SYMBOLS};

/// The command that regenerates the image, which its header names.
const REGENERATE: &str = "UPDATE_IMAGE=1 cargo test -p koan --lib the_builtin_image_is_fresh";

/// The checked-in image.
const PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/builtin_image/generated.rs"
);

/// The path the generated file imports the lattice's image module by.
const LATTICE: &str = "crate::type_lattice::image";

/// The image as the recipes build it over the seed, through a chain the load's shape: `Result`,
/// `Error` and its variants, every native overload's shape type, then every dispatched builtin
/// shape's.
fn generated() -> String {
    let (bump, scratch) = (Bump::new(), Bump::new());
    let builder = RootBuilder::over(&bump, &SEED);
    let types = &Types::building(Halves::alone(Half::load(builder)));
    let symbols = SymbolInterner::new();
    let result = builtin_result(types, &symbols, &scratch);
    let error = builtin_error(types, &symbols, &scratch);
    let variants: Vec<KType> = FaultKind::ALL
        .iter()
        .map(|kind| {
            let label = TypeSymbol::declared(kind.name(), &symbols).expect("a kind names a type");
            variant_named(types, error, label.symbol()).expect("a kind's variant")
        })
        .collect();
    let overloads = overload_types(types, &symbols, &scratch);
    let shapes: Vec<&[KType]> = BUILTIN_SHAPES
        .iter()
        .map(|shape| {
            if !shape.dispatched() {
                return &[][..];
            }
            // A shape type names its keywords by symbol, so their texts are recorded here.
            for element in shape.elements {
                if let ShapeElement::Keyword(name) = element {
                    symbols.record(*name);
                }
            }
            builtin_shape_types(shape, types, &scratch)
        })
        .collect();
    let mut out = emit(builder.freeze(), &symbols, LATTICE, REGENERATE);
    out.push_str(&constants(result, error, &variants, overloads, &shapes));
    out
}

/// The constants the load reads, each a handle of the image's own node.
fn constants(
    result: KType,
    error: KType,
    variants: &[KType],
    overloads: &[KType],
    shapes: &[&[KType]],
) -> String {
    let handle = |kt: KType| {
        let name = static_name(kt.digest());
        assert!(name.starts_with("N_"), "{name} is no node of the image");
        format!("KType::imaged(&{name})")
    };
    let list = |handles: &[KType]| {
        let handles: Vec<String> = handles.iter().map(|kt| handle(*kt)).collect();
        format!("&[{}]", handles.join(", "))
    };
    let mut out = String::new();
    writeln!(out, "\npub const RESULT: KType = {};", handle(result)).expect("a String takes it");
    writeln!(out, "pub const ERROR: KType = {};", handle(error)).expect("a String takes it");
    writeln!(
        out,
        "pub const ERROR_VARIANTS: &[KType] = {};",
        list(variants)
    )
    .expect("a String takes it");
    writeln!(
        out,
        "pub const OVERLOAD_TYPES: &[KType] = {};",
        list(overloads)
    )
    .expect("a String takes it");
    out.push_str("pub const SHAPE_TYPES: &[&[KType]] = &[\n");
    for handles in shapes {
        writeln!(out, "    {},", list(handles)).expect("a String takes it");
    }
    out.push_str("];\n");
    out
}

#[test]
fn the_builtin_image_is_fresh() {
    compare_or_update(PATH, &generated(), REGENERATE);
}

#[test]
fn the_builtin_image_reads_back_as_written() {
    let mut written = emit(
        &IMAGE,
        &SymbolInterner::resting_on(&SYMBOLS),
        LATTICE,
        REGENERATE,
    );
    written.push_str(&constants(
        RESULT,
        ERROR,
        ERROR_VARIANTS,
        OVERLOAD_TYPES,
        SHAPE_TYPES,
    ));
    compare_or_update(PATH, &written, REGENERATE);
}

#[test]
fn every_builtin_image_digest_is_its_content() {
    assert_eq!(check(&IMAGE), Vec::<String>::new());
    assert_eq!(check_texts(&SYMBOLS), Vec::<String>::new());
}

/// An empty program, loaded: what a load builds beside the image.
fn empty() -> CellSubstrate {
    let output = Output {
        print: |_| {},
        error: |_| {},
    };
    CellSubstrate::load::<Koan>("", "<test>", 8, BuildBudget::default(), output)
        .unwrap_or_else(|error| panic!("the empty program loads: {error}"))
}

/// The load builds no builtin type: the empty program's root holds no node and no verdict of its
/// own, and its interner records none of the image's names.
#[test]
fn an_empty_program_loads_no_type_and_records_no_verdict() {
    empty().with(|running| {
        let frozen = running.program().tiers().frozen;
        assert_eq!(frozen.len(), 0);
        assert_eq!(frozen.verdict_count(), 0);
    });
}

#[test]
fn the_image_s_names_render_without_the_load_recording_them() {
    empty().with(|running| {
        let symbols = running.program().symbols();
        SYMBOLS.each(|text| {
            assert_eq!(symbols.render(text.symbol()), text.text());
            assert!(
                !symbols.records(text.symbol()),
                "{} is recorded",
                text.text()
            );
        });
    });
}
