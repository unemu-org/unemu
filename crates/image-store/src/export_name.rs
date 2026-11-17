// SPDX-License-Identifier: AGPL-3.0-or-later
//! Export file names of artifact layers; machine-images §2 and recipes.md
//! §3.10.
//!
//! Every layer of an installation set or a firmware package names the plain
//! file it unpacks to in the annotation `org.opencontainers.image.title`, a
//! pre-defined key of the OCI Image Format Specification (Annotations). ORAS
//! names the files it pulls by that annotation, so a generic OCI client
//! writes a set's disks and extras, or a package's ROM images, under their
//! own names. A layer stored as one Zstandard frame (a `+zstd` media type)
//! is titled with `.zst` after its file name, so plain `zstd -d` on the
//! pulled file gives back the original file under its own name. The writers
//! of sets and packages check the names here before anything is stored.
//! Readers treat the annotation as informational and never refuse an
//! artifact over it, so artifacts written before the names existed stay
//! readable.
//!
//! A name is one file name that macOS, Linux and Windows all accept as it is,
//! never a path. No two layers of one artifact occupy names that are equal
//! ignoring case, because the default file systems of macOS and Windows
//! ignore case: a compressed layer occupies both its title and its decoded
//! name. Case is compared character by character in upper case. Two
//! spellings that differ in Unicode normalization alone count as different,
//! although macOS treats them as one name. The names an export writes beside
//! the layers ([`RESERVED`]) are no input's name.

use crate::{invalid, media_type, Descriptor, Result};
use std::collections::{btree_map::Entry, BTreeMap};

/// The OCI pre-defined annotation that holds a layer's export file name.
pub const TITLE: &str = "org.opencontainers.image.title";
/// What a compressed layer's title adds to its file name.
pub const ZST: &str = ".zst";
/// The export file name of an installation set's metadata layer.
pub const METADATA: &str = "metadata.json";
/// The export file name of a firmware package's README layer.
pub const README: &str = "README.md";
/// The file an export writes the config to, as stored.
pub const CONFIG: &str = "config.toml";
/// The file an export writes the manifest to, as stored.
pub const MANIFEST: &str = "manifest.json";
/// The checksum list an export writes over every file it writes.
pub const SUMS: &str = "SHA256SUMS";
/// The file naming the version of the export layout.
pub const VERSION: &str = "EXPORT-VERSION";
/// The names an export writes beside the layers. No medium, ROM image,
/// source archive or extra takes one, ignoring case; only an installation
/// set's metadata layer is called [`METADATA`]. A firmware package's README
/// layer takes [`README`], so no other layer of a package can.
pub const RESERVED: [&str; 5] = [CONFIG, METADATA, MANIFEST, SUMS, VERSION];

/// Longest name in bytes of UTF-8: the limit of Linux file systems, and
/// within the 255 UTF-16 units of Windows and APFS.
const MAX_BYTES: usize = 255;

/// The key two names are compared by: each character in upper case, so
/// `ΟΔΟΣ`, `οδος` and `οδοσ` are one name, as on a file system that ignores
/// case.
pub fn key(name: &str) -> String {
    name.chars().flat_map(char::to_uppercase).collect()
}

/// Whether `name` is one of the names an export writes itself.
pub fn reserved(name: &str) -> bool {
    let name = key(name);
    RESERVED.iter().any(|reserved| key(reserved) == name)
}

/// The title `layer` carries, if any: the name a pull writes.
pub fn of(layer: &Descriptor) -> Option<&str> {
    layer.annotations.get(TITLE).map(String::as_str)
}

/// The file name an export writes `layer`'s decoded bytes under: its title,
/// without `.zst` on a compressed layer. `None` when the layer has no title,
/// or is compressed and its title does not end in `.zst`.
pub fn decoded(layer: &Descriptor) -> Option<&str> {
    let title = of(layer)?;
    if layer.is_compressed() {
        title.strip_suffix(ZST)
    } else {
        Some(title)
    }
}

/// The title of a layer whose decoded bytes are the file `name`: `name`,
/// with `.zst` after it when the layer is stored compressed.
pub fn title_for(layer: &Descriptor, name: &str) -> String {
    if layer.is_compressed() {
        format!("{name}{ZST}")
    } else {
        name.to_owned()
    }
}

