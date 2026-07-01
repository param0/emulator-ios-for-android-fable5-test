//! IPA discovery and bundle-metadata extraction for the app manager.
//!
//! An `.ipa` is a zip archive whose payload lives under `Payload/<App>.app/`.
//! This crate scans a directory for IPAs, opens each one, locates the app
//! bundle, parses `Info.plist` (binary or XML) for display metadata, and pulls
//! out the primary icon bytes — everything the Android frontend needs to render
//! its app grid without launching the emulator core.

#![forbid(unsafe_code)]

pub mod discovery;
pub mod extract;
pub mod plist;

pub use discovery::{inspect_ipa, scan_directory, AppMetadata, IpaBundle};
pub use extract::extract_app;
pub use plist::{PlistError, PlistValue};
