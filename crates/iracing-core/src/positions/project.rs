//! From latitude and longitude to the lap's frame: metres, `x` east, `z` north, `y` up, with the
//! origin at the lap's crossing of the start/finish line.
//!
//! The projection is equirectangular on a sphere of [`EARTH_RADIUS_M`], with no rotation and no
//! scale: the one measured against the connector's dead reckoning.

/// The radius of the sphere, in metres.
pub const EARTH_RADIUS_M: f64 = 6_378_137.0;

/// A point of the file: latitude and longitude in degrees, altitude in metres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fix {
    /// Latitude in decimal degrees.
    pub lat: f64,
    /// Longitude in decimal degrees.
    pub lon: f64,
    /// Altitude in metres.
    pub alt: f64,
}

impl Fix {
    /// Whether the fix is a position: the simulator writes latitude and longitude both 0 while
    /// the car is out of the world.
    pub fn in_world(&self) -> bool {
        !(self.lat == 0.0 && self.lon == 0.0) && self.lat.is_finite() && self.lon.is_finite()
    }
}

/// The fix at time `at` on the straight line between `a` (at `ta`) and `b` (at `tb`).
///
/// `None` when the times do not give an interval or `at` is outside it.
pub fn interpolate(a: Fix, ta: f64, b: Fix, tb: f64, at: f64) -> Option<Fix> {
    if tb.partial_cmp(&ta) != Some(std::cmp::Ordering::Greater) || at < ta || at > tb {
        return None;
    }
    let f = (at - ta) / (tb - ta);
    Some(Fix {
        lat: a.lat + (b.lat - a.lat) * f,
        lon: a.lon + (b.lon - a.lon) * f,
        alt: a.alt + (b.alt - a.alt) * f,
    })
}

/// `(x, z, y)` of `p` in the frame whose origin is `origin`: east, north and up, in metres.
pub fn project(origin: &Fix, p: &Fix) -> (f32, f32, f32) {
    let k = std::f64::consts::PI / 180.0 * EARTH_RADIUS_M;
    let x = (p.lon - origin.lon) * k * origin.lat.to_radians().cos();
    let z = (p.lat - origin.lat) * k;
    (x as f32, z as f32, (p.alt - origin.alt) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn east_north_and_up() {
        let o = Fix {
            lat: 53.0,
            lon: -2.6,
            alt: 100.0,
        };
        let (x, z, y) = project(&o, &o);
        assert_eq!((x, z, y), (0.0, 0.0, 0.0));
        // One millionth of a degree north is 0.111 m.
        let n = Fix {
            lat: 53.000_001,
            alt: 101.5,
            ..o
        };
        let (x, z, y) = project(&o, &n);
        assert_eq!(x, 0.0);
        assert!((z - 0.111_32).abs() < 1e-3, "{z}");
        assert_eq!(y, 1.5);
        // East shrinks with the cosine of the latitude.
        let e = Fix {
            lon: -2.599_999,
            ..o
        };
        let (x, z, _) = project(&o, &e);
        assert!(
            (x - 0.111_32 * 53f32.to_radians().cos()).abs() < 1e-3,
            "{x}"
        );
        assert_eq!(z, 0.0);
    }

    #[test]
    fn interpolation_and_world() {
        let a = Fix {
            lat: 1.0,
            lon: 2.0,
            alt: 3.0,
        };
        let b = Fix {
            lat: 3.0,
            lon: 4.0,
            alt: 5.0,
        };
        let m = interpolate(a, 10.0, b, 12.0, 11.0).unwrap();
        assert_eq!((m.lat, m.lon, m.alt), (2.0, 3.0, 4.0));
        assert_eq!(interpolate(a, 10.0, b, 12.0, 12.0), Some(b));
        assert!(interpolate(a, 10.0, b, 12.0, 12.1).is_none());
        assert!(interpolate(a, 10.0, b, 10.0, 10.0).is_none());
        assert!(a.in_world());
        assert!(
            !Fix {
                lat: 0.0,
                lon: 0.0,
                alt: 0.0
            }
            .in_world()
        );
        assert!(
            Fix {
                lat: 0.0,
                lon: 5.0,
                alt: 0.0
            }
            .in_world()
        );
    }
}
