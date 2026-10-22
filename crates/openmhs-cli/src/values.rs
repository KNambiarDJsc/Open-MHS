//! Parsing values typed on the command line.
//!
//! `5V`, `5 V` and `--value 5 --unit V` are quantities; `true`/`false` are booleans; a
//! bare number stays a bare number, so the gateway can reject it where a quantity is
//! required (units are never guessed); anything else is text.

use serde_json::{Value, json};

/// Split `"12.5mm/s"` into `(12.5, "mm/s")`. `None` if there is no leading number.
fn split_number(raw: &str) -> Option<(Value, &str)> {
    let raw = raw.trim();
    let end = raw
        .char_indices()
        .find(|&(i, c)| !(c.is_ascii_digit() || c == '.' || ((c == '-' || c == '+') && i == 0)))
        .map_or(raw.len(), |(i, _)| i);
    let (num, rest) = raw.split_at(end);
    let number = if let Ok(i) = num.parse::<i64>() {
        json!(i)
    } else {
        json!(num.parse::<f64>().ok()?)
    };
    Some((number, rest.trim()))
}

pub fn parse_value(raw: &str, unit: Option<&str>) -> Result<Value, String> {
    if let Some(unit) = unit {
        let (number, rest) = split_number(raw)
            .filter(|(_, rest)| rest.is_empty())
            .ok_or_else(|| format!("'{raw}' is not a number (needed with --unit {unit})"))?;
        debug_assert!(rest.is_empty());
        return Ok(json!({ "value": number, "unit": unit }));
    }
    match raw.trim() {
        "true" => return Ok(json!(true)),
        "false" => return Ok(json!(false)),
        _ => {}
    }
    match split_number(raw) {
        Some((number, "")) => Ok(number),
        Some((number, unit)) => Ok(json!({ "value": number, "unit": unit })),
        None => Ok(json!(raw)),
    }
}

/// Parse `name=value` pairs given with `--param`.
pub fn parse_param(raw: &str) -> Result<(String, Value), String> {
    let (name, value) = raw
        .split_once('=')
        .ok_or_else(|| format!("--param '{raw}' must look like name=value, e.g. position=120mm"))?;
    Ok((name.trim().to_owned(), parse_value(value, None)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_quantities_booleans_numbers_and_text() {
        assert_eq!(
            parse_value("5V", None).unwrap(),
            json!({"value": 5, "unit": "V"})
        );
        assert_eq!(
            parse_value("5 V", None).unwrap(),
            json!({"value": 5, "unit": "V"})
        );
        assert_eq!(
            parse_value("12.5mm/s", None).unwrap(),
            json!({"value": 12.5, "unit": "mm/s"})
        );
        assert_eq!(
            parse_value("-3.5", Some("Cel")).unwrap(),
            json!({"value": -3.5, "unit": "Cel"})
        );
        assert_eq!(parse_value("true", None).unwrap(), json!(true));
        assert_eq!(parse_value("5", None).unwrap(), json!(5));
        assert_eq!(parse_value("idle", None).unwrap(), json!("idle"));
        assert!(parse_value("five", Some("V")).is_err());
        assert!(parse_value("5mV", Some("V")).is_err());
    }

    #[test]
    fn parses_params() {
        assert_eq!(
            parse_param("position=120mm").unwrap(),
            ("position".into(), json!({"value": 120, "unit": "mm"}))
        );
        assert!(parse_param("position").is_err());
    }
}
