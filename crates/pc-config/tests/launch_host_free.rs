// SPDX-License-Identifier: AGPL-3.0-or-later
//! Logical media keys must never be interpreted relative to the host cwd.

#[cfg(unix)]
struct TempDir(std::path::PathBuf);

#[cfg(unix)]
impl TempDir {
    fn new() -> Self {
        use std::{
            sync::atomic::{AtomicU64, Ordering},
            time::{SystemTime, UNIX_EPOCH},
        };

        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "pc-launch-host-free-{}-{nanos}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&dir).unwrap();
        Self(dir)
    }
}

#[cfg(unix)]
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(unix)]
#[test]
fn temporary_directories_are_unique_and_removed_on_unwind() {
    let first = TempDir::new();
    let second = TempDir::new();
    assert_ne!(first.0, second.0);
    let first_path = first.0.clone();
    let second_path = second.0.clone();
    drop(first);
    let removed_normally = !first_path.exists();
    let result = std::panic::catch_unwind(move || {
        let _dir = second;
        panic!("simulate a setup or spawn failure");
    });
    let removed_on_unwind = !second_path.exists();
    // Keep a cleanup regression from leaving its own temporary files behind.
    let _ = std::fs::remove_dir_all(first_path);
    let _ = std::fs::remove_dir_all(second_path);
    assert!(result.is_err());
    assert!(removed_normally);
    assert!(removed_on_unwind);
}

#[cfg(unix)]
#[test]
fn journalled_cmos_validation_ignores_dangling_host_symlinks() {
    use pc_config::{image, launch};
    use std::{collections::BTreeMap, process::Command};
    use unemu_core_api::{Content, ContentSource};

    const CHILD: &str = "UNEMU_TEST_LAUNCH_HOST_FREE_CHILD";
    if std::env::var_os(CHILD).is_none() {
        // A child owns the cwd so parallel tests never observe it changing.
        let dir = TempDir::new();
        std::os::unix::fs::symlink("absent-cmos-journal", dir.0.join("cmos.upfj")).unwrap();
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "journalled_cmos_validation_ignores_dangling_host_symlinks",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .current_dir(&dir.0)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }

    struct Memory(BTreeMap<[u8; 32], Vec<u8>>);
    impl ContentSource for Memory {
        fn open(&self, sha256: &[u8; 32], size: u64) -> Result<Content, unemu_core_api::Error> {
            let bytes = &self.0[sha256];
            assert_eq!(bytes.len() as u64, size);
            Ok(Content::Bytes(bytes.clone()))
        }
    }
    let bios = vec![0xf4; 65_536];
    let cmos = vec![0; 64];
    let b = image::Digest::of(&bios);
    let c = image::Digest::of(&cmos);
    let config = image::Config::decode(
        format!(
            r#"
schema = 1
platform = "ibm-pc"
[machine]
schema = 1
[machine.board]
model = "ibm-5170"
[machine.board.rtc.nvram]
image = "assets/cmos.bin"
[machine.firmware.bios]
image = "assets/bios.bin"
sha256 = "{}"
[[firmware]]
key = "firmware.bios"
source = "assets/bios.bin"
blob = "{b}"
[nvram]
mode = "journal"
source = "assets/cmos.bin"
head = "{c}"
"#,
            b.hex()
        )
        .parse()
        .unwrap(),
    )
    .unwrap();
    let content = Memory(BTreeMap::from([(b.sha256(), bios), (c.sha256(), cmos)]));
    let rows = content
        .0
        .iter()
        .map(|(sha256, bytes)| launch::ContentRow {
            digest: image::Digest::from_sha256(*sha256),
            sha256: *sha256,
            size: bytes.len() as u64,
        })
        .collect();
    let document = launch::Launch {
        config,
        content: rows,
        records: vec![launch::RecordRow {
            medium: "cmos".into(),
            name: "cmos.upfj".into(),
            keep_generations: 3,
        }],
    };
    let options = assembly::AssemblyOptions {
        allow_unlisted_firmware: true,
    };
    launch::check(&document, options).unwrap();
    let lowered = launch::lower(&document, &content, options).unwrap();
    assert!(lowered.machine.devices.iter().any(|device| matches!(
        &device.kind,
        assembly::DeviceKind::Rtc(assembly::CmosCfg {
            nvram: assembly::NvramSource::Journal {
                keep_generations: 3,
                ..
            },
            ..
        })
    )));
}
