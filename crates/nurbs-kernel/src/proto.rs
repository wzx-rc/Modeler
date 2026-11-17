//! Between the app and the kernel process: frames of `u32` length (little
//! endian) then bytes. A request is JSON (`Request`), the answer a binary
//! `Model`. The process answers each request before reading the next one.
//!
//! Before each operation that could crash inside OpenCascade, the process
//! writes `STEP <what>:<feature id>:<feature hash>` on stderr; the client
//! sends the same request again with that step in `skip`.

use crate::doc::Doc;
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Request {
    pub serial: u64,
    /// Fine meshing (idle) or coarse (while something moves).
    pub fine: bool,
    /// Steps not to try (they crashed the process).
    #[serde(default)]
    pub skip: Vec<String>,
    /// The part as quads too (`quads.rs`).
    #[serde(default)]
    pub quads: bool,
    pub doc: Doc,
}

pub fn write_frame(w: &mut impl Write, bytes: &[u8]) -> std::io::Result<()> {
    w.write_all(&(bytes.len() as u32).to_le_bytes())?;
    w.write_all(bytes)?;
    w.flush()
}

/// None at the end of the stream (or when it breaks).
pub fn read_frame(r: &mut impl Read) -> Option<Vec<u8>> {
    let mut n = [0u8; 4];
    r.read_exact(&mut n).ok()?;
    let n = u32::from_le_bytes(n) as usize;
    let mut b = vec![0u8; n];
    r.read_exact(&mut b).ok()?;
    Some(b)
}

/// The step name of an operation on a feature.
pub fn step(what: &str, f: &crate::doc::Feature) -> String {
    format!("{what}:{}:{:x}", f.id, f.hash())
}
