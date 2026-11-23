// SPDX-License-Identifier: AGPL-3.0-or-later
use super::*;
use sha2::Digest as _;
use std::{
    fs,
    sync::atomic::{AtomicU64, Ordering},
};
use unemu_core_api::{
    test_support::{ExportOperation, MemoryExportSink, ScriptedCore},
    *,
};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "unemu-neutral-export-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn sink(&self) -> FileSink<'_> {
        FileSink::new(&self.0).unwrap()
    }
    fn names(&self) -> Vec<String> {
        fs::read_dir(&self.0)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect()
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

struct Source(&'static [u8]);
impl Snapshot for Source {
    fn config(&self) -> ConfigDocument<'_> {
        ConfigDocument::new("text/plain", self.0)
    }
    fn baseline(&self) -> anyhow::Result<Vec<MediaDescription>> {
        Ok(vec![MediaDescription::new(SlotDescription::new(
            SlotId(0),
            SlotKind::Floppy,
            "Baseline",
            true,
        ))
        .with_size(Some(3))
        .with_detail("baseline detail")])
    }
    fn current(&self, _label: &str) -> anyhow::Result<Box<dyn ContentReader + '_>> {
        Ok(Box::new(Bytes))
    }
}
struct Bytes;
impl ContentReader for Bytes {
    fn size(&self) -> u64 {
        3
    }
    fn read_at(&self, offset: u64, bytes: &mut [u8]) -> Result<(), Error> {
        let source = b"abc"
            .get(offset as usize..offset as usize + bytes.len())
            .ok_or_else(|| usage("out of range"))?;
        bytes.copy_from_slice(source);
        Ok(())
    }
}

#[test]
fn lists_advertised_formats_and_optional_lengths_through_dyn_core() {
    let core: &dyn Core = &ScriptedCore;
    for (config, size) in [
        (b"fixed".as_slice(), "3"),
        (b"variable".as_slice(), "unknown"),
    ] {
        let mut out = Vec::new();
        list(core, "scripted", &Source(config), &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("format:raw\tRaw bytes\t.raw\tslot\tavailable"),
            "{text}"
        );
        assert!(text.contains("format:machine\tScripted machine\t.txt\tmachine\tavailable"));
        assert!(text.contains(&format!("Media\tfloppy\t{size}\tScripted raw medium\n")));
    }
}

