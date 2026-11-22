// SPDX-License-Identifier: AGPL-3.0-or-later
#[path = "src/locked_source.rs"]
mod locked_source;

fn main() {
    println!("cargo:rerun-if-changed=../../Cargo.lock");
    let lock = std::fs::read_to_string("../../Cargo.lock").expect("read front-end Cargo.lock");
    let allow_local = std::env::var("PROFILE").expect("Cargo build profile") != "release";
    for (variable, packages, repository) in [
        (
            "UNEMU_PC_SOURCE",
            locked_source::PC_PACKAGES,
            "https://github.com/unemu-org/unemupc",
        ),
        (
            "UNEMU_NES_SOURCE",
            locked_source::NES_PACKAGES,
            "https://github.com/unemu-org/unemunes",
        ),
    ] {
        let source = locked_source::core_source(&lock, packages, repository, allow_local)
            .unwrap_or_else(|error| panic!("core source notice: {error}"));
        println!("cargo:rustc-env={variable}={source}");
    }
}