/// Give `layer` the export file name `name` ([`title_for`]).
pub fn assign(layer: &mut Descriptor, name: &str) {
    let title = title_for(layer, name);
    layer.annotations.insert(TITLE.into(), title);
}

/// Why `name` is not one file name that macOS, Linux and Windows all accept
/// unchanged, or `None` when it is. Windows decides most rules (Microsoft,
/// "Naming Files, Paths, and Namespaces"): no `\ / : * ? " < >` or `|`, no
/// control character, no trailing space or dot, which Windows drops, and no
/// device name it reserves, with or without an extension.
pub fn problem(name: &str) -> Option<&'static str> {
    if name.is_empty() {
        return Some("is empty");
    }
    if name.len() > MAX_BYTES {
        return Some("is longer than 255 bytes");
    }
    if matches!(name, "." | "..") {
        return Some("names a directory");
    }
    if name.chars().any(|c| c <= '\u{1f}' || c == '\u{7f}') {
        return Some("contains a control character");
    }
    if name.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|']) {
        return Some("contains one of / \\ : * ? \" < > |");
    }
    if name.ends_with([' ', '.']) {
        return Some("ends with a space or a dot");
    }
    let stem = name.split('.').next().unwrap_or(name).trim_end_matches(' ');
    if device(stem) {
        return Some(
            "is a device name Windows reserves (CON, CONIN$, CONOUT$, PRN, AUX, NUL, COM0-9, \
             COM¹-³, LPT0-9, LPT¹-³)",
        );
    }
    None
}

/// Why `name` cannot be the export file name of an input (a medium, ROM
/// image, source archive or extra), or `None`: it must pass [`problem`] also
/// with `.zst` after it, since the input may be stored compressed, and it is
/// none of the [`RESERVED`] names.
pub fn input_problem(name: &str) -> Option<&'static str> {
    if let Some(problem) = problem(name) {
        return Some(problem);
    }
    if problem(&format!("{name}{ZST}")).is_some() {
        return Some("is longer than 251 bytes, which leaves no room for .zst");
    }
    if reserved(name) {
        return Some(
            "is a name the export writes itself (config.toml, metadata.json, manifest.json, \
             SHA256SUMS, EXPORT-VERSION)",
        );
    }
    None
}

/// CON, CONIN$, CONOUT$, PRN, AUX, NUL, and COM or LPT followed by one
/// digit, a superscript one, two or three included, in any case.
fn device(stem: &str) -> bool {
    let upper = stem.to_ascii_uppercase();
    if matches!(
        upper.as_str(),
        "CON" | "CONIN$" | "CONOUT$" | "PRN" | "AUX" | "NUL"
    ) {
        return true;
    }
    ["COM", "LPT"].iter().any(|prefix| {
        upper.strip_prefix(prefix).is_some_and(|rest| {
            let mut chars = rest.chars();
            matches!(
                (chars.next(), chars.next()),
                (Some('0'..='9' | '¹' | '²' | '³'), None)
            )
        })
    })
}

/// [`check_with`], naming each layer by its title or digest.
pub fn check(layers: &[Descriptor]) -> Result<()> {
    check_with(layers, &|index| {
        let layer = &layers[index];
        match of(layer) {
            Some(title) => format!("layer {title:?}"),
            None => format!("layer {}", layer.digest),
        }
    })
}

