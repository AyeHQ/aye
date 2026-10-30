//! Python's `repr` of the values the adapters spell as Python would:
//! floats, complex numbers, and integers written as literals.

use std::fmt::Write as _;

/// `repr(float)`.
pub fn float_repr(f: f64) -> String {
    format_float(f, true, false)
}

/// `repr(complex)`.
pub fn complex_repr(re: f64, im: f64) -> String {
    if re == 0.0 && re.is_sign_positive() {
        format!("{}j", format_float(im, false, false))
    } else {
        format!(
            "({}{}j)",
            format_float(re, false, false),
            format_float(im, false, true)
        )
    }
}

/// Python's `format_float_short` in `'r'` mode.
fn format_float(f: f64, add_dot_0: bool, always_sign: bool) -> String {
    let sign = if f.is_sign_negative() && !f.is_nan() {
        "-"
    } else if always_sign {
        "+"
    } else {
        ""
    };
    if f.is_nan() {
        return format!("{sign}nan");
    }
    if f.is_infinite() {
        return format!("{sign}inf");
    }
    // Rust's `{:e}` gives the shortest round-trip digits, like dtoa mode 0.
    let sci = format!("{:e}", f.abs());
    let (mantissa, exponent) = sci.split_once('e').expect("exponent");
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let exponent: i64 = exponent.parse().expect("integer exponent");
    let decpt = exponent + 1;
    let len = i64::try_from(digits.len()).expect("short");

    let mut out = String::from(sign);
    if decpt <= -4 || decpt > 16 {
        out.push_str(&digits[..1]);
        if digits.len() > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        let exp = decpt - 1;
        let _ = write!(out, "e{}{:02}", if exp < 0 { '-' } else { '+' }, exp.abs());
        return out;
    }
    if decpt <= 0 {
        out.push_str("0.");
        out.push_str(&"0".repeat(usize::try_from(-decpt).expect("small")));
        out.push_str(&digits);
    } else if decpt < len {
        let at = usize::try_from(decpt).expect("small");
        out.push_str(&digits[..at]);
        out.push('.');
        out.push_str(&digits[at..]);
    } else {
        out.push_str(&digits);
        out.push_str(&"0".repeat(usize::try_from(decpt - len).expect("small")));
        if add_dot_0 {
            out.push_str(".0");
        }
    }
    out
}

/// The decimal spelling of a Python integer literal (`0x_ff`, `1_000`).
pub fn int_literal_decimal(token: &str) -> Option<String> {
    let token: String = token.chars().filter(|c| *c != '_').collect();
    let lower = token.to_ascii_lowercase();
    let (radix, digits) = if let Some(rest) = lower.strip_prefix("0x") {
        (16, rest)
    } else if let Some(rest) = lower.strip_prefix("0o") {
        (8, rest)
    } else if let Some(rest) = lower.strip_prefix("0b") {
        (2, rest)
    } else {
        (10, lower.as_str())
    };
    if digits.is_empty() {
        return None;
    }
    // Little-endian limbs of 10^9.
    let mut limbs: Vec<u64> = vec![0];
    for c in digits.chars() {
        let digit = u64::from(c.to_digit(radix)?);
        let mut carry = digit;
        for limb in &mut limbs {
            let value = *limb * u64::from(radix) + carry;
            *limb = value % 1_000_000_000;
            carry = value / 1_000_000_000;
        }
        while carry > 0 {
            limbs.push(carry % 1_000_000_000);
            carry /= 1_000_000_000;
        }
    }
    let mut out = limbs.last().expect("one limb").to_string();
    for limb in limbs.iter().rev().skip(1) {
        let _ = write!(out, "{limb:09}");
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floats_print_like_python_repr() {
        // Expected values from `pytest --collect-only` on pytest 9.1.1.
        for (value, expected) in [
            (1.0, "1.0"),
            (1e16, "1e+16"),
            (1e-5, "1e-05"),
            (0.1, "0.1"),
            (1e22, "1e+22"),
            (123_456_789.0, "123456789.0"),
            (1.5e-7, "1.5e-07"),
            (f64::INFINITY, "inf"),
            (f64::NEG_INFINITY, "-inf"),
            (f64::NAN, "nan"),
            (-0.0, "-0.0"),
            (0.0, "0.0"),
            (2.5, "2.5"),
            (1e15, "1000000000000000.0"),
            (0.0001, "0.0001"),
            (1.234_567_890_123_45, "1.23456789012345"),
            (1.0 / 3.0, "0.3333333333333333"),
            (-1.5, "-1.5"),
            (1e100, "1e+100"),
            (5e-324, "5e-324"),
            (123.456, "123.456"),
        ] {
            assert_eq!(float_repr(value), expected, "{value}");
        }
    }

    #[test]
    fn complex_numbers_print_like_python_repr() {
        assert_eq!(complex_repr(0.0, 1.0), "1j");
        assert_eq!(complex_repr(1.0, 2.0), "(1+2j)");
        assert_eq!(complex_repr(-0.0, -1.0), "(-0-1j)");
        assert_eq!(complex_repr(0.0, 10.5), "10.5j");
        assert_eq!(complex_repr(1.5, -1e16), "(1.5-1e+16j)");
        assert_eq!(complex_repr(0.0, 0.0), "0j");
    }

    #[test]
    fn integer_literals_in_any_base() {
        assert_eq!(int_literal_decimal("100_000").as_deref(), Some("100000"));
        assert_eq!(int_literal_decimal("0x_ff").as_deref(), Some("255"));
        assert_eq!(int_literal_decimal("0o17").as_deref(), Some("15"));
        assert_eq!(int_literal_decimal("0b101").as_deref(), Some("5"));
        assert_eq!(
            int_literal_decimal("0xFFFFFFFFFFFFFFFFFFFF").as_deref(),
            Some("1208925819614629174706175")
        );
        assert_eq!(
            int_literal_decimal("100000000000000000000").as_deref(),
            Some("100000000000000000000")
        );
        assert_eq!(int_literal_decimal("0").as_deref(), Some("0"));
    }
}
