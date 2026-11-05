//! A line scanner over the decoded session text, with no YAML parser: it recovers the key facts
//! when a section did not parse.

/// The facts the scanner looks for; `None` / empty when a line was not found.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Fallback {
    /// `TrackID`.
    pub track_id: Option<i64>,
    /// `TrackName`.
    pub track_name: Option<String>,
    /// `TrackDisplayName`.
    pub track_display_name: Option<String>,
    /// `TrackConfigName`.
    pub track_config_name: Option<String>,
    /// `TrackLength`, with its unit.
    pub track_length: Option<String>,
    /// `SubSessionID`.
    pub sub_session_id: Option<i64>,
    /// `SessionID`.
    pub session_id: Option<i64>,
    /// `CurrentSessionNum`.
    pub current_session_num: Option<i64>,
    /// `DriverCarIdx`.
    pub driver_car_idx: Option<i64>,
    /// `DriverCarFuelMaxLtr`.
    pub fuel_max_ltr: Option<f64>,
    /// `DriverCarMaxFuelPct`.
    pub max_fuel_pct: Option<f64>,
    /// The player's `CarPath`.
    pub player_car_path: Option<String>,
    /// The player's `CarScreenName`.
    pub player_car_screen_name: Option<String>,
    /// The player's `CarClassID`.
    pub player_car_class_id: Option<i64>,
    /// The player's `CarClassShortName`.
    pub player_car_class_short_name: Option<String>,
    /// The player's `CarID`.
    pub player_car_id: Option<i64>,
    /// `(SessionNum, SessionType)` of each session.
    pub session_types: Vec<(i64, String)>,
    /// Each `SectorStartPct`, in order.
    pub sector_start_pcts: Vec<f64>,
    /// The `UpdateCount` at the top of `CarSetup`.
    pub car_setup_update_count: Option<i64>,
}

/// The fields of one `Drivers` item the scanner keeps.
#[derive(Default)]
struct Item {
    car_idx: Option<i64>,
    path: Option<String>,
    screen_name: Option<String>,
    class_id: Option<i64>,
    class_short: Option<String>,
    car_id: Option<i64>,
}

/// Splits a line into its indentation, its key and its value (quotes removed). A leading `- ` is
/// part of the indentation. `None` for a line without a key.
fn split_line(line: &str) -> Option<(usize, bool, &str, &str)> {
    let body = line.trim_start_matches(' ');
    let mut indent = line.len() - body.len();
    let item = body.starts_with("- ");
    let body = if item {
        let r = body[2..].trim_start_matches(' ');
        indent += body.len() - r.len();
        r
    } else {
        body
    };
    let (key, value) = match body.split_once(": ") {
        Some(kv) => kv,
        None => (body.trim_end().strip_suffix(':')?, ""),
    };
    if key.is_empty() || key.contains([' ', '\t']) {
        return None;
    }
    let value = value.trim();
    let value = value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .unwrap_or(value);
    Some((indent, item, key, value))
}

/// Scans the decoded session text line by line.
pub fn extract(text: &str) -> Fallback {
    let mut f = Fallback::default();
    let mut section = "";
    let mut items: Vec<Item> = Vec::new();
    let mut session_num: Option<i64> = None;
    let mut dash_indent = usize::MAX;
    let mut update_seen = false;
    for line in text.lines() {
        if !line.starts_with([' ', '\t', '-', '.', '#'])
            && let Some(name) = line.trim_end().strip_suffix(':')
            && !name.is_empty()
            && !name.contains([' ', ':'])
        {
            section = name;
            session_num = None;
            continue;
        }
        let Some((indent, is_item, key, value)) = split_line(line) else {
            continue;
        };
        let int = || value.parse::<i64>().ok();
        let num = || value.parse::<f64>().ok();
        let first = |slot: &mut Option<String>| {
            if slot.is_none() && !value.is_empty() {
                *slot = Some(value.to_owned());
            }
        };
        match (section, key) {
            ("WeekendInfo", "TrackID") => f.track_id = f.track_id.or_else(int),
            ("WeekendInfo", "TrackName") => first(&mut f.track_name),
            ("WeekendInfo", "TrackDisplayName") => first(&mut f.track_display_name),
            ("WeekendInfo", "TrackConfigName") => first(&mut f.track_config_name),
            ("WeekendInfo", "TrackLength") => first(&mut f.track_length),
            ("WeekendInfo", "SubSessionID") => f.sub_session_id = f.sub_session_id.or_else(int),
            ("WeekendInfo", "SessionID") => f.session_id = f.session_id.or_else(int),
            ("SessionInfo", "CurrentSessionNum") => {
                f.current_session_num = f.current_session_num.or_else(int);
            }
            ("SessionInfo", "SessionNum") => session_num = int(),
            ("SessionInfo", "SessionType") => {
                if let Some(n) = session_num.take() {
                    f.session_types.push((n, value.to_owned()));
                }
            }
            ("SplitTimeInfo", "SectorStartPct") => f.sector_start_pcts.extend(num()),
            ("CarSetup", "UpdateCount") if !update_seen => {
                update_seen = true;
                f.car_setup_update_count = int();
            }
            ("DriverInfo", "DriverCarIdx") => f.driver_car_idx = f.driver_car_idx.or_else(int),
            ("DriverInfo", "DriverCarFuelMaxLtr") => f.fuel_max_ltr = f.fuel_max_ltr.or_else(num),
            ("DriverInfo", "DriverCarMaxFuelPct") => f.max_fuel_pct = f.max_fuel_pct.or_else(num),
            ("DriverInfo", "CarIdx") if is_item => {
                dash_indent = indent;
                items.push(Item {
                    car_idx: int(),
                    ..Item::default()
                });
            }
            ("DriverInfo", _) if indent >= dash_indent => {
                if let Some(item) = items.last_mut() {
                    let s = || Some(value.to_owned()).filter(|v| !v.is_empty());
                    match key {
                        "CarPath" => item.path = s(),
                        "CarScreenName" => item.screen_name = s(),
                        "CarClassID" => item.class_id = int(),
                        "CarClassShortName" => item.class_short = s(),
                        "CarID" => item.car_id = int(),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    if let Some(idx) = f.driver_car_idx
        && let Some(p) = items.into_iter().find(|i| i.car_idx == Some(idx))
    {
        f.player_car_path = p.path;
        f.player_car_screen_name = p.screen_name;
        f.player_car_class_id = p.class_id;
        f.player_car_class_short_name = p.class_short;
        f.player_car_id = p.car_id;
    }
    f
}
