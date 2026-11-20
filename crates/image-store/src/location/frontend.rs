// SPDX-License-Identifier: AGPL-3.0-or-later
//! Front-end data directory selection; core-contract §12.13 Q5.
//! Existing trees stay in use: copying path-bearing documents and journals
//! could split writable data or change the meaning of relative paths.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

/// Use the legacy tree while it exists, otherwise the `unemu` default.
/// Selection never traverses, copies or changes either tree. The old tree
/// wins even over an existing new one, so startup cannot hide existing data.
/// New writes use the selected tree until the user explicitly moves it.
pub fn directory(data: &Path, name: &str) -> io::Result<PathBuf> {
    if !matches!(name, "machines" | "diagnostics" | "saves" | "states") {
        return Err(io::Error::other("unknown front-end directory"));
    }
    let old = data.join("unemupc").join(name);
    match fs::symlink_metadata(&old) {
        Ok(_) => Ok(old),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(data.join("unemu").join(name)),
        Err(error) => Err(error),
    }
}
