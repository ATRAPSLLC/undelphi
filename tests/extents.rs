//! The extents [`DelphiBinary::structure_extents`] states, checked against
//! what the structures' own fields say, on the sample corpus.
//!
//! The committed samples must be present; a test that reads one fails without
//! it. The gitignored corpus is optional: a sample that is not present is
//! skipped, and its path is named on stderr.

#![allow(
    clippy::arithmetic_side_effects,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::{collections::HashSet, env, fs, path::Path};

use undelphi::{
    DelphiBinary,
    extents::{StructureExtent, StructureKind},
    fields::Field,
    rtti::{InitTableLayout, RecordInfo, StringLayout, TypeDetail, TypeKind},
    vmt::VmtFlavor,
    vmttables::AutoEntry,
};

/// Reads a sample under `tests/samples`, or `None` when it is absent.
fn load(rel: &str) -> Option<Vec<u8>> {
    fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/samples")
            .join(rel),
    )
    .ok()
}

/// The bytes `extent` covers in `bin`'s file.
fn bytes<'a>(bin: &DelphiBinary<'a>, extent: &StructureExtent) -> &'a [u8] {
    let ctx = bin.ctx();
    let start = ctx.va_to_file(extent.extent.start).expect("in the file");
    let len = usize::try_from(extent.extent.end - extent.extent.start).expect("fits");
    ctx.data()
        .get(start..start + len)
        .expect("whole in the file")
}

/// Checks the extents of one sample: every class has a header of the size
/// its flavor documents and a class name whose bytes are its short string,
/// and every Delphi published method's inline name lies in its table's
/// extent.
fn check(rel: &str) {
    let Some(data) = load(rel) else {
        eprintln!("skipping: sample missing");
        return;
    };
    let bin = DelphiBinary::parse(&data).expect("a Delphi or FPC binary");
    let extents = bin.structure_extents();
    let of_kind = |kind: StructureKind| {
        extents
            .iter()
            .filter(move |structure| structure.kind == kind)
            .collect::<Vec<_>>()
    };
    assert!(!bin.classes().is_empty(), "{rel}: classes");
    for class in bin.classes().iter() {
        let vmt = &class.vmt;
        let pointer = u64::from(vmt.pointer_size);
        let header = vmt.header_extent().expect("a header");
        let slots = (header.end - header.start) / pointer;
        match vmt.flavor {
            // At least `vmtSelfPtr` to `vmtParent`.
            VmtFlavor::Delphi => assert!(slots >= 11, "{rel}: {} has {slots}", class.name()),
            VmtFlavor::Fpc => assert_eq!(slots, 12, "{rel}: {}", class.name()),
        }
        let name = StructureExtent {
            extent: vmt.class_name_extent().expect("a name"),
            kind: StructureKind::ClassName,
        };
        let held = bytes(&bin, &name);
        assert_eq!(
            usize::from(held[0]),
            held.len() - 1,
            "{rel}: {}",
            class.name()
        );
        assert_eq!(&held[1..], class.name().as_bytes(), "{rel}");
        if vmt.flavor == VmtFlavor::Delphi {
            let tables = of_kind(StructureKind::MethodTable);
            let table = tables
                .iter()
                .find(|structure| structure.extent.start == vmt.method_table);
            for method in bin.methods(class) {
                let table = table.expect("a table holding its methods");
                let held = bytes(&bin, table);
                let at = method.name_bytes().as_ptr() as usize;
                let range = held.as_ptr() as usize..held.as_ptr() as usize + held.len();
                assert!(
                    range.contains(&at),
                    "{rel}: {}.{} outside {:x?}",
                    class.name(),
                    method.name(),
                    table.extent
                );
            }
        }
    }
    for kind in [
        StructureKind::VmtHeader,
        StructureKind::ClassName,
        StructureKind::TypeInfo,
    ] {
        assert!(!of_kind(kind).is_empty(), "{rel}: no {kind:?}");
    }
}

#[test]
fn delphi_7_extents_match_their_structures() {
    check("lightalloy/LA.exe");
}

#[test]
fn delphi_12_x64_extents_match_their_structures() {
    check("heidisql/portable_x64/heidisql.exe");
}

#[test]
fn fpc_extents_match_their_structures() {
    check("known-rtti/known.win32.exe");
}

