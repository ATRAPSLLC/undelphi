//! Floating-point values as the knowledge bases print them: Delphi's
//! `FloatToStr`, which converts every float to the 80-bit `Extended` and
//! shows 15 significant digits.
//!
//! The digits come from the exact decimal expansion of the value (a
//! mantissa times a power of two), so 80-bit values beyond `f64`'s range
//! (`MaxExtended`, `1.1E4932`) and `Extended` images of `f64` limits print
//! as Delphi prints them.

/// Formats an 80-bit `Extended` image times `10^scale` (`-4` for a
/// `Currency` constant, which holds the value times 10 000).
#[must_use]
pub fn extended(bytes: [u8; 10], scale: i32) -> String {
    let [m0, m1, m2, m3, m4, m5, m6, m7, e0, e1] = bytes;
    let mantissa = u64::from_le_bytes([m0, m1, m2, m3, m4, m5, m6, m7]);
    let se = u16::from_le_bytes([e0, e1]);
    let negative = se & 0x8000 != 0;
    let exponent = i32::from(se & 0x7FFF);
    if exponent == 0x7FFF {
        return special(negative, mantissa << 1 == 0);
    }
    // A denormal's exponent is that of the smallest normal.
    let e = exponent.max(1).saturating_sub(16383 + 63);
    general(negative, mantissa, e, scale)
}

/// Formats an `f64` (a `Double`, or a `Single` widened to one).
#[must_use]
pub fn double(v: f64) -> String {
    let bits = v.to_bits();
    let negative = bits >> 63 != 0;
    let exponent = i32::try_from((bits >> 52) & 0x7FF).unwrap_or(0);
    let fraction = bits & ((1 << 52) - 1);
    if exponent == 0x7FF {
        return special(negative, fraction == 0);
    }
    let (mantissa, e) = if exponent == 0 {
        (fraction, 1 - 1023 - 52)
    } else {
        (fraction | 1 << 52, exponent.saturating_sub(1023 + 52))
    };
    general(negative, mantissa, e, 0)
}

/// Formats a `Currency` value given as its 64-bit image (the value times
/// 10 000).
#[must_use]
pub fn currency(v: i64) -> String {
    let digits = v.unsigned_abs().to_string();
    let exp10 = i32::try_from(digits.len())
        .unwrap_or(0)
        .saturating_sub(1 + 4);
    layout(v < 0, &digits, exp10)
}

/// `INF`, `-INF` or `NAN`.
fn special(negative: bool, infinite: bool) -> String {
    match (infinite, negative) {
        (false, _) => "NAN".to_string(),
        (true, false) => "INF".to_string(),
        (true, true) => "-INF".to_string(),
    }
}

/// Formats `mantissa * 2^e * 10^scale`.
fn general(negative: bool, mantissa: u64, e: i32, scale: i32) -> String {
    if mantissa == 0 {
        return "0".to_string();
    }
    let mut n = Decimal::from(mantissa);
    let exp10 = if e >= 0 {
        // Multiply by 2^e, 29 bits at a time.
        let mut left = e;
        while left > 0 {
            let step = left.min(29);
            n.mul_small(1u32 << step);
            left = left.saturating_sub(step);
        }
        n.digit_count().saturating_sub(1)
    } else {
        // mantissa / 2^k = mantissa * 5^k / 10^k
        let k = e.saturating_neg();
        let mut left = k;
        while left > 0 {
            let step = left.min(13);
            n.mul_small(5u32.pow(u32::try_from(step).unwrap_or(0)));
            left = left.saturating_sub(step);
        }
        n.digit_count().saturating_sub(1).saturating_sub(k)
    };
    layout(negative, &n.digits(), exp10.saturating_add(scale))
}

