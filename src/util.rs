//! Shared low-level readers and decoders used by the RTTI / methods /
//! interfaces / form modules. The returned slices borrow from the binary
//! context's underlying byte buffer.
//!
//! Every offset arithmetic here uses [`usize::checked_add`] /
//! [`usize::checked_sub`] so adversarial inputs (e.g. an enormous length
//! field on disk) cannot overflow into a panic in debug builds or a
//! wrap-around in release builds. The result is a clean `None`
//! propagated up to the caller.

use crate::formats::BinaryContext;

/// Read a little-endian pointer of `size` bytes (4 or 8) at a file offset.
pub fn read_ptr(bytes: &[u8], off: usize, size: usize) -> Option<u64> {
    match size {
        4 => {
            let end = off.checked_add(4)?;
            let slice = bytes.get(off..end)?;
            Some(u32::from_le_bytes(slice.try_into().ok()?) as u64)
        }
        8 => {
            let end = off.checked_add(8)?;
            let slice = bytes.get(off..end)?;
            Some(u64::from_le_bytes(slice.try_into().ok()?))
        }
        _ => None,
    }
}

/// Read an unsigned little-endian `u16` at a file offset.
#[inline]
pub fn read_u16(bytes: &[u8], off: usize) -> Option<u16> {
    let end = off.checked_add(2)?;
    let slice = bytes.get(off..end)?;
    Some(u16::from_le_bytes(slice.try_into().ok()?))
}

/// Read an unsigned little-endian `u32` at a file offset.
#[inline]
pub fn read_u32(bytes: &[u8], off: usize) -> Option<u32> {
    let end = off.checked_add(4)?;
    let slice = bytes.get(off..end)?;
    Some(u32::from_le_bytes(slice.try_into().ok()?))
}

/// Resolve `va` to a file offset and read a short-string's body.
///
/// Returns the bytes of the string, excluding the length-byte prefix.
pub fn read_short_string_at_va<'a>(ctx: &BinaryContext<'a>, va: u64) -> Option<&'a [u8]> {
    if va == 0 {
        return None;
    }
    let off = ctx.va_to_file(va)?;
    read_short_string_at_file(ctx.data(), off)
}

/// Read a short-string starting at `off` within `data`. Returns the body
/// slice (no length byte).
pub fn read_short_string_at_file(data: &[u8], off: usize) -> Option<&[u8]> {
    let len = *data.get(off)? as usize;
    let body_start = off.checked_add(1)?;
    let body_end = body_start.checked_add(len)?;
    data.get(body_start..body_end)
}

/// Read a pointer at `va`, then interpret the result as another VA. Used by
/// FPC's double-indirection fields (PShortString, PPGuid, etc.).
pub fn deref_va(ctx: &BinaryContext<'_>, va: u64, ptr_size: usize) -> Option<u64> {
    if va == 0 {
        return None;
    }
    let off = ctx.va_to_file(va)?;
    read_ptr(ctx.data(), off, ptr_size)
}

/// Converts an x87 80-bit extended-precision value (64-bit mantissa with
/// an explicit integer bit, 15-bit exponent biased by 16383, sign) to the
/// nearest `f64` (ties to even): values beyond `f64`'s range become
/// infinities, values below half its least subnormal zeros of the same
/// sign, and NaNs a NaN.
pub fn extended_to_f64(bytes: [u8; 10]) -> f64 {
    let [m0, m1, m2, m3, m4, m5, m6, m7, e0, e1] = bytes;
    let mantissa = u64::from_le_bytes([m0, m1, m2, m3, m4, m5, m6, m7]);
    let se = u16::from_le_bytes([e0, e1]);
    let sign = u64::from(se >> 15) << 63;
    let exponent = i32::from(se & 0x7FFF);
    let infinity = f64::from_bits(sign | 0x7FF0_0000_0000_0000);
    if exponent == 0x7FFF {
        return if mantissa << 1 == 0 {
            infinity
        } else {
            f64::NAN
        };
    }
    if mantissa == 0 {
        return f64::from_bits(sign);
    }
    extended_bits(mantissa, exponent).map_or(infinity, |bits| f64::from_bits(sign | bits))
}