#[test]
fn exports_slot_and_machine_and_prints_every_note() {
    let core: &dyn Core = &ScriptedCore;
    for (format, slot, count) in [("raw", Some("Media"), 1), ("machine", None, 2)] {
        let temp = Temp::new();
        let report = export(
            core,
            "scripted",
            &Source(b"fixed"),
            format,
            slot,
            &mut temp.sink(),
        )
        .unwrap();
        assert_eq!(temp.names().len(), count);
        assert_eq!(fs::read(temp.0.join("slot-0.raw")).unwrap(), b"abc");
        assert_eq!(
            report.outputs[0].sha256,
            <[u8; 32]>::from(sha2::Sha256::digest(b"abc"))
        );
        let mut out = Vec::new();
        print(&report, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        for note in &report.notes {
            assert!(text.contains(note));
        }
        if format == "raw" {
            assert!(!report.notes.is_empty());
        }
    }
}

#[test]
fn refuses_unknown_format_slot_and_wrong_scope_cardinality() {
    for (format, slot) in [
        ("absent", Some("Media")),
        ("raw", Some("absent")),
        ("raw", None),
        ("machine", Some("Media")),
    ] {
        let temp = Temp::new();
        let error = export(
            &ScriptedCore,
            "scripted",
            &Source(b"fixed"),
            format,
            slot,
            &mut temp.sink(),
        )
        .unwrap_err();
        assert_eq!(
            error.downcast_ref::<Error>().unwrap().kind,
            ErrorKind::Usage
        );
        assert!(temp.names().is_empty());
    }
}

#[derive(Clone, Copy)]
enum Behavior {
    Baseline,
    QueryAbsent,
    Detail,
    Missing,
    Extra,
    Duplicate,
    Name,
    Size,
    Hash,
    Unfinished,
    Failure(ExportOperation),
    Pair,
    InvalidName,
    FinalCollision,
    ObserveFailure,
    ObserveMismatch,
    PublicationTrio,
}
struct Changed(Behavior);
impl Changed {
    fn scripted_config<'a>(&self, config: ConfigDocument<'a>) -> ConfigDocument<'a> {
        if matches!(
            self.0,
            Behavior::FinalCollision | Behavior::ObserveFailure | Behavior::ObserveMismatch
        ) {
            ConfigDocument::new("text/plain", b"fixed")
        } else {
            config
        }
    }
}
impl Core for Changed {
    fn description(&self) -> &CoreDescription {
        ScriptedCore.description()
    }
    fn create(&self, _request: &CreateRequest<'_>) -> Result<Box<dyn Machine>, Error> {
        panic!("offline export must not create a machine")
    }
    fn describe_media(
        &self,
        platform: &str,
        config: ConfigDocument<'_>,
    ) -> Result<Vec<MediaDescription>, Error> {
        if matches!(self.0, Behavior::Baseline) {
            return Err(unsupported("no media description"));
        }
        let mut media = ScriptedCore.describe_media(platform, self.scripted_config(config))?;
        if matches!(self.0, Behavior::Detail) {
            media[0].detail = "opaque\tCHS nonsense\nsize=do not parse".into();
        }
        if matches!(self.0, Behavior::Pair) {
            media.push(MediaDescription::new(SlotDescription::new(
                SlotId(4),
                SlotKind::Tape,
                "Tape",
                true,
            )));
            media.push(MediaDescription::new(SlotDescription::new(
                SlotId(7),
                SlotKind::Optical,
                "Excluded",
                true,
            )));
        }
        Ok(media)
    }
    fn export_slots(
        &self,
        platform: &str,
        config: ConfigDocument<'_>,
        format: &str,
    ) -> Result<Vec<SlotId>, Error> {
        if matches!(self.0, Behavior::QueryAbsent) {
            return Err(unsupported("no query"));
        }
        if matches!(self.0, Behavior::Pair) {
            return Ok(vec![SlotId(4), SlotId(0)]);
        }
        ScriptedCore.export_slots(platform, self.scripted_config(config), format)
    }
    fn export(
        &self,
        platform: &str,
        config: ConfigDocument<'_>,
        request: ExportRequest<'_>,
        sink: &mut dyn ExportSink,
    ) -> Result<ExportReport, Error> {
        if let Behavior::Failure(operation) = self.0 {
            // Reference faults define the allowed failure kind at each boundary.
            let mut memory = MemoryExportSink::new();
            memory.fail_next(operation);
            let error = ScriptedCore
                .export(platform, config, request, &mut memory)
                .unwrap_err();
            assert_eq!(error.kind, ErrorKind::Storage);
            let output = sink.create("partial.raw")?;
            if operation != ExportOperation::Create {
                output.write(b"partial")?;
            }
            return Err(error);
        }
        if matches!(self.0, Behavior::Pair) {
            assert_eq!(
                request.media.iter().map(|m| m.slot).collect::<Vec<_>>(),
                [SlotId(4), SlotId(0)]
            );
            let mut report = ExportReport::new();
            for medium in request.media {
                let output = sink.create(&format!("slot-{}.raw", medium.slot.0))?;
                let mut bytes = vec![0; medium.size as usize];
                medium.bytes.read_at(0, &mut bytes)?;
                output.write(&bytes)?;
                report.outputs.push(output.finish()?);
            }
            return Ok(report);
        }
        let mut report =
            ScriptedCore.export(platform, self.scripted_config(config), request, sink)?;
        match self.0 {
            Behavior::PublicationTrio => {
                let output = sink.create("tail.raw")?;
                output.write(b"tail")?;
                report.outputs.push(output.finish()?);
            }
            Behavior::FinalCollision => {
                let directory = Path::new(std::str::from_utf8(config.bytes).unwrap());
                assert!(!directory.join("slot-0.raw").exists());
                fs::write(directory.join("slot-0.raw"), b"other party").unwrap();
            }
            Behavior::ObserveFailure | Behavior::ObserveMismatch => {
                let directory = Path::new(std::str::from_utf8(config.bytes).unwrap());
                assert!(!directory.join("slot-0.raw").exists());
                assert!(fs::read_dir(directory).unwrap().any(|e| e
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".part")));
                if matches!(self.0, Behavior::ObserveFailure) {
                    return Err(Error::new(ErrorKind::Storage, "injected core failure"));
                }
                report.outputs[0].size += 1;
            }
            Behavior::InvalidName => {
                sink.create("../escape")?;
            }
            Behavior::Missing => {
                report.outputs.pop();
            }
            Behavior::Extra => report
                .outputs
                .push(ExportedOutput::new("extra", 0, [0; 32])),
            Behavior::Duplicate => {
                report.outputs[1] = report.outputs[0].clone();
            }
            Behavior::Name => report.outputs[0].name = "different".into(),
            Behavior::Size => report.outputs[0].size += 1,
            Behavior::Hash => report.outputs[0].sha256[0] ^= 1,
            Behavior::Unfinished => {
                sink.create("unfinished")?.write(b"partial")?;
                report
                    .outputs
                    .push(ExportedOutput::new("unfinished", 7, [0; 32]));
            }
            _ => (),
        }
        Ok(report)
    }
}

