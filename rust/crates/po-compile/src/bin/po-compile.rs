// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `po-compile <catalogue.po> <catalogue.mo>` — the build step that turns a
//! translation into something gettext can open.
//!
//! Deliberately two positional arguments and no options: it is called by the
//! build system, once per language, and every choice it could offer is one the
//! build system would have to make the same way every time.
//!
//! A failure exits non-zero with the reason on stderr, which is what makes a
//! broken translation a red build rather than a language that quietly reverts
//! to English.

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
