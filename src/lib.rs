//! # emote-psb
//!
//! Serde-based serialization and deserialization library for the E-mote PSB/MDF binary
//! data format.
//!
//! ## Overview
//!
//! E-mote PSB (`.psb`, `.scn`) is a proprietary binary format used by
//! E-mote animation data. MDF (`.mdf`, `.psb.m`) is a compressed wrapper around
//! a PSB file.  Some game archives use a FreeMote-compatible MT19937/XOR MDF
//! shell; see [`mdf::MdfReader::open_with_seed`] and
//! [`mdf::MdfReader::open_with_key`].
//!
//! ## Reading a PSB file
//!
//! ```no_run
//! use emote_psb::{psb::read::PsbFile, value::PsbValue};
//! use std::{fs::File, io::BufReader};
//!
//! let file = BufReader::new(File::open("sample.psb").unwrap());
//! let mut psb = PsbFile::open(file).unwrap();
//! let root: PsbValue = psb.deserialize_root().unwrap();
//! ```
//!
//! ## Reading an MT19937 MDF file
//!
//! ```no_run
//! use emote_psb::mdf::MdfReader;
//! use std::{fs::File, io::{BufReader, Read}};
//!
//! let file = BufReader::new(File::open("scenario_info.psb.m").unwrap());
//! let mut reader = MdfReader::open_with_key(file, "game-key", "scenario_info.psb.m", 0x83).unwrap();
//! let mut psb_bytes = Vec::new();
//! reader.read_to_end(&mut psb_bytes).unwrap();
//! ```
//!
//! ## Writing a PSB file
//!
//! ```no_run
//! use emote_psb::{psb::write::PsbWriter, value::PsbValue};
//! use std::{fs::File, io::BufWriter};
//!
//! let root = PsbValue::Null;
//! let out = BufWriter::new(File::create("out.psb").unwrap());
//! let writer = PsbWriter::new(3, false, &root, out).unwrap();
//! writer.finish().unwrap();
//! ```

pub mod mdf;
pub mod psb;
pub mod value;

/// PSB file signature (`"PSB"` as a little-endian `u32`).
pub const PSB_SIGNATURE: u32 = 0x425350;

/// MDF (compressed PSB) file signature (`"mdf"` as a little-endian `u32`).
pub const PSB_MDF_SIGNATURE: u32 = 0x66646D;
