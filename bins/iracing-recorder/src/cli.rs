//! The recorder's command line: the runtime's options plus `--sdk-map` and `--sdk-event`.

use stintmate_connector_link::boot::RecorderArgs;

/// Names of a test mapping and its data-valid event, opened instead of the simulator's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SdkNames {
    /// The memory mapping's name.
    pub map: String,
    /// The event's name.
    pub event: String,
}

/// What the command line asked for.
#[derive(Debug)]
pub struct Options {
    /// The runtime's options.
    pub args: RecorderArgs,
    /// The test mapping to read instead of the simulator's, when given.
    pub sdk: Option<SdkNames>,
}

/// Parses the arguments (without the program name). An unknown option is an error.
///
/// `--sdk-map <name>` and `--sdk-event <name>` come together or not at all; they make the
/// recorder open those names and treat the simulator as running, for tests and for runs
/// against a replay.
pub fn parse(argv: Vec<String>) -> Result<Options, String> {
    let (mut map, mut event, mut rest) = (None, None, Vec::new());
    let mut it = argv.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--sdk-map" => map = Some(it.next().ok_or("--sdk-map needs a name")?),
            "--sdk-event" => event = Some(it.next().ok_or("--sdk-event needs a name")?),
            _ => rest.push(a),
        }
    }
    let args = RecorderArgs::parse(rest)?;
    if let Some(unknown) = args.rest.first() {
        return Err(format!("unknown option {unknown}"));
    }
    let sdk = match (map, event) {
        (Some(map), Some(event)) => Some(SdkNames { map, event }),
        (None, None) => None,
        _ => return Err("--sdk-map and --sdk-event go together".to_owned()),
    };
    Ok(Options { args, sdk })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn the_sdk_names_come_in_a_pair() {
        let o = parse(v(&["--sdk-map", "m", "--sdk-event", "e", "--hz", "30"])).unwrap();
        assert_eq!(
            o.sdk,
            Some(SdkNames {
                map: "m".into(),
                event: "e".into()
            })
        );
        assert_eq!(o.args.hz, Some(30.0));
        assert!(parse(v(&["--sdk-map", "m"])).is_err());
        assert!(parse(v(&["--sdk-event"])).is_err());
        assert!(parse(v(&[])).unwrap().sdk.is_none());
    }

    #[test]
    fn an_unknown_option_is_refused() {
        assert!(parse(v(&["--nope"])).unwrap_err().contains("--nope"));
    }
}