/// Checks the RTTI records of one sample: an ordinal type's record is its
/// Kind byte, its name and nine bytes of data, an enumeration's ends with its
/// unit name's short string, and a string's ends at its name or, for an ANSI
/// string on a code-paged runtime, at the code page after it.
fn check_type_records(rel: &str, layout: StringLayout) {
    let Some(data) = load(rel) else {
        eprintln!("skipping: sample missing");
        return;
    };
    let bin = DelphiBinary::parse(&data).expect("a Delphi or FPC binary");
    assert_eq!(bin.string_layout(), layout, "{rel}");
    let (mut ordinals, mut enumerations, mut strings) = (0, 0, 0);
    for detail in bin.types() {
        let Some(extent) = detail.extent() else {
            continue;
        };
        let record = StructureExtent {
            extent: extent.clone(),
            kind: StructureKind::TypeInfo,
        };
        let held = bytes(&bin, &record);
        match &detail {
            TypeDetail::Ordinal(ordinal) => {
                ordinals += 1;
                assert_eq!(
                    held.len(),
                    2 + ordinal.header.name_bytes().len() + 9,
                    "{rel}"
                );
            }
            TypeDetail::Enumeration(enumeration) => {
                if let Some(unit) = enumeration.unit_name_bytes() {
                    enumerations += 1;
                    let tail = &held[held.len() - unit.len() - 1..];
                    assert_eq!(usize::from(tail[0]), unit.len(), "{rel}");
                    assert_eq!(&tail[1..], unit, "{rel}");
                }
            }
            TypeDetail::String(string) => {
                strings += 1;
                let name = string.header.name_bytes();
                let data = if string.code_page.is_some() { 2 } else { 0 };
                assert_eq!(held.len(), 2 + name.len() + data, "{rel}");
                let expected = match name {
                    b"UTF8String" => Some(65001),
                    b"RawByteString" => Some(65535),
                    _ => None,
                };
                if let Some(code_page) = expected
                    && layout == StringLayout::CodePaged
                {
                    assert_eq!(string.code_page, Some(code_page), "{rel}");
                }
            }
            _ => {}
        }
    }
    assert!(
        ordinals > 0 && enumerations > 0 && strings > 0,
        "{rel}: {ordinals} ordinals, {enumerations} enumerations, {strings} strings"
    );
}

#[test]
fn delphi_7_type_records_end_where_their_data_does() {
    check_type_records("lightalloy/LA.exe", StringLayout::Plain);
}

#[test]
fn delphi_xe5_type_records_end_where_their_data_does() {
    check_type_records(
        "heidisql/portable_x86_xe5/heidisql.unpacked.exe",
        StringLayout::CodePaged,
    );
}

#[test]
fn fpc_type_records_end_where_their_data_does() {
    check_type_records("known-rtti/known.win32.exe", StringLayout::CodePaged);
}

/// Checks the string constants of one sample: each is a record whose
/// reference count is `-1`, whose length states its characters and whose
/// terminator ends it, one of them holds `text`, there are `expected` of
/// them, and none lies across another structure. In a 32-bit image the code
/// loads a constant by its characters' absolute address, so each one's
/// address is somewhere in the file: a record the walk matched by chance in
/// the middle of code would have no such pointer.
fn check_string_literals(rel: &str, expected: usize, text: &str) {
    let Some(data) = load(rel) else {
        eprintln!("skipping: sample missing");
        return;
    };
    let bin = DelphiBinary::parse(&data).expect("a Delphi or FPC binary");
    let extents = bin.structure_extents();
    let literals: Vec<&StructureExtent> = extents
        .iter()
        .filter(|structure| structure.kind == StructureKind::StringLiteral)
        .collect();
    let code_paged = bin.string_layout() == StringLayout::CodePaged;
    let words: HashSet<u32> = if bin.ctx().pointer_size() == Some(4) {
        data.windows(4)
            .map(|word| u32::from_le_bytes(word.try_into().expect("four bytes")))
            .collect()
    } else {
        HashSet::new()
    };
    let mut found = false;
    for literal in &literals {
        let held = bytes(&bin, literal);
        let (count, width) = if code_paged {
            let width = usize::from(u16::from_le_bytes([held[2], held[3]]));
            (4, width)
        } else {
            (0, 1)
        };
        assert_eq!(
            held[count..count + 4],
            [0xFF; 4],
            "{rel}: {:#x?}",
            literal.extent
        );
        let length =
            u32::from_le_bytes(held[count + 4..count + 8].try_into().expect("four bytes")) as usize;
        let characters = &held[count + 8..held.len() - width];
        if !words.is_empty() {
            let address = u32::try_from(literal.extent.start + count as u64 + 8).expect("32-bit");
            assert!(
                words.contains(&address),
                "{rel}: nothing points at {:#x?}",
                literal.extent
            );
        }
        assert_eq!(
            characters.len(),
            length * width,
            "{rel}: {:#x?}",
            literal.extent
        );
        assert!(
            held[held.len() - width..].iter().all(|byte| *byte == 0),
            "{rel}: {:#x?}",
            literal.extent
        );
        let decoded = if width == 2 {
            String::from_utf16_lossy(
                &characters
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|unit| u16::from_le_bytes(*unit))
                    .collect::<Vec<_>>(),
            )
        } else {
            String::from_utf8_lossy(characters).into_owned()
        };
        found |= decoded == text;
    }
    assert_eq!(literals.len(), expected, "{rel}");
    assert!(expected == 0 || found, "{rel}: no literal {text:?}");
    for literal in &literals {
        for other in extents
            .iter()
            .filter(|structure| structure.kind != StructureKind::StringLiteral)
        {
            assert!(
                literal.extent.end <= other.extent.start
                    || other.extent.end <= literal.extent.start,
                "{rel}: literal {:#x?} across {:?} {:#x?}",
                literal.extent,
                other.kind,
                other.extent
            );
        }
    }
}