#[test]
fn unsupported_description_falls_back_and_detail_is_verbatim() {
    for (behavior, expected) in [
        (Behavior::Baseline, "Baseline\tfloppy\t3\tbaseline detail"),
        (Behavior::Detail, "opaque\tCHS nonsense\nsize=do not parse"),
    ] {
        let mut out = Vec::new();
        list(&Changed(behavior), "scripted", &Source(b"fixed"), &mut out).unwrap();
        assert!(String::from_utf8(out).unwrap().contains(expected));
    }
}
#[test]
fn unsupported_query_disables_formats_and_prevents_export() {
    let core: &dyn Core = &Changed(Behavior::QueryAbsent);
    let mut out = Vec::new();
    list(core, "scripted", &Source(b"fixed"), &mut out).unwrap();
    assert_eq!(
        String::from_utf8(out)
            .unwrap()
            .matches("unavailable")
            .count(),
        2
    );
    let temp = Temp::new();
    assert!(export(
        core,
        "scripted",
        &Source(b"fixed"),
        "raw",
        Some("Media"),
        &mut temp.sink()
    )
    .is_err());
    assert!(temp.names().is_empty());
}
#[test]
fn machine_uses_exact_queried_set_even_when_description_lists_other_slots() {
    let temp = Temp::new();
    let core: &dyn Core = &Changed(Behavior::Pair);
    let report = export(
        core,
        "scripted",
        &Source(b"fixed"),
        "machine",
        None,
        &mut temp.sink(),
    )
    .unwrap();
    assert_eq!(report.outputs.len(), 2);
}
#[test]
fn rejects_every_report_mismatch_and_cleans_only_attempt_outputs() {
    for behavior in [
        Behavior::Missing,
        Behavior::Extra,
        Behavior::Duplicate,
        Behavior::Name,
        Behavior::Size,
        Behavior::Hash,
        Behavior::Unfinished,
    ] {
        let temp = Temp::new();
        fs::write(temp.0.join("keep"), b"existing").unwrap();
        let core: &dyn Core = &Changed(behavior);
        assert!(export(
            core,
            "scripted",
            &Source(b"fixed"),
            "machine",
            None,
            &mut temp.sink()
        )
        .is_err());
        assert_eq!(temp.names(), ["keep"]);
        assert_eq!(fs::read(temp.0.join("keep")).unwrap(), b"existing");
    }
}
#[test]
fn planted_create_write_and_finish_failures_clean_partial_outputs() {
    for operation in [
        ExportOperation::Create,
        ExportOperation::Write,
        ExportOperation::Finish,
    ] {
        let temp = Temp::new();
        let error = export(
            &Changed(Behavior::Failure(operation)),
            "scripted",
            &Source(b"fixed"),
            "raw",
            Some("Media"),
            &mut temp.sink(),
        )
        .unwrap_err();
        assert_eq!(
            error.downcast_ref::<Error>().unwrap().kind,
            ErrorKind::Storage
        );
        assert!(temp.names().is_empty());
    }
}
#[test]
fn file_sink_reserves_names_rejects_invalid_and_preserves_preexisting_files() {
    let temp = Temp::new();
    fs::write(temp.0.join("existing"), b"keep").unwrap();
    let mut sink = temp.sink();
    for name in ["../escape", "bad..name", "", "Upper", "a/b"] {
        assert_eq!(sink.create(name).err().unwrap().kind, ErrorKind::Usage);
    }
    sink.create("unfinished")
        .unwrap()
        .write(b"partial")
        .unwrap();
    assert_eq!(
        sink.create("unfinished").err().unwrap().kind,
        ErrorKind::Usage
    );
    sink.cleanup().unwrap();
    assert_eq!(temp.names(), ["existing"]);
}
#[test]
fn host_special_names_and_closed_output_rules() {
    let temp = Temp::new();
    let mut sink = temp.sink();
    for name in ["con", "nul", "com1", "lpt1.txt"] {
        let output = sink.create(name).unwrap();
        output.write(b"abc").unwrap();
        let receipt = output.finish().unwrap();
        assert_eq!(receipt.name, name);
        assert_eq!(output.write(b"late").unwrap_err().kind, ErrorKind::Storage);
        assert_eq!(output.finish().unwrap_err().kind, ErrorKind::Storage);
    }
    sink.cleanup().unwrap();
    assert!(temp.names().is_empty());
}
#[test]
fn parser_requires_explicit_format_and_directory() {
    let words = |s: &str| s.split_whitespace().map(OsString::from).collect::<Vec<_>>();
    assert!(parse(&words("id")).is_ok());
    assert!(parse(&words("id --format machine --to out")).is_ok());
    assert!(parse(&words("id --format raw --slot Media --to out")).is_ok());
    for args in [
        "id --slot Media --to file",
        "id --format raw",
        "id --to out",
        "id --format raw --format raw --to out",
        "id --slot Media --slot Media --format raw --to out",
    ] {
        assert!(parse(&words(args)).is_err(), "{args}");
    }
}

