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
#[derive(Clone, Debug, PartialEq, Eq)]
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

    pub(super) fn accepts_size(&self, length: u64) -> bool {
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
    control: Option<&'a DiscoveryControl>,
    warnings: &'a mut Vec<String>,
    visited: &'a mut BTreeSet<PathBuf>,
    files: &'a mut Vec<ListedFile>,
    listed_directories: &'a mut BTreeSet<PathBuf>,
}

impl FrontendScan<'_> {
    fn warn(&mut self, path: &Path, reason: impl std::fmt::Display) {
        let text = format!("{}: skipped ({reason})", self.root.name(path));
        if let Some(control) = self.control {
            control.result(text.clone());
        }
        self.warnings.push(text);
    }

    fn visit(&mut self, path: &Path) {
        if self.control.is_some_and(DiscoveryControl::is_cancelled) {
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
        if !self.visited.insert(canonical.clone()) {
            return;
        }
        if metadata.is_dir() {
            let children = match fs::read_dir(path) {
                Ok(children) => children,
                Err(error) => {
                    self.warn(path, error.kind());
                    return;
                }
            };
            let mut paths = Vec::new();
            let mut complete = true;
            for child in children {
                if self.control.is_some_and(DiscoveryControl::is_cancelled) {
                    return;
                }
                match child {
                    Ok(child) => paths.push(child.path()),
                    Err(error) => {
                        complete = false;
                        self.warn(path, error.kind());
                    }
                }
            }
            if complete {
                self.listed_directories.insert(canonical);
            }
            paths.sort();
            for child in paths {
                self.visit(&child);
            }
        } else if metadata.is_file() {
            if let Some(control) = self.control {
                control.update(|progress| progress.listed += 1);
            }
            self.files
                .push((path.into(), canonical, metadata.len(), self.root.name(path)));
        }
    }
}

fn collect_candidates(
    roots: &[FirmwareRoot],
    pins: &[&FirmwarePin],
    warnings: &mut Vec<String>,
    cache: &mut HashCache,
) -> Vec<Candidate> {
    if cache.control.is_none() {
        cache.listing = None;
        cache.candidates.clear();
        cache.checked_candidates.clear();
        cache.matched_candidates.clear();
        cache.reported_mismatches.clear();
    }
    let union = if cache.pins.is_empty() {
        pins.iter().map(|pin| (*pin).clone()).collect::<Vec<_>>()
    } else {
        cache.pins.clone()
    };
    if cache
        .listing
        .as_ref()
        .is_none_or(|(listed_roots, _)| listed_roots != roots)
    {
        let mut files = Vec::new();
        let mut visited = BTreeSet::new();
        if let Some(control) = &cache.control {
            control.update(|progress| progress.phase = DiscoveryPhase::Listing);
        }
        for root in roots {
            if cache
                .control
                .as_ref()
                .is_some_and(DiscoveryControl::is_cancelled)
            {
                break;
            }
            let canonical_root = match root.path.canonicalize() {
                Ok(path) if path.is_dir() => path,
                result => {
                    let reason = match result {
                        Ok(_) => "not a directory".into(),
                        Err(error) => {
                            if root.origin == RootOrigin::Vendor
                                && error.kind() == io::ErrorKind::NotFound
                            {
                                continue;
                            }
                            error.kind().to_string()
                        }
                    };
                    let warning = format!("{}: skipped ({reason})", root.origin.label());
                    if let Some(control) = &cache.control {
                        control.result(warning.clone());
                    }
                    warnings.push(warning);
                    continue;
                }
            };
            FrontendScan {
                root: SearchRoot {
                    path: root.path.clone(),
                    label: root.origin.label(),
                    external: false,
                },
                canonical_root,
                control: cache.control.as_ref(),
                warnings,
                visited: &mut visited,
                files: &mut files,
                listed_directories: &mut cache.listed_directories,
            }
            .visit(&root.path);
        }
        cache.seen.extend(visited);
        cache.listing = Some((roots.to_vec(), files));
    }
    let files = cache
        .listing
        .as_ref()
        .map(|(_, files)| files.clone())
        .unwrap_or_default();
    if let Some(control) = &cache.control {
        control.update(|progress| {
            progress.phase = DiscoveryPhase::Hashing;
            progress.candidates += files
                .iter()
                .filter(|(_, canonical, size, _)| {
                    union.iter().any(|pin| pin.accepts_size(*size))
                        && !cache.checked_candidates.contains(canonical)
                })
                .count();
            progress.size_skipped = files
                .iter()
                .filter(|(_, _, size, _)| !union.iter().any(|pin| pin.accepts_size(*size)))
                .count();
        });
    }
    for (path, canonical, size, name) in files {
        if cache
            .control
            .as_ref()
            .is_some_and(DiscoveryControl::is_cancelled)
        {
            break;
        }
        if !union.iter().any(|pin| pin.accepts_size(size)) {
            continue;
        }
        if cache.checked_candidates.insert(canonical.clone()) {
            let result = cache.hash(&canonical);
            if let Some(control) = &cache.control {
                control.update(|progress| progress.checked += 1);
            }
            match result {
                Ok(hash) => {
                    cache.candidates.push(Candidate {
                        canonical: canonical.clone(),
                        path,
                        name: name.clone(),
                        size,
                        hash,
                    });
                }
                Err(error) => {
                    let reason = if error.kind() == io::ErrorKind::Other {
                        "file changed while hashing".into()
                    } else {
                        error.kind().to_string()
                    };
                    let warning = format!("{name}: skipped ({reason})");
                    if let Some(control) = &cache.control {
                        control.result(warning.clone());
                    }
                    warnings.push(warning);
                }
            }
        }
    }
    for file in &cache.candidates {
        let found = union
            .iter()
            .any(|pin| pin.accepts_size(file.size) && pin.sha256 == file.hash);
        if found && cache.matched_candidates.insert(file.canonical.clone()) {
            if let Some(control) = &cache.control {
                control.update(|progress| progress.matches += 1);
                control.found(format!("{}: found (SHA-256 match)", file.name));
            }
        } else if !found && cache.reported_mismatches.insert(file.canonical.clone()) {
            if let Some(control) = &cache.control {
                control.result(format!("{}: skipped (SHA-256 mismatch)", file.name));
            }
        }
    }
    cache.candidates.clone()
}