#[test]
fn delphi_7_string_literals_are_their_records() {
    check_string_literals("lightalloy/LA.exe", 6_383, "PlayList.Font");
}

#[test]
fn delphi_xe5_string_literals_are_their_records() {
    check_string_literals(
        "heidisql/portable_x86_xe5/heidisql.unpacked.exe",
        8_646,
        "SELECT ",
    );
}

#[test]
fn delphi_12_x64_string_literals_are_their_records() {
    check_string_literals("heidisql/portable_x64/heidisql.exe", 24_386, "SELECT ");
}

/// Free Pascal keeps its string constants in read-only data, so none lies in
/// the code.
#[test]
fn fpc_string_literals_are_not_in_the_code() {
    check_string_literals("known-rtti/known.win32.exe", 0, "");
}

/// **Delphi 6 ground truth.** `tables.exe` is built from known source whose
/// `Literals` procedure assigns three constants and a 302-character one the
/// compiler folds from a concatenation, each a record in `CODE` before the
/// procedure; its `-GD` map names every routine's start. Each constant is a
/// literal the walk finds, and no symbol the map places in the code section
/// lies inside one.
#[test]
fn delphi_6_string_literals_match_the_source_and_the_map() {
    let Some(data) = load("bcb6/tables.exe") else {
        eprintln!("skipping: sample missing");
        return;
    };
    let Some(map) = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/samples/bcb6/tables.map"),
    )
    .ok() else {
        eprintln!("skipping: map missing");
        return;
    };
    let bin = DelphiBinary::parse(&data).expect("a Delphi binary");
    let literals: Vec<StructureExtent> = bin
        .structure_extents()
        .into_iter()
        .filter(|structure| structure.kind == StructureKind::StringLiteral)
        .collect();
    let texts: Vec<&[u8]> = literals
        .iter()
        .map(|literal| {
            let held = bytes(&bin, literal);
            &held[8..held.len() - 1]
        })
        .collect();
    for text in [
        &b"undelphi-literal-alpha"[..],
        b"undelphi-literal-beta-0123456789",
        b"undelphi-literal-gamma",
    ] {
        assert!(texts.contains(&text), "{}", String::from_utf8_lossy(text));
    }
    assert!(
        texts
            .iter()
            .any(|text| text.len() == 302 && text.starts_with(b"undelphi-long-literal-")),
        "the folded constant is one literal"
    );

    let code = bin.ctx().sections().text.expect("a code section").va;
    let symbols: Vec<u64> = map
        .lines()
        .skip_while(|line| !line.contains("Publics by Value"))
        .filter_map(|line| {
            let offset = line.trim_start().strip_prefix("0001:")?.get(..8)?;
            Some(code + u64::from_str_radix(offset, 16).ok()?)
        })
        .collect();
    assert!(symbols.len() > 1_000, "{} symbols", symbols.len());
    for symbol in symbols {
        assert!(
            !literals
                .iter()
                .any(|literal| literal.extent.contains(&symbol)),
            "{symbol:#x} inside a literal"
        );
    }
}

