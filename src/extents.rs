//! Where the structures this crate reads lie in the image.
//!
//! A Delphi or FPC binary keeps its class metadata in the same section as its
//! code: VMT headers, virtual-method arrays, class names, the published
//! method, field, field class, dynamic, interface and automation tables,
//! interface vtables and RTTI records sit between functions, and Delphi puts
//! its string constants there too. A disassembler that does not know where
//! they are decodes them as instructions.
//! [`DelphiBinary::structure_extents`] states each one's extent, measured by
//! the walk that reads it.

use std::{collections::HashSet, ops::Range};

use crate::{
    DelphiBinary,
    fields::Field,
    interfaces::{Guid, InterfaceEntry},
    methods::MethodEntry,
    rtti::{StringLayout, TypeDetail},
    util::{read_ptr, read_u16, read_u32},
    vmt::VmtFlavor,
    vmttables::{AutoEntry, DynamicSlot},
};

/// The reference count of a string constant, which the runtime never frees.
const CONSTANT_REFERENCE_COUNT: [u8; 4] = [0xFF; 4];
/// The flag an interface's RTTI sets when it states a GUID (`ifHasGuid`).
const INTERFACE_HAS_GUID: u8 = 0x01;
/// The code page a code-paged runtime stamps on a UTF-16 string.
const UTF16_CODE_PAGE: u16 = 1200;

/// What a [`StructureExtent`] holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StructureKind {
    /// A VMT's fixed header, before its first virtual-method slot.
    VmtHeader,
    /// A VMT's virtual-method pointers.
    VirtualMethods,
    /// A class name's short string.
    ClassName,
    /// A published-method table, with Delphi's extended section.
    MethodTable,
    /// A published-field table.
    FieldTable,
    /// The class-types table a legacy published-field table names: the
    /// classes its fields hold, one pointer each.
    FieldClassTable,
    /// An automation table: the dispatch entries of a class's `automated`
    /// section.
    AutoTable,
    /// A dynamic-dispatch table.
    DynamicTable,
    /// An interface table.
    InterfaceTable,
    /// The method pointers of an interface a class implements.
    InterfaceVtable,
    /// An RTTI record - its Kind byte, its name and the kind-specific data
    /// the decode read - and on Delphi the pointer to it that precedes it.
    TypeInfo,
    /// A string constant the compiler placed among the code: its string
    /// record header, its characters and its terminator.
    StringLiteral,
    /// A `resourcestring`'s record: the module handle cell and the string's
    /// identifier the runtime loads it by.
    ResourceString,
    /// The unit initialization table: its header and each unit's
    /// initialization and finalization pointers.
    UnitInitTable,
    /// A copy of an interface's GUID that a routine keeps among the code and
    /// passes by address, as `Supports` and `QueryInterface` take it.
    GuidConstant,
}

impl StructureKind {
    /// Returns a short human-readable name, for provenance labels.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::VmtHeader => "VMT header",
            Self::VirtualMethods => "VMT virtual methods",
            Self::ClassName => "class name",
            Self::MethodTable => "published-method table",
            Self::FieldTable => "published-field table",
            Self::FieldClassTable => "field class table",
            Self::AutoTable => "automation table",
            Self::DynamicTable => "dynamic-dispatch table",
            Self::InterfaceTable => "interface table",
            Self::InterfaceVtable => "interface vtable",
            Self::TypeInfo => "RTTI type record",
            Self::StringLiteral => "string literal",
            Self::ResourceString => "resourcestring record",
            Self::UnitInitTable => "unit initialization table",
            Self::GuidConstant => "interface GUID",
        }
    }
}

/// The virtual addresses one structure occupies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructureExtent {
    /// Its half-open virtual-address range.
    pub extent: Range<u64>,
    /// What it is.
    pub kind: StructureKind,
}

