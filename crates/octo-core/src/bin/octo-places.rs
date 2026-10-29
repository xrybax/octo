//! Converts GeoNames' postal code dump into the places file getPlace reads:
//!
//!   octo-places allCountries.txt places.bin
//!
//! The image build runs it on `https://download.geonames.org/export/zip/allCountries.zip`
//! (unzipped); see the Dockerfile.

use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [input, output] = args.as_slice() else {
        eprintln!("usage: octo-places <allCountries.txt> <places.bin>");
        return ExitCode::from(2);
    };
    let converted = File::open(input).and_then(|source| {
        let target = File::create(output)?;
        octo_core::places::convert(BufReader::new(source), BufWriter::new(target))
    });
    match converted {
        Ok(summary) => {
            println!(
                "{output}: {} postal codes, {} cities and {} grid points from {} rows ({} left out), {} bytes",
                summary.codes, summary.cities, summary.points, summary.rows, summary.skipped, summary.bytes
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("octo-places: {e}");
            ExitCode::FAILURE
        }
    }
}