/// **The tables Delphi 6 ground truth names.** In `tables.exe`, `TAutoThing`
/// has an automation table of four entries, a count and twenty bytes each,
/// and `TOwnerThing`'s published fields name two classes, a count and a
/// pointer each; both tables lie in the code section, and both are declared.
#[test]
fn delphi_6_automation_and_field_class_tables_are_declared() {
    let Some(data) = load("bcb6/tables.exe") else {
        eprintln!("skipping: sample missing");
        return;
    };
    let bin = DelphiBinary::parse(&data).expect("a Delphi binary");
    let extents = bin.structure_extents();
    let class = |name: &str| {
        bin.classes()
            .iter()
            .find(|class| class.name() == name)
            .unwrap_or_else(|| panic!("{name}"))
            .vmt
    };
    let declared = |kind: StructureKind, len: u64| {
        extents
            .iter()
            .filter(|structure| structure.kind == kind)
            .map(|structure| structure.extent.end - structure.extent.start)
            .any(|size| size == len)
    };
    assert!(AutoEntry::table_extent(bin.ctx(), &class("TAutoThing")).is_some());
    assert!(declared(StructureKind::AutoTable, 4 + 4 * 20));
    assert!(Field::class_table_extent(bin.ctx(), &class("TOwnerThing")).is_some());
    assert!(declared(StructureKind::FieldClassTable, 2 + 2 * 4));
}

/// **The runtime's own records in Delphi 6's code section.** `tables.exe`
/// keeps a record per `resourcestring`, the module handle cell and the
/// identifier, two pointers each, `ResAlpha`'s among them; `known.exe` keeps
/// its unit initialization table there; and `gtforms.exe` keeps copies of
/// the interface GUIDs it hands `Supports` among its code, one before
/// `THelpManager.ShowHelp`. Each is declared, and none overlaps another
/// structure.
#[test]
fn delphi_6_runtime_records_are_declared() {
    let of_kind = |rel: &str, kind: StructureKind| -> Option<Vec<StructureExtent>> {
        let data = load(rel)?;
        let bin = DelphiBinary::parse(&data).expect("a Delphi binary");
        let extents = bin.structure_extents();
        for (index, one) in extents.iter().enumerate() {
            for other in &extents[index + 1..] {
                assert!(
                    one.extent == other.extent
                        || one.extent.end <= other.extent.start
                        || other.extent.end <= one.extent.start,
                    "{rel}: {:?} {:#x?} across {:?} {:#x?}",
                    one.kind,
                    one.extent,
                    other.kind,
                    other.extent
                );
            }
        }
        Some(
            extents
                .into_iter()
                .filter(|structure| structure.kind == kind)
                .collect(),
        )
    };
    let Some(resources) = of_kind("bcb6/tables.exe", StructureKind::ResourceString) else {
        eprintln!("skipping: sample missing");
        return;
    };
    assert_eq!(resources.len(), 117);
    assert!(
        resources
            .iter()
            .all(|record| record.extent.end - record.extent.start == 8)
    );
    assert!(
        resources
            .iter()
            .any(|record| record.extent.start == 0x41_ded0)
    );
    let tables = of_kind("bcb6/known.exe", StructureKind::UnitInitTable).expect("known.exe");
    assert_eq!(tables.len(), 1);
    assert_eq!(tables[0].extent.start, 0x41_e28c);
    let guids: Vec<u64> = of_kind("bcb6/gtforms.exe", StructureKind::GuidConstant)
        .expect("gtforms.exe")
        .iter()
        .map(|guid| guid.extent.start)
        .collect();
    // `THelpManager.ShowHelp`'s copy of an interface the binary declares; the
    // one `TWriter.WriteVariant` keeps is of an interface it declares nowhere,
    // so nothing here can name it.
    assert!(guids.contains(&0x42_c478), "{guids:#x?}");
    assert!(!guids.contains(&0x42_0864), "{guids:#x?}");
}

/// Reads a committed sample, failing when it is absent.
fn committed(rel: &str) -> Vec<u8> {
    load(rel).unwrap_or_else(|| panic!("committed sample {rel} is missing"))
}

