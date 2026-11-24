// SPDX-License-Identifier: AGPL-3.0-or-later
//! The `ibm-pc` publisher of recipe builds (recipes §6.14): a built
//! artifact goes through the same plan-and-publish functions the importers
//! end in, never a writer of its own.
//!
//! - A Mediafile's set: [`crate::media_import::plan_with`] with the media in
//!   installer order, their labels, formats and export file names, the
//!   extras with their DOS names and kinds, the merged title record and set
//!   labels unchanged, and the open statement of `[license]`; then the
//!   recipe's medium ids ([`crate::media_import::Plan::name_media`]) and
//!   minimum release; then [`crate::media_import::publish`], which copies
//!   and hashes every file and tags last.
//! - A Firmwarefile's package: [`crate::firmware_import::scan`] identifies
//!   the ROM by its bytes in the firmware catalogue, exactly as `image
//!   firmware import` does; `[package]` is the statement of the metadata
//!   flags, the `[license]` source the `--source` archive, other extras the
//!   attached documents; then [`crate::firmware_import::publish_as`] under
//!   the recipe's tag. Without `[license]` the package is user-supplied,
//!   as an import that states nothing open leaves it (recipes §6.6).
//! - A pinned recipe's locked manifest, which the recipe crate has stored
//!   unchanged after its payloads matched the lock, is tagged with the
//!   validator of its artifact type ([`PcPublisher::tag_locked`]); no
//!   importer writes anything for it.

use crate::firmware_import::{self, Attachment, Statement};
use crate::media_import::{self, ExtraSource, OpenStatement, Outcome, Request, Source};
use image_store::compat::AppVersion;
use image_store::media_set::ExtraKind;
use image_store::{Descriptor, Store};
use recipe::build::{Published, Publisher, Staged};
use recipe::model::Kind;
use std::path::PathBuf;

/// Publishes `ibm-pc` recipe builds through the importers.
pub struct PcPublisher;

impl Publisher for PcPublisher {
    fn publish(&self, store: &Store, artifact: &Staged) -> Result<Published, String> {
        match artifact.kind {
            Kind::Media => media(store, artifact),
            Kind::Firmware => firmware(store, artifact),
            Kind::Machine => Err("machine recipes are not built yet".into()),
        }
    }

    /// The tag of a pinned build, written with the validator `image media
    /// import` or `image firmware import` writes its tag with.
    fn tag_locked(
        &self,
        store: &Store,
        reference: &str,
        manifest: &Descriptor,
    ) -> Result<Published, String> {
        use image_store::BlobSource as _;
        let bytes = store
            .read_verified(manifest, image_store::MAX_SMALL_BLOB)
            .map_err(|e| e.to_string())?;
        let kind = image_store::Manifest::decode_for_summary(&bytes)
            .map_err(|e| e.to_string())?
            .artifact_type()
            .to_owned();
        if kind == image_store::media_type::MACHINE {
            crate::recipe::template_facts::tag_locked(store, reference, manifest)
                .map_err(|e| e.to_string())
        } else if kind == image_store::media_type::FIRMWARE {
            let rules = crate::firmware::PcFirmwareRules(crate::firmware::catalogue());
            recipe::lock::tag(
                store,
                reference,
                manifest,
                &image_store::firmware::Validator(&rules),
            )
        } else {
            recipe::lock::tag(
                store,
                reference,
                manifest,
                &image_store::media_set::Validator(&crate::media_set::PcMediaFormats),
            )
        }
    }
}

fn kind(name: &str) -> Result<ExtraKind, String> {
    Ok(match name {
        "document" => ExtraKind::Document,
        "archive" => ExtraKind::Archive,
        "executable" => ExtraKind::Executable,
        "other" => ExtraKind::Other,
        other => return Err(format!("no extra kind {other:?}")),
    })
}

fn min_app_version(artifact: &Staged) -> Result<Option<AppVersion>, String> {
    artifact
        .min_app_version
        .as_deref()
        .map(|v| {
            v.parse::<AppVersion>()
                .map_err(|e| format!("min_app_version {v:?}: {e}"))
        })
        .transpose()
}

fn media(store: &Store, artifact: &Staged) -> Result<Published, String> {
    let title = artifact
        .title
        .clone()
        .ok_or("a media recipe without a title record")?;
    let request = Request {
        tag: artifact.reference.clone(),
        title,
        labels: artifact.labels.clone(),
        media: artifact
            .media
            .iter()
            .map(|medium| Source {
                path: medium.file.path.clone(),
                label: Some(medium.label.clone()),
                format: Some(medium.format.clone()),
                // The staged file carries the export file name.
                file_name: None,
            })
            .collect(),
        extras: artifact
            .extras
            .iter()
            .map(|extra| {
                Ok(ExtraSource {
                    path: extra.file.path.clone(),
                    dos_name: extra.dos_name.clone(),
                    kind: extra.kind.as_deref().map(kind).transpose()?,
                })
            })
            .collect::<Result<_, String>>()?,
        compress: true,
    };
    let open = artifact.license.as_ref().map(|license| OpenStatement {
        license: license.expression.clone(),
        terms: license.terms.clone().unwrap_or_default(),
        source: license.source.clone(),
    });
    let mut plan =
        media_import::plan_with(&request, open.as_ref()).map_err(|refusal| refusal.to_string())?;
    let ids: Vec<String> = artifact.media.iter().map(|m| m.id.clone()).collect();
    plan.name_media(&ids)
        .map_err(|refusal| refusal.to_string())?;
    if let Some(version) = min_app_version(artifact)? {
        plan.raise_min_app_version_at(version, crate::compat::source_levels(store).core)
            .map_err(|refusal| refusal.to_string())?;
    }
    match media_import::publish(store, &plan, &|_, _| {}, &|| false)
        .map_err(|refusal| refusal.to_string())?
    {
        Outcome::Imported(descriptor) => Ok(Published {
            descriptor,
            created: true,
        }),
        Outcome::AlreadyImported(descriptor) => Ok(Published {
            descriptor,
            created: false,
        }),
    }
}