impl<'a> DelphiBinary<'a> {
    /// Returns the extent of every class-metadata structure this crate reads,
    /// ascending by address.
    ///
    /// Each is measured by the walk that decodes it, so a table whose walk
    /// bails out is left out rather than guessed at. Structures several
    /// classes share, such as an RTTI record two properties name, appear once.
    pub fn structure_extents(&self) -> Vec<StructureExtent> {
        let ctx = self.ctx();
        let mut out = Vec::new();
        let mut push = |extent: Option<Range<u64>>, kind: StructureKind| {
            if let Some(extent) = extent.filter(|extent| extent.start < extent.end) {
                out.push(StructureExtent { extent, kind });
            }
        };
        for class in self.classes().iter() {
            let vmt = &class.vmt;
            let pointer = u64::from(vmt.pointer_size);
            push(vmt.header_extent(), StructureKind::VmtHeader);
            push(vmt.class_name_extent(), StructureKind::ClassName);
            let slots = self.virtual_methods(class);
            push(
                slots
                    .iter()
                    .map(|slot| slot.slot_va)
                    .min()
                    .zip(
                        slots
                            .iter()
                            .map(|slot| slot.slot_va)
                            .max()
                            .and_then(|last| last.checked_add(pointer)),
                    )
                    .map(|(first, end)| first..end),
                StructureKind::VirtualMethods,
            );
            push(
                MethodEntry::table_extent(ctx, vmt),
                StructureKind::MethodTable,
            );
            push(Field::table_extent(ctx, vmt), StructureKind::FieldTable);
            push(
                Field::class_table_extent(ctx, vmt),
                StructureKind::FieldClassTable,
            );
            push(AutoEntry::table_extent(ctx, vmt), StructureKind::AutoTable);
            push(
                DynamicSlot::table_extent(ctx, vmt),
                StructureKind::DynamicTable,
            );
            push(
                InterfaceEntry::table_extent(ctx, vmt),
                StructureKind::InterfaceTable,
            );
            for interface in self.interfaces_with_methods(class) {
                let len = u64::try_from(interface.methods.len())
                    .ok()
                    .and_then(|count| count.checked_mul(pointer));
                push(
                    len.and_then(|len| {
                        let start = interface.interface.vtable_va;
                        Some(start..start.checked_add(len)?)
                    }),
                    StructureKind::InterfaceVtable,
                );
            }
        }
        // Delphi precedes a type record with a pointer to it, the cell a
        // `PPTypeInfo` names; FPC does not. Taken only where the cell holds
        // the record's own address.
        let pointer = ctx.pointer_size().unwrap_or(4);
        for detail in self.types() {
            let Some(mut extent) = detail.extent() else {
                continue;
            };
            let cell = u64::try_from(pointer)
                .ok()
                .and_then(|width| extent.start.checked_sub(width));
            if let Some(cell) = cell
                && ctx
                    .va_to_file(cell)
                    .and_then(|offset| read_ptr(ctx.data(), offset, pointer))
                    == Some(extent.start)
            {
                extent.start = cell;
            }
            push(Some(extent), StructureKind::TypeInfo);
        }
        for literal in self.string_literals() {
            push(Some(literal), StructureKind::StringLiteral);
        }
        let width = u64::try_from(pointer).unwrap_or(4);
        for resource in self.resource_strings() {
            push(
                resource
                    .record_va
                    .and_then(|record| Some(record..record.checked_add(width.checked_mul(2)?)?)),
                StructureKind::ResourceString,
            );
        }
        push(
            self.unit_init_table()
                .and_then(|table| table.extent(pointer)),
            StructureKind::UnitInitTable,
        );
        let held: Vec<Range<u64>> = out
            .iter()
            .map(|structure| structure.extent.clone())
            .collect();
        out.extend(
            self.guid_constants(&held)
                .into_iter()
                .map(|extent| StructureExtent {
                    extent,
                    kind: StructureKind::GuidConstant,
                }),
        );
        out.sort_by_key(|structure| (structure.extent.start, structure.extent.end));
        out.dedup();
        out
    }

    /// Returns the extent of every copy of a known interface GUID in the code
    /// section that no structure in `held` contains.
    ///
    /// A routine that passes an interface's GUID by address - `Supports(x,
    /// IFoo)` - keeps a sixteen-byte copy of it after its own code, 4-aligned,
    /// and nothing but the code names it. The GUIDs are the ones the binary
    /// declares: every interface a class implements and every interface type
    /// whose RTTI states one. A copy inside an interface table or a type
    /// record is that structure's, and is left to it.
    fn guid_constants(&self, held: &[Range<u64>]) -> Vec<Range<u64>> {
        let ctx = self.ctx();
        let guid_bytes = |guid: &Guid| {
            let mut bytes = [0u8; 16];
            bytes[..4].copy_from_slice(&guid.data1.to_le_bytes());
            bytes[4..6].copy_from_slice(&guid.data2.to_le_bytes());
            bytes[6..8].copy_from_slice(&guid.data3.to_le_bytes());
            bytes[8..].copy_from_slice(&guid.data4);
            bytes
        };
        let mut known: HashSet<[u8; 16]> = self
            .classes()
            .iter()
            .flat_map(|class| self.interfaces(class))
            .map(|entry| guid_bytes(&entry.guid))
            .collect();
        known.extend(self.types().iter().filter_map(|detail| match detail {
            TypeDetail::Interface(interface) if interface.flags & INTERFACE_HAS_GUID != 0 => {
                Some(guid_bytes(&interface.guid))
            }
            _ => None,
        }));
        known.remove(&[0u8; 16]);
        let (Some(text), false) = (ctx.sections().text, known.is_empty()) else {
            return Vec::new();
        };
        let Some(code) = ctx.section_data(&text) else {
            return Vec::new();
        };
        let mut constants = Vec::new();
        for start in (0..code.len()).step_by(4) {
            let Some(candidate) = start
                .checked_add(16)
                .and_then(|end| code.get(start..end))
                .and_then(|bytes| <[u8; 16]>::try_from(bytes).ok())
            else {
                break;
            };
            if !known.contains(&candidate) {
                continue;
            }
            let Some(va) = text
                .va
                .checked_add(u64::try_from(start).unwrap_or(u64::MAX))
            else {
                continue;
            };
            let extent = va..va.saturating_add(16);
            if held
                .iter()
                .any(|other| other.start < extent.end && extent.start < other.end)
            {
                continue;
            }
            constants.push(extent);
        }
        constants
    }