/// Check the layers of an artifact before it is written: every layer
/// carries a title, a compressed layer's ends in `.zst`, its title and
/// decoded name pass [`problem`], no layer but a set's metadata takes a
/// [`RESERVED`] name, and no two layers occupy names that are equal ignoring
/// case ([`key`]). The refusal names the input of each layer it is about by
/// `input`, given the layer's position.
pub fn check_with(layers: &[Descriptor], input: &dyn Fn(usize) -> String) -> Result<()> {
    let mut seen = BTreeMap::<String, usize>::new();
    for (index, layer) in layers.iter().enumerate() {
        let title = of(layer)
            .ok_or_else(|| invalid(format!("{} has no export file name", input(index))))?;
        let name = decoded(layer).ok_or_else(|| {
            invalid(format!(
                "{}: the export file name {title:?} of a compressed layer must end in .zst",
                input(index)
            ))
        })?;
        for name in [title, name] {
            if let Some(problem) = problem(name) {
                return Err(invalid(format!(
                    "{}: export file name {name:?} {problem}; rename the file",
                    input(index)
                )));
            }
        }
        if reserved(name)
            && !(key(name) == key(METADATA) && layer.media_type == media_type::METADATA)
        {
            return Err(invalid(format!(
                "{}: export file name {name:?} is a name the export writes itself; rename the file",
                input(index)
            )));
        }
        for occupied in [title, name] {
            match seen.entry(key(occupied)) {
                Entry::Vacant(entry) => {
                    entry.insert(index);
                }
                Entry::Occupied(entry) if *entry.get() != index => {
                    return Err(invalid(format!(
                        "{} and {} would both be exported as {occupied:?}, ignoring case; \
                         rename one",
                        input(*entry.get()),
                        input(index)
                    )));
                }
                Entry::Occupied(_) => {}
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{codec, Digest};

    #[test]
    fn a_name_is_one_file_name_every_desktop_system_accepts() {
        let longest = format!("{}.img", "a".repeat(MAX_BYTES - 4));
        for name in [
            "x86BOOT.img",
            "freedos-1.4-floppy-src.tar",
            "README.md",
            "Disk 1 (boot) — installer.img",
            ".profile",
            " leading space.txt",
            "COM10.txt",
            "CONSOLE.txt",
            "CONIN.txt",
            "con-1.img",
            "LPT",
            longest.as_str(),
        ] {
            assert_eq!(problem(name), None, "{name}");
        }
        let too_long = format!("{}é.img", "a".repeat(MAX_BYTES - 5));
        for (name, reason) in [
            ("", "is empty"),
            (".", "names a directory"),
            ("..", "names a directory"),
            ("disk\n1.img", "contains a control character"),
            ("disk\u{7f}.img", "contains a control character"),
            ("tab\t.img", "contains a control character"),
            ("disks/1.img", "contains one of"),
            ("disks\\1.img", "contains one of"),
            ("Disk 1: Setup.img", "contains one of"),
            ("disk*.img", "contains one of"),
            ("disk?.img", "contains one of"),
            ("\"disk\".img", "contains one of"),
            ("<disk>.img", "contains one of"),
            ("disk|1.img", "contains one of"),
            ("disk.", "ends with a space or a dot"),
            ("disk.img ", "ends with a space or a dot"),
            ("CON", "device name"),
            ("con.txt", "device name"),
            ("conin$.txt", "device name"),
            ("CONOUT$", "device name"),
            ("Nul.tar.gz", "device name"),
            ("aux .txt", "device name"),
            ("COM1", "device name"),
            ("com0.bin", "device name"),
            ("lpt9.bin", "device name"),
            ("COM¹.txt", "device name"),
            ("lpt³.bin", "device name"),
            ("PRN.img", "device name"),
            (too_long.as_str(), "longer than 255 bytes"),
        ] {
            assert!(
                problem(name).is_some_and(|p| p.contains(reason)),
                "{name:?}: {:?}",
                problem(name)
            );
        }
    }

    #[test]
    fn an_input_leaves_room_for_zst_and_never_takes_a_name_the_export_writes() {
        let longest = format!("{}.img", "a".repeat(MAX_BYTES - 8));
        assert_eq!(input_problem(&longest), None);
        let longer = format!("{}.img", "a".repeat(MAX_BYTES - 7));
        assert_eq!(problem(&longer), None);
        assert!(input_problem(&longer).is_some_and(|p| p.contains("no room for .zst")));
        for name in [
            "config.toml",
            "CONFIG.TOML",
            "Metadata.json",
            "manifest.json",
            "sha256sums",
            "EXPORT-VERSION",
        ] {
            assert!(reserved(name), "{name}");
            assert!(
                input_problem(name).is_some_and(|p| p.contains("the export writes itself")),
                "{name}"
            );
        }
        for name in ["README.md", "config.toml.zst", "metadata.json.bak", "SUMS"] {
            assert!(!reserved(name), "{name}");
        }
    }

    #[test]
    fn case_is_compared_character_by_character_in_upper_case() {
        assert_eq!(key("disk.img"), key("DISK.IMG"));
        assert_eq!(key("ΟΔΟΣ"), key("οδος"));
        assert_eq!(key("οδοσ"), key("οδος"));
        assert_ne!(key("disk1.img"), key("disk2.img"));
    }

    fn layer(bytes: &[u8], name: Option<&str>) -> Descriptor {
        let mut d = Descriptor::new(media_type::EXTRA, Digest::of(bytes), bytes.len() as u64);
        if let Some(name) = name {
            assign(&mut d, name);
        }
        d
    }

    fn compressed(bytes: &[u8], name: &str) -> Descriptor {
        let mut d = Descriptor::new(
            codec::compressed_type(media_type::EXTRA),
            Digest::of(b"frame"),
            5,
        );
        d.annotations
            .insert(codec::DECODED_DIGEST.into(), Digest::of(bytes).to_string());
        d.annotations
            .insert(codec::DECODED_SIZE.into(), bytes.len().to_string());
        assign(&mut d, name);
        d
    }

    #[test]
    fn a_compressed_layer_is_titled_with_zst_after_its_file_name() {
        let raw = layer(b"raw", Some("LICENSES.TXT"));
        assert_eq!(of(&raw), Some("LICENSES.TXT"));
        assert_eq!(decoded(&raw), Some("LICENSES.TXT"));
        let frame = compressed(b"disk", "x86BOOT.img");
        assert_eq!(of(&frame), Some("x86BOOT.img.zst"));
        assert_eq!(frame.annotations[TITLE], "x86BOOT.img.zst");
        assert_eq!(decoded(&frame), Some("x86BOOT.img"));
        assert_eq!(title_for(&frame, "a.img"), "a.img.zst");
        let mut unmarked = frame.clone();
        unmarked
            .annotations
            .insert(TITLE.into(), "x86BOOT.img".into());
        assert_eq!(decoded(&unmarked), None);
        let error = check(&[unmarked]).unwrap_err().to_string();
        assert!(error.contains("must end in .zst"), "{error}");
    }

    #[test]
    fn every_layer_has_a_valid_name_unique_ignoring_case() {
        let mut metadata = layer(b"three", Some(METADATA));
        metadata.media_type = media_type::METADATA.into();
        let named = [
            layer(b"one", Some("DISK1.IMG")),
            compressed(b"two", "DISK2.IMG"),
            metadata,
        ];
        check(&named).unwrap();

        let unnamed = [layer(b"one", Some("DISK1.IMG")), layer(b"two", None)];
        let error = check(&unnamed).unwrap_err().to_string();
        assert!(error.contains("has no export file name"), "{error}");

        for (first, second) in [
            (
                layer(b"one", Some("DISK.IMG")),
                layer(b"two", Some("DISK.IMG")),
            ),
            (
                layer(b"one", Some("disk.img")),
                layer(b"two", Some("DISK.IMG")),
            ),
            (
                layer(b"one", Some("ΟΔΟΣ.TXT")),
                layer(b"two", Some("οδος.txt")),
            ),
            // A pulled `a.img.zst` decodes to `a.img`, and a raw layer may
            // already be called either.
            (layer(b"one", Some("a.img")), compressed(b"two", "A.IMG")),
            (
                layer(b"one", Some("a.img.zst")),
                compressed(b"two", "a.img"),
            ),
        ] {
            let error = check(&[first, second]).unwrap_err().to_string();
            assert!(error.contains("would both be exported as"), "{error}");
        }
        let error = check_with(
            &[layer(b"one", Some("a.img")), layer(b"two", Some("A.img"))],
            &|index| format!("input {index}"),
        )
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("input 0 and input 1 would both be exported as \"A.img\"")
                && error.contains("rename one"),
            "{error}"
        );

        let error = check(&[layer(b"one", Some("a/b.img"))])
            .unwrap_err()
            .to_string();
        assert!(error.contains("\"a/b.img\""), "{error}");
        for name in [METADATA, "SHA256SUMS", "config.toml"] {
            let error = check(&[layer(b"one", Some(name))]).unwrap_err().to_string();
            assert!(error.contains("the export writes itself"), "{error}");
        }
    }
}
