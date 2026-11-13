// SPDX-License-Identifier: AGPL-3.0-or-later
//! Bounded, best-effort host scans; firmware-and-machine-classes.md §3.5.

use super::*;

/// The host setting that supplied a search directory, used in warnings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RootOrigin {
    Preferences,
    CommandLine,
    Environment,
    Vendor,
}

impl RootOrigin {
    /// Stable warning label without a private absolute path.
    pub fn label(self) -> &'static str {
        match self {
            Self::Preferences => "<preferences ROM folder>",
            Self::CommandLine => "<--rom-dir>",
            Self::Environment => "<UNEMUPC98_USER_ROMS_DIR>",
            Self::Vendor => "vendor/user-roms",
        }
    }
}

/// An explicit host directory with the origin needed to explain skipped entries.
#[derive(Clone, Debug)]
pub struct FirmwareRoot {
    pub path: PathBuf,
    pub origin: RootOrigin,
}

impl FirmwareRoot {
    pub fn new(path: impl Into<PathBuf>, origin: RootOrigin) -> Self {
        Self {
            path: path.into(),
            origin,
        }
    }
}

/// Environment/vendor defaults for the GUI and legacy required-ROM test lane.
/// The command-line frontend deliberately uses only explicitly supplied roots.
pub fn default_firmware_roots() -> Vec<FirmwareRoot> {
    let mut roots = vec![FirmwareRoot::new(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vendor/user-roms"),
        RootOrigin::Vendor,
    )];
    if let Some(path) = std::env::var_os("UNEMUPC98_USER_ROMS_DIR") {
        roots.push(FirmwareRoot::new(
            PathBuf::from(path),
            RootOrigin::Environment,
        ));
    }
    roots
}

impl FirmwarePin {
    /// Resolve exact container/chip lengths from the catalogue. An explicit
    /// declared length is used for identities outside that catalogue.
    pub fn from_catalogue(
        sha256: &str,
        catalogue: &[FirmwareEntry],
        declared_length: Option<u64>,
    ) -> Self {
        let length = catalogue
            .iter()
            .find_map(|entry| {
                if entry.sha256 == sha256 {
                    return Some(entry.size);
                }
                entry
                    .parts
                    .iter()
                    .flat_map(|layout| layout.iter())
                    .find(|part| part.sha256 == sha256)
                    .map(|part| part.size)
            })
            .or(declared_length);
        Self {
            sha256: sha256.into(),
            length,
        }
    }

    fn accepts_size(&self, length: u64) -> bool {
        if length > 1024 * 1024 {
            return false;
        }
        if let Some(exact) = self.length {
            return length == exact;
        }
        // Unknown identities must not turn a mistakenly chosen home directory
        // into an unrestricted full-file hasher. Keep block-sized ROM containers eligible.
        (512..=1024 * 1024).contains(&length) && length.is_multiple_of(512)
    }
}

struct FrontendScan<'a> {
    root: SearchRoot,
    canonical_root: PathBuf,
    pin: &'a FirmwarePin,
    cache: &'a mut HashCache,
    warnings: &'a mut Vec<String>,
    visited: BTreeSet<PathBuf>,
    found: Option<PathBuf>,
}

impl FrontendScan<'_> {
    fn warn(&mut self, path: &Path, reason: impl std::fmt::Display) {
        self.warnings
            .push(format!("{}: skipped ({reason})", self.root.name(path)));
    }

    fn visit(&mut self, path: &Path) {
        if self.found.is_some() {
            return;
        }
        let canonical = match path.canonicalize() {
            Ok(path) => path,
            Err(error) => {
                self.warn(path, error.kind());
                return;
            }
        };
        if !canonical.starts_with(&self.canonical_root) {
            self.warn(path, "symlink leaves search root");
            return;
        }
        let metadata = match fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(error) => {
                self.warn(path, error.kind());
                return;
            }
        };
        if metadata.is_dir() {
            if !self.visited.insert(canonical) {
                return;
            }
            let children = match fs::read_dir(path) {
                Ok(children) => children,
                Err(error) => {
                    self.warn(path, error.kind());
                    return;
                }
            };
            let mut paths = Vec::new();
            for child in children {
                match child {
                    Ok(child) => paths.push(child.path()),
                    Err(error) => self.warn(path, error.kind()),
                }
            }
            paths.sort();
            for child in paths {
                self.visit(&child);
            }
        } else if metadata.is_file() && self.pin.accepts_size(metadata.len()) {
            match self.cache.hash(&canonical) {
                Ok(hash) if hash == self.pin.sha256 => self.found = Some(path.into()),
                Ok(_) => {}
                Err(error) => self.warn(
                    path,
                    if error.kind() == io::ErrorKind::Other {
                        "file changed while hashing".into()
                    } else {
                        error.kind().to_string()
                    },
                ),
            }
        }
    }
}

