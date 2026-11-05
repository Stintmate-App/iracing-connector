// The constants and layouts in this crate follow the iRacing SDK header irsdk_defines.h,
// Copyright (c) 2013, iRacing.com Motorsport Simulations, LLC, used under the BSD 3-Clause
// licence: see THIRD-PARTY-NOTICES-irsdk.txt at the root of the repository.

//! A reader of the iRacing SDK data.
//!
//! The data layouts are parsed from `&[u8]` with bounds checks, so the same functions serve the
//! live memory map (through copies of its parts), `.ibt` files and synthetic images. Variables
//! are resolved by name into typed handles that read a preallocated [`row::Row`].
//!
//! This crate is read-only towards the simulator: it defines no message to send to it.

#![deny(missing_docs)]
#![deny(unsafe_code)]

pub mod defs;
pub mod header;
#[cfg(windows)]
pub mod live;
pub mod row;
pub mod sampler;
pub mod session;
pub mod vars;
pub mod view;
pub mod yaml;

#[cfg(any(test, feature = "testing"))]
pub mod testing;