fn firmware(store: &Store, artifact: &Staged) -> Result<Published, String> {
    let [rom] = artifact.roms.as_slice() else {
        return Err(format!(
            "{} ROMs: a firmware package built from a recipe holds one ROM in this version",
            artifact.roms.len()
        ));
    };
    let fit = crate::firmware::Fit {
        boards: artifact
            .package
            .as_ref()
            .and_then(|p| p.boards.clone())
            .unwrap_or_default(),
        machine_classes: artifact
            .package
            .as_ref()
            .and_then(|p| p.machine_classes.clone())
            .unwrap_or_default(),
    };
    let fitted_catalogue = fit
        .catalogue(crate::firmware::catalogue())
        .map_err(|e| e.to_string())?;
    let catalogue = fitted_catalogue.as_slice();
    let paths: Vec<PathBuf> = rom.files.iter().map(|f| f.path.clone()).collect();
    let scan = firmware_import::scan(&paths, catalogue).map_err(|e| e.to_string())?;
    let mut candidates: Vec<_> = scan
        .candidates
        .into_iter()
        .filter(|c| {
            rom.catalogue_id
                .as_deref()
                .is_none_or(|id| c.catalogue_id == id)
        })
        .collect();
    let candidate = match candidates.len() {
        1 => candidates.remove(0),
        0 => {
            return Err(format!(
                "rom {}: its bytes are not {} in the firmware catalogue; this version builds packages of catalogue ROMs",
                rom.id,
                rom.catalogue_id
                    .as_deref()
                    .map_or_else(|| "an entry".to_owned(), |id| format!("the entry {id}"))
            ))
        }
        _ => {
            return Err(format!(
                "rom {}: its bytes match several catalogue entries; state catalogue_id",
                rom.id
            ))
        }
    };
    if let Some(config_rom) = candidate.config.roms.first() {
        if config_rom.role != rom.role {
            return Err(format!(
                "rom {}: the recipe states the role {}, the catalogue entry {} has {}",
                rom.id, rom.role, candidate.catalogue_id, config_rom.role
            ));
        }
    }
    let source = artifact.license.as_ref().and_then(|l| l.source.as_deref());
    let mut statement = Statement {
        min_app_version: min_app_version(artifact)?,
        user_supplied: artifact.license.is_none(),
        ..Statement::default()
    };
    if let Some(package) = &artifact.package {
        statement.title = Some(package.title.clone());
        statement.description.clone_from(&package.description);
        statement.notes.clone_from(&package.notes);
        statement.vendor.clone_from(&package.vendor);
        statement.version.clone_from(&package.version);
        statement.date.clone_from(&package.date);
        statement.labels.clone_from(&package.labels);
        statement.boards.clone_from(&package.boards);
        statement
            .machine_classes
            .clone_from(&package.machine_classes);
    }
    let mut attachments = Vec::new();
    for extra in &artifact.extras {
        let name = extra
            .file
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if source == Some(name) {
            statement.source = Some(extra.file.path.clone());
        } else {
            attachments.push(Attachment {
                path: extra.file.path.clone(),
                dos_name: extra.dos_name.clone(),
            });
        }
    }
    let preview = firmware_import::preview_manifest_at(
        &candidate,
        &attachments,
        &statement,
        catalogue,
        crate::compat::source_levels(store),
    )
    .map_err(|e| e.to_string())?;
    // The importer gives a document its kind from its file name; a recipe
    // that states another is refused rather than published as something
    // it did not say.
    for extra in &artifact.extras {
        let Some(stated) = extra.kind.as_deref() else {
            continue;
        };
        let name = extra.file.path.file_name().and_then(|n| n.to_str());
        if let Some(planned) = preview
            .config
            .extras
            .iter()
            .find(|e| Some(e.filename.as_str()) == name)
        {
            if planned.kind != kind(stated)? {
                return Err(format!(
                    "extra {}: the recipe states the kind {stated}, the importer gives {:?} from its file name",
                    extra.id, planned.kind
                ));
            }
        }
    }
    let before = match store.tag_descriptor(&artifact.reference) {
        Ok(descriptor) => Some(descriptor.digest),
        Err(image_store::Error::TagNotFound(_)) => None,
        Err(error) => return Err(error.to_string()),
    };
    let descriptor = firmware_import::publish_as(
        store,
        &candidate,
        &attachments,
        &statement,
        catalogue,
        &artifact.reference,
        None,
    )
    .map_err(|e| e.to_string())?;
    Ok(Published {
        created: before.as_ref() != Some(&descriptor.digest),
        descriptor,
    })
}
