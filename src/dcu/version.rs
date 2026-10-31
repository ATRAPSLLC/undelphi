//! DCU format versions and target platforms, decoded from the magic.
//!
//! A DCU starts with a 32-bit magic that names the compiler and, from XE2
//! on, the target platform. Delphi 2 - XE use one fixed value per release
//! (two for some); XE2 and later follow a scheme (DCU32INT `ReadMagic`):
//!
//! ```text
//! byte 3: release       0x17 = XE2 ... 0x1E = 10 Seattle ... 0x21 = 10.3 Rio
//!                       ... 0x25 = 13 Florence
//! byte 1: platform      0x03 Win32, 0x23 Win64, 0x04 OS X 32, 0x14 iOS
//!                       simulator, 0x76 iOS device, 0x94 iOS 64-bit device,
//!                       0x77 Android, 0x21 Linux 64
//! byte 0: 0x4B (XE2 - XE5) or 0x4D (XE6 and later)
//! byte 2: 0x00
//! ```
//!
//! Releases after 10.3 Rio follow the same scheme with a higher release
//! byte, up to `0x25` for Delphi 13; a higher byte is rejected as unknown.
//! DCU32INT stops at 10.3; the record layouts of later releases are our
//! own findings, verified on Delphi 13 and assumed from 10.4 (10.4 - 12
//! are unsampled; see the `parse` module).

/// A DCU format version: the compiler release that wrote the unit.
///
/// The numbering is DCU32INT's: Delphi releases count by product version
/// (2 - 8, then 9 = 2005, 10 = 2006, 12 = 2009, 14 = 2010, 15 = XE, ... 23 =
/// 10 Seattle, 26 = 10.3 Rio), and Kylix releases start at 100. Comparisons
/// that only hold for the Delphi line go through [`Version::delphi_at_least`]
/// so that Kylix (numerically higher) never satisfies them by accident.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version(u8);

impl Version {
    /// Returns whether this is a Delphi (not Kylix) release at or after
    /// `v`.
    #[must_use]
    pub fn delphi_at_least(self, v: Version) -> bool {
        !self.is_kylix() && self >= v
    }

    /// Returns whether this release is numerically after `v`, Kylix
    /// included (DCU32INT's plain `Ver>X` tests).
    #[must_use]
    pub fn after(self, v: Version) -> bool {
        self > v
    }

    /// Delphi 2.
    pub const DELPHI_2: Self = Self(2);
    /// Delphi 3.
    pub const DELPHI_3: Self = Self(3);
    /// Delphi 4.
    pub const DELPHI_4: Self = Self(4);
    /// Delphi 5.
    pub const DELPHI_5: Self = Self(5);
    /// Delphi 6 (also the Pascal compiler of C++Builder 6).
    pub const DELPHI_6: Self = Self(6);
    /// Delphi 7.
    pub const DELPHI_7: Self = Self(7);
    /// Delphi 8 (.NET only).
    pub const DELPHI_8: Self = Self(8);
    /// Delphi 2005.
    pub const DELPHI_2005: Self = Self(9);
    /// Delphi 2006.
    pub const DELPHI_2006: Self = Self(10);
    /// Delphi 2009.
    pub const DELPHI_2009: Self = Self(12);
    /// Delphi 2010.
    pub const DELPHI_2010: Self = Self(14);
    /// Delphi XE.
    pub const DELPHI_XE: Self = Self(15);
    /// Delphi XE2.
    pub const DELPHI_XE2: Self = Self(16);
    /// Delphi XE3.
    pub const DELPHI_XE3: Self = Self(17);
    /// Delphi XE4.
    pub const DELPHI_XE4: Self = Self(18);
    /// Delphi XE5.
    pub const DELPHI_XE5: Self = Self(19);
    /// Delphi XE6.
    pub const DELPHI_XE6: Self = Self(20);
    /// Delphi XE7.
    pub const DELPHI_XE7: Self = Self(21);
    /// Delphi XE8.
    pub const DELPHI_XE8: Self = Self(22);
    /// Delphi 10 Seattle.
    pub const DELPHI_10_SEATTLE: Self = Self(23);
    /// Delphi 10.1 Berlin.
    pub const DELPHI_10_1_BERLIN: Self = Self(24);
    /// Delphi 10.2 Tokyo.
    pub const DELPHI_10_2_TOKYO: Self = Self(25);
    /// Delphi 10.3 Rio.
    pub const DELPHI_10_3_RIO: Self = Self(26);
    /// Delphi 10.4 Sydney.
    ///
    /// Every rule this crate applies "from 10.4" (the records added after
    /// 10.3 and Delphi 13's export-name changes) was established on Delphi
    /// 13 alone; 10.4 is assumed as the first release, and 10.4 - 12 are
    /// unsampled. A misplaced record boundary fails loudly (the decoder
    /// requires the image's last tag at its end); a misplaced mangling rule
    /// would give a wrong export name silently.
    pub const DELPHI_10_4_SYDNEY: Self = Self(27);
    /// Delphi 11 Alexandria.
    pub const DELPHI_11_ALEXANDRIA: Self = Self(28);
    /// Delphi 12 Athens.
    pub const DELPHI_12_ATHENS: Self = Self(29);
    /// Delphi 13 Florence.
    pub const DELPHI_13_FLORENCE: Self = Self(30);
    /// Kylix 1.
    pub const KYLIX_1: Self = Self(100);
    /// Kylix 2.
    pub const KYLIX_2: Self = Self(101);
    /// Kylix 3.
    pub const KYLIX_3: Self = Self(102);