#[test]
fn unknown_scope_classifier_marks_formats_unavailable_without_querying() {
    let mut out = Vec::new();
    list_with_scopes(
        &Changed(Behavior::QueryAbsent),
        "scripted",
        &Source(b"fixed"),
        &mut out,
        |_| None,
    )
    .unwrap();
    let text = String::from_utf8(out).unwrap();
    assert_eq!(text.matches("unknown\tunavailable").count(), 2);
}
#[test]
fn file_sink_io_failures_close_and_remove_partial_creations() {
    for operation in [
        ExportOperation::Create,
        ExportOperation::Write,
        ExportOperation::Finish,
    ] {
        let temp = Temp::new();
        fs::write(temp.0.join("keep"), b"existing").unwrap();
        let mut sink = temp.sink();
        sink.fail_next(operation);
        let core: &dyn Core = &ScriptedCore;
        let error = export(
            core,
            "scripted",
            &Source(b"fixed"),
            "raw",
            Some("Media"),
            &mut sink,
        )
        .unwrap_err();
        assert_eq!(
            error.downcast_ref::<Error>().unwrap().kind,
            ErrorKind::Storage
        );
        assert_eq!(temp.names(), ["keep"]);
    }
}

#[test]
fn contract_errors_keep_their_cli_exit_categories() {
    for (kind, code) in [
        (ErrorKind::Usage, 2),
        (ErrorKind::Config, 2),
        (ErrorKind::Content, 1),
        (ErrorKind::Storage, 1),
        (ErrorKind::Unsupported, 1),
    ] {
        let error = anyhow::Error::new(Error::new(kind, "export diagnostic"));
        assert_eq!(exit_code(&error), code);
        assert!(format!("{error:#}").contains("export diagnostic"));
    }
}

