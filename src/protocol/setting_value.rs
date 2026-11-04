use super::{SETTINGS_METADATA, SettingType};

// Programmer 2.52.15 UI.cs supplies decimal places and raw bounds to Setting;
// Setting.cs divides/multiplies the EEPROM integer by 10^decimal_places.
fn decimal_places(index: usize) -> usize {
    match index {
        4 | 12 => 2,
        5 | 6 | 13 | 39 | 44 => 1,
        _ => 0,
    }
}

pub fn edit_value(index: usize, raw: u32) -> String {
    let places = decimal_places(index);
    let scale = 10u32.pow(places as u32);
    if places == 0 {
        raw.to_string()
    } else {
        format!("{}.{:0places$}", raw / scale, raw % scale)
    }
}

pub fn display_value(index: usize, raw: u32) -> String {
    let Some(meta) = SETTINGS_METADATA.get(index) else {
        return "N/A".to_string();
    };
    match meta.setting_type {
        SettingType::Boolean => match raw {
            0 => "Off".to_string(),
            1 => "On".to_string(),
            _ => format!("Unknown ({raw})"),
        },
        SettingType::Enum(options) => options
            .get(raw as usize)
            .map(|value| value.to_string())
            .unwrap_or_else(|| format!("Unknown ({raw})")),
        SettingType::Numeric { unit, .. } => {
            let value = edit_value(index, raw);
            if unit.is_empty() {
                value
            } else {
                format!("{value} {unit}")
            }
        }
    }
}

pub fn allowed_values(index: usize) -> String {
    let Some(meta) = SETTINGS_METADATA.get(index) else {
        return "Unknown setting".to_string();
    };
    match meta.setting_type {
        SettingType::Boolean => "Off or On".to_string(),
        SettingType::Enum(options) => options.join(" / "),
        SettingType::Numeric { min, max, unit } => {
            let range = format!(
                "{}-{}",
                edit_value(index, min as u32),
                edit_value(index, max as u32)
            );
            if unit.is_empty() {
                range
            } else {
                format!("{range} {unit}")
            }
        }
    }
}

pub fn validate_raw_value(index: usize, raw: u32) -> Result<(), String> {
    let meta = SETTINGS_METADATA
        .get(index)
        .ok_or_else(|| "Unknown setting".to_string())?;
    let valid = match meta.setting_type {
        SettingType::Boolean => raw <= 1,
        SettingType::Enum(options) => (raw as usize) < options.len(),
        SettingType::Numeric { min, max, .. } => {
            (i64::from(min)..=i64::from(max)).contains(&i64::from(raw))
        }
    };
    if valid {
        Ok(())
    } else {
        Err(format!("{} accepts {}", meta.name, allowed_values(index)))
    }
}

pub fn parse_numeric_value(index: usize, input: &str) -> Result<u32, String> {
    let meta = SETTINGS_METADATA
        .get(index)
        .ok_or_else(|| "Unknown setting".to_string())?;
    if !matches!(meta.setting_type, SettingType::Numeric { .. }) {
        return Err("Setting is not numeric".to_string());
    }
    let places = decimal_places(index);
    let scale = 10u32.pow(places as u32);
    let input = input.trim();
    let (whole, fraction) = input.split_once('.').unwrap_or((input, ""));
    let fraction = fraction.trim_end_matches('0');
    if whole.is_empty()
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
        || fraction.len() > places
    {
        return Err(format!(
            "{} requires a number with at most {places} decimal places",
            meta.name
        ));
    }
    let invalid = || format!("{} accepts {}", meta.name, allowed_values(index));
    let whole = whole.parse::<u32>().map_err(|_| invalid())?;
    let fraction_value = if fraction.is_empty() {
        0
    } else {
        fraction.parse::<u32>().map_err(|_| invalid())?
            * 10u32.pow((places - fraction.len()) as u32)
    };
    let raw = whole
        .checked_mul(scale)
        .and_then(|value| value.checked_add(fraction_value))
        .ok_or_else(invalid)?;
    validate_raw_value(index, raw)?;
    Ok(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn physical_units_match_programmer_storage() {
        for (index, raw, expected) in [
            (4, 500, "5.00 kHz"),
            (5, 2800, "280.0 MHz"),
            (6, 2800, "280.0 MHz"),
            (12, 1900, "19.00 MHz"),
            (13, 200, "20.0 s"),
            (39, 20, "2.0 s"),
            (44, 10, "1.0 s"),
            (55, 5, "5 s"),
            (19, 5, "5 s"),
        ] {
            assert_eq!(display_value(index, raw), expected);
            assert_eq!(parse_numeric_value(index, &edit_value(index, raw)), Ok(raw));
        }
    }

    #[test]
    fn numeric_boundaries_and_midpoints_have_exact_edit_roundtrips() {
        for (index, meta) in SETTINGS_METADATA.iter().enumerate() {
            if let SettingType::Numeric { min, max, .. } = meta.setting_type {
                for raw in [min as u32, max as u32, (min + (max - min) / 2) as u32] {
                    assert_eq!(parse_numeric_value(index, &edit_value(index, raw)), Ok(raw));
                }
            }
        }
    }

    #[test]
    fn rejects_invalid_or_unrepresentable_numbers_without_rounding() {
        for input in [
            "",
            "bad",
            "NaN",
            "inf",
            "-1",
            "1.2.3",
            "0.001",
            "500.01",
            "4294967295",
            "4294967296",
        ] {
            assert!(parse_numeric_value(4, input).is_err(), "{input}");
        }
        assert!(parse_numeric_value(0, "256").is_err());
        assert_eq!(parse_numeric_value(4, "12.5000"), Ok(1250));
        assert_eq!(parse_numeric_value(13, "2.0"), Ok(20));
    }

    #[test]
    fn ranges_use_the_same_units_as_values_and_inputs() {
        assert_eq!(allowed_values(4), "0.01-500.00 kHz");
        assert_eq!(allowed_values(12), "0.01-600.00 MHz");
        assert_eq!(allowed_values(13), "0.0-20.0 s");
        assert_eq!(allowed_values(39), "0.5-5.0 s");
    }

    #[test]
    fn unknown_choice_codes_are_visible_not_reinterpreted() {
        assert_eq!(display_value(1, 2), "Unknown (2)");
        assert!(validate_raw_value(1, 2).is_err());
        assert!(validate_raw_value(3, 2).is_err());
        assert_eq!(display_value(usize::MAX, 0), "N/A");
    }

    #[test]
    fn filter_and_pin_labels_preserve_programmer_option_codes() {
        assert_eq!(display_value(48, 1), "Unlock");
        assert_eq!(display_value(50, 0), "All");
        assert_eq!(display_value(50, 7), "None");
        assert_eq!(display_value(50, 8), "FSK Mode");
        assert!(validate_raw_value(50, 9).is_err());
    }
}