/// The magnitude bits of the `f64` nearest a finite, non-zero extended
/// value `mantissa * 2^(max(exponent, 1) - 16383 - 63)`, or `None` when it
/// overflows `f64`.
fn extended_bits(mantissa: u64, exponent: i32) -> Option<u64> {
    const HALF: u64 = 1 << 63;
    let lz = mantissa.leading_zeros();
    // Normalised: `m / 2^63 * 2^e` with the top bit of `m` set.
    let m = mantissa << lz;
    let e = exponent
        .max(1)
        .checked_sub(16383)?
        .checked_sub(i32::try_from(lz).ok()?)?;
    if e > 1023 {
        return None;
    }
    // Significant bits the result keeps: 53 for a normal, fewer for a
    // subnormal (below 2^-1022).
    let keep = if e >= -1022 {
        53
    } else {
        e.checked_add(1022 + 53)?
    };
    if keep <= 0 {
        // Below the least subnormal (2^-1074): at keep 0 the value lies in
        // [2^-1075, 2^-1074) and rounds up unless it is the exact tie.
        return Some(u64::from(keep == 0 && m > HALF));
    }
    let drop = 64u32.checked_sub(u32::try_from(keep).ok()?)?;
    let kept = m >> drop;
    let rest = m << 64u32.checked_sub(drop)?;
    let up = rest > HALF || rest == HALF && kept & 1 == 1;
    let kept = kept.checked_add(u64::from(up))?;
    if e < -1022 {
        // A subnormal's bits are its significand; rounding up to 2^52 lands
        // on the least normal, which has the same bits.
        return Some(kept);
    }
    let (kept, e) = if kept == 1 << 53 {
        (kept >> 1, e.checked_add(1)?)
    } else {
        (kept, e)
    };
    if e > 1023 {
        return None;
    }
    let biased = u64::try_from(e.checked_add(1023)?).ok()?;
    Some(biased << 52 | kept & ((1 << 52) - 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The extended encoding of `mantissa * 2^(exponent - 16383 - 63)`.
    fn ext(negative: bool, exponent: u16, mantissa: u64) -> [u8; 10] {
        let mut out = [0u8; 10];
        out[..8].copy_from_slice(&mantissa.to_le_bytes());
        out[8..].copy_from_slice(&(exponent | if negative { 0x8000 } else { 0 }).to_le_bytes());
        out
    }

    /// The extended encoding of a finite, non-zero `f64` (always exact).
    fn from_f64(x: f64) -> [u8; 10] {
        let bits = x.to_bits();
        let biased = (bits >> 52) & 0x7FF;
        let frac = bits & ((1 << 52) - 1);
        let (mantissa, exponent) = if biased == 0 {
            let lz = frac.leading_zeros();
            (frac << lz, 63 - lz as i32 - 1074 + 16383)
        } else {
            ((1 << 63) | frac << 11, biased as i32 - 1023 + 16383)
        };
        ext(x < 0.0, exponent as u16, mantissa)
    }

    #[test]
    fn extended_specials() {
        assert_eq!(extended_to_f64(ext(false, 0, 0)).to_bits(), 0f64.to_bits());
        assert_eq!(
            extended_to_f64(ext(true, 0, 0)).to_bits(),
            (-0f64).to_bits()
        );
        assert_eq!(extended_to_f64(ext(false, 0x7FFF, 1 << 63)), f64::INFINITY);
        assert_eq!(
            extended_to_f64(ext(true, 0x7FFF, 1 << 63)),
            f64::NEG_INFINITY
        );
        assert!(extended_to_f64(ext(false, 0x7FFF, 0xC000_0000_0000_0000)).is_nan());
        // Beyond f64's range, and an extended denormal far below it.
        assert_eq!(
            extended_to_f64(ext(false, 16383 + 1024, 1 << 63)),
            f64::INFINITY
        );
        assert_eq!(
            extended_to_f64(ext(true, 0, 1)).to_bits(),
            (-0f64).to_bits()
        );
    }

    #[test]
    fn extended_round_trips_every_f64_kind() {
        let subnormal = f64::from_bits(1 << 40);
        for x in [
            1.0,
            -1.5,
            0.75,
            12.3456,
            1e-300,
            f64::MAX,
            f64::MIN_POSITIVE,
            subnormal,
            f64::from_bits(1),
            -f64::from_bits(1),
        ] {
            assert_eq!(extended_to_f64(from_f64(x)).to_bits(), x.to_bits(), "{x:e}");
        }
    }

    #[test]
    fn extended_rounds_to_nearest_even() {
        let one = 1u64 << 63;
        // 1 + 2^-53: a tie, kept even at 1.
        assert_eq!(extended_to_f64(ext(false, 16383, one | 1 << 10)), 1.0);
        // Just above the tie: up to 1 + 2^-52.
        assert_eq!(
            extended_to_f64(ext(false, 16383, one | 1 << 10 | 1)),
            1.0 + f64::EPSILON
        );
        // Rounding up past the largest significand carries into the exponent.
        assert_eq!(extended_to_f64(ext(false, 16383, u64::MAX)), 2.0);
        // Rounding the largest f64 up overflows.
        assert_eq!(
            extended_to_f64(ext(false, 16383 + 1023, u64::MAX)),
            f64::INFINITY
        );
        // Around the least subnormal 2^-1074: 2^-1075 is a tie and rounds to
        // zero, anything above it to 2^-1074, 2^-1076 to zero.
        let least = f64::from_bits(1);
        assert_eq!(extended_to_f64(ext(false, 16383 - 1075, one)), 0.0);
        assert_eq!(extended_to_f64(ext(false, 16383 - 1075, one | 1)), least);
        assert_eq!(extended_to_f64(ext(false, 16383 - 1076, u64::MAX)), 0.0);
        // A subnormal that rounds up to the least normal.
        assert_eq!(
            extended_to_f64(ext(false, 16383 - 1023, u64::MAX)),
            f64::MIN_POSITIVE
        );
    }
}
