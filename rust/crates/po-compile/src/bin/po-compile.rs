// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Build helper: `po-compile <catalogue.po> <catalogue.mo>`.
//! Keeps a two-argument interface for direct build-system use.

use std::process::ExitCode;

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().collect();
    let [_, input, output] = arguments.as_slice() else {
        eprintln!("usage: po-compile <catalogue.po> <catalogue.mo>");
        return ExitCode::FAILURE;
    };

    let po = match std::fs::read_to_string(input) {
        Ok(po) => po,
        Err(why) => {
            eprintln!("po-compile: {input}: {why}");
            return ExitCode::FAILURE;
        }
    };

    let mo = match po_compile::compile(&po) {
        Ok(mo) => mo,
        Err(why) => {
            eprintln!("po-compile: {input}: {why}");
            return ExitCode::FAILURE;
        }
    };

    if let Err(why) = std::fs::write(output, mo) {
        eprintln!("po-compile: {output}: {why}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