/// A `jmp [reg*4 + table]` whose jump table follows it puts the table's
/// address one pointer past the instruction's own operand, which is the shape
/// of a Delphi type record's self-referencing cell. `bcb6/deep.exe` holds one
/// at `0x40fff9`: `jmp [eax*4 + 0x410000]`, then the table, whose first entry
/// `0x4101dd` reads as Kind byte `0xdd`, a one-character name and a record.
/// That record is no type, so neither `types()` nor `structure_extents` states
/// it, and the table stays free for the code that owns it.
#[test]
fn a_jump_table_operand_is_not_a_type_cell() {
    let data = committed("bcb6/deep.exe");
    let bin = DelphiBinary::parse(&data).expect("a Delphi binary");
    let ctx = bin.ctx();
    let at = |va: u64, len: usize| {
        let start = ctx.va_to_file(va).expect("mapped");
        &data[start..start + len]
    };
    assert_eq!(at(0x40_fff9, 3), [0xff, 0x24, 0x85], "jmp [eax*4 + disp32]");
    assert_eq!(at(0x40_fffc, 4), 0x41_0000u32.to_le_bytes(), "its operand");
    assert_eq!(at(0x41_0000, 1), [0xdd], "the table's first entry");
    assert!(
        bin.types()
            .iter()
            .all(|detail| detail.header().va != 0x41_0000),
        "the jump table is read as a type record"
    );
    assert!(
        bin.structure_extents()
            .iter()
            .all(|structure| structure.extent.end <= 0x40_fff9
                || 0x41_0003 <= structure.extent.start),
        "a structure covers the jump table's first entry"
    );
}

/// Light Alloy holds two of those jump tables, at `0x4201d4` and `0x4afea8`;
/// neither is stated as a type record or in any extent.
#[test]
fn delphi_7_jump_tables_are_not_type_records() {
    let Some(data) = load("lightalloy/LA.exe") else {
        eprintln!("skipping: sample lightalloy/LA.exe missing");
        return;
    };
    let bin = DelphiBinary::parse(&data).expect("a Delphi binary");
    let types = bin.types();
    let extents = bin.structure_extents();
    for table in [0x42_01d4u64, 0x4a_fea8] {
        assert!(
            types.iter().all(|detail| detail.header().va != table),
            "{table:#x} is read as a type record"
        );
        assert!(
            extents
                .iter()
                .all(|structure| structure.extent.end <= table - 4
                    || table + 3 <= structure.extent.start),
            "a structure covers {table:#x}"
        );
    }
}

/// Checks that no type record of one sample is of kind `Unknown`, which is
/// not a valid type info: every one that decodes names a real kind.
fn check_no_unknown_types(data: &[u8], rel: &str) {
    let bin = DelphiBinary::parse(data).expect("a Delphi or FPC binary");
    let types = bin.types();
    assert!(!types.is_empty(), "{rel}: no types");
    for detail in &types {
        let header = detail.header();
        assert_ne!(
            header.kind,
            TypeKind::Unknown,
            "{rel}: {:#x} {:?} (kind byte {:#x})",
            header.va,
            header.name(),
            header.kind_byte
        );
    }
}

/// The committed samples must be present. The gitignored corpus is optional,
/// as this file's header states, and a skipped sample is named.
#[test]
fn no_sample_type_record_is_of_unknown_kind() {
    for rel in [
        "bcb6/deep.exe",
        "bcb6/gtforms.exe",
        "d13/win32/gtforms.exe",
        "d13/win64/gtforms.exe",
    ] {
        check_no_unknown_types(&committed(rel), rel);
    }
    for rel in [
        "lightalloy/LA.exe",
        "idr-builds/Idr64.exe",
        "heidisql/portable_x64/heidisql.exe",
        "known-rtti/known.win32.exe",
        "doublecmd/linux_x86_64/doublecmd.elf",
    ] {
        match load(rel) {
            Some(data) => check_no_unknown_types(&data, rel),
            None => eprintln!("skipping: sample {rel} missing"),
        }
    }
}

