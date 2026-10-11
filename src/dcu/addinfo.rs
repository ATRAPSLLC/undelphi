//! Constant values and the `drConstAddInfo` record (`\x9C`): deprecation
//! messages, attributes, inline bodies and the other per-declaration extras
//! (DCU32INT `TConstValInfo.Read`, `TUnit.ReadConstAddInfo`,
//! `TAttributeDeclModifier.Read`, `ReadInlineInfo`).

use crate::dcu::{
    Attribute, AttributeArg, ConstData, ConstKind, ConstValue, Error, ErrorKind, ExtraArg,
    InlineBody, Modifier, TypeSpelling, decode_name,
    parse::Parser,
    version::{Platform, Version},
};

impl Parser<'_> {
    /// Reads a constant value (DCU32INT `TConstValInfo.Read`): from Delphi
    /// 5 a kind, then a size; a zero size is followed by the value as a
    /// signed index (except for an XE2+ nil pointer), a non-zero size by
    /// that many bytes.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Malformed`] for an unknown kind.
    pub fn read_const_value(&mut self, type_index: u32) -> Result<ConstValue, Error> {
        let mut kind = None;
        let mut need_value = true;
        if self.after(Version::DELPHI_4) {
            let k = self.r.uindex()?;
            kind = Some(match k {
                0 => ConstKind::Scalar,
                1 => ConstKind::String,
                2 => ConstKind::ResourceString,
                3 => ConstKind::Float,
                4 => ConstKind::Set,
                5 if self.d(Version::DELPHI_2009) => ConstKind::UnicodeString,
                _ => return Err(self.error(ErrorKind::Malformed("unknown constant kind"))),
            });
            if self.d(Version::DELPHI_XE2) {
                need_value = k != 4;
            }
        }
        let size = self.r.uindex()?;
        let data = if size == 0 {
            ConstData::Int(if need_value { self.r.index64()? } else { 0 })
        } else {
            ConstData::Bytes(self.r.bytes(size as usize)?.to_vec())
        };
        Ok(ConstValue {
            type_index,
            kind,
            data,
        })
    }

    /// Queues `m` for the declaration at address `addr`.
    fn attach(&mut self, addr: u32, m: Modifier) {
        if let Some(id) = self.addr_decl(addr) {
            self.decl_mods.push((id, m));
        }
    }

    /// Reads a `drConstAddInfo` record: tagged sub-records up to a
    /// version-dependent stop byte (`0x06` up to Delphi 7, `0x0D` Delphi 8,
    /// `0x0F` 2005 - 2007, `0xFF` from 2009).
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] for a malformed sub-record or a wrong stop byte.
    pub fn read_const_add_info(&mut self) -> Result<(), Error> {
        let stop: u8 = if self.version <= Version::DELPHI_7 || self.version.is_kylix() {
            0x06
        } else if self.version >= Version::DELPHI_2009 {
            0xFF
        } else if self.version >= Version::DELPHI_2005 {
            0x0F
        } else {
            0x0D
        };
        let mobile = matches!(
            self.platform,
            Platform::IosSimulator
                | Platform::IosDevice32
                | Platform::IosDevice64
                | Platform::Android
        );
        let mut t;
        loop {
            t = self.r.u8()?;
            if t >= stop {
                break;
            }
            match t {
                0x01 => {
                    let addr = self.r.uindex()?;
                    let f = self.r.uindex()?;
                    if let Some(id) = self.addr_decl(addr) {
                        self.decl_add_flags.push((id, f));
                    }
                    if self.d(Version::DELPHI_2006)
                        && self.version <= Version::DELPHI_10_SEATTLE
                        && !mobile
                        && f & 0x0100_0000 != 0
                    {
                        self.r.uindex()?;
                    }
                    if self.msil {
                        if self.version >= Version::DELPHI_2005 && f & 0x1_0000 != 0 {
                            let len = self.r.uindex()? as usize;
                            self.r.skip(len)?;
                        }
                        let len = self.r.uindex()?;
                        for _ in 0..len {
                            let h = self.r.uindex()?;
                            self.ref_addr(h)?;
                            self.r.uindex()?;
                            let s = self.r.index_string()?;
                            if !s.is_empty() {
                                let n = self.r.uindex()?;
                                for _ in 0..n {
                                    let h = self.r.uindex()?;
                                    self.ref_addr(h)?;
                                }
                            }
                        }
                    }
                    if self.d(Version::DELPHI_2005) {
                        let (mut inline_flag, mut big_value) = (0x8_0000, 0x10_0000);
                        if self.version >= Version::DELPHI_2009 {
                            if f & 0x80_0000 != 0 {
                                self.r.uindex()?;
                            }
                            if f & 0x01 != 0 {
                                let msg = decode_name(self.r.index_string()?);
                                self.attach(addr, Modifier::Deprecated(msg));
                            }
                            if self.version >= Version::DELPHI_XE6 && f & 0x8000_0000 != 0 {
                                self.read_attributes(addr)?;
                            }
                            inline_flag = 0x4_0000;
                            big_value = 0x8_0000;
                        }
                        if f & inline_flag != 0 {
                            let body = self.read_inline_body()?;
                            self.attach(addr, Modifier::Inline(body));
                        }
                        if f & big_value != 0 {
                            self.r.uindex()?;
                        }
                    }
                }
                0x04 => {
                    if !self.d(Version::DELPHI_2006) {
                        break;
                    }
                    self.r.uindex()?;
                    self.r.uindex()?;
                }
                0x06 | 0x07 => {
                    for _ in 0..4 {
                        self.r.uindex()?;
                    }
                }
                0x09 => {
                    self.r.uindex()?;
                    self.r.uindex()?;
                }
                0x0A => self.read_generated_info()?,
                0x0C => {
                    for _ in 0..3 {
                        self.r.uindex()?;
                    }
                }
                0x0D => {
                    if !self.d(Version::DELPHI_2005) {
                        break;
                    }
                    self.r.uindex()?;
                    self.r.index_string()?;
                }
                0x10 => {
                    if !self.d(Version::DELPHI_2009) {
                        break;
                    }
                    self.r.uindex()?;
                    // From 10.4 the second index can name an address not
                    // yet created, which a later `drProcAddInfo` claims.
                    let h = self.r.uindex()?;
                    if self.d(Version::DELPHI_10_4_SYDNEY) {
                        self.ref_addr(h)?;
                    }
                    self.r.uindex()?;
                }
                0x11 => {
                    if !self.d(Version::DELPHI_XE4) {
                        break;
                    }
                    for _ in 0..2 {
                        let h = self.r.uindex()?;
                        self.ref_addr(h)?;
                    }
                }
                0x12 => {
                    if !self.d(Version::DELPHI_2009) {
                        break;
                    }
                    self.r.uindex()?;
                    self.r.uindex()?;
                }
                0x13 => {
                    if !self.d(Version::DELPHI_2009) {
                        break;
                    }
                    let addr = self.r.uindex()?;
                    self.ref_addr(addr)?;
                    let a = self.r.uindex()?;
                    let b = self.r.uindex()?;
                    let names = [
                        decode_name(self.r.index_string()?),
                        decode_name(self.r.index_string()?),
                        decode_name(self.r.index_string()?),
                    ];
                    self.decl_mods_at
                        .push((addr, Modifier::CppNames { a, b, names }));
                }
                0x14 => {
                    if !self.d(Version::DELPHI_2009) {
                        break;
                    }
                    self.r.uindex()?;
                    self.r.uindex()?;
                    let n = self.r.uindex()?;
                    for _ in 0..n {
                        for _ in 0..3 {
                            self.r.uindex()?;
                        }
                        self.r.index_string()?;
                    }
                }
                0x15 => {
                    if !self.d(Version::DELPHI_XE2) {
                        break;
                    }
                    for _ in 0..3 {
                        self.r.uindex()?;
                    }
                }
                0x08 => {
                    if !self.d(Version::DELPHI_10_4_SYDNEY) {
                        break;
                    }
                    let addr = self.r.uindex()?;
                    let line = decode_name(self.r.index_string()?);
                    // The comment can precede the declaration it documents.
                    self.decl_mods_at
                        .push((addr, Modifier::Documentation(line)));
                }
                0x16 => {
                    if !self.d(Version::DELPHI_10_4_SYDNEY) {
                        break;
                    }
                    self.r.uindex()?;
                    self.r.index_string()?;
                }
                0x17 => {
                    if !self.d(Version::DELPHI_10_4_SYDNEY) {
                        break;
                    }
                    let addr = self.r.uindex()?;
                    let value = self.read_type_spelling()?;
                    let result = self.read_type_spelling()?;
                    self.decl_mods_at
                        .push((addr, Modifier::DeclaredType { value, result }));
                }
                _ => break,
            }
        }
        if t != stop {
            return Err(self.error(ErrorKind::Malformed("unexpected extra-information tag")));
        }
        Ok(())
    }

    /// Reads a [`TypeSpelling`] (Delphi 10.4 and later): a flag set, then a
    /// field per set flag - `0x01` an address, `0x02` a name, `0x04` the
    /// unit declaring it, `0x08` a C++ kind.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Malformed`] for a flag outside `0x0F`.
    pub fn read_type_spelling(&mut self) -> Result<TypeSpelling, Error> {
        let flags = self.r.uindex()?;
        if flags & !0x0F != 0 {
            return Err(self.error(ErrorKind::Malformed("unknown type-spelling flags")));
        }
        let mut spelling = TypeSpelling::default();
        if flags & 0x01 != 0 {
            spelling.address = Some(self.r.uindex()?);
        }
        if flags & 0x02 != 0 {
            spelling.name = self.read_spelled_name()?;
        }
        if flags & 0x04 != 0 {
            spelling.unit = self.read_spelled_name()?;
        }
        if flags & 0x08 != 0 {
            spelling.cpp_kind = Some(self.r.uindex()?);
        }
        Ok(spelling)
    }

    /// Reads a name stored as its length plus one, then its bytes; a
    /// stored length of 0 means no name (Delphi 10.4 and later).
    pub fn read_spelled_name(&mut self) -> Result<Option<String>, Error> {
        let len = self.r.uindex()? as usize;
        if len == 0 {
            return Ok(None);
        }
        let bytes = self.r.bytes(len.saturating_sub(1))?;
        Ok(Some(decode_name(bytes)))
    }

    /// Reads the .NET generated-code sub-record `0x0A`.
    fn read_generated_info(&mut self) -> Result<(), Error> {
        let addr = self.r.uindex()?;
        self.r.uindex()?;
        let f = self.r.uindex()?;
        for bit in [0x01, 0x02, 0x04, 0x08, 0x10, 0x20] {
            if f & bit != 0 {
                self.r.uindex()?;
            }
        }
        if f & 0x40 != 0 {
            let n = self.r.uindex()?;
            let mut args = Vec::new();
            for _ in 0..n {
                let name = decode_name(self.r.index_string()?);
                self.r.uindex()?;
                self.r.uindex()?;
                let type_index = self.r.uindex()?;
                args.push(ExtraArg { name, type_index });
            }
            self.attach(addr, Modifier::ExtraArgs(args));
        }
        if f & 0x80 != 0 {
            for _ in 0..3 {
                self.r.uindex()?;
            }
        }
        if f & 0x100 != 0 {
            let name = decode_name(self.r.index_string()?);
            self.attach(addr, Modifier::GeneratedName(name));
        }
        if f & 0x200 != 0 {
            self.r.index_string()?;
        }
        for bit in [0x400, 0x800, 0x1000, 0x2000, 0x4000] {
            if f & bit != 0 {
                let h = self.r.uindex()?;
                if self.msil {
                    self.ref_addr(h)?;
                }
            }
        }
        Ok(())
    }

    /// Reads the custom attributes of a declaration (XE6 and later).
    fn read_attributes(&mut self, addr: u32) -> Result<(), Error> {
        let count = self.r.uindex()?;
        for _ in 0..count {
            let constructor = self.r.uindex()?;
            self.ref_addr(constructor)?;
            let member = self.r.uindex()?;
            let attribute_type = self.r.uindex()?;
            let arg_count = self.r.uindex()?;
            let mut args = Vec::new();
            for _ in 0..arg_count {
                match self.r.uindex()? {
                    0 => {
                        let type_index = self.r.uindex()?;
                        args.push(AttributeArg::Const(self.read_const_value(type_index)?));
                    }
                    1 => {
                        let type_index = self.r.uindex()?;
                        let type_addr = self.r.uindex()?;
                        self.ref_addr(type_addr)?;
                        args.push(AttributeArg::TypeInfo {
                            type_index,
                            type_addr,
                        });
                    }
                    _ => {
                        return Err(self.error(ErrorKind::Malformed("attribute argument kind")));
                    }
                }
            }
            self.attach(
                addr,
                Modifier::Attribute(Attribute {
                    constructor,
                    member,
                    attribute_type,
                    args,
                }),
            );
        }
        Ok(())
    }

    /// Reads an inline routine body (DCU32INT `ReadInlineInfo`). The byte
    /// code is a length-prefixed block; the tables after it list the
    /// addresses, types and units it refers to, and reserve forward
    /// addresses the way any other reference does.
    fn read_inline_body(&mut self) -> Result<InlineBody, Error> {
        if self.d(Version::DELPHI_2006) {
            self.r.uindex()?;
            self.r.uindex()?;
        }
        let code_size = self.r.uindex()? as usize;
        let start = self.r.pos();
        self.r.skip(code_size)?;
        let code = start..self.r.pos();
        self.r.uindex()?;
        self.r.uindex()?;
        if self.d(Version::DELPHI_XE2) {
            self.r.uindex()?;
        }
        let root = self.r.uindex()?;
        let result_vars = self.r.uindex()?;
        self.r.uindex()?;
        self.r.uindex()?;
        let addr_count = self.r.uindex()?;
        if self.version >= Version::DELPHI_2009 {
            self.r.uindex()?;
            self.r.uindex()?;
            let lines = self.r.uindex()? as usize;
            self.r.skip(lines.saturating_mul(4))?;
        }
        let mut addrs = Vec::new();
        for _ in 0..addr_count {
            let addr;
            if self.version < Version::DELPHI_2009 {
                if self.r.uindex()? == 0 {
                    addr = self.r.uindex()?;
                    self.r.uindex()?;
                } else {
                    addr = 0;
                    self.r.uindex()?;
                }
            } else {
                addr = self.r.uindex()?;
                self.r.uindex()?;
                self.r.uindex()?;
            }
            addrs.push(addr);
            self.ref_addr(addr)?;
            if self.version >= Version::DELPHI_2009 {
                self.r.uindex()?;
                let z = self.r.uindex()?;
                if self.version >= Version::DELPHI_2010 {
                    self.r.uindex()?;
                }
                for _ in 0..z {
                    self.r.uindex()?;
                }
            }
        }
        let type_count = self.r.uindex()?;
        for _ in 0..type_count {
            let kind = self.r.uindex()?;
            let type_ref = self.r.uindex()?;
            if self.version >= Version::DELPHI_2009 {
                let xe = u32::from(self.version >= Version::DELPHI_XE);
                let berlin = u32::from(self.version >= Version::DELPHI_10_1_BERLIN);
                let fields: u32 = match kind {
                    1 => {
                        if self.version >= Version::DELPHI_XE {
                            self.ref_addr(type_ref)?;
                        }
                        self.r.uindex()?;
                        1u32.saturating_add(xe)
                    }
                    2 => 1,
                    3 => 3,
                    4 => 2,
                    5 => 4,
                    6 => 1u32.saturating_add(berlin),
                    7 if self.version >= Version::DELPHI_XE8 => 1u32.saturating_add(berlin),
                    _ => {
                        return Err(self.error(ErrorKind::Malformed("inline type-table kind")));
                    }
                };
                for _ in 1..fields {
                    self.r.uindex()?;
                }
            } else if kind == 1 {
                self.r.uindex()?;
                self.r.uindex()?;
            }
        }
        let unit_count = self.r.uindex()?;
        for _ in 0..unit_count {
            let unit = self.r.uindex()?;
            let n = self.r.uindex()?;
            for _ in 0..n {
                let v = self.r.uindex()?;
                if unit == 0 {
                    self.ref_addr(v)?;
                }
            }
        }
        if self.version >= Version::DELPHI_2006 {
            let n = self.r.uindex()?;
            for _ in 0..n {
                self.r.uindex()?;
            }
            if self.version >= Version::DELPHI_2009 {
                self.r.uindex()?;
                let v = self.r.uindex()?;
                self.ref_addr(v)?;
                self.r.uindex()?;
            }
        }
        Ok(InlineBody {
            code,
            root,
            result_vars,
            addrs,
        })
    }
}