/// Rounds `digits` (a value `d.ddd * 10^exp10`) to 15 significant digits,
/// half up, and lays it out as `FloatToStr` does: scientific
/// (`1.6387064E-5`) below `1E-4` or from `1E15`, fixed otherwise.
fn layout(negative: bool, digits: &str, exp10: i32) -> String {
    let mut d: Vec<u8> = digits.bytes().map(|b| b.saturating_sub(b'0')).collect();
    let mut exp10 = exp10;
    if d.len() > 15 {
        let round_up = d.get(15).is_some_and(|&x| x >= 5);
        d.truncate(15);
        if round_up {
            let mut i = d.len();
            loop {
                let Some(next) = i.checked_sub(1) else {
                    // Every digit carried: 999.. became 1000..
                    d.insert(0, 1);
                    d.truncate(15);
                    exp10 = exp10.saturating_add(1);
                    break;
                };
                i = next;
                match d.get_mut(i) {
                    Some(x) if *x == 9 => *x = 0,
                    Some(x) => {
                        *x = x.saturating_add(1);
                        break;
                    }
                    None => break,
                }
            }
        }
    }
    while d.len() > 1 && d.last() == Some(&0) {
        d.pop();
    }
    let digits: String = d
        .iter()
        .map(|&x| char::from(x.saturating_add(b'0')))
        .collect();
    let sign = if negative { "-" } else { "" };
    let len = i32::try_from(digits.len()).unwrap_or(15);
    if !(-4..15).contains(&exp10) {
        let (first, rest) = digits.split_at_checked(1).unwrap_or((&digits, ""));
        let frac = if rest.is_empty() {
            String::new()
        } else {
            format!(".{rest}")
        };
        return format!("{sign}{first}{frac}E{exp10}");
    }
    if exp10 < 0 {
        let zeros =
            "0".repeat(usize::try_from(exp10.saturating_neg().saturating_sub(1)).unwrap_or(0));
        format!("{sign}0.{zeros}{digits}")
    } else if exp10.saturating_add(1) >= len {
        let zeros =
            "0".repeat(usize::try_from(exp10.saturating_add(1).saturating_sub(len)).unwrap_or(0));
        format!("{sign}{digits}{zeros}")
    } else {
        let at = usize::try_from(exp10.saturating_add(1)).unwrap_or(1);
        let (int_part, frac) = digits.split_at_checked(at).unwrap_or((&digits, ""));
        format!("{sign}{int_part}.{frac}")
    }
}

/// A non-negative integer in base 10^9 limbs, least significant first.
struct Decimal(Vec<u32>);

/// The limb base.
const BASE: u64 = 1_000_000_000;

impl From<u64> for Decimal {
    fn from(mut v: u64) -> Self {
        let mut limbs = Vec::new();
        while v > 0 {
            limbs.push(u32::try_from(v % BASE).unwrap_or(0));
            v /= BASE;
        }
        Self(limbs)
    }
}

impl Decimal {
    /// Multiplies by `f` (below 2^31).
    fn mul_small(&mut self, f: u32) {
        let mut carry = 0u64;
        for limb in &mut self.0 {
            let v = u64::from(*limb)
                .saturating_mul(u64::from(f))
                .saturating_add(carry);
            *limb = u32::try_from(v % BASE).unwrap_or(0);
            carry = v / BASE;
        }
        while carry > 0 {
            self.0.push(u32::try_from(carry % BASE).unwrap_or(0));
            carry /= BASE;
        }
    }

    /// The decimal digits, most significant first.
    fn digits(&self) -> String {
        let mut limbs = self.0.iter().rev();
        let mut out = limbs.next().map(u32::to_string).unwrap_or_default();
        for limb in limbs {
            out.push_str(&format!("{limb:09}"));
        }
        out
    }

    /// The number of decimal digits.
    fn digit_count(&self) -> i32 {
        i32::try_from(self.digits().len()).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delphi_float_to_str() {
        assert_eq!(double(0.0), "0");
        assert_eq!(double(365.25), "365.25");
        assert_eq!(double(2_400_000.5), "2400000.5");
        assert_eq!(double(1.638_706_4e-5), "1.6387064E-5");
        assert_eq!(double(0.0001), "0.0001");
        assert_eq!(double(1e15), "1E15");
        assert_eq!(
            double(f64::MIN_POSITIVE * f64::EPSILON),
            "4.94065645841247E-324"
        );
        assert_eq!(currency(i64::MAX), "922337203685478");
        assert_eq!(currency(-10_000), "-1");
        // The largest finite Extended, beyond f64's range.
        let max = [0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFE, 0x7F];
        assert_eq!(extended(max, 0), "1.18973149535723E4932");
        // Currency constants are Extended images of the value * 10 000.
        let ten_thousand = [0, 0, 0, 0, 0, 0, 0x40, 0x9C, 0x0C, 0x40];
        assert_eq!(extended(ten_thousand, -4), "1");
    }
}
