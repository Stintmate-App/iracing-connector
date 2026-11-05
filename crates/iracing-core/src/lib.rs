//! Core types of the iRacing connector.
//!
//! This crate will hold the connector description, the texts of its control app and the
//! interfaces between the recorder parts. Today it holds only the three interfaces, without
//! methods.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

/// The recorder frame source, live or replayed.
pub trait Source {}

/// Where the tracker records go.
pub trait Sink {}

/// The tracker time source.
pub trait Clock {}

#[cfg(test)]
mod tests {
    use super::*;

    struct Nothing;
    impl Source for Nothing {}
    impl Sink for Nothing {}
    impl Clock for Nothing {}

    #[test]
    fn traits_can_be_implemented() {
        fn takes<T: Source + Sink + Clock>(_: &T) {}
        takes(&Nothing);
    }
}
