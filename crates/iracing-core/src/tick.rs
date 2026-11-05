//! One row of telemetry as plain values, filled without allocation.

use irsdk::row::Row;
use irsdk::vars::VarValue;

use crate::vars::{CARS, Vars, variables};

macro_rules! define_tick {
    (
        req: [$($rf:ident : $rt:ty = $rn:literal,)*],
        opt: [$($of:ident : $ot:ty = $on:literal,)*],
        req_arr: [$($raf:ident : $rat:ty = $ran:literal,)*],
        opt_arr: [$($oaf:ident : $oat:ty = $oan:literal,)*],
    ) => {
        /// One row of telemetry, in the SDK's units.
        ///
        /// Optional variables are `None` when the layout lacks them, never zero. The per-car
        /// arrays have one element per car slot. Allocate it once (`Tick::new`) and refill it
        /// with [`Tick::fill`]: filling allocates nothing.
        #[derive(Clone, Debug, PartialEq)]
        pub struct Tick {
            $(#[doc = concat!("The `", $rn, "` variable.")] pub $rf: $rt,)*
            $(#[doc = concat!("The `", $on, "` variable, when the layout has it.")] pub $of: Option<$ot>,)*
            $(#[doc = concat!("The `", $ran, "` variable of every car slot.")] pub $raf: [$rat; CARS],)*
            $(#[doc = concat!("The `", $oan, "` variable of every car slot, when the layout has it.")] pub $oaf: Option<[$oat; CARS]>,)*
            /// The throttle, 0 to 1: `ThrottleRaw`, else `Throttle`.
            pub throttle: f32,
            /// The brake, 0 to 1: `BrakeRaw`, else `Brake`.
            pub brake: f32,
            /// The clutch, 0 to 1: `ClutchRaw`, else `Clutch`, when the layout has one.
            /// Which end of the travel is "pressed" is not observed yet.
            pub clutch: Option<f32>,
            /// Carcass temperature in degrees Celsius per wheel (left front, right front, left
            /// rear, right rear) and position (left, middle, right), when the layout has all.
            /// Whether these are carcass or surface readings is not observed yet.
            pub tyre_temp: Option<[[f32; 3]; 4]>,
            /// Tyre wear per wheel and position, in the order of `tyre_temp`.
            pub tyre_wear: Option<[[f32; 3]; 4]>,
            /// The tick count of the row.
            pub tick: i32,
            /// Whether the header's status bit said the simulator was connected. Set by the
            /// caller; [`Tick::fill`] does not touch it.
            pub connected: bool,
            /// Whether the row is a new one (not a repeat of the previous). Set by the caller;
            /// [`Tick::fill`] does not touch it.
            pub fresh: bool,
        }

        impl Tick {
            fn blank() -> Tick {
                Tick {
                    $($rf: <$rt as VarValue>::ZERO,)*
                    $($of: None,)*
                    $($raf: [<$rat as VarValue>::ZERO; CARS],)*
                    $($oaf: None,)*
                    throttle: 0.0,
                    brake: 0.0,
                    clutch: None,
                    tyre_temp: None,
                    tyre_wear: None,
                    tick: 0,
                    connected: false,
                    fresh: false,
                }
            }

            /// Reads every variable of `row` through `vars`. The row must belong to the table
            /// `vars` was resolved from. Allocates nothing.
            pub fn fill(&mut self, vars: &Vars, row: &Row) {
                $(self.$rf = vars.$rf.read(row);)*
                $(self.$of = vars.$of.as_ref().map(|v| v.read(row));)*
                $(fill_array(&mut self.$raf, &vars.$raf, row);)*
                $(
                    match vars.$oaf.as_ref() {
                        Some(v) => fill_array(self.$oaf.get_or_insert([<$oat as VarValue>::ZERO; CARS]), v, row),
                        None => self.$oaf = None,
                    }
                )*
                self.throttle = vars.throttle.read(row);
                self.brake = vars.brake.read(row);
                self.clutch = vars.clutch.as_ref().map(|v| v.read(row));
                fill_tyres(&mut self.tyre_temp, vars.tyre_temp.as_ref(), row);
                fill_tyres(&mut self.tyre_wear, vars.tyre_wear.as_ref(), row);
                self.tick = row.tick();
            }
        }
    };
}
variables!(define_tick);

fn fill_array<T: VarValue>(out: &mut [T; CARS], var: &irsdk::vars::ArrayVar<T>, row: &Row) {
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = var.get(row, i).unwrap_or(T::ZERO);
    }
}

fn fill_tyres(
    out: &mut Option<[[f32; 3]; 4]>,
    vars: Option<&[[irsdk::vars::Var<f32>; 3]; 4]>,
    row: &Row,
) {
    match vars {
        Some(vars) => {
            let values = out.get_or_insert([[0.0; 3]; 4]);
            for (wheel, handles) in values.iter_mut().zip(vars) {
                for (slot, handle) in wheel.iter_mut().zip(handles) {
                    *slot = handle.read(row);
                }
            }
        }
        None => *out = None,
    }
}

impl Tick {
    /// A zeroed tick on the heap (about 5 KB), to be refilled for every row.
    pub fn new() -> Box<Tick> {
        Box::new(Tick::blank())
    }
}

#[cfg(all(test, feature = "testing"))]
mod tests {
    use super::*;
    use crate::testing::{full_layout, table_of};
    use irsdk::testing::RowBuilder;

    fn fixture() -> (Vars, irsdk::testing::Layout) {
        let layout = full_layout();
        (Vars::resolve(&table_of(&layout)).expect("vars"), layout)
    }

    #[test]
    fn fill_reads_scalars_arrays_and_options() {
        let (vars, layout) = fixture();
        let mut b = RowBuilder::new(&layout);
        b.set_f64("SessionTime", 12.5)
            .set_i32("SessionNum", 2)
            .set_u32("SessionFlags", 0x8)
            .set_bool("IsOnTrack", true)
            .set_i32("Lap", 4)
            .set_f32("Speed", 50.0)
            .set_f32("ThrottleRaw", 0.75)
            .set_f32("ClutchRaw", 1.0)
            .set_i32("PlayerCarMyIncidentCount", 3)
            .set_f32_at("CarIdxLapDistPct", 5, 0.25)
            .set_i32_at("CarIdxPosition", 63, 7)
            .set_f32("LFtempCM", 81.5)
            .set_f32("RRwearR", 0.9);
        let mut row = Row::from_slice(b.bytes());
        row.set_tick(77);
        let mut t = Tick::new();
        t.fill(&vars, &row);
        assert_eq!(t.session_time, 12.5);
        assert_eq!(t.session_num, 2);
        assert_eq!(t.session_flags, 0x8);
        assert!(t.is_on_track && !t.is_in_garage);
        assert_eq!((t.lap, t.speed, t.throttle), (4, 50.0, 0.75));
        assert_eq!(t.clutch, Some(1.0));
        assert_eq!(t.incidents, Some(3));
        assert_eq!(t.car_pct[5], 0.25);
        assert_eq!(t.car_position.expect("present")[63], 7);
        assert_eq!(t.tyre_temp.expect("present")[0][1], 81.5);
        assert_eq!(t.tyre_wear.expect("present")[3][2], 0.9);
        assert_eq!(t.tick, 77);
    }

    #[test]
    fn absent_optional_variables_read_as_none_not_zero() {
        let mut b = irsdk::testing::LayoutBuilder::new();
        for v in full_layout().vars().iter().filter(|v| {
            !matches!(
                v.name.as_str(),
                "PitsOpen" | "CarIdxEstTime" | "ClutchRaw" | "Clutch"
            )
        }) {
            b = b.var(&v.name, v.ty, v.count, &v.unit, &v.desc);
        }
        let layout = b.build();
        let vars = Vars::resolve(&table_of(&layout)).expect("vars");
        let row = Row::from_slice(RowBuilder::new(&layout).bytes());
        let mut t = Tick::new();
        // Stale values from an earlier fill must not survive.
        t.pits_open = Some(true);
        t.car_est = Some([1.0; CARS]);
        t.clutch = Some(1.0);
        t.fill(&vars, &row);
        assert_eq!((t.pits_open, t.car_est, t.clutch), (None, None, None));
        assert_eq!(t.incidents, Some(0));
    }

    #[test]
    fn processed_pedals_are_used_when_the_raw_ones_are_absent() {
        let mut b = irsdk::testing::LayoutBuilder::new();
        for v in full_layout()
            .vars()
            .iter()
            .filter(|v| v.name != "ThrottleRaw")
        {
            b = b.var(&v.name, v.ty, v.count, &v.unit, &v.desc);
        }
        let layout = b.build();
        let vars = Vars::resolve(&table_of(&layout)).expect("vars");
        let mut rb = RowBuilder::new(&layout);
        rb.set_f32("Throttle", 0.5);
        let mut t = Tick::new();
        t.fill(&vars, &Row::from_slice(rb.bytes()));
        assert_eq!(t.throttle, 0.5);
    }

    #[test]
    fn refilling_replaces_every_value() {
        let (vars, layout) = fixture();
        let mut t = Tick::new();
        let mut b = RowBuilder::new(&layout);
        b.set_i32("Gear", 5).set_i32_at("CarIdxLap", 2, 9);
        t.fill(&vars, &Row::from_slice(b.bytes()));
        assert_eq!((t.gear, t.car_lap[2]), (5, 9));
        t.fill(&vars, &Row::from_slice(RowBuilder::new(&layout).bytes()));
        assert_eq!((t.gear, t.car_lap[2]), (0, 0));
    }
}