struct BadSource(ErrorKind);
impl Snapshot for BadSource {
    fn config(&self) -> ConfigDocument<'_> {
        ConfigDocument::new("text/plain", b"fixed")
    }
    fn baseline(&self) -> anyhow::Result<Vec<MediaDescription>> {
        Source(b"fixed").baseline()
    }
    fn current(&self, _label: &str) -> anyhow::Result<Box<dyn ContentReader + '_>> {
        Ok(Box::new(BadReader(self.0)))
    }
}
struct BadReader(ErrorKind);
impl ContentReader for BadReader {
    fn size(&self) -> u64 {
        3
    }
    fn read_at(&self, _offset: u64, bytes: &mut [u8]) -> Result<(), Error> {
        bytes.fill(42);
        Err(Error::new(self.0, "planted content read failure"))
    }
}
#[test]
fn failed_content_read_never_publishes_its_partial_buffer() {
    for kind in [ErrorKind::Content, ErrorKind::Storage] {
        let temp = Temp::new();
        let error = export(
            &ScriptedCore,
            "scripted",
            &BadSource(kind),
            "raw",
            Some("Media"),
            &mut temp.sink(),
        )
        .unwrap_err();
        assert_eq!(error.downcast_ref::<Error>().unwrap().kind, kind);
        assert!(temp.names().is_empty());
    }
}

#[test]
fn unfinished_temp_from_an_earlier_attempt_is_never_overwritten_or_removed() {
    let temp = Temp::new();
    let mut previous = temp.sink();
    previous
        .create("slot-0.raw")
        .unwrap()
        .write(b"unfinished earlier bytes")
        .unwrap();
    previous.close();
    let earlier_temp = temp.names().pop().unwrap();
    export(
        &ScriptedCore,
        "scripted",
        &Source(b"fixed"),
        "raw",
        Some("Media"),
        &mut temp.sink(),
    )
    .unwrap();
    assert_eq!(
        fs::read(temp.0.join(&earlier_temp)).unwrap(),
        b"unfinished earlier bytes"
    );
    assert_eq!(fs::read(temp.0.join("slot-0.raw")).unwrap(), b"abc");
    previous.cleanup().unwrap();
    assert_eq!(temp.names(), ["slot-0.raw"]);
}

#[test]
fn invalid_core_output_name_cleans_already_finished_outputs() {
    let temp = Temp::new();
    let core: &dyn Core = &Changed(Behavior::InvalidName);
    let error = export(
        core,
        "scripted",
        &Source(b"fixed"),
        "machine",
        None,
        &mut temp.sink(),
    )
    .unwrap_err();
    assert_eq!(
        error.downcast_ref::<Error>().unwrap().kind,
        ErrorKind::Usage
    );
    assert!(temp.names().is_empty());
}
#[test]
fn publication_failure_preserves_previous_file_and_reports_published_images() {
    let temp = Temp::new();
    fs::write(temp.0.join("machine.txt"), b"earlier document").unwrap();
    let error = export(
        &Changed(Behavior::PublicationTrio),
        "scripted",
        &Source(b"fixed"),
        "machine",
        None,
        &mut temp.sink(),
    )
    .unwrap_err();
    assert_eq!(
        error.downcast_ref::<Error>().unwrap().kind,
        ErrorKind::Usage
    );
    assert!(
        error.to_string().contains("cannot publish machine.txt"),
        "{error:#}"
    );
    assert!(
        error.to_string().contains("published: slot-0.raw"),
        "{error:#}"
    );
    assert_eq!(temp.names().len(), 2);
    assert_eq!(fs::read(temp.0.join("slot-0.raw")).unwrap(), b"abc");
    assert_eq!(
        fs::read(temp.0.join("machine.txt")).unwrap(),
        b"earlier document"
    );
}