    /// Returns the extent of every string constant in the code section.
    ///
    /// A Delphi string constant is a string record whose reference count is
    /// `-1`, so the runtime never frees it, laid out as [`LiteralLayout`]
    /// states. The compiler places each one in `CODE`, 4-aligned, before the
    /// routine that loads it, and no table names it: the code's own
    /// references are the only pointers to it. Free Pascal keeps its
    /// constants in read-only data, outside the code, so its binaries have
    /// none here.
    fn string_literals(&self) -> Vec<Range<u64>> {
        let ctx = self.ctx();
        if self.flavor() != VmtFlavor::Delphi {
            return Vec::new();
        }
        let Some(text) = ctx.sections().text else {
            return Vec::new();
        };
        let Some(code) = ctx.section_data(&text) else {
            return Vec::new();
        };
        let layout = LiteralLayout {
            code_paged: self.string_layout() == StringLayout::CodePaged,
        };
        let va = |offset: usize| text.va.checked_add(u64::try_from(offset).ok()?);
        let mut literals = Vec::new();
        let mut at = 0usize;
        while let Some(count) = code
            .get(at..)
            .and_then(|rest| {
                rest.windows(CONSTANT_REFERENCE_COUNT.len())
                    .position(|window| window == CONSTANT_REFERENCE_COUNT)
            })
            .and_then(|found| at.checked_add(found))
        {
            let record = va(count)
                .filter(|address| address % 4 == 0)
                .and_then(|_| layout.record_at(code, count));
            at = match record {
                Some(record) => {
                    if let (Some(start), Some(end)) = (va(record.start), va(record.end)) {
                        literals.push(start..end);
                    }
                    record.end
                }
                None => count.saturating_add(1),
            };
        }
        literals
    }
}

/// How a runtime lays out a string constant's record.
///
/// Before Delphi 2009 the record is a reference count and a length, then the
/// characters, one byte each, and a NUL. From Delphi 2009 a code page and an
/// element size precede them - 1 for an ANSI string, 2 for a UTF-16 one, whose
/// code page is 1200 - and the terminator is one element wide. A constant
/// begins at its code page on 64-bit too: the padding dword `StrRec` declares
/// there is not emitted, and HeidiSQL's x64 build keeps a `resourcestring`
/// record in the four bytes before one.
#[derive(Debug, Clone, Copy)]
struct LiteralLayout {
    /// Whether the record carries a code page and an element size.
    code_paged: bool,
}

impl LiteralLayout {
    /// The file range of the record whose reference count is at `count` in
    /// `code`, where every field agrees: a non-empty length, characters none
    /// of which is the terminator, then the terminator, and on a code-paged
    /// runtime an element size of 1, or of 2 with UTF-16's code page.
    fn record_at(self, code: &[u8], count: usize) -> Option<Range<usize>> {
        let length = usize::try_from(read_u32(code, count.checked_add(4)?)?)
            .ok()
            .filter(|length| *length > 0)?;
        let (start, width) = if self.code_paged {
            let header = count.checked_sub(4)?;
            let width = match (
                read_u16(code, header)?,
                read_u16(code, header.checked_add(2)?)?,
            ) {
                (_, 1) => 1,
                (UTF16_CODE_PAGE, 2) => 2,
                _ => return None,
            };
            (header, width)
        } else {
            (count, 1)
        };
        let characters = count.checked_add(8)?;
        let terminator = characters.checked_add(length.checked_mul(width)?)?;
        let end = terminator.checked_add(width)?;
        let body = code.get(characters..terminator)?;
        let terminated = code.get(terminator..end)?.iter().all(|byte| *byte == 0);
        let unbroken = body
            .chunks_exact(width)
            .all(|unit| unit.iter().any(|byte| *byte != 0));
        (terminated && unbroken).then_some(start..end)
    }
}
