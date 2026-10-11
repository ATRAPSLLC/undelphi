//! # undelphi: Delphi / C++Builder / Free Pascal binary parser
//!
//! A Rust library for statically analyzing compiled Delphi,
//! C++Builder, and Free Pascal / Lazarus binaries. Given an arbitrary byte
//! slice, `undelphi` identifies whether the binary was produced by one of
//! these toolchains and extracts the rich metadata that their runtimes
//! embed in every executable.
//!
//! ## Motivation
//!
//! Delphi / FPC binaries are unusually rich targets for static analysis.
//! Unlike C/C++ output, stripped Delphi binaries still carry:
//!
//! - **Published class names and unit names** (via the `vmtClassName` field
//!   and `tkClass` RTTI records).
//! - **Published property names, getters, setters, defaults** (the VCL /
//!   Object Inspector depends on these at runtime).
//! - **Form definitions** - entire UI trees, including embedded binaries
//!   (icons, images, scripts) - inside `TPF0` resource streams.
//! - **Package dependency graph** via `PACKAGEINFO`.
//! - **Compiler version** (Delphi 12 Athens reports itself as
//!   `Embarcadero Delphi for Win64 compiler version 36.0`; FPC reports as
//!   `FPC 3.2.2 [2021/05/15] for i386 - Win32`).
//! - **Interface GUIDs**, **virtual method tables**, **dynamic dispatch
//!   tables**, **init/finalization tables**.
//!
//! This metadata survives stripping because the VCL runtime depends on it
//! for form streaming, property inspection, interface dispatch, and package
//! loading.
//!
//! ## Feature overview
//!
//! - **Toolchain identification** - Embarcadero Delphi / C++Builder or
//!   Free Pascal / Lazarus; product release name (e.g. `Delphi 12 Athens`);
//!   DVCLAL edition (Personal / Professional / Enterprise).
//! - **Container support** - Windows PE32 / PE32+, macOS Mach-O, Linux ELF.
//! - **Class discovery** - VMT shape-test scanner for both Delphi and FPC
//!   layouts on 32-bit and 64-bit, automatic parent-chain resolution,
//!   indexed by class name and by VMT VA.
//! - **RTTI decoding** - per-Kind decoders for `tkClass`, `tkEnumeration`,
//!   `tkInteger`, `tkChar`, `tkFloat`, `tkSet`, `tkClassRef`, `tkDynArray`,
//!   `tkInterface`, `tkRecord`, `tkMethod`, `tkProcedure`, `tkLString` /
//!   `tkUString` / `tkWString`.
//! - **Class metadata** - published properties (classic + extended-RTTI with
//!   visibility flags), published fields, published methods, virtual method
//!   pointer table, init (managed-fields) table, dynamic / message
//!   dispatch table, interface table with GUID and Corba IIDStr,
//!   `automated` (OLE automation) table, and the field class table that
//!   types every published field.
//! - **Attributes** - `[attribute]` annotation decoding with constructor
//!   argument extraction (string / integer / raw bytes).
//! - **Form streams** - full TPF0 / TPF1 parser with the complete
//!   `TValueType` set; recursive component tree; symbolic value rendering
//!   against declared RTTI types (e.g. `Align = alClient` instead of
//!   `Align = 2`).
//! - **Package metadata** - `DVCLAL` + `PACKAGEINFO` resource decoders; PE
//!   resource directory walker; Free-Pascal internal-resources
//!   (`fpc.resources` / `.fpc.resources`) tree walker; runtime-package
//!   imports demangled into the types and classes a binary linked against
//!   `.bpl` packages references.
//! - **Cross-references** - interface implementors, DFM class
//!   instantiations, event-handler bindings, per-unit aggregate stats,
//!   external-parent reports.
//! - **Instance memory layout** - byte-by-byte reconstruction with
//!   managed-field markers and gap-fill.
//!
//! ## Virtual addresses: code vs data, and RVA conversion
//!
//! Every `…_va` field this crate exposes is an **absolute virtual
//! address** - the value that would appear in the loaded image. PE
//! consumers operating in RVA space must subtract the image base.
//!
//! Fields fall into two categories:
//!
//! | Field | Kind |
//! |-------|------|
//! | [`MethodEntry::code_va`](crate::methods::MethodEntry::code_va) | code (function entry point) |
//! | [`vtable::VirtualMethodEntry::code_va`](crate::vtable::VirtualMethodEntry::code_va) | code |
//! | [`vmttables::DynamicSlot::handler_va`](crate::vmttables::DynamicSlot::handler_va) | code |
//! | [`vmttables::AutoEntry::address`](crate::vmttables::AutoEntry::address) | code, or a VMT offset when `is_virtual()` |
//! | [`imports::Import::iat_va`](crate::imports::Import::iat_va) | data (import address table slot) |
//! | [`InterfaceEntry::getter_va`](crate::interfaces::InterfaceEntry::getter_va) | code |
//! | [`InterfaceEntry::vtable_va`](crate::interfaces::InterfaceEntry::vtable_va) | data (pointer-array) |
//! | [`extrtti::AttributeEntry::attr_ctor`](crate::extrtti::AttributeEntry::attr_ctor) | code |
//! | [`extrtti::AttributeEntry::attr_type_ref`](crate::extrtti::AttributeEntry::attr_type_ref) | data (`PPTypeInfo`) |
//! | [`Access::value`](crate::properties::Access::value) when `kind == AccessKind::Static` | code |
//! | [`Access::value`](crate::properties::Access::value) when `kind == AccessKind::Virtual` | **VMT byte offset** - *not* a VA. Resolve with [`Access::resolve`](crate::properties::Access::resolve) |
//! | [`Class::vmt_va`](crate::classes::Class::vmt_va) | data (VMT base) |
//! | [`vtable::VirtualMethodEntry::slot_va`](crate::vtable::VirtualMethodEntry::slot_va) | data (slot storage in VMT) |
//! | [`Property::prop_type_ref`](crate::properties::Property::prop_type_ref) | data (`PPTypeInfo`) |
//!
//! For a complete enumeration of code entrypoints, see
//! [`DelphiBinary::code_entrypoints`].
//!
//! ## Robustness contract
//!
//! Every parser in this crate treats its input as untrusted: parsers
//! return `Option<T>` / empty `Vec`s on malformed or adversarial input
//! rather than panic. The `clippy::unwrap_used` / `clippy::expect_used`
//! / `clippy::panic` lints are denied crate-wide in library code; the
//! `tests/malformed.rs` regression suite exercises every public
//! entrypoint against truncated / all-zero / all-0xFF / randomised /
//! impossible-length inputs.
//!
//! ## Quick start
//!
//! ```no_run
//! use undelphi::DelphiBinary;
//!
//! let bytes = std::fs::read("my_app.exe").unwrap();
//! if let Ok(bin) = DelphiBinary::parse(&bytes) {
//!     if let Some(info) = bin.compiler() {
//!         println!("Compiler: {:?} v{:?} ({:?} {:?})", info.compiler, info.version, info.os, info.arch);
//!     }
//!     if let Some(ed) = bin.edition() {
//!         println!("Edition: {:?}", ed);
//!     }
//!     if let Some(pkg) = bin.package_info() {
//!         println!("{} units, {} required packages",
//!             pkg.contains.len(), pkg.requires.len());
//!     }
//! }
//! ```

// The `missing_docs`, `clippy::unwrap_used`, `clippy::expect_used`,
// `clippy::panic`, `clippy::arithmetic_side_effects`, and
// `clippy::indexing_slicing` lints are declared in `Cargo.toml` under
// `[lints]` so they enforce on every build regardless of the consuming
// workspace. undelphi is used in malware-analysis pipelines where every
// input byte is adversarial and the parser must not panic.
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::arithmetic_side_effects,
        clippy::indexing_slicing
    )
)]

/// Re-export of the `goblin` crate, whose PE type
/// [`formats::BinaryContext::pe`] hands out.
pub use goblin;