#[test]
fn final_created_after_export_is_never_overwritten_or_removed() {
    let temp = Temp::new();
    let config = temp.0.to_str().unwrap().as_bytes();
    let source = DirectorySource(config);
    let error = export(
        &Changed(Behavior::FinalCollision),
        "scripted",
        &source,
        "raw",
        Some("Media"),
        &mut temp.sink(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("slot-0.raw"), "{error:#}");
    assert_eq!(fs::read(temp.0.join("slot-0.raw")).unwrap(), b"other party");
    assert_eq!(temp.names(), ["slot-0.raw"]);
}

struct DirectorySource<'a>(&'a [u8]);
impl Snapshot for DirectorySource<'_> {
    fn config(&self) -> ConfigDocument<'_> {
        ConfigDocument::new("text/plain", self.0)
    }
    fn baseline(&self) -> anyhow::Result<Vec<MediaDescription>> {
        Source(b"fixed").baseline()
    }
    fn current(&self, label: &str) -> anyhow::Result<Box<dyn ContentReader + '_>> {
        Source(b"fixed").current(label)
    }
}

#[test]
fn core_failure_and_report_mismatch_remove_temps_without_publishing() {
    for behavior in [Behavior::ObserveFailure, Behavior::ObserveMismatch] {
        let temp = Temp::new();
        let source = DirectorySource(temp.0.to_str().unwrap().as_bytes());
        assert!(export(
            &Changed(behavior),
            "scripted",
            &source,
            "raw",
            Some("Media"),
            &mut temp.sink()
        )
        .is_err());
        assert!(temp.names().is_empty());
    }
}

#[test]
fn temp_name_collision_preserves_the_existing_entry() {
    let temp = Temp::new();
    let name = format!(".slot-0.raw.unemu-export-{}-0.part", std::process::id());
    fs::write(temp.0.join(&name), b"earlier temp").unwrap();
    let mut sink = temp.sink();
    let output = sink.create("slot-0.raw").unwrap();
    output.write(b"abc").unwrap();
    output.finish().unwrap();
    assert!(!temp.0.join("slot-0.raw").exists());
    sink.cleanup().unwrap();
    assert_eq!(temp.names().as_slice(), std::slice::from_ref(&name));
    assert_eq!(fs::read(temp.0.join(name)).unwrap(), b"earlier temp");
}

#[cfg(unix)]
#[test]
fn directory_swap_during_export_cannot_redirect_creation_or_publication() {
    use std::os::unix::fs::symlink;
    let temp = Temp::new();
    let other = Temp::new();
    let directory = temp.0.join("out");
    let held = temp.0.join("held");
    fs::create_dir(&directory).unwrap();
    let mut swap = |_| {
        fs::rename(&directory, &held)?;
        symlink(&other.0, &directory)?;
        Ok(())
    };
    let mut sink = FileSink::new(&directory).unwrap();
    sink.write_hook(&mut swap);
    export(
        &ScriptedCore,
        "scripted",
        &Source(b"fixed"),
        "machine",
        None,
        &mut sink,
    )
    .unwrap();
    assert!(other.names().is_empty());
    assert_eq!(fs::read(held.join("slot-0.raw")).unwrap(), b"abc");
    assert!(held.join("machine.txt").is_file());
    assert_eq!(fs::read_dir(&held).unwrap().count(), 2);
}

#[cfg(unix)]
#[test]
fn directory_swap_cannot_redirect_cleanup_after_core_or_report_failure() {
    use std::os::unix::fs::symlink;
    for behavior in [Behavior::Failure(ExportOperation::Write), Behavior::Hash] {
        let temp = Temp::new();
        let other = Temp::new();
        let directory = temp.0.join("out");
        let held = temp.0.join("held");
        fs::create_dir(&directory).unwrap();
        let mut swap = |_| {
            fs::rename(&directory, &held)?;
            symlink(&other.0, &directory)?;
            Ok(())
        };
        let mut sink = FileSink::new(&directory).unwrap();
        sink.write_hook(&mut swap);
        assert!(export(
            &Changed(behavior),
            "scripted",
            &Source(b"fixed"),
            "raw",
            Some("Media"),
            &mut sink
        )
        .is_err());
        assert!(other.names().is_empty());
        assert_eq!(fs::read_dir(&held).unwrap().count(), 0);
    }
}

