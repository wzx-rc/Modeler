//! forme-nurbs-kernel: the Forme NURBS kernel process. Reads requests
//! (`nurbs_kernel::proto`) on stdin, builds each document with OpenCascade,
//! answers with the model on stdout, until stdin closes. Keeps its caches
//! between requests, so a feature dragged around costs only the booleans
//! after it.

use nurbs_kernel::build::{build, Cache};
use nurbs_kernel::proto::{read_frame, write_frame, Request};
use std::io::{BufReader, BufWriter};

fn main() {
    let mut input = BufReader::new(std::io::stdin().lock());
    let mut output = BufWriter::new(std::io::stdout().lock());
    let mut cache = Cache::default();
    while let Some(frame) = read_frame(&mut input) {
        let model = match serde_json::from_slice::<Request>(&frame) {
            Ok(req) => {
                let mut m = build(&req.doc, req.fine, req.quads, &req.skip, &mut cache);
                m.serial = req.serial;
                m
            }
            Err(e) => nurbs_kernel::Model { errors: vec![format!("bad request: {e}")], ..Default::default() },
        };
        if write_frame(&mut output, &model.encode()).is_err() {
            break;
        }
    }
}
