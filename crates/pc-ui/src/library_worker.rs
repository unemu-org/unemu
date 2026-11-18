// SPDX-License-Identifier: AGPL-3.0-or-later
//! Host image-store jobs and recoverable desktop sessions; machine-images §§3–4, 7.
//!
//! The library's worker lives with the PC library models
//! (`pc_library::library_worker`) and is re-exported here under the path the
//! disk-layer dialog names it by. One of its tests is here, since it reviews
//! a publication with the disk-layer dialog's helpers.

pub(crate) use pc_library::library_worker::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library_test_support::Fixture;
    use library_model::listing::ImageRow;
    use std::fs;
    fn until(worker: &Worker, mut predicate: impl FnMut(&Event) -> bool) -> Event {
        loop {
            let event = worker
                .receive
                .recv_timeout(crate::test_wait::HANG_GUARD)
                .expect("worker notification");
            if let Event::Error(e) = &event {
                panic!("worker: {e}");
            }
            if predicate(&event) {
                return event;
            }
        }
    }
    fn worker(f: &Fixture) -> Worker {
        restarted(f).0
    }
    fn restarted(f: &Fixture) -> (Worker, Vec<Recovery>) {
        let w = Worker::synthetic(f.store.root().to_owned());
        let Event::Recoveries(rows) = until(&w, |e| matches!(e, Event::Recoveries(_))) else {
            unreachable!()
        };
        (w, rows)
    }
    fn launch(
        w: &Worker,
        row: &ImageRow,
        kind: WorkspaceKind,
    ) -> (pc_image::workspace::PreparedSession, String) {
        w.send
            .send(Request::Launch(row.clone(), kind, None))
            .unwrap();
        let Event::Launched(transfer, _, name, actual, _) =
            until(w, |e| matches!(e, Event::Launched(..)))
        else {
            unreachable!()
        };
        assert_eq!(actual, kind);
        (transfer.restore().unwrap(), name)
    }
    fn finish(w: &Worker, outcome: Finish) -> Vec<Recovery> {
        w.send.send(Request::Finish(outcome)).unwrap();
        let Event::Recoveries(rows) = until(w, |e| matches!(e, Event::Recoveries(_))) else {
            unreachable!()
        };
        rows
    }
    #[test]
    fn keep_publishes_then_removes_disposable_but_regular_publication_keeps_workspace() {
        for kind in [WorkspaceKind::Disposable, WorkspaceKind::Regular] {
            let f = Fixture::new();
            let mut w = worker(&f);
            let (prepared, name) = launch(&w, &f.row, kind);
            f.change(&prepared, 42);
            drop(prepared);
            finish(&w, Finish::Retain);
            let details =
                pc_image::desktop::check(&f.store, &f.row.descriptor, Some(&name), || false)
                    .unwrap();
            let crate::layers_dialog::Action::Publish(workspace, target) =
                crate::layers_dialog::tests::reviewed_publication(
                    &name,
                    kind,
                    details,
                    crate::layers_dialog::tests::source(&f.row.reference, &f.row.descriptor.digest),
                    "test:kept",
                    kind == WorkspaceKind::Regular,
                    None,
                )
            else {
                panic!()
            };
            assert_eq!(target.flatten, kind == WorkspaceKind::Regular);
            w.send
                .send(Request::Publish(f.row.clone(), workspace, target))
                .unwrap();
            until(&w, |e| matches!(e, Event::Published(_)));
            assert_eq!(
                f.store.workspace_path(&name).unwrap().exists(),
                kind == WorkspaceKind::Regular
            );
            let details = pc_image::desktop::check(
                &f.store,
                &f.store.tag_descriptor("test:kept").unwrap(),
                None,
                || false,
            )
            .unwrap();
            assert_eq!(details.media.len(), 4);
            assert!(details
                .media
                .iter()
                .all(|m| m.chain.len() == 1 || kind.disposable()));
            let resolved = pc_image::test_support::with_synthetic_firmware(|| {
                pc_image::resolve(&f.store, "test:kept", &f.temp.path().join("inspect"))
            })
            .unwrap();
            let image = pc_image::test_support::with_synthetic_firmware(|| {
                f.store.resolve("test:kept", &pc_image::PcProvider)
            })
            .unwrap();
            let assert_medium = |source: &assembly::ImageSource, sha256: Option<[u8; 32]>| {
                let assembly::ImageSource::File(path) = source else {
                    panic!("expected file")
                };
                // A compressed floppy has no file; read its verified payload.
                let bytes = if path.is_file() {
                    fs::read(path).unwrap()
                } else {
                    let hex: String = sha256.unwrap().iter().map(|b| format!("{b:02x}")).collect();
                    let mut bytes = Vec::new();
                    let digest = format!("sha256:{hex}").parse().unwrap();
                    std::io::Read::read_to_end(
                        &mut image.content_reader(&digest).unwrap(),
                        &mut bytes,
                    )
                    .unwrap();
                    bytes
                };
                assert_eq!(&bytes[..512], &[42; 512]);
            };
            for drive in &resolved.drives {
                if let Some(list) = &drive.media {
                    for entry in &list.entries {
                        assert_medium(&entry.image, entry.expected_sha256);
                    }
                } else {
                    assert_medium(&drive.image, drive.expected_sha256);
                }
            }
            let mut cmos = false;
            for device in &resolved.devices {
                if let assembly::DeviceKind::Rtc(rtc) = &device.kind {
                    if let assembly::NvramSource::Journal { image, .. } = &rtc.nvram {
                        assert_eq!(fs::read(image).unwrap()[32], 42);
                        cmos = true;
                    }
                }
            }
            assert!(cmos);
            w.shutdown();
        }
    }
}