pub mod blobs;
pub mod classes;
pub mod dcu;
pub mod detection;
pub mod dfm;
pub mod dvclal;
pub mod entrypoints;
pub mod extents;
pub mod extrtti;
pub mod fields;
pub mod formats;
pub mod fpcresources;
pub mod imports;
pub mod initfini;
pub mod interfaces;
pub mod intfrtti;
pub mod kb;
pub mod layout;
pub mod limits;
pub mod methods;
pub mod packageinfo;
pub mod properties;
pub mod render;
pub mod resources;
pub mod resstrings;
pub mod rtti;
pub mod signatures;
pub mod unitinit;
pub mod vmt;
pub mod vmttables;
pub mod vtable;
pub mod xref;

mod util;

/// Emit a warning event when the `tracing` feature is enabled. No-op
/// otherwise - keeps `tracing` strictly opt-in.
#[macro_export]
#[doc(hidden)]
macro_rules! __undelphi_trace_warn {
    ($($arg:tt)*) => {
        #[cfg(feature = "tracing")]
        {
            ::tracing::warn!($($arg)*);
        }
    };
}

use std::{
    collections::{HashMap, HashSet},
    error, fmt,
    sync::OnceLock,
};

use crate::{
    blobs::{EmbeddedBlob, catalog as catalog_blobs},
    classes::{Class, ClassSet},
    detection::{Compiler, CompilerInfo, Confidence, DetectionReport, TargetArch, TargetOs},
    dfm::DfmObject,
    dvclal::Edition,
    fields::Field,
    formats::{BinaryContext, BinaryFormat},
    interfaces::InterfaceEntry,
    limits::{MAX_FORMS_RAW_SCAN, MAX_RTTI_TYPES},
    methods::MethodEntry,
    packageinfo::PackageInfo,
    properties::Property,
    resources::{find_rcdata, iter_rcdata_named},
    rtti::TkClassInfo,
};

/// Major `dcc` version of Delphi 2010, the first release with extended RTTI.
const DELPHI_2010_COMPILER: u32 = 21;

/// A published property with its declared type resolved in the same pass.
#[derive(Debug, Clone, Copy)]
pub struct PropertyWithType<'a> {
    /// Published property entry.
    pub property: Property<'a>,
    /// Resolved `PTypeInfo` header, when the type pointer can be decoded.
    pub ty: Option<rtti::TypeHeader<'a>>,
}

/// A published field with its declared type resolved in the same pass.
#[derive(Debug, Clone, Copy)]
pub struct FieldWithType<'a> {
    /// Published field entry.
    pub field: Field<'a>,
    /// Resolved `PTypeInfo` header, when the type pointer can be decoded.
    pub ty: Option<rtti::TypeHeader<'a>>,
}

/// An interface entry with its vtable methods resolved in the same pass.
#[derive(Debug, Clone)]
pub struct InterfaceWithMethods<'a> {
    /// Class interface-table entry.
    pub interface: InterfaceEntry<'a>,
    /// Methods decoded from the interface vtable.
    pub methods: Vec<interfaces::InterfaceMethod<'a>>,
}

/// A class attribute with its common string argument decoded in the same pass.
#[derive(Debug, Clone, Copy)]
pub struct AttributeWithStringArg<'a> {
    /// Attribute-table entry.
    pub attribute: extrtti::AttributeEntry<'a>,
    /// Decoded single string argument, when the packed argument body has that shape.
    pub string_arg: Option<&'a [u8]>,
}

/// Why [`DelphiBinary::parse`] failed.
///
/// Use this to decide log severity. `NotRecognized` is the common, quiet
/// case - non-Delphi binaries land here. The other variants represent
/// actually broken or unfamiliar input and are usually worth surfacing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ParseError {
    /// Container parsed cleanly but no Delphi / C++Builder / FPC markers
    /// were found. Expected on non-Delphi binaries; safe to ignore.
    NotRecognized,
    /// A recognised container magic (`MZ`, `\x7fELF`, Mach-O) is present
    /// but goblin failed to walk the headers. Either the file is truncated
    /// or the headers are deliberately malformed (some packers do this).
    TruncatedContainer,
    /// The input does not start with any container magic this crate knows
    /// how to handle. Heuristic scans still ran but found nothing.
    UnrecognizedFormat,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::NotRecognized => f.write_str("no Delphi / C++Builder / FPC markers found"),
            ParseError::TruncatedContainer => {
                f.write_str("container magic present but headers could not be parsed")
            }
            ParseError::UnrecognizedFormat => {
                f.write_str("input has no recognised container magic")
            }
        }
    }
}

impl error::Error for ParseError {}

/// A parsed Delphi / C++Builder / FPC binary.
///
/// Created via [`DelphiBinary::parse`]. Owns a borrowed reference to the
/// input byte slice; everything returned by accessor methods ultimately
/// points back into that slice.
#[derive(Debug)]
pub struct DelphiBinary<'a> {
    ctx: BinaryContext<'a>,
    confidence: Confidence,
    compiler: Option<CompilerInfo<'a>>,
    edition: Option<Edition>,
    package_info: Option<PackageInfo<'a>>,
    tpf0_count: usize,
    classes: ClassSet<'a>,
    /// Lazy-initialised list of every parsed DFM/FMX/LFM/XFM form. Built on
    /// first call to [`DelphiBinary::forms`] and reused by every later call
    /// (including the xref views, which would otherwise re-parse every form
    /// on every access).
    forms_cache: OnceLock<Vec<(String, DfmObject<'a>)>>,
    /// Lazy-initialised binary-wide index of FPC `tkInterface` records,
    /// keyed by GUID. Built on first call to
    /// [`DelphiBinary::interface_methods`] and reused thereafter.
    /// Empty on Delphi binaries (Delphi's classic `tkInterface` doesn't
    /// carry the method-name table this index is for).
    tkinterface_index: OnceLock<HashMap<interfaces::Guid, u64>>,
    /// Lazy-initialised list of the binary's named imports. Built on first
    /// call to [`DelphiBinary::imports`].
    imports_cache: OnceLock<Vec<imports::Import>>,
    /// Lazy-initialised index of Delphi `tkInterface` records by GUID,
    /// built from [`DelphiBinary::types`] on first call to
    /// [`DelphiBinary::interface_methods`].
    delphi_interface_index: OnceLock<HashMap<interfaces::Guid, u64>>,
    /// Lazy-initialised index of [`Self::imports`] by address-table slot,
    /// for [`DelphiBinary::import_at`].
    import_index: OnceLock<HashMap<u64, usize>>,
    /// Lazy-initialised [`DelphiBinary::string_layout`].
    string_layout_cache: OnceLock<rtti::StringLayout>,
}

impl<'a> DelphiBinary<'a> {
    /// Analyzes a byte slice.
    ///
    /// Runs detection in order of decreasing reliability:
    ///
    /// 1. Embarcadero / FPC build-string scan (High confidence).
    /// 2. `DVCLAL` resource lookup via the PE resource walker (High).
    /// 3. `PACKAGEINFO` resource lookup (High).
    /// 4. `TPF0` occurrence count (Medium).
    ///
    /// A match at any level above `Low` succeeds. Otherwise the call returns
    /// a [`ParseError`] describing why detection failed:
    ///
    /// - [`ParseError::NotRecognized`] - the input is a valid container but
    ///   carries no Delphi/FPC markers. Expected on non-Delphi binaries; safe
    ///   to swallow without logging.
    /// - [`ParseError::TruncatedContainer`] - a recognised container magic is
    ///   present but goblin failed to walk the headers (truncated PE / ELF /
    ///   Mach-O). Worth logging.
    /// - [`ParseError::UnrecognizedFormat`] - the input has no known
    ///   container magic at all. Worth logging if you expected an executable.
    pub fn parse(data: &'a [u8]) -> Result<Self, ParseError> {
        let ctx = BinaryContext::new(data);
        let DetectionReport {
            mut confidence,
            compiler_info: compiler,
            tpf0_count,
        } = detection::analyze(&ctx);

        let edition = find_rcdata(&ctx, "DVCLAL").and_then(|e| dvclal::decode(e.data));
        if edition.is_some() {
            confidence = Confidence::High;
        }

        let package_info =
            find_rcdata(&ctx, "PACKAGEINFO").and_then(|e| packageinfo::parse(e.data));
        if package_info.is_some() {
            confidence = Confidence::High;
        }

        let classes = ClassSet::from_ctx(&ctx);
        // Finding any VMTs is independent structural proof - elevate
        // confidence so that stripped FPC/Mach-O binaries (which lack
        // DVCLAL / PACKAGEINFO / build-strings) can still be detected.
        if !classes.is_empty() && confidence < Confidence::Medium {
            confidence = Confidence::Medium;
        }

        if confidence == Confidence::None {
            return Err(if ctx.format() == BinaryFormat::Unknown {
                ParseError::UnrecognizedFormat
            } else if !ctx.container_parsed() {
                ParseError::TruncatedContainer
            } else {
                ParseError::NotRecognized
            });
        }

        Ok(Self {
            ctx,
            confidence,
            compiler,
            edition,
            package_info,
            tpf0_count,
            classes,
            forms_cache: OnceLock::new(),
            tkinterface_index: OnceLock::new(),
            imports_cache: OnceLock::new(),
            delphi_interface_index: OnceLock::new(),
            import_index: OnceLock::new(),
            string_layout_cache: OnceLock::new(),
        })
    }