#[cfg(test)]
fn find_image(
    roots: &[FirmwareRoot],
    pin: &FirmwarePin,
    warnings: &mut Vec<String>,
    cache: &mut HashCache,
) -> Option<PathBuf> {
    collect_candidates(roots, &[pin], warnings, cache)
        .into_iter()
        .find(|file| file.hash == pin.sha256)
        .map(|file| file.path)
}

fn matches_cached(path: &Path, pin: &FirmwarePin, cache: &mut HashCache) -> bool {
    fs::metadata(path).is_ok_and(|metadata| metadata.is_file() && pin.accepts_size(metadata.len()))
        && cache.hash(path).is_ok_and(|hash| hash == pin.sha256)
}

fn declared_match(cache: &HashCache, path: &Path) {
    if let Some(control) = &cache.control {
        control.update(|progress| {
            progress.candidates += 1;
            progress.checked += 1;
            progress.matches += 1;
        });
        control.found(format!(
            "<declared ROM>/{}: found (declared path)",
            path.file_name().unwrap_or_default().to_string_lossy()
        ));
    }
}

fn resolve(
    roots: &[FirmwareRoot],
    image: Option<&Path>,
    assembled: Option<&FirmwarePin>,
    parts: &[PinnedPart],
    warnings: &mut Vec<String>,
    cache: &mut HashCache,
) -> Option<ImageSource> {
    if let (Some(path), Some(pin)) = (image, assembled) {
        if matches_cached(path, pin, cache) {
            declared_match(cache, path);
            return Some(ImageSource::File(path.into()));
        }
    }
    if !parts.is_empty() && parts_match(parts, assembled, cache) {
        for part in parts {
            declared_match(cache, &part.part.path);
        }
        return Some(ImageSource::Interleaved(
            parts.iter().map(|p| p.part.clone()).collect(),
        ));
    }
    let pins = assembled
        .into_iter()
        .chain(parts.iter().filter_map(|part| part.pin.as_ref()))
        .collect::<Vec<_>>();
    if pins.is_empty() {
        return None;
    }
    let candidates = collect_candidates(roots, &pins, warnings, cache);
    let mut resolved = parts.to_vec();
    for part in &mut resolved {
        if let Some(pin) = &part.pin {
            if matches_cached(&part.part.path, pin, cache) {
                continue;
            }
            if let Some(file) = candidates
                .iter()
                .find(|file| pin.accepts_size(file.size) && file.hash == pin.sha256)
            {
                part.part.path = file.path.clone();
            }
        }
    }
    if !resolved.is_empty() && parts_match(&resolved, assembled, cache) {
        return Some(ImageSource::Interleaved(
            resolved.into_iter().map(|p| p.part).collect(),
        ));
    }
    if let Some(pin) = assembled {
        if let Some(file) = candidates
            .iter()
            .find(|file| pin.accepts_size(file.size) && file.hash == pin.sha256)
        {
            return Some(ImageSource::File(file.path.clone()));
        }
    }
    None
}