/// Checks the init tables of one sample: each is a record of Kind `tkRecord`
/// with an empty name, a size, a managed-field count and as many pairs of two
/// pointers, then, when the sample's compiler emits it (`with_operators`), the
/// operator section of `NumOps` and as many pointers. Its extent ends where
/// the stored counts say, whatever they are, and the next structure the
/// binary states starts exactly there. Returns how many tables the sample
/// states.
fn check_init_tables(data: &[u8], rel: &str, with_operators: bool) -> usize {
    let bin = DelphiBinary::parse(data).expect("a Delphi binary");
    assert_eq!(
        bin.init_table_layout(),
        if with_operators {
            InitTableLayout::WithOperators
        } else {
            InitTableLayout::ManagedFields
        },
        "{rel}: layout"
    );
    let pointer = u64::try_from(bin.ctx().pointer_size().expect("a pointer size")).expect("fits");
    let extents = bin.structure_extents();
    let tables: Vec<&StructureExtent> = extents
        .iter()
        .filter(|structure| structure.kind == StructureKind::InitTable)
        .collect();
    let mut starts: HashSet<u64> = HashSet::new();
    let mut followed = 0usize;
    for class in bin.classes().iter() {
        let Some(table) = bin.init_table(class) else {
            continue;
        };
        starts.insert(class.vmt.init_table);
        let stated = tables
            .iter()
            .find(|structure| structure.extent.start == class.vmt.init_table)
            .unwrap_or_else(|| panic!("{rel}: {} has no init table extent", class.name()));
        let held = bytes(&bin, stated);
        assert_eq!(held[0], 0x0e, "{rel}: {} Kind byte", class.name());
        assert_eq!(held[1], 0, "{rel}: {} name length", class.name());
        let count = u64::from(u32::from_le_bytes(
            held[6..10].try_into().expect("four bytes"),
        ));
        assert_eq!(
            count,
            u64::try_from(table.managed_fields.len()).expect("fits"),
            "{rel}: {} managed fields",
            class.name()
        );
        let pairs_end = 10 + count * 2 * pointer;
        let expected = if with_operators {
            let num_ops = u64::from(
                *held
                    .get(usize::try_from(pairs_end).expect("fits"))
                    .unwrap_or_else(|| panic!("{rel}: {} has no NumOps", class.name())),
            );
            assert!(num_ops <= 4, "{rel}: {} NumOps {num_ops}", class.name());
            assert_eq!(
                table.operators.is_some(),
                num_ops > 0,
                "{rel}: {} operators",
                class.name()
            );
            pairs_end + 1 + num_ops * pointer
        } else {
            assert!(table.operators.is_none(), "{rel}: {}", class.name());
            pairs_end
        };
        assert_eq!(
            stated.extent.end - stated.extent.start,
            expected,
            "{rel}: {} extent",
            class.name()
        );
        let next = extents
            .iter()
            .map(|structure| structure.extent.start)
            .filter(|&start| start >= stated.extent.end)
            .min();
        if let Some(next) = next {
            assert_eq!(
                next,
                stated.extent.end,
                "{rel}: {} is not followed by the next structure",
                class.name()
            );
            followed += 1;
        }
    }
    assert_eq!(tables.len(), starts.len(), "{rel}: init table extents");
    assert!(
        followed > 0,
        "{rel}: no init table is followed by a structure"
    );
    for table in &tables {
        for other in extents
            .iter()
            .filter(|structure| structure.kind != StructureKind::InitTable)
        {
            assert!(
                table.extent.end <= other.extent.start || other.extent.end <= table.extent.start,
                "{rel}: init table {:#x?} across {:?} {:#x?}",
                table.extent,
                other.kind,
                other.extent
            );
        }
    }
    tables.len()
}

/// Delphi 6 ends an init table at its last managed field; Delphi 13, on both
/// Win32 and Win64, follows it with the operator section.
#[test]
fn delphi_init_tables_end_where_their_count_does() {
    for (rel, with_operators) in [
        ("bcb6/deep.exe", false),
        ("bcb6/gtforms.exe", false),
        ("d13/win32/gtforms.exe", true),
        ("d13/win64/gtforms.exe", true),
    ] {
        let data = committed(rel);
        assert!(
            check_init_tables(&data, rel, with_operators) > 0,
            "{rel}: no init table"
        );
    }
}