    /// Returns DCU32INT's numeric code for this version.
    #[must_use]
    pub fn code(self) -> u8 {
        self.0
    }

    /// Returns whether this is a Kylix release.
    #[must_use]
    pub fn is_kylix(self) -> bool {
        self.0 >= 100
    }

    /// Returns the compiler version `dcc` reports (`VER140` is 14.0 for
    /// Delphi 6, `VER300` is 30.0 for 10 Seattle), or `None` for Kylix and
    /// the unused codes.
    #[must_use]
    pub fn compiler_version(self) -> Option<u8> {
        // VER90 (Delphi 2), VER100, then VER120 (Delphi 4) - VER160 (Delphi
        // 8): Delphi 3 to 4 skipped 11.0.
        match self.0 {
            2 | 3 => Some(self.0.saturating_add(7)),
            4..=8 => Some(self.0.saturating_add(8)),
            9 => Some(17),
            10 => Some(18),
            12 => Some(20),
            14..=99 => Some(self.0.saturating_add(7)),
            _ => None,
        }
    }

    /// Returns the Delphi release whose compiler reports version `major`
    /// (`14` for Delphi 6, `37` for Delphi 13): the inverse of
    /// [`Version::compiler_version`]. `None` for a number no release
    /// reports, and for 18 and 18.5's shared 18, which is Delphi 2006.
    #[must_use]
    pub fn of_compiler(major: u8) -> Option<Self> {
        (2..=99)
            .map(Self)
            .find(|v| v.compiler_version() == Some(major))
    }

    /// Returns the product name, `"Delphi 10 Seattle"` style.
    #[must_use]
    pub fn product_name(self) -> String {
        let delphi = match self.0 {
            2..=8 => return format!("Delphi {}", self.0),
            9 => "2005",
            10 => "2006",
            12 => "2009",
            14 => "2010",
            15 => "XE",
            16 => "XE2",
            17 => "XE3",
            18 => "XE4",
            19 => "XE5",
            20 => "XE6",
            21 => "XE7",
            22 => "XE8",
            23 => "10 Seattle",
            24 => "10.1 Berlin",
            25 => "10.2 Tokyo",
            26 => "10.3 Rio",
            27 => "10.4 Sydney",
            28 => "11 Alexandria",
            29 => "12 Athens",
            30 => "13 Florence",
            100..=255 => return format!("Kylix {}", self.0.saturating_sub(99)),
            _ => return format!("Delphi (DCU version {})", self.0),
        };
        format!("Delphi {delphi}")
    }
}

/// The target a DCU was compiled for.
///
/// Before XE2 every native DCU is [`Platform::Win32`] (or Linux for Kylix,
/// which the magic does not distinguish further).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Platform {
    /// 32-bit Windows.
    Win32,
    /// 64-bit Windows.
    Win64,
    /// 32-bit OS X.
    Osx32,
    /// The iOS simulator.
    IosSimulator,
    /// 32-bit iOS devices (code lives in a companion `.o`).
    IosDevice32,
    /// 64-bit iOS devices (code lives in a companion `.o`).
    IosDevice64,
    /// Android (code lives in a companion `.o`).
    Android,
    /// 64-bit Linux (code lives in a companion `.o`).
    Linux64,
}

impl Platform {
    /// Returns the size of a pointer on this platform.
    #[must_use]
    pub fn pointer_size(self) -> u8 {
        match self {
            Self::Win64 | Self::IosDevice64 | Self::Linux64 => 8,
            _ => 4,
        }
    }

    /// Returns whether the platform's DCUs omit the code block because an
    /// LLVM back end keeps the machine code in a companion object file.
    #[must_use]
    pub fn is_llvm(self) -> bool {
        matches!(
            self,
            Self::IosDevice32 | Self::IosDevice64 | Self::Android | Self::Linux64
        )
    }

    /// Returns whether this is a mobile platform.
    #[must_use]
    pub fn is_mobile(self) -> bool {
        matches!(
            self,
            Self::IosSimulator | Self::IosDevice32 | Self::IosDevice64 | Self::Android
        )
    }
}