fn find_image(
    roots: &[FirmwareRoot],
    pin: &FirmwarePin,
    warnings: &mut Vec<String>,
    cache: &mut HashCache,
) -> Option<PathBuf> {
    for root in roots {
        let canonical_root = match root.path.canonicalize() {
            Ok(path) if path.is_dir() => path,
            Ok(_) => {
                warnings.push(format!(
                    "{}: skipped (not a directory)",
                    root.origin.label()
                ));
                continue;
            }
            Err(error) => {
                // The optional vendor directory is normally absent in a checkout.
                if root.origin != RootOrigin::Vendor || error.kind() != io::ErrorKind::NotFound {
                    warnings.push(format!(
                        "{}: skipped ({})",
                        root.origin.label(),
                        error.kind()
                    ));
                }
                continue;
            }
        };
        let mut scan = FrontendScan {
            root: SearchRoot {
                path: root.path.clone(),
                label: root.origin.label(),
                external: false,
            },
            canonical_root,
            pin,
            cache,
            warnings,
            visited: BTreeSet::new(),
            found: None,
        };
        scan.visit(&root.path);
        if scan.found.is_some() {
            return scan.found;
        }
    }
    None
}

/// Resolve pinned firmware before assembly. Unusable roots/entries are warnings;
/// they cannot prevent a later directory from supplying a valid image. Directory
/// symlinks are followed only while their target stays inside the selected root.
pub fn locate_firmware_with_roots(
    roots: &[FirmwareRoot],
    image: Option<&Path>,
    assembled: Option<&FirmwarePin>,
    parts: &[PinnedPart],
    warnings: &mut Vec<String>,
) -> Result<Option<ImageSource>, UserFirmwareError> {
    if let (Some(path), Some(pin)) = (image, assembled) {
        if matches_file(path, pin).unwrap_or(false) {
            return Ok(Some(ImageSource::File(path.into())));
        }
    }
    if !parts.is_empty() && parts_match(parts, assembled) {
        return Ok(Some(ImageSource::Interleaved(
            parts.iter().map(|p| p.part.clone()).collect(),
        )));
    }
    let mut cache = shared_cache()
        .lock()
        .map_err(|_| UserFirmwareError::CacheUnavailable)?;
    let mut resolved = parts.to_vec();
    for part in &mut resolved {
        if let Some(pin) = &part.pin {
            if cache
                .hash(&part.part.path)
                .is_ok_and(|hash| hash == pin.sha256)
            {
                continue;
            }
            if let Some(path) = find_image(roots, pin, warnings, &mut cache) {
                part.part.path = path;
            }
        }
    }
    // Layout verification uses the same cache, so release the scan lock first.
    drop(cache);
    if !resolved.is_empty() && parts_match(&resolved, assembled) {
        return Ok(Some(ImageSource::Interleaved(
            resolved.into_iter().map(|p| p.part).collect(),
        )));
    }
    if let Some(pin) = assembled {
        let mut cache = shared_cache()
            .lock()
            .map_err(|_| UserFirmwareError::CacheUnavailable)?;
        if let Some(path) = find_image(roots, pin, warnings, &mut cache) {
            return Ok(Some(ImageSource::File(path)));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    fn pin(bytes: &[u8], length: Option<u64>) -> FirmwarePin {
        FirmwarePin {
            sha256: format!("{:x}", Sha256::digest(bytes)),
            length,
        }
    }

    fn roots(paths: &[&Path]) -> Vec<FirmwareRoot> {
        paths
            .iter()
            .map(|path| FirmwareRoot::new(*path, RootOrigin::Preferences))
            .collect()
    }

    #[test]
    fn catalogue_pins_supply_assembled_and_chip_lengths() {
        let catalogue = crate::BoardModel::Ibm5170.firmware_catalogue();
        for entry in catalogue {
            assert_eq!(
                FirmwarePin::from_catalogue(entry.sha256, catalogue, None).length,
                Some(entry.size)
            );
            for part in entry.parts.iter().flat_map(|layout| layout.iter()) {
                assert_eq!(
                    FirmwarePin::from_catalogue(part.sha256, catalogue, None).length,
                    Some(part.size)
                );
            }
        }
        assert_eq!(
            FirmwarePin::from_catalogue("unknown", catalogue, Some(1234)).length,
            Some(1234)
        );
    }

    #[test]
    fn large_folder_hashes_only_rom_candidates_and_exact_lengths() {
        let temp = TempDir::new("bounded-firmware-scan");
        for index in 0..600 {
            fs::File::create(temp.path().join(format!("archive-{index:04}")))
                .unwrap()
                .set_len(1001 + index * 2)
                .unwrap();
        }
        fs::File::create(temp.path().join("large-archive"))
            .unwrap()
            .set_len(2 * 1024 * 1024)
            .unwrap();
        let bytes = vec![7; 32768];
        let expected = temp.path().join("rom");
        fs::write(&expected, &bytes).unwrap();
        let mut cache = HashCache::default();
        let mut warnings = Vec::new();
        let dirs = roots(&[temp.path()]);
        assert_eq!(
            find_image(&dirs, &pin(&bytes, None), &mut warnings, &mut cache),
            Some(expected.clone())
        );
        assert_eq!(cache.calls, 1);
        let mut cache = HashCache::default();
        assert!(find_image(&dirs, &pin(&bytes, Some(65536)), &mut warnings, &mut cache).is_none());
        assert_eq!(cache.calls, 0);
        assert!(warnings.is_empty());
        assert!(!pin(&bytes, Some(2 * 1024 * 1024)).accepts_size(2 * 1024 * 1024));
        assert!(pin(&bytes, None).accepts_size(24 * 1024));
    }

    #[test]
    fn unusable_roots_warn_with_their_origins_and_search_continues() {
        let temp = TempDir::new("firmware-root-warnings");
        let good = temp.path().join("good");
        fs::create_dir(&good).unwrap();
        let bytes = [1, 2, 3, 4];
        let file = good.join("rom");
        fs::write(&file, bytes).unwrap();
        let dirs = vec![
            FirmwareRoot::new(temp.path().join("unmounted"), RootOrigin::Preferences),
            FirmwareRoot::new(&file, RootOrigin::CommandLine),
            FirmwareRoot::new(temp.path().join("absent"), RootOrigin::Environment),
            FirmwareRoot::new(temp.path().join("optional"), RootOrigin::Vendor),
            FirmwareRoot::new(good, RootOrigin::Preferences),
        ];
        let mut warnings = Vec::new();
        assert_eq!(
            find_image(
                &dirs,
                &pin(&bytes, Some(4)),
                &mut warnings,
                &mut HashCache::default()
            ),
            Some(file)
        );
        assert_eq!(warnings.len(), 3);
        assert!(warnings[0].starts_with("<preferences ROM folder>"));
        assert!(warnings[1].starts_with("<--rom-dir>"));
        assert!(warnings[2].starts_with("<UNEMUPC98_USER_ROMS_DIR>"));
        assert!(warnings
            .iter()
            .all(|w| !w.contains(&temp.path().display().to_string())));
    }

    #[cfg(unix)]
    #[test]
    fn dangling_links_and_symlink_escapes_are_skipped() {
        use std::os::unix::fs::symlink;
        let temp = TempDir::new("firmware-symlink-boundary");
        let inside = temp.path().join("inside");
        let outside = temp.path().join("outside");
        fs::create_dir(&inside).unwrap();
        fs::create_dir(&outside).unwrap();
        let bytes = [1, 2, 3, 4];
        fs::write(outside.join("rom"), bytes).unwrap();
        symlink(outside.join("rom"), inside.join("file-link")).unwrap();
        symlink(&outside, inside.join("directory-link")).unwrap();
        symlink(inside.join("absent"), inside.join("dangling")).unwrap();
        let mut warnings = Vec::new();
        let mut cache = HashCache::default();
        assert!(find_image(
            &roots(&[&inside]),
            &pin(&bytes, Some(4)),
            &mut warnings,
            &mut cache
        )
        .is_none());
        assert_eq!(cache.calls, 0);
        assert_eq!(warnings.len(), 3);
        assert_eq!(
            warnings
                .iter()
                .filter(|w| w.contains("symlink leaves"))
                .count(),
            2
        );
        assert_eq!(
            find_image(
                &roots(&[&inside, &outside]),
                &pin(&bytes, Some(4)),
                &mut warnings,
                &mut cache
            ),
            Some(outside.join("rom"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_files_and_directories_warn_and_do_not_abort() {
        use std::os::unix::fs::PermissionsExt;
        let temp = TempDir::new("firmware-permission-warnings");
        let locked_dir = temp.path().join("a-locked-dir");
        let locked_file = temp.path().join("b-locked-file");
        let good = temp.path().join("z-good");
        fs::create_dir(&locked_dir).unwrap();
        let bytes = [1, 2, 3, 4];
        fs::write(&locked_file, bytes).unwrap();
        fs::write(&good, bytes).unwrap();
        fs::set_permissions(&locked_dir, fs::Permissions::from_mode(0o0)).unwrap();
        fs::set_permissions(&locked_file, fs::Permissions::from_mode(0o0)).unwrap();
        let mut warnings = Vec::new();
        let found = find_image(
            &roots(&[temp.path()]),
            &pin(&bytes, Some(4)),
            &mut warnings,
            &mut HashCache::default(),
        );
        fs::set_permissions(&locked_dir, fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(&locked_file, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(found, Some(good));
        assert_eq!(warnings.len(), 2);
    }

    #[test]
    fn files_changed_during_hashing_warn_and_search_continues() {
        let temp = TempDir::new("firmware-write-in-progress");
        let changing = temp.path().join("a-changing");
        let good = temp.path().join("z-good");
        let bytes = [1, 2, 3, 4];
        fs::write(&changing, bytes).unwrap();
        fs::write(&good, bytes).unwrap();
        let mut cache = HashCache {
            after_read: Some(|path| {
                if path.file_name().unwrap() == "a-changing" {
                    fs::write(path, [0]).unwrap();
                }
            }),
            ..HashCache::default()
        };
        let mut warnings = Vec::new();
        assert_eq!(
            find_image(
                &roots(&[temp.path()]),
                &pin(&bytes, Some(4)),
                &mut warnings,
                &mut cache
            ),
            Some(good)
        );
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("a-changing"));
        assert_eq!(cache.calls, 1);
    }
}