/// The gitignored corpus holds the other compilers: the Delphi 12 builds of
/// HeidiSQL and the DelphiLint client (36.0), dated by their build strings,
/// and two binaries no build string dates, HeidiSQL's Delphi XE5 build and
/// the C++Builder build of IDR64, whose own tables show that they end at
/// their managed fields.
#[test]
fn delphi_init_tables_follow_each_compilers_layout() {
    for (rel, with_operators) in [
        ("heidisql/portable_x86_xe5/heidisql.unpacked.exe", false),
        ("heidisql/portable_x64/heidisql.exe", true),
        ("delphilint/DelphiLintClient-1.3.0-Athens.bpl", true),
        ("idr-builds/Idr64.exe", false),
    ] {
        match load(rel) {
            Some(data) => {
                assert!(check_init_tables(&data, rel, with_operators) > 0, "{rel}");
            }
            None => eprintln!("skipping: sample {rel} missing"),
        }
    }
}

/// Returns `data` with its Delphi build string overwritten with zero bytes,
/// so no compiler version dates the binary.
fn without_build_string(mut data: Vec<u8>) -> Vec<u8> {
    let marker = b"Embarcadero Delphi for ";
    let start = data
        .windows(marker.len())
        .position(|window| window == marker)
        .expect("a build string");
    let len = data[start..]
        .iter()
        .position(|&byte| byte == 0)
        .expect("a terminated build string");
    data[start..start + len].fill(0);
    data
}

/// Checks that `bin`, which no compiler version dates, reads its init tables
/// with the operator section, and that the reading without it is refuted by
/// the binary itself: each table then ends one byte short of the structure
/// that follows it.
fn check_undated_with_operators(data: &[u8], rel: &str) -> usize {
    let bin = DelphiBinary::parse(data).expect("a Delphi binary");
    assert_eq!(
        bin.compiler().and_then(|info| info.version),
        None,
        "{rel}: dated"
    );
    let tables = check_init_tables(data, rel, true);
    let extents = bin.structure_extents();
    let mut short = 0usize;
    for class in bin.classes().iter() {
        let Some(table) =
            RecordInfo::init_table_extent(bin.ctx(), &class.vmt, InitTableLayout::ManagedFields)
        else {
            continue;
        };
        if let Some(next) = extents
            .iter()
            .map(|structure| structure.extent.start)
            .filter(|&start| start > table.start)
            .min()
        {
            assert_eq!(next, table.end + 1, "{rel}: {}", class.name());
            short += 1;
        }
    }
    assert!(short > 0, "{rel}: no table is followed");
    tables
}

/// A binary no compiler version dates shows its init table layout by its own
/// tables. The committed Delphi 13 programs with their build strings zeroed
/// read the operator section, as their dated originals do, and the committed
/// C++Builder 6 programs, whose marker carries no version, end every table at
/// its managed fields.
#[test]
fn undated_init_tables_show_their_layout() {
    for rel in ["d13/win32/gtforms.exe", "d13/win64/gtforms.exe"] {
        let dated = check_init_tables(&committed(rel), rel, true);
        let undated = check_undated_with_operators(&without_build_string(committed(rel)), rel);
        assert_eq!(undated, dated, "{rel}: tables");
    }
    for rel in ["bcb6/deep.exe", "bcb6/gtforms.exe"] {
        let data = committed(rel);
        let bin = DelphiBinary::parse(&data).expect("a Delphi binary");
        assert_eq!(
            bin.compiler().and_then(|info| info.version),
            None,
            "{rel}: dated"
        );
        assert!(check_init_tables(&data, rel, false) > 0, "{rel}");
    }
}

/// The RAD Studio 13 command-line compiler carries only the product marker
/// `Embarcadero RAD Studio 37.0`, which dates nothing, and imports no RTL
/// package; its 65 init tables show the operator section. Read from the
/// install `UNDELPHI_D13` names.
#[test]
fn undated_delphi_13_compiler_reads_the_operator_section() {
    let Some(root) = env::var_os("UNDELPHI_D13") else {
        eprintln!("skipping: UNDELPHI_D13 not set");
        return;
    };
    let path = Path::new(&root).join("bin/dcc32.EXE");
    let Ok(data) = fs::read(&path) else {
        eprintln!("skipping: {} missing", path.display());
        return;
    };
    assert_eq!(check_undated_with_operators(&data, "dcc32.EXE"), 65);
}

/// Light Alloy's 124 init tables hold 3,640 bytes between them, in the code
/// section among the routines.
#[test]
fn delphi_7_init_tables_are_declared() {
    let Some(data) = load("lightalloy/LA.exe") else {
        eprintln!("skipping: sample lightalloy/LA.exe missing");
        return;
    };
    assert_eq!(check_init_tables(&data, "lightalloy/LA.exe", false), 124);
}