/// Resolve all pins in one folder walk before assembly. Unusable roots and
/// entries are warnings; directory symlinks cannot leave the selected root.
/// A scoped discovery session enables persistence, progress and cancellation.
pub fn locate_firmware_with_roots(
    roots: &[FirmwareRoot],
    image: Option<&Path>,
    assembled: Option<&FirmwarePin>,
    parts: &[PinnedPart],
    warnings: &mut Vec<String>,
) -> Result<Option<ImageSource>, UserFirmwareError> {
    persistent::with_cache(|cache| {
        let found = resolve(roots, image, assembled, parts, warnings, cache);
        if cache
            .control
            .as_ref()
            .is_some_and(DiscoveryControl::is_cancelled)
        {
            return Err(UserFirmwareError::Cancelled);
        }
        Ok(found)
    })
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
    fn all_chip_pins_share_one_listing() {
        let temp = TempDir::new("single-firmware-walk");
        let mut parts = Vec::new();
        for index in 0..4 {
            let bytes = [index; 512];
            fs::write(temp.path().join(format!("chip-{index}")), bytes).unwrap();
            parts.push(PinnedPart {
                part: ImagePart {
                    path: temp.path().join("absent"),
                    lane: if index % 2 == 0 {
                        crate::Lane::Even
                    } else {
                        crate::Lane::Odd
                    },
                    offset: (index / 2) as u64 * 1024,
                },
                pin: Some(pin(&bytes, Some(512))),
            });
        }
        let control = DiscoveryControl::default();
        let assembled = [[0, 1].repeat(512), [2, 3].repeat(512)].concat();
        with_discovery_session(None, false, &control, || {
            assert!(matches!(
                locate_firmware_with_roots(
                    &roots(&[temp.path()]),
                    None,
                    Some(&pin(&assembled, Some(2048))),
                    &parts,
                    &mut Vec::new(),
                )
                .unwrap(),
                Some(ImageSource::Interleaved(_))
            ));
        });
        assert_eq!(control.snapshot().listed, 4);
        assert_eq!(
            control.snapshot().files_read,
            if cfg!(windows) { 8 } else { 4 }
        );
        assert_eq!(control.snapshot().candidates, 4);
    }

    #[test]
    fn declared_oversized_images_and_parts_are_not_read() {
        let temp = TempDir::new("bounded-declared-firmware");
        let large = temp.path().join("oversized");
        fs::File::create(&large)
            .unwrap()
            .set_len(2 * 1024 * 1024)
            .unwrap();
        let oversized = FirmwarePin {
            sha256: format!("{:x}", Sha256::digest(vec![0; 2 * 1024 * 1024])),
            length: Some(2 * 1024 * 1024),
        };
        assert!(!matches_file(&large, &oversized).unwrap());
        let mut cache = HashCache::default();
        assert!(resolve(
            &[],
            Some(&large),
            Some(&oversized),
            &[],
            &mut Vec::new(),
            &mut cache
        )
        .is_none());
        assert_eq!(cache.calls, 0);
        let parts = [PinnedPart {
            part: ImagePart {
                path: large,
                lane: crate::Lane::Even,
                offset: 0,
            },
            pin: Some(oversized),
        }];
        assert!(resolve(&[], None, None, &parts, &mut Vec::new(), &mut cache).is_none());
        assert_eq!(cache.calls, 0);
    }

    #[test]
    fn session_reuses_one_listing_and_keeps_found_lines_after_size_skips() {
        let temp = TempDir::new("firmware-session-listing");
        for index in 0..300 {
            fs::write(temp.path().join(format!("a-skip-{index}")), [0; 3]).unwrap();
        }
        fs::write(temp.path().join("z-first"), [1; 512]).unwrap();
        fs::write(temp.path().join("z-second"), [2; 1024]).unwrap();
        let pins = [pin(&[1; 512], Some(512)), pin(&[2; 1024], Some(1024))];
        let control = DiscoveryControl::default();
        with_discovery_session(None, false, &control, || {
            with_firmware_pins(&pins, || {
                for expected in &pins {
                    assert!(locate_firmware_with_roots(
                        &roots(&[temp.path()]),
                        None,
                        Some(expected),
                        &[],
                        &mut Vec::new()
                    )
                    .unwrap()
                    .is_some());
                    assert_eq!(control.snapshot().listed, 302);
                    assert_eq!(control.snapshot().candidates, 2);
                    assert_eq!(control.snapshot().checked, 2);
                    assert_eq!(control.snapshot().phase, DiscoveryPhase::Hashing);
                }
            })
        });
        let progress = control.snapshot();
        assert_eq!(progress.matches, 2);
        assert_eq!(progress.size_skipped, 300);
        assert_eq!(
            progress
                .results
                .iter()
                .filter(|line| line.contains("size is not a candidate"))
                .count(),
            1
        );
        assert_eq!(
            progress
                .results
                .iter()
                .filter(|line| line.contains(": found ("))
                .count(),
            2
        );
        let control = DiscoveryControl::default();
        for index in 0..300 {
            control.result(format!("skip {index}"));
        }
        for index in 0..300 {
            control.found(format!("match {index}"));
        }
        assert_eq!(control.snapshot().results.len(), 556);
    }

    #[test]
    fn unpinned_odd_sized_declared_parts_do_not_walk_folders() {
        let temp = TempDir::new("odd-declared-parts");
        let mut parts = Vec::new();
        for lane in [crate::Lane::Even, crate::Lane::Odd] {
            let path = temp.path().join(format!("{lane:?}"));
            fs::write(&path, [1; 3]).unwrap();
            parts.push(PinnedPart {
                part: ImagePart {
                    path,
                    lane,
                    offset: 0,
                },
                pin: None,
            });
        }
        let control = DiscoveryControl::default();
        with_discovery_session(None, false, &control, || {
            assert!(locate_firmware_with_roots(
                &roots(&[temp.path()]),
                None,
                None,
                &parts,
                &mut Vec::new()
            )
            .unwrap()
            .is_some());
        });
        assert_eq!(control.snapshot().listed, 0);
    }

    #[cfg(all(unix, not(windows)))]
    #[test]
    fn failed_subdirectory_listing_keeps_its_cached_children() {
        use std::os::unix::fs::PermissionsExt;
        let temp = TempDir::new("firmware-failed-listing");
        let root = temp.path().join("roms");
        let locked = root.join("locked");
        fs::create_dir_all(&locked).unwrap();
        let file = locked.join("rom");
        fs::write(&file, [1; 512]).unwrap();
        let path = temp.path().join("index");
        with_discovery_session(Some(&path), false, &DiscoveryControl::default(), || {
            persistent::with_cache(|cache| {
                cache.hash_time =
                    Some(std::time::SystemTime::now() + std::time::Duration::from_secs(10));
                cache.hash(&file).unwrap();
                Ok(())
            })
            .unwrap();
        });
        let before = fs::read(&path).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o0)).unwrap();
        let mut warnings = Vec::new();
        with_discovery_session(Some(&path), false, &DiscoveryControl::default(), || {
            assert!(locate_firmware_with_roots(
                &roots(&[&root]),
                None,
                Some(&pin(&[1; 512], Some(512))),
                &[],
                &mut warnings
            )
            .unwrap()
            .is_none());
        });
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(!warnings.is_empty());
        assert_eq!(fs::read(&path).unwrap(), before);
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
