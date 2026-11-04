// Values and option codes follow Programmer 2.52.15 Presets.cs, DTMF.cs and UI.cs.
pub const BAND_MODULATIONS: &[&str] = &[
    "Ignore",
    "FM",
    "AM",
    "USB",
    "Enforce FM",
    "Enforce AM",
    "Enforce USB",
    "Enforce None",
];
pub const BAND_BANDWIDTHS: &[&str] = &[
    "Ignore",
    "Wide",
    "Narrow",
    "Enforce Wide",
    "Enforce Narrow",
    "FM Tuner",
];
pub const SCAN_MODULATIONS: &[&str] = &["FM", "AM", "USB"];

pub fn option_label(options: &[&str], raw: u8) -> String {
    options
        .get(raw as usize)
        .map_or_else(|| format!("Raw ({raw})"), |value| value.to_string())
}

pub fn decimal_value(raw: u32, places: u32) -> String {
    let scale = 10u32.pow(places);
    if places == 0 {
        raw.to_string()
    } else {
        format!(
            "{}.{:0width$}",
            raw / scale,
            raw % scale,
            width = places as usize
        )
    }
}

pub fn parse_decimal(input: &str, places: u32, min: u32, max: u32) -> Result<u32, String> {
    let input = input.trim();
    let (whole, fraction) = input.split_once('.').unwrap_or((input, ""));
    let fraction = fraction.trim_end_matches('0');
    if whole.is_empty()
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || !fraction.bytes().all(|b| b.is_ascii_digit())
        || fraction.len() > places as usize
    {
        return Err(format!("Use a number with at most {places} decimal places"));
    }
    let invalid = || {
        format!(
            "Allowed: {}-{}",
            decimal_value(min, places),
            decimal_value(max, places)
        )
    };
    let whole = whole.parse::<u32>().map_err(|_| invalid())?;
    let fraction_value = if fraction.is_empty() {
        0
    } else {
        fraction.parse::<u32>().map_err(|_| invalid())? * 10u32.pow(places - fraction.len() as u32)
    };
    let raw = whole
        .checked_mul(10u32.pow(places))
        .and_then(|value| value.checked_add(fraction_value))
        .ok_or_else(invalid)?;
    if (min..=max).contains(&raw) {
        Ok(raw)
    } else {
        Err(invalid())
    }
}

pub fn dtmf_text(digits: &[u8]) -> String {
    digits
        .iter()
        .map(|digit| match digit {
            0..=9 => char::from(b'0' + digit).to_string(),
            10..=13 => char::from(b'A' + digit - 10).to_string(),
            14 => "*".to_string(),
            15 => "#".to_string(),
            _ => format!("[raw {digit}]"),
        })
        .collect()
}

pub fn parse_dtmf(input: &str) -> Result<Vec<u8>, String> {
    if input.chars().count() > 9 {
        return Err("DTMF accepts at most 9 digits".to_string());
    }
    input
        .chars()
        .map(|ch| match ch.to_ascii_uppercase() {
            '0'..='9' => Ok(ch as u8 - b'0'),
            'A'..='D' => Ok(ch.to_ascii_uppercase() as u8 - b'A' + 10),
            '*' => Ok(14),
            '#' => Ok(15),
            _ => Err("DTMF accepts only 0-9, A-D, * and #".to_string()),
        })
        .collect()
}

pub fn validate_label(input: &str, max: usize) -> Result<(), String> {
    if input.len() <= max && input.bytes().all(|b| b.is_ascii_graphic() || b == b' ') {
        Ok(())
    } else {
        Err(format!(
            "Label accepts at most {max} printable ASCII characters"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimals_are_exact_and_reject_loss_or_invalid_numbers() {
        assert_eq!(parse_decimal("12.5000", 2, 1, 50000), Ok(1250));
        assert_eq!(decimal_value(1900, 2), "19.00");
        for input in ["NaN", "inf", "-1", "", "0.001", "4294967295"] {
            assert!(parse_decimal(input, 2, 1, 50000).is_err());
        }
    }

    #[test]
    fn dtmf_symbols_roundtrip_every_nibble_without_hex_aliases() {
        for digit in 0..16 {
            assert_eq!(parse_dtmf(&dtmf_text(&[digit])), Ok(vec![digit]));
        }
        assert_eq!(parse_dtmf("aD*#"), Ok(vec![10, 13, 14, 15]));
        for input in ["E", "F", "1234567890", "1 2", "é"] {
            assert!(parse_dtmf(input).is_err());
        }
    }

    #[test]
    fn unknown_enums_and_digits_are_not_misrepresented() {
        assert_eq!(option_label(BAND_MODULATIONS, 7), "Enforce None");
        assert_eq!(option_label(BAND_BANDWIDTHS, 5), "FM Tuner");
        assert_eq!(option_label(SCAN_MODULATIONS, 3), "Raw (3)");
        assert_eq!(dtmf_text(&[255]), "[raw 255]");
        assert!(validate_label("12345678", 7).is_err());
    }
}
