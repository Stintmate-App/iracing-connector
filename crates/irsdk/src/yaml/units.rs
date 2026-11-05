//! Numbers with units, as the session text writes them ("5.79 km", "80.00 kph", "200 kPa").

/// Splits a text such as `"5.79 km"` into the number and the unit (empty when there is none).
///
/// Returns `None` when the text does not start with a finite number.
pub fn number_unit(s: &str) -> Option<(f64, &str)> {
    let s = s.trim();
    let end = s.find(char::is_whitespace).unwrap_or(s.len());
    let n: f64 = s[..end].parse().ok()?;
    n.is_finite().then(|| (n, s[end..].trim()))
}

/// A length in metres from `"5.79 km"`, `"3.60 mi"` or `"120 m"`. A bare number is metres.
pub fn length_m(s: &str) -> Option<f64> {
    let (n, unit) = number_unit(s)?;
    let k = match unit.to_ascii_lowercase().as_str() {
        "km" => 1000.0,
        "mi" => 1609.344,
        "m" | "" => 1.0,
        _ => return None,
    };
    Some(n * k)
}

/// A speed in km/h from `"80.00 kph"`, `"80 km/h"` or `"50 mph"`. A bare number is km/h.
pub fn speed_kmh(s: &str) -> Option<f64> {
    let (n, unit) = number_unit(s)?;
    let k = match unit.to_ascii_lowercase().as_str() {
        "kph" | "km/h" | "" => 1.0,
        "mph" => 1.609344,
        _ => return None,
    };
    Some(n * k)
}

/// A pressure in kPa from `"200 kPa"`, `"29 psi"` or `"1.2 bar"`. A bare number is kPa.
pub fn pressure_kpa(s: &str) -> Option<f64> {
    let (n, unit) = number_unit(s)?;
    let k = match unit.to_ascii_lowercase().as_str() {
        "kpa" | "" => 1.0,
        "psi" => 6.894_757,
        "bar" => 100.0,
        _ => return None,
    };
    Some(n * k)
}

/// A temperature in degrees Celsius from `"25.0 C"` or `"77 F"`. A bare number is Celsius.
pub fn temp_c(s: &str) -> Option<f64> {
    let (n, unit) = number_unit(s)?;
    match unit.to_ascii_lowercase().as_str() {
        "c" | "" => Some(n),
        "f" => Some((n - 32.0) / 1.8),
        _ => None,
    }
}
