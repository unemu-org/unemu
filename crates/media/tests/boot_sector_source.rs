// SPDX-License-Identifier: AGPL-3.0-or-later
//! The boot sector the FAT12 builder writes is the assembled
//! `src/fat12/boot.asm`: the source in the repository is the code on every
//! disk. Assembling needs nasm (in the Nix devShell); without it the test
//! says so and passes, unless `UNEMUPC_REQUIRE_FIXTURES=1` asks for every
//! fixture tool. A test binary of its own, because it starts a process.

use media::fat12::{format_metadata, volume_label, FloppyFormat};
use std::path::Path;
use std::process::Command;

#[test]
fn every_boot_sector_is_the_assembled_source() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/fat12/boot.asm");
    let output = std::env::temp_dir().join(format!("unemupc-boot-{}.bin", std::process::id()));
    let run = Command::new("nasm")
        .arg("-f")
        .arg("bin")
        .arg(&source)
        .arg("-o")
        .arg(&output)
        .output();
    let run = match run {
        Ok(run) => run,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            assert!(
                std::env::var_os("UNEMUPC_REQUIRE_FIXTURES").as_deref()
                    != Some(std::ffi::OsStr::new("1")),
                "nasm is required to check the boot sector source; run inside nix develop"
            );
            eprintln!("skipping: nasm not found; run inside nix develop");
            return;
        }
        Err(error) => panic!("nasm: {error}"),
    };
    assert!(
        run.status.success(),
        "nasm: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let assembled = std::fs::read(&output).unwrap();
    let _ = std::fs::remove_file(&output);
    assert_eq!(assembled.len(), 512);
    assert_eq!(&assembled[..3], &[0xEB, 0x3C, 0x90], "jump over the BPB");
    assert!(
        assembled[3..62].iter().all(|&byte| byte == 0),
        "the source leaves the BPB to the builder"
    );
    assert_eq!(&assembled[510..], &[0x55, 0xAA]);
    for format in FloppyFormat::ALL {
        for label in [None, Some(volume_label("DATA").unwrap())] {
            let sector = &format_metadata(format, label)[..512];
            assert_eq!(&sector[..3], &assembled[..3], "{format}");
            assert_eq!(&sector[62..], &assembled[62..], "{format}");
        }
    }
}