#[cfg(unix)]
#[test]
fn destination_resolving_into_store_is_rejected_before_creating_entries() {
    let temp = Temp::new();
    let store = Store::open(temp.0.join("store")).unwrap();
    let alias = temp.0.join("alias");
    std::os::unix::fs::symlink(store.root(), &alias).unwrap();
    for path in [store.root().to_path_buf(), alias.join("new-directory")] {
        let error = destination(&path, &store).unwrap_err();
        assert!(
            error.to_string().contains("outside the read-only store"),
            "{error:#}"
        );
    }
    assert!(!store.root().join("new-directory").exists());
}

#[cfg(unix)]
#[test]
fn replaced_temp_is_neither_published_nor_removed() {
    use std::os::unix::fs::symlink;
    for replacement_is_symlink in [false, true] {
        let temp = Temp::new();
        let mut replaced = None;
        let mut swap = |_| {
            let name = temp.names().pop().unwrap();
            let path = temp.0.join(&name);
            fs::remove_file(&path)?;
            if replacement_is_symlink {
                symlink("keep", &path)?;
            } else {
                fs::write(&path, b"other party")?;
            }
            replaced = Some(name);
            Ok(())
        };
        let mut sink = temp.sink();
        sink.write_hook(&mut swap);
        let result = export(
            &ScriptedCore,
            "scripted",
            &Source(b"fixed"),
            "raw",
            Some("Media"),
            &mut sink,
        );
        drop(sink);
        let name = replaced.unwrap();
        assert!(temp.0.join("slot-0.raw").symlink_metadata().is_err());
        assert_eq!(temp.names().as_slice(), std::slice::from_ref(&name));
        if replacement_is_symlink {
            assert_eq!(
                fs::read_link(temp.0.join(&name)).unwrap(),
                Path::new("keep")
            );
        } else {
            assert_eq!(fs::read(temp.0.join(&name)).unwrap(), b"other party");
        }
        let error = result.unwrap_err();
        assert!(format!("{error:#}").contains(&name), "{error:#}");
    }
}

#[cfg(unix)]
#[test]
fn opened_destination_reaching_store_through_intermediate_symlink_is_refused() {
    use std::os::unix::fs::symlink;
    let temp = Temp::new();
    let store = Store::open(temp.0.join("store")).unwrap();
    let parent = temp.0.join("parent");
    fs::create_dir(&parent).unwrap();
    fs::create_dir(store.root().join("output")).unwrap();
    let directory = destination(&parent.join("output"), &store).unwrap();
    fs::rename(&parent, temp.0.join("previous-parent")).unwrap();
    symlink(store.root(), &parent).unwrap();
    let sink = FileSink::new(&directory).unwrap();
    let error = sink.refuse_store(store.root()).unwrap_err();
    assert!(
        error.to_string().contains("outside the read-only store"),
        "{error:#}"
    );
    assert_eq!(
        fs::read_dir(store.root().join("output")).unwrap().count(),
        0
    );
}

#[cfg(unix)]
#[test]
fn opened_destination_remains_outside_store_after_its_path_is_redirected() {
    use std::os::unix::fs::symlink;
    let temp = Temp::new();
    let store = Store::open(temp.0.join("store")).unwrap();
    let parent = temp.0.join("parent");
    fs::create_dir(&parent).unwrap();
    let directory = destination(&parent.join("output"), &store).unwrap();
    let mut sink = FileSink::new(&directory).unwrap();
    let held = temp.0.join("held");
    fs::rename(&parent, &held).unwrap();
    symlink(store.root(), &parent).unwrap();
    sink.refuse_store(store.root()).unwrap();
    export(
        &ScriptedCore,
        "scripted",
        &Source(b"fixed"),
        "raw",
        Some("Media"),
        &mut sink,
    )
    .unwrap();
    assert_eq!(fs::read(held.join("output/slot-0.raw")).unwrap(), b"abc");
    assert!(!store.root().join("output").exists());
}