/// What the magic of a DCU says about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Magic {
    /// The compiler release.
    pub version: Version,
    /// The target platform.
    pub platform: Platform,
    /// Whether the unit is .NET IL (Delphi 8, 2005 and 2006 `.dcuil`).
    pub msil: bool,
}

impl Magic {
    /// Decodes a DCU magic, or returns `None` for an unknown value.
    #[must_use]
    pub fn decode(magic: u32) -> Option<Self> {
        let native = |version| Self {
            version,
            platform: Platform::Win32,
            msil: false,
        };
        let msil = |version| Self {
            version,
            platform: Platform::Win32,
            msil: true,
        };
        let fixed = match magic {
            0x5050_5348 => Some(native(Version::DELPHI_2)),
            0x4451_8641 => Some(native(Version::DELPHI_3)),
            0x4768_A6D8 => Some(native(Version::DELPHI_4)),
            0xF21F_148B => Some(native(Version::DELPHI_5)),
            0x0E00_00DD | 0x0E80_00DD => Some(native(Version::DELPHI_6)),
            0xFF00_00DF | 0x0F00_00DF | 0x0F80_00DF => Some(native(Version::DELPHI_7)),
            0x1000_0229 => Some(msil(Version::DELPHI_8)),
            0x1100_0239 => Some(msil(Version::DELPHI_2005)),
            0x1100_000D | 0x1180_0009 => Some(native(Version::DELPHI_2005)),
            0x1200_024D => Some(msil(Version::DELPHI_2006)),
            0x1200_0023 => Some(native(Version::DELPHI_2006)),
            0x1400_0039 => Some(native(Version::DELPHI_2009)),
            0x1500_0045 => Some(native(Version::DELPHI_2010)),
            0x1600_034B => Some(native(Version::DELPHI_XE)),
            0xF21F_148C => Some(native(Version::KYLIX_1)),
            0x0E10_11DD | 0x0E00_01DD => Some(native(Version::KYLIX_2)),
            0x0F10_01DD | 0x0F00_01DD => Some(native(Version::KYLIX_3)),
            _ => None,
        };
        if fixed.is_some() {
            return fixed;
        }
        if magic & 0x00FF_00F9 != 0x0000_0049 {
            return None;
        }
        let release = (magic >> 24) as u8;
        let low = (magic & 0xFF) as u8;
        let scheme_ok =
            (release >= 0x1B && low == 0x4D) || ((0x17..=0x1A).contains(&release) && low == 0x4B);
        if !scheme_ok {
            return None;
        }
        // 0x17 is XE2, DCU32INT code 16. A release byte past the latest
        // known release is a format nobody has decoded (and from 0x71 on
        // its code would fall in the Kylix range).
        let version = Version(
            release
                .checked_sub(0x17)?
                .checked_add(Version::DELPHI_XE2.0)?,
        );
        if version > Version::DELPHI_13_FLORENCE {
            return None;
        }
        let platform = match (magic >> 8) as u8 {
            0x03 => Platform::Win32,
            0x23 => Platform::Win64,
            0x04 => Platform::Osx32,
            0x14 if version >= Version::DELPHI_XE4 => Platform::IosSimulator,
            0x76 if version >= Version::DELPHI_XE4 => Platform::IosDevice32,
            0x94 if version >= Version::DELPHI_XE8 => Platform::IosDevice64,
            0x77 if version >= Version::DELPHI_XE5 => Platform::Android,
            0x21 if version >= Version::DELPHI_10_2_TOKYO => Platform::Linux64,
            _ => return None,
        };
        Some(Self {
            version,
            platform,
            msil: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_the_magics_we_ship_or_reference() {
        let d6 = Magic::decode(0x0E00_00DD).unwrap();
        assert_eq!(d6.version, Version::DELPHI_6);
        let s32 = Magic::decode(0x1E00_034D).unwrap();
        assert_eq!(
            (s32.version, s32.platform),
            (Version::DELPHI_10_SEATTLE, Platform::Win32)
        );
        let s64 = Magic::decode(0x1E00_234D).unwrap();
        assert_eq!(s64.platform, Platform::Win64);
        assert_eq!(s64.platform.pointer_size(), 8);
        assert_eq!(Version::DELPHI_10_SEATTLE.compiler_version(), Some(30));
        assert_eq!(Version::DELPHI_6.compiler_version(), Some(14));
        assert_eq!(Version::DELPHI_2009.compiler_version(), Some(20));
        assert_eq!(Version::of_compiler(30), Some(Version::DELPHI_10_SEATTLE));
        assert_eq!(Version::of_compiler(37), Some(Version::DELPHI_13_FLORENCE));
        assert_eq!(Version::of_compiler(14), Some(Version::DELPHI_6));
        assert_eq!(Version::of_compiler(11), None);
        assert!(Magic::decode(0xDEAD_BEEF).is_none());
    }
}