    /// Detected executable container format.
    #[inline]
    pub fn format(&self) -> BinaryFormat {
        self.ctx.format()
    }

    /// PE image base, when the parsed container is PE / PE+.
    #[inline]
    pub fn image_base(&self) -> Option<u64> {
        self.ctx.image_base()
    }

    /// Converts an absolute VA to a PE RVA using the parsed image base.
    #[inline]
    pub fn va_to_rva(&self, va: u64) -> Option<u64> {
        self.ctx.va_to_rva(va)
    }

    /// Overall confidence in the detection.
    #[inline]
    pub fn confidence(&self) -> Confidence {
        self.confidence
    }

    /// Parsed compiler build string, when present.
    #[inline]
    pub fn compiler(&self) -> Option<&CompilerInfo<'a>> {
        self.compiler.as_ref()
    }

    /// Compiler family (Delphi / C++Builder / FPC), if identified.
    #[inline]
    pub fn compiler_kind(&self) -> Option<Compiler> {
        self.compiler.as_ref().map(|c| c.compiler)
    }

    /// Target operating system. Prefers the value from the parsed compiler
    /// build-string when available; otherwise falls back to container-level
    /// inference (PE → Windows, Mach-O → Darwin, ELF → Linux).
    pub fn target_os(&self) -> TargetOs {
        if let Some(info) = &self.compiler
            && info.os != TargetOs::Unknown
        {
            return info.os;
        }
        self.ctx.target_os()
    }

    /// Target architecture. Prefers the value from the parsed compiler
    /// build-string when available; otherwise falls back to the
    /// container-level machine code (PE `Machine`, ELF `e_machine`,
    /// Mach-O `cputype`).
    pub fn target_arch(&self) -> TargetArch {
        if let Some(info) = &self.compiler
            && info.arch != TargetArch::Unknown
        {
            return info.arch;
        }
        self.ctx.target_arch()
    }

    /// Delphi / C++Builder SKU (Personal / Professional / Enterprise), if the
    /// `DVCLAL` resource is present and decodable.
    #[inline]
    pub fn edition(&self) -> Option<Edition> {
        self.edition
    }

    /// Parsed PACKAGEINFO resource, if present.
    #[inline]
    pub fn package_info(&self) -> Option<&PackageInfo<'a>> {
        self.package_info.as_ref()
    }

    /// Imports of a PE binary: the import table's in its order, then the
    /// delay-loaded ones ([`imports::Import::is_delayed`]); an ordinal
    /// import is named `ORDINAL n`. Empty for other containers. See
    /// [`imports`] for what a Borland package import names.
    pub fn imports(&self) -> &[imports::Import] {
        self.imports_cache
            .get_or_init(|| imports::collect(&self.ctx))
    }

    /// Named exports of a PE binary as `(name, va)`, in export-table order;
    /// empty for other containers.
    ///
    /// A runtime package exports every class VMT, typeinfo, routine and
    /// global it defines under its Borland-mangled name, which
    /// [`imports::BorlandSymbol::demangle`] reads (e.g.
    /// `@Classes@TPersistent@Assign$qqrp19Classes@TPersistent`).
    pub fn exports(&self) -> Vec<(&'a str, u64)> {
        let (Some(pe), Some(base)) = (self.ctx.pe(), self.ctx.image_base()) else {
            return Vec::new();
        };
        pe.exports
            .iter()
            .filter_map(|export| Some((export.name?, base.checked_add(export.rva as u64)?)))
            .collect()
    }

    /// Decodes the module's string table (`RT_STRING`) and attach each
    /// string's `resourcestring` record (`TResStringRec`). See
    /// [`resstrings`]. Empty for non-PE input.
    pub fn resource_strings(&self) -> Vec<resstrings::ResourceString> {
        resstrings::resource_strings(&self.ctx)
    }

    /// Returns the import whose address-table slot is at `va`.
    ///
    /// In a binary linked against runtime packages, a reference that has no
    /// target inside the binary - a property's `PPTypeInfo`, a class's
    /// `vmtParent`, a published field's class cell - lands on such a slot,
    /// and [`imports::Import::symbol`] names the type or class it imports.
    pub fn import_at(&self, va: u64) -> Option<&imports::Import> {
        let index = self.import_index.get_or_init(|| {
            let mut index = HashMap::new();
            for (i, import) in self.imports().iter().enumerate() {
                index.entry(import.iat_va).or_insert(i);
            }
            index
        });
        self.imports().get(*index.get(&va)?)
    }

    /// Runtime packages the binary imports from (`rtl60.bpl`, ...), each
    /// once, in import-table order.
    ///
    /// An executable's own `PACKAGEINFO` carries no `requires` list, so the
    /// imports are its only record of the packages it needs.
    pub fn required_packages(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        for import in self.imports().iter().filter(|import| import.is_package()) {
            if !out.contains(&import.module()) {
                out.push(import.module());
            }
        }
        out
    }

    /// Number of `TPF0` magic occurrences - a proxy for the number of form
    /// resources embedded in the binary.
    #[inline]
    pub fn tpf0_count(&self) -> usize {
        self.tpf0_count
    }

    /// All classes discovered by scanning VMTs in the read-only-data section.
    #[inline]
    pub fn classes(&self) -> &ClassSet<'a> {
        &self.classes
    }

    /// Looks up a class by its discovery index.
    #[inline]
    pub fn class_by_index(&self, index: usize) -> Option<&Class<'a>> {
        self.classes.get(index)
    }

    /// Resolves a class's parent through the binary's class index.
    #[inline]
    pub fn parent_class(&self, class: &Class<'a>) -> Option<&Class<'a>> {
        class.parent_index.and_then(|idx| self.classes.get(idx))
    }

    /// Returns the import a class's parent reference lands on when the
    /// parent lives in a runtime package (e.g. `@Classes@TPersistent@` in
    /// `rtl60.bpl`). `None` when the parent is in this binary or the class
    /// is a root.
    pub fn external_parent(&self, class: &Class<'a>) -> Option<&imports::Import> {
        if class.parent_index.is_some() || class.vmt.parent_vmt == 0 {
            return None;
        }
        self.import_at(class.vmt.parent_vmt)
    }

    /// Decodes the `tkClass` RTTI record referenced by `class.vmt.type_info`.
    ///
    /// Returns `None` when the class carries no RTTI (stripped, or compiled
    /// without `{$M+}` equivalent), when the record cannot be parsed, or
    /// when the Kind byte is not `tkClass`.
    pub fn tkclass(&self, class: &Class<'a>) -> Option<TkClassInfo<'a>> {
        TkClassInfo::from_vmt(&self.ctx, &class.vmt)
    }

    /// Unit name of the class (the Pascal unit the class was declared in),
    /// extracted from the `tkClass` RTTI. Returns `None` when no RTTI.
    pub fn unit_name(&self, class: &Class<'a>) -> Option<&'a str> {
        self.tkclass(class).map(|c| c.unit_name())
    }

    /// Decodes the class's published-method table.
    ///
    /// Returns an empty vector when the class has no method table.
    pub fn methods(&self, class: &Class<'a>) -> Vec<MethodEntry<'a>> {
        MethodEntry::iter(&self.ctx, &class.vmt)
    }

    /// Converts a published method's absolute code VA to a PE RVA.
    #[inline]
    pub fn method_rva(&self, method: &MethodEntry<'a>) -> Option<u64> {
        self.va_to_rva(method.code_va)
    }

    /// Decodes method signatures (calling convention, ordered parameters with
    /// names / types / passing mode, and return type) for `class`.
    ///
    /// The result is era-tagged: [`signatures::SignatureReport::Decoded`]
    /// carries the signatures, [`signatures::SignatureReport::Absent`] means
    /// the compiler emitted no signature RTTI for this class (classic /
    /// pre-2010 Delphi, or FPC without extended method RTTI), and
    /// [`signatures::SignatureReport::Unsupported`] means signature RTTI is
    /// present but in a layout this parser does not decode yet.
    ///
    /// Decodes two Delphi sources: the published-method trailer
    /// (`{$METHODINFO ON}` binaries, rare) and the **extended-method
    /// section** of the `vmtMethodTable` (default extended RTTI on Delphi
    /// 2010+ - the primary source). Pre-2010 Delphi and FPC carry neither
    /// and report [`Absent`](signatures::SignatureReport::Absent); the FPC
    /// `TVmtMethodExTable` is not decoded.
    pub fn method_signatures(&self, class: &Class<'a>) -> signatures::SignatureReport<'a> {
        let ptr_size = class.vmt.pointer_size as usize;
        let flavor = class.vmt.flavor;
        let mut out = Vec::new();

        // Published-method trailers (only present under `{$METHODINFO ON}`).
        for m in self.methods(class) {
            if m.trailer.is_empty() {
                continue;
            }
            if let Some(sig) = signatures::MethodSignature::from_published_trailer(
                &self.ctx,
                m.name(),
                m.code_va,
                m.trailer,
                ptr_size,
                flavor,
            ) {
                out.push(sig);
            }
        }

        // Delphi 2010+ extended-method section (the common source).
        for ext in self.extended_methods(class) {
            if ext.trailer.is_empty() {
                continue;
            }
            let info = signatures::ExtendedMethodInfo {
                flags: ext.flags,
                vmt_index: ext.vmt_index,
            };
            if let Some(sig) = signatures::MethodSignature::from_delphi_extended(
                &self.ctx,
                ext.name(),
                ext.code_va,
                ext.trailer,
                ptr_size,
                flavor,
                info,
            ) {
                out.push(sig);
            }
        }

        if out.is_empty() {
            signatures::SignatureReport::Absent
        } else {
            signatures::SignatureReport::Decoded(out)
        }
    }

    /// Decodes the class's interface table.
    pub fn interfaces(&self, class: &Class<'a>) -> Vec<InterfaceEntry<'a>> {
        InterfaceEntry::iter(&self.ctx, &class.vmt)
    }

    /// Iterates the method-pointer array referenced by an
    /// [`InterfaceEntry::vtable_va`], populating method names from
    /// the matching `tkInterface` RTTI when reachable.
    ///
    /// Slot count + code VAs come from a vtable walk that terminates
    /// at the first slot that doesn't contain a plausible code VA
    /// (zero, unmapped, or outside the binary's `.text` / `__text` /
    /// `CODE` section) - works on both Delphi and FPC, including
    /// stripped binaries.
    ///
    /// Method *names* come from a binary-wide index of `tkInterface` RTTI
    /// records keyed by GUID, built lazily on first call: FPC records carry
    /// a method table, and Delphi records do for interfaces compiled with
    /// method info (`IInvokable` descendants; see [`intfrtti`]), whose own
    /// methods follow their ancestors' in the vtable. Other Delphi
    /// interfaces record no names, and `name` stays `None`.
    pub fn interface_methods(
        &self,
        entry: &InterfaceEntry<'a>,
    ) -> Vec<interfaces::InterfaceMethod<'a>> {
        let ptr_size = self.ctx.pointer_size().unwrap_or(
            self.classes
                .iter()
                .next()
                .map_or(0, |c| c.pointer_size() as usize),
        );
        if ptr_size == 0 {
            return Vec::new();
        }
        let mut methods = interfaces::InterfaceMethod::iter(&self.ctx, entry, ptr_size);
        // Try to populate names from the FPC tkInterface index.
        let index = self
            .tkinterface_index
            .get_or_init(|| rtti::scan_fpc_tkinterface_index(&self.ctx));
        if let Some(&tkintf_va) = index.get(&entry.guid)
            && let Some(table) = rtti::IntfMethodTable::from_tkinterface(&self.ctx, tkintf_va)
        {
            for rtti_entry in table.entries() {
                if let Some(m) = methods.get_mut(rtti_entry.slot() as usize) {
                    m.name = Some(rtti_entry.name());
                }
            }
        }
        if self.flavor() == vmt::VmtFlavor::Delphi {
            let index = self.delphi_interface_index.get_or_init(|| {
                self.types()
                    .into_iter()
                    .filter_map(|t| match t {
                        rtti::TypeDetail::Interface(i)
                            if i.guid != interfaces::Guid::from_bytes(&[0; 16]) =>
                        {
                            Some((i.guid, i.header.va))
                        }
                        _ => None,
                    })
                    .collect()
            });
            if let Some(&va) = index.get(&entry.guid)
                && let Some(info) =
                    rtti::InterfaceTypeInfo::from_va(&self.ctx, va, vmt::VmtFlavor::Delphi)
                && let Some(table) = self.interface_rtti(&info)
                && let Some(first) = intfrtti::InterfaceRtti::ancestor_method_count(
                    &self.ctx,
                    &info,
                    vmt::VmtFlavor::Delphi,
                )
            {
                for (i, method) in table.methods.iter().enumerate() {
                    if let Some(m) = first.checked_add(i).and_then(|slot| methods.get_mut(slot)) {
                        m.name = Some(method.name());
                    }
                }
            }
        }
        methods
    }

    /// Decodes the method table of a Delphi interface's RTTI: the methods it
    /// declares with their kinds, calling conventions, parameters and result
    /// types, when it was compiled with method info. See [`intfrtti`].
    ///
    /// The Delphi 2010+ layout (with attribute blocks) is tried first on
    /// binaries whose VMTs date from Delphi 2009 or later, the classic
    /// layout first otherwise; each validates structurally.
    pub fn interface_rtti(
        &self,
        info: &rtti::InterfaceTypeInfo<'a>,
    ) -> Option<intfrtti::InterfaceRtti<'a>> {
        let modern = self.string_layout() == rtti::StringLayout::CodePaged;
        intfrtti::InterfaceRtti::decode(&self.ctx, info, modern)
            .or_else(|| intfrtti::InterfaceRtti::decode(&self.ctx, info, !modern))
    }

    /// Resolves the code VA of the method that implements `method` for
    /// instances of `class`: the thunk's direct target, or for a virtual
    /// implementation the code in `class`'s VMT at the thunk's offset.
    ///
    /// `None` when the slot is not a recognised adjustor thunk or the VMT
    /// offset does not land on a decoded slot of `class`.
    pub fn interface_method_target(
        &self,
        class: &Class<'a>,
        method: &interfaces::InterfaceMethod<'a>,
    ) -> Option<u64> {
        match method.thunk?.target {
            interfaces::ThunkTarget::Code(va) => Some(va),
            interfaces::ThunkTarget::VmtOffset(offset) => self
                .virtual_methods(class)
                .iter()
                .find(|slot| slot.vmt_offset == u64::from(offset))
                .map(|slot| slot.code_va),
        }
    }

    /// Decodes the class's interface table and resolves each interface's method
    /// pointers in one pass.
    pub fn interfaces_with_methods(&self, class: &Class<'a>) -> Vec<InterfaceWithMethods<'a>> {
        self.interfaces(class)
            .into_iter()
            .map(|interface| {
                let methods = self.interface_methods(&interface);
                InterfaceWithMethods { interface, methods }
            })
            .collect()
    }

    /// Decodes the virtual-method-pointer array that follows the class's
    /// VMT header. One entry per user-declared (or inherited) virtual
    /// method slot.
    pub fn virtual_methods(&self, class: &Class<'a>) -> Vec<vtable::VirtualMethodEntry> {
        let bound = vtable::VirtualMethodEntry::upper_bound_for(&self.classes, &class.vmt);
        vtable::VirtualMethodEntry::iter(&self.ctx, &class.vmt, bound)
    }

    /// Reads `TObject`'s own virtual methods from `class`'s VMT: `Destroy`,
    /// `FreeInstance`, `NewInstance`, `DefaultHandler`, `Dispatch`,
    /// `BeforeDestruction`, `AfterConstruction`, `SafeCallException` (and
    /// FPC's further slots). Delphi keeps them at negative offsets inside the
    /// header, so [`Self::virtual_methods`] does not list them; this is where
    /// a class's destructor is found. See [`vtable::StandardVirtual`].
    pub fn standard_virtuals(&self, class: &Class<'a>) -> Vec<vtable::StandardVirtual> {
        vtable::StandardVirtual::iter(&self.ctx, &class.vmt)
    }

    /// Decodes the class's init (managed-fields) table - the list of
    /// instance offsets the runtime needs to refcount-manage.
    pub fn init_table(&self, class: &Class<'a>) -> Option<rtti::RecordInfo<'a>> {
        rtti::RecordInfo::from_init_table(&self.ctx, &class.vmt)
    }

    /// Decodes the class's dynamic-dispatch table (message handlers +
    /// `dynamic` method slots).
    pub fn dynamic_slots(&self, class: &Class<'a>) -> Vec<vmttables::DynamicSlot> {
        vmttables::DynamicSlot::iter(&self.ctx, &class.vmt)
    }

    /// Decodes the automation table (`vmtAutoTable`) for `class`: the
    /// members of its `automated` section with their dispids, accessor
    /// kinds, `Variant` parameter types and code addresses.
    ///
    /// Empty when the class declares no `automated` section, and on 64-bit
    /// or FPC binaries, which have no such table.
    pub fn auto_entries(&self, class: &Class<'a>) -> Vec<vmttables::AutoEntry<'a>> {
        vmttables::AutoEntry::iter(&self.ctx, &class.vmt)
    }

    /// Walks the extended-RTTI property table for `class` - includes
    /// non-published members (private / protected / public) with their
    /// visibility flags and attached attribute bytes.
    pub fn extended_properties(&self, class: &Class<'a>) -> Vec<extrtti::ExtendedProperty<'a>> {
        if self.predates_extended_rtti() {
            return Vec::new();
        }
        extrtti::ExtendedProperty::iter(&self.ctx, class)
    }

    /// Recovers per-unit `initialization` / `finalization` procedure VAs.
    ///
    /// FPC: walks the `INITFINAL` table, located via symbol lookup
    /// (`INITFINAL` / `_INITFINAL` / `FPC_INITFINAL` in ELF symtab, Mach-O
    /// `LC_SYMTAB`, or PE export table) with a heuristic shape scan over data
    /// sections as a fallback for stripped builds; see [`initfini`].
    ///
    /// Delphi: walks the compiler's `PackageInfoTable`
    /// ([`Self::unit_init_table`]), in initialization order. Its entries carry no
    /// unit names, so `unit_name` is `"<unknown>"`; consumers that want the
    /// unit list can read [`DelphiBinary::package_info`].
    pub fn unit_init_procs(&self) -> Vec<initfini::UnitInitProc<'a>> {
        if self.flavor() == vmt::VmtFlavor::Delphi
            && let Some(table) = self.unit_init_table()
        {
            return table
                .units
                .iter()
                .map(|unit| initfini::UnitInitProc {
                    unit_name: "<unknown>",
                    init_va: unit.init_va,
                    finalize_va: unit.finalize_va,
                })
                .collect();
        }
        initfini::iter_unit_init_procs(&self.ctx)
    }

    /// Decodes the Delphi unit initialization table (`PackageInfoTable`):
    /// each unit's `initialization` / `finalization` procedure in startup
    /// order and, on Delphi 2010+, every RTTI type grouped by declaring
    /// unit. See [`unitinit`]. `None` for non-PE input, FPC, or when no
    /// candidate validates.
    pub fn unit_init_table(&self) -> Option<unitinit::UnitInitTable<'a>> {
        unitinit::UnitInitTable::locate(&self.ctx)
    }

    /// Locates the module's DCU declarations - routines, typed constants,
    /// variables, thread variables - in this binary with the units of
    /// `library` (the release and platform that built it, plus any
    /// directory of the program's own units), and name each slot of the
    /// unit initialization table after the unit or class it belongs to.
    /// Routines and typed constants are placed only where the image holds
    /// their bytes, variables only where every relocation that reaches
    /// them agrees; see [`dcu::locate`]. Empty when the binary has no unit
    /// initialization table.
    pub fn locate(&self, library: &dcu::library::Library) -> dcu::locate::Location {
        let Some(table) = self.unit_init_table() else {
            return dcu::locate::Location::default();
        };
        let mut module = dcu::locate::Module::default();
        for class in self.classes.iter() {
            let Some(unit) = self.unit_name(class) else {
                continue;
            };
            module.class_units.push(unit);
            let published = self
                .methods(class)
                .into_iter()
                .map(|m| (m.name(), m.code_va));
            let extended = self
                .extended_methods(class)
                .into_iter()
                .map(|m| (m.name(), m.code_va));
            for (name, va) in published.chain(extended) {
                module
                    .methods
                    .push((unit, format!("{}.{name}", class.name()), va));
            }
        }
        module.class_units.sort_unstable();
        module.class_units.dedup();
        module.methods.sort_unstable();
        module.methods.dedup();
        match self.package_info() {
            Some(info) => {
                module.contains = info
                    .contains
                    .iter()
                    .filter(|u| !u.is_main())
                    .map(|u| u.name)
                    .collect();
                module.is_program = info.module_type() == packageinfo::ModuleType::Program;
                module.self_contained = module.is_program && info.requires.is_empty();
            }
            None => module.is_program = self.ctx.pe().is_none_or(|pe| !pe.is_lib),
        }
        dcu::locate::locate(library, &self.ctx, &table, &module)
    }

    /// Aggregates every code address this crate can confidently label -
    /// published methods, virtual-method slots, dynamic-message handlers,
    /// interface getters and methods, property accessors (with
    /// `AccessKind::Virtual` slots resolved to code VAs), attribute
    /// constructors, and unit init / finalize procedures.
    ///
    /// Duplicates are emitted across [`entrypoints::EntrypointKind`]
    /// variants when the same VA is reached via more than one path; the
    /// caller dedups by their preferred priority.
    ///
    /// See the
    /// [`entrypoints`] module docs for kind semantics. This is the
    /// one-stop helper meant to replace the ~100 LoC of glue every
    /// disassembler-driving consumer would otherwise write.
    pub fn code_entrypoints(&'a self) -> Vec<entrypoints::CodeEntrypoint<'a>> {
        entrypoints::collect(self)
    }

    /// Resolves and decodes an attribute (from a class, field, property,
    /// method, parameter or record): its type name, constructor and
    /// arguments. See [`extrtti::DecodedAttribute::decode`].
    pub fn decode_attribute(
        &self,
        entry: &extrtti::AttributeEntry<'a>,
    ) -> extrtti::DecodedAttribute<'a> {
        extrtti::DecodedAttribute::decode(&self.ctx, &self.classes, entry, self.string_layout())
    }

    /// Reads `class`'s indexed (array) properties from its Delphi 2010+
    /// extended RTTI (`property Items[Index: Integer]: T`), which classic and
    /// extended property tables do not list. See
    /// [`extrtti::ArrayProperty`]; resolve an accessor through
    /// [`Self::extended_methods`].
    pub fn indexed_properties(&self, class: &Class<'a>) -> Vec<extrtti::ArrayProperty<'a>> {
        if self.predates_extended_rtti() {
            return Vec::new();
        }
        extrtti::ArrayProperty::iter(&self.ctx, class)
    }

    /// Reads `class`'s Delphi 2010+ extended-method entries: every method
    /// `{$RTTI}` selects, of any visibility, with its decoded flags
    /// ([`methods::DelphiExtMethod::info`]). Indexed properties refer to
    /// their accessors by position in this list.
    pub fn extended_methods(&self, class: &Class<'a>) -> Vec<methods::DelphiExtMethod<'a>> {
        if self.predates_extended_rtti() {
            return Vec::new();
        }
        methods::DelphiExtMethod::iter(&self.ctx, &class.vmt)
    }

    /// Decodes the class-level attribute table for `class`, when one is
    /// present.
    ///
    /// Walks past the classic `TPropData` and the extended-property
    /// block to find the `AttrData` trailer (Delphi 2010 and later), then
    /// decodes its packed attribute entries. Empty for a class without
    /// attributes, for FPC and Delphi before 2010 (no such trailer), and
    /// when the property tables do not walk.
    pub fn class_attributes(&self, class: &Class<'a>) -> Vec<extrtti::AttributeEntry<'a>> {
        if self.predates_extended_rtti() {
            return Vec::new();
        }
        extrtti::AttributeEntry::iter_class(&self.ctx, class)
    }

    /// Decodes class-level attributes and their common single string argument
    /// shape in one pass.
    pub fn class_attributes_with_string_args(
        &self,
        class: &Class<'a>,
    ) -> Vec<AttributeWithStringArg<'a>> {
        self.class_attributes(class)
            .into_iter()
            .map(|attribute| AttributeWithStringArg {
                string_arg: attribute.arg_as_string(),
                attribute,
            })
            .collect()
    }

    /// Looks up a published method by name on `class`, walking the
    /// ancestry chain so inherited handlers are found. Returns the
    /// method's code VA, or `None` when unresolved.
    pub fn resolve_event_handler(&self, class: &Class<'a>, method_name: &str) -> Option<u64> {
        let mut walker: Option<&Class<'a>> = Some(class);
        while let Some(c) = walker {
            for m in self.methods(c) {
                if m.name().eq_ignore_ascii_case(method_name) {
                    return Some(m.code_va);
                }
            }
            walker = c.parent_index.and_then(|idx| self.classes.get(idx));
        }
        None
    }

    /// Decodes the class's published-property table.
    ///
    /// Iterates the `TPropData` block that follows the class's tkClass
    /// TypeData `UnitName`. Returns an empty vector when the class has no
    /// RTTI (stripped binary, or class compiled without `{$M+}`).
    pub fn properties(&self, class: &Class<'a>) -> Vec<Property<'a>> {
        Property::iter(&self.ctx, &class.vmt)
    }

    /// Decodes published properties and resolves their declared types in one pass.
    pub fn properties_with_types(&self, class: &Class<'a>) -> Vec<PropertyWithType<'a>> {
        self.properties(class)
            .into_iter()
            .map(|property| PropertyWithType {
                ty: self.property_type(class, &property),
                property,
            })
            .collect()
    }

    /// Resolves the declared type of a property from its `PropType` pointer.
    ///
    /// Returns `None` when the pointer is null or the referenced type info
    /// cannot be decoded (e.g. stripped or modern-extended RTTI we don't
    /// yet walk). When the type lives in a runtime package the pointer is
    /// an import slot: [`Self::import_at`] on `prop.prop_type_ref` names it.
    pub fn property_type(
        &self,
        class: &Class<'a>,
        prop: &Property<'a>,
    ) -> Option<rtti::TypeHeader<'a>> {
        rtti::TypeHeader::from_pptr(
            &self.ctx,
            prop.prop_type_ref,
            class.vmt.pointer_size as usize,
            class.vmt.flavor,
        )
    }

    /// Resolves the declared type of a field: from its `TypeInfoPtr`, or for
    /// a legacy `TypeIndex` field, from the `vmtTypeInfo` of the class
    /// [`Self::field_class`] resolves.
    pub fn field_type(&self, class: &Class<'a>, field: &Field<'a>) -> Option<rtti::TypeHeader<'a>> {
        match field.type_ref {
            fields::FieldTypeRef::TypeInfoPtr(pptr) => rtti::TypeHeader::from_pptr(
                &self.ctx,
                pptr,
                class.vmt.pointer_size as usize,
                class.vmt.flavor,
            ),
            fields::FieldTypeRef::TypeIndex(_) => {
                let target = self.field_class(class, field)?;
                rtti::TypeHeader::from_va(&self.ctx, target.vmt.type_info, target.vmt.flavor)
            }
        }
    }

    /// Resolves the class a legacy published field is declared as, through
    /// the field table's class-types table (see [`Field::class_table`]).
    ///
    /// `None` for a modern `TypeInfoPtr` field, or when the class lives in
    /// another module (a runtime package) and is not in [`Self::classes`].
    pub fn field_class(&self, class: &Class<'a>, field: &Field<'a>) -> Option<&Class<'a>> {
        let fields::FieldTypeRef::TypeIndex(index) = field.type_ref else {
            return None;
        };
        let cells = Field::class_table(&self.ctx, &class.vmt);
        let cell = *cells.get(usize::from(index))?;
        self.class_at_ref(cell, class.vmt.pointer_size as usize)
    }

    /// Returns the import a legacy published field's class cell lands on
    /// when its class lives in a runtime package (where
    /// [`Self::field_class`] answers `None`).
    pub fn field_class_import(
        &self,
        class: &Class<'a>,
        field: &Field<'a>,
    ) -> Option<&imports::Import> {
        let fields::FieldTypeRef::TypeIndex(index) = field.type_ref else {
            return None;
        };
        let cells = Field::class_table(&self.ctx, &class.vmt);
        self.import_at(*cells.get(usize::from(index))?)
    }

    /// Returns whether the detected compiler version predates Delphi 2010,
    /// which introduced extended RTTI. The VMT header only dates a binary
    /// to Delphi 2009 or later, so the extended walkers also skip a binary
    /// whose build string or RTL package names an older compiler.
    fn predates_extended_rtti(&self) -> bool {
        self.compiler()
            .filter(|info| matches!(info.compiler, Compiler::Delphi | Compiler::CppBuilder))
            .and_then(|info| info.version)
            .and_then(|version| version.split('.').next()?.parse::<u32>().ok())
            .is_some_and(|major| major < DELPHI_2010_COMPILER)
    }

    /// Resolves a class reference: a VMT base VA, or a `PClass` cell that
    /// holds one.
    fn class_at_ref(&self, va: u64, ptr_size: usize) -> Option<&Class<'a>> {
        let classes = self.classes();
        classes.find_by_vmt_va(va).or_else(|| {
            let target = rtti::TypeHeader::deref_pptypeinfo(&self.ctx, va, ptr_size)?;
            classes.find_by_vmt_va(target)
        })
    }

    /// Decodes an enumeration type at `type_info_va` into its element
    /// names and min/max bounds.
    pub fn decode_enum(
        &self,
        type_info_va: u64,
        flavor: detection::Compiler,
    ) -> Option<rtti::EnumInfo<'a>> {
        let flavor = match flavor {
            detection::Compiler::FreePascal => vmt::VmtFlavor::Fpc,
            _ => vmt::VmtFlavor::Delphi,
        };
        rtti::EnumInfo::from_va(&self.ctx, type_info_va, flavor)
    }

    /// Picks the `VmtFlavor` matching this binary's toolchain.
    pub fn flavor(&self) -> vmt::VmtFlavor {
        match self.compiler_kind() {
            Some(detection::Compiler::FreePascal) => vmt::VmtFlavor::Fpc,
            _ => vmt::VmtFlavor::Delphi,
        }
    }

    /// Returns the string RTTI layout this binary's runtime emits.
    ///
    /// Delphi 2009 introduced code-paged strings together with the `Equals`,
    /// `GetHashCode` and `ToString` VMT slots, so the class headers date a
    /// Delphi runtime; Free Pascal stamps code pages from 3.0, read from the
    /// build string. A runtime that cannot be dated is taken as
    /// [`rtti::StringLayout::Plain`], which never reads past a string's name.
    pub fn string_layout(&self) -> rtti::StringLayout {
        *self.string_layout_cache.get_or_init(|| {
            let (dated, undated): (Vec<&Class<'a>>, Vec<_>) = self
                .classes()
                .iter()
                .filter(|class| class.vmt.flavor == vmt::VmtFlavor::Delphi)
                .partition(|class| class.vmt.is_delphi_2009_or_later());
            let fpc_3 = self
                .compiler()
                .filter(|info| info.compiler == Compiler::FreePascal)
                .and_then(|info| info.version)
                .and_then(|version| version.split('.').next()?.parse::<u32>().ok())
                .is_some_and(|major| major >= 3);
            if fpc_3 || dated.len() > undated.len() {
                rtti::StringLayout::CodePaged
            } else {
                rtti::StringLayout::Plain
            }
        })
    }

    /// Full per-Kind RTTI decode for the type referenced by a property's
    /// `PropType` pointer.
    pub fn property_type_detail(
        &self,
        class: &Class<'a>,
        prop: &Property<'a>,
    ) -> Option<rtti::TypeDetail<'a>> {
        let ptr_size = class.vmt.pointer_size as usize;
        let type_info_va =
            rtti::TypeHeader::deref_pptypeinfo(&self.ctx, prop.prop_type_ref, ptr_size)?;
        rtti::TypeDetail::from_va(
            &self.ctx,
            type_info_va,
            class.vmt.flavor,
            self.string_layout(),
        )
    }

    /// Full per-Kind RTTI decode for the type referenced by a field.
    pub fn field_type_detail(
        &self,
        class: &Class<'a>,
        field: &Field<'a>,
    ) -> Option<rtti::TypeDetail<'a>> {
        let ptr_size = class.vmt.pointer_size as usize;
        match field.type_ref {
            fields::FieldTypeRef::TypeInfoPtr(pptr) => {
                let type_info_va = rtti::TypeHeader::deref_pptypeinfo(&self.ctx, pptr, ptr_size)?;
                rtti::TypeDetail::from_va(
                    &self.ctx,
                    type_info_va,
                    class.vmt.flavor,
                    self.string_layout(),
                )
            }
            fields::FieldTypeRef::TypeIndex(_) => {
                let target = self.field_class(class, field)?;
                rtti::TypeDetail::from_va(
                    &self.ctx,
                    target.vmt.type_info,
                    target.vmt.flavor,
                    self.string_layout(),
                )
            }
        }
    }

    /// Decodes the class's published-field table.
    ///
    /// Returns an empty vector when the class declares no published fields
    /// (many classes don't) or the table is malformed.
    pub fn fields(&self, class: &Class<'a>) -> Vec<Field<'a>> {
        Field::iter(&self.ctx, &class.vmt)
    }

    /// Decodes the Delphi 2010+ extended field table of `class`: every field
    /// it declares (private, protected, public and published) with
    /// visibility, offset, type and attributes. Inherited fields are on the
    /// ancestors. Empty before Delphi 2010 and on FPC; see
    /// [`fields::ExtendedField`].
    pub fn extended_fields(&self, class: &Class<'a>) -> Vec<fields::ExtendedField<'a>> {
        if self.predates_extended_rtti() {
            return Vec::new();
        }
        fields::ExtendedField::iter(&self.ctx, &class.vmt)
    }

    /// Resolves the declared type of an extended field. `None` when the
    /// type lives in another module (its `type_ref` is then an import slot,
    /// see [`Self::import_at`]) or cannot be decoded.
    pub fn extended_field_type(
        &self,
        class: &Class<'a>,
        field: &fields::ExtendedField<'a>,
    ) -> Option<rtti::TypeHeader<'a>> {
        rtti::TypeHeader::from_pptr(
            &self.ctx,
            field.type_ref,
            class.vmt.pointer_size as usize,
            class.vmt.flavor,
        )
    }

    /// Decodes published fields and resolves their declared types in one pass.
    pub fn fields_with_types(&self, class: &Class<'a>) -> Vec<FieldWithType<'a>> {
        self.fields(class)
            .into_iter()
            .map(|field| FieldWithType {
                ty: self.field_type(class, &field),
                field,
            })
            .collect()
    }

    /// Enumerates every RTTI type record reachable by following `PPTypeInfo`
    /// pointers from the class graph and the published property / field type
    /// references - a transitive closure over the binary's type graph, with
    /// **no raw scanning**.
    ///
    /// This surfaces types the per-class accessors never reach on their own:
    /// enumerations (with their value names), sets, records, dynamic arrays,
    /// method-pointer types, and even class records for classes that are
    /// referenced only as a member type and never instantiated (so they have
    /// no scannable VMT).
    ///
    /// On Delphi binaries a second, **self-cell pass** then surfaces types the
    /// compiler emits but references only from *code* (`TypeInfo(X)`), which no
    /// data-pointer walk can reach. Every Delphi `PTypeInfo` record is preceded
    /// by a self-referencing `PPTypeInfo` cell (`va - ptr_size` points at `va`),
    /// the same structural signature the VMT scanner uses for `vmtSelfPtr`, so
    /// a pointer-aligned pass over the read-only sections recovers them with
    /// negligible false-positive risk. FPC does not emit these cells, so the
    /// closure alone covers FPC. Together these reach ~97 % of every
    /// `PTypeInfo` referenced anywhere in a Delphi image (HeidiSQL 12: ~6 600
    /// types; the enum dictionary alone grows from ~250 to ~880).
    ///
    /// Results are deduplicated by `PTypeInfo` VA and bounded by
    /// [`crate::limits::MAX_RTTI_TYPES`]. Match on the [`rtti::TypeDetail`]
    /// variant for kind-specific views - e.g. collect every
    /// [`rtti::TypeDetail::Enumeration`] for the binary's enum dictionary.
    pub fn types(&self) -> Vec<rtti::TypeDetail<'a>> {
        let classes = self.classes();
        let flavor = classes
            .iter()
            .next()
            .map(|c| c.vmt.flavor)
            .unwrap_or(vmt::VmtFlavor::Delphi);
        let ptr_size = self.ctx.pointer_size().unwrap_or(4);
        let layout = self.string_layout();

        // Seed the walk with class type-info records plus published property
        // and field type references. The latter live in the `TPropData` /
        // field-table structures (not inside the type records), so they must
        // be seeded explicitly; the closure propagates from there.
        let mut work: Vec<u64> = Vec::new();
        for c in classes.iter() {
            if c.vmt.type_info != 0 {
                work.push(c.vmt.type_info);
            }
            for p in self.properties_with_types(c) {
                if let Some(t) = p.ty {
                    work.push(t.va);
                }
            }
            for f in self.fields_with_types(c) {
                if let Some(t) = f.ty {
                    work.push(t.va);
                }
            }
            // Method-signature parameter and return types (the extended
            // method section). These are `PPTypeInfo` references, so they
            // are dereferenced once to the target `PTypeInfo` before seeding.
            for sig in self.method_signatures(c).decoded() {
                let mut refs: Vec<u64> = sig.params.iter().map(|p| p.type_va).collect();
                refs.push(sig.result_type_va);
                for pp in refs {
                    if pp != 0
                        && let Some(t) = rtti::TypeHeader::deref_pptypeinfo(&self.ctx, pp, ptr_size)
                    {
                        work.push(t);
                    }
                }
            }
            // Extended-RTTI property types (non-published visibilities the
            // classic `TPropData` doesn't list). `prop_type_ref` is a
            // `PPTypeInfo`.
            for ep in self.extended_properties(c) {
                if ep.prop_type_ref != 0
                    && let Some(t) =
                        rtti::TypeHeader::deref_pptypeinfo(&self.ctx, ep.prop_type_ref, ptr_size)
                {
                    work.push(t);
                }
            }
        }

        let mut seen: HashSet<u64> = HashSet::new();
        let mut out: Vec<rtti::TypeDetail<'a>> = Vec::new();
        while let Some(va) = work.pop() {
            if out.len() >= MAX_RTTI_TYPES {
                break;
            }
            if va == 0 || !seen.insert(va) {
                continue;
            }
            let Some(detail) = rtti::TypeDetail::from_va(&self.ctx, va, flavor, layout) else {
                continue;
            };
            for pp in detail.referenced_pptrs() {
                if pp == 0 {
                    continue;
                }
                if let Some(target) = rtti::TypeHeader::deref_pptypeinfo(&self.ctx, pp, ptr_size) {
                    work.push(target);
                }
            }
            out.push(detail);
        }

        // Self-cell pass (Delphi only). Every Delphi `PTypeInfo` record is
        // immediately preceded by a self-referencing `PPTypeInfo` cell:
        // `va - ptr_size` holds a pointer to `va`. This is the direct analogue
        // of the `vmtSelfPtr` heuristic the VMT scanner uses, and a structural
        // signature - not a magic-byte guess - because a random pointer-aligned
        // slot points at exactly `itself + ptr_size` with negligible
        // probability, and the target must additionally decode as a valid type
        // record. Scanning these cells surfaces types the compiler emits but
        // references only from code (`TypeInfo(X)`), which the class-graph
        // closure never reaches. FPC does not emit the cells, so this finds
        // nothing there and the closure stands.
        if flavor == vmt::VmtFlavor::Delphi {
            for range in self.ctx.sections().scan_targets.iter() {
                if out.len() >= MAX_RTTI_TYPES {
                    break;
                }
                let Some(section) = self.ctx.section_data(range) else {
                    continue;
                };
                let mut o = 0usize;
                while o.saturating_add(ptr_size) <= section.len() {
                    if out.len() >= MAX_RTTI_TYPES {
                        break;
                    }
                    let cell_va = range.va.wrapping_add(o as u64);
                    if let Some(val) = util::read_ptr(section, o, ptr_size)
                        && val == cell_va.wrapping_add(ptr_size as u64)
                        && !seen.contains(&val)
                        && let Some(detail) =
                            rtti::TypeDetail::from_va(&self.ctx, val, flavor, layout)
                    {
                        seen.insert(val);
                        out.push(detail);
                    }
                    o = o.saturating_add(ptr_size);
                }
            }
        }

        out
    }

    /// Decodes every DFM / FMX / LFM / XFM form stream embedded as a
    /// resource. Combines two source paths:
    ///
    /// - PE `RT_RCDATA` resources (Delphi / C++Builder / FPC Windows).
    /// - FPC internal-resources tree (Mach-O / ELF, plus some FPC builds
    ///   on Windows that disable `FPC_HAS_WINLIKERESOURCES`).
    ///
    /// Form streams outside these two resource sources are not found: there
    /// is no raw `TPF0` / `TPF1` magic-byte scan for stripped or
    /// unconventionally-packaged binaries.
    ///
    /// Duplicates are de-duplicated by resource name. The result is cached
    /// on `self`, so repeated calls (e.g. from multiple `xref::*` views)
    /// don't re-walk and re-parse every resource.
    pub fn forms(&self) -> &[(String, DfmObject<'a>)] {
        self.forms_cache.get_or_init(|| self.extract_forms())
    }

    fn extract_forms(&self) -> Vec<(String, DfmObject<'a>)> {
        let mut out: Vec<(String, DfmObject<'a>)> = Vec::new();
        let mut seen_names: HashSet<String> = HashSet::new();

        // 1. PE RCDATA.
        for (name, body) in iter_rcdata_named(&self.ctx) {
            let Some(magic) = body.data.get(..4) else {
                continue;
            };
            if magic != dfm::TPF0_MAGIC && magic != dfm::TPF1_MAGIC {
                continue;
            }
            if let Some(obj) = DfmObject::parse(body.data)
                && seen_names.insert(name.clone())
            {
                out.push((name, obj));
            }
        }

        // 2. FPC tree.
        for r in fpcresources::iter_rcdata(&self.ctx) {
            let Some(magic) = r.data.get(..4) else {
                continue;
            };
            if magic != dfm::TPF0_MAGIC && magic != dfm::TPF1_MAGIC {
                continue;
            }
            let name = r
                .name
                .clone()
                .unwrap_or_else(|| format!("#{}", r.name_id.unwrap_or(0)));
            if let Some(obj) = DfmObject::parse(r.data)
                && seen_names.insert(name.clone())
            {
                out.push((name, obj));
            }
        }

        // 3. Last-resort raw magic-byte scan. Only when the structured
        //    passes above found nothing - stripped or unconventionally
        //    packaged binaries whose form streams aren't reachable through
        //    the PE resource directory or the FPC internal-resources tree,
        //    but whose `TPF0`/`TPF1` bytes survive verbatim in the image
        //    (commonly in an overlay a packer appended, or in a `.rsrc`
        //    section whose directory a protector mangled). Every candidate
        //    is validated by re-parsing; coincidental matches that don't
        //    decode are discarded.
        if out.is_empty() {
            self.scan_forms_raw(&mut out, &mut seen_names);
        }

        out
    }

    /// Scans the whole input buffer for `TPF0` / `TPF1` form streams and pushes
    /// every validated, not-yet-seen form into `out`.
    ///
    /// This is the last-resort path described in [`Self::extract_forms`]; it
    /// only runs when the structured resource passes yielded nothing. The
    /// scanning and per-hit validation live in [`dfm::scan_streams`], bounded
    /// by [`crate::limits::MAX_FORMS_RAW_SCAN`]; this wrapper only assigns
    /// each hit a name and deduplicates against the structured passes.
    fn scan_forms_raw(
        &self,
        out: &mut Vec<(String, DfmObject<'a>)>,
        seen_names: &mut HashSet<String>,
    ) {
        for (offset, obj) in dfm::scan_streams(self.ctx.data(), MAX_FORMS_RAW_SCAN) {
            // The structured passes key on the resource name; a raw hit has
            // none, so key on the form's own object name, falling back to
            // `<class>@<file-offset>` when the root object is anonymous.
            let object_name = obj.object_name();
            let name = if object_name.is_empty() {
                format!("{}@0x{offset:x}", obj.class_name())
            } else {
                object_name.to_owned()
            };
            if seen_names.insert(name.clone()) {
                out.push((name, obj));
            }
        }
    }

    /// Walks every parsed form and surface every embedded `vaBinary` blob,
    /// classified by magic bytes (PNG / JPEG / ZIP / PE / Mach-O / ICO /
    /// BMP / etc. - see [`blobs::BlobKind`]).
    ///
    /// Useful for malware triage: every blob carries the form resource it
    /// came from, the dotted path of the component property that holds it,
    /// the blob bytes (borrowed from the input), and a magic-byte
    /// classification.
    ///
    /// Allocates a `Vec<EmbeddedBlob>`. Each blob carries borrowed
    /// references into the cached form tree (`component`, `property`),
    /// so consumers can attach blobs to their owning components without
    /// re-walking. The blob payloads themselves are not copied.
    pub fn blobs(&self) -> Vec<EmbeddedBlob<'a, '_>> {
        catalog_blobs(self.forms())
    }

    /// Accesses the low-level binary context (sections, VA mapping, segments).
    #[inline]
    pub fn ctx(&self) -> &BinaryContext<'a> {
        &self.ctx
    }
}

const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<DelphiBinary<'_>>();
};
