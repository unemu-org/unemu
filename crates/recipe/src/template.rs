// SPDX-License-Identifier: AGPL-3.0-or-later
//! Core-neutral planning, acquisition and verified content for machine recipes.

use crate::{
    build,
    model::Recipe,
    plan::{Plan, Status},
};
use image_store::{BlobSource, Descriptor, Digest, Manifest, Store};
use unemu_core_api::{BindingUse, Content, ContentSource, Core, Error, ErrorKind};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsePlan {
    pub id: String,
    pub image: String,
    pub digest: String,
    pub present: bool,
}
impl UsePlan {
    pub fn availability(&self) -> String {
        if self.present {
            "present in the store with this manifest digest".into()
        } else {
            let registry = self
                .image
                .split('/')
                .next()
                .filter(|host| host.contains(['.', ':']) || *host == "localhost")
                .unwrap_or("the configured default registry");
            format!("missing; would pull by manifest digest from {registry} with --fetch")
        }
    }

    /// The reference's kind, independent of any tag's current contents.
    pub fn artifact_type(&self) -> Result<&'static str, String> {
        let repository = self
            .image
            .rsplit_once(':')
            .map_or(self.image.as_str(), |(r, _)| r);
        match repository.rsplit('/').nth(2) {
            Some("firmware") => Ok(image_store::media_type::FIRMWARE),
            Some("media") => Ok(image_store::media_type::MEDIA_SET),
            _ => Err(format!(
                "use {}: image kind must be firmware or media",
                self.id
            )),
        }
    }
}

/// Discover an untagged OCI manifest too: acquisition never needs to set a tag.
/// Only its length is read here; consumers verify through the store afterwards.
pub fn descriptor(store: &Store, digest: &Digest) -> image_store::Result<Option<Descriptor>> {
    let path = store.root().join("images/blobs/sha256").join(digest.hex());
    match std::fs::metadata(path) {
        Ok(metadata) => Ok(Some(Descriptor::new(
            image_store::media_type::MANIFEST,
            digest.clone(),
            metadata.len(),
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn extend_plan(recipe: &Recipe, plan: &mut Plan) {
    if recipe.kind != crate::model::Kind::Machine {
        return;
    }
    let store = plan
        .store
        .as_ref()
        .and_then(|root| Store::open_existing(root).ok());
    plan.uses = recipe
        .uses
        .iter()
        .filter_map(|usage| {
            let digest = usage.digest.as_ref()?.value.clone();
            let present = store.as_ref().is_some_and(|store| {
                let Ok(digest) = digest.parse() else {
                    return false;
                };
                let Ok(Some(descriptor)) = descriptor(store, &digest) else {
                    return false;
                };
                let Ok(bytes) = store.read_verified(&descriptor, image_store::MAX_SMALL_BLOB)
                else {
                    return false;
                };
                let Ok(manifest) = Manifest::decode_for_summary(&bytes) else {
                    return false;
                };
                std::iter::once(&manifest.config)
                    .chain(&manifest.layers)
                    .all(|d| store.open_blob(&d.digest, d.size).is_ok())
            });
            Some(UsePlan {
                id: usage.id.value.clone(),
                image: usage.image.as_ref()?.value.clone(),
                digest,
                present,
            })
        })
        .collect();
    if let Some(template) = &recipe.template {
        plan.disks = template
            .disks
            .iter()
            .filter_map(|d| Some((d.id.as_ref()?.value.clone(), d.size.as_ref()?.value)))
            .collect();
    }
    for output in &mut plan.outputs {
        output
            .from
            .extend(plan.uses.iter().map(|u| format!("use.{}", u.id)));
        output
            .from
            .extend(plan.disks.iter().map(|(id, _)| format!("disk.{id}")));
        let mut missing = match &output.status {
            Status::Missing(ids) => ids.clone(),
            _ => Vec::new(),
        };
        missing.extend(
            plan.uses
                .iter()
                .filter(|u| !u.present)
                .map(|u| format!("use.{}", u.id)),
        );
        if !missing.is_empty() {
            output.status = Status::Missing(missing);
        }
    }
}

/// One verified package, including a source restricted to exactly its item identities.
pub struct VerifiedUse {
    pub facts: BindingUse,
    pub content: Box<dyn ContentSource>,
}

/// Acquires a package's closure by manifest digest without interpreting it.
pub trait Acquire {
    fn acquire(&self, store: &Store, usage: &UsePlan, platform: &str) -> Result<(), String>;
}

/// Owned verified facts, reusable by later composition without reopening a tag.
pub struct Facts {
    pub uses: Vec<BindingUse>,
    sources: Vec<Box<dyn ContentSource>>,
}
impl ContentSource for Facts {
    fn open(&self, sha256: &[u8; 32], size: u64) -> Result<Content, Error> {
        for (usage, source) in self.uses.iter().zip(&self.sources) {
            if usage
                .items
                .iter()
                .any(|i| &i.sha256 == sha256 && i.size == size)
            {
                return source.open(sha256, size);
            }
        }
        Err(Error::new(
            ErrorKind::Content,
            "content is not declared by a template use",
        ))
    }
}

pub enum Prepared {
    Missing(Box<Plan>),
    Ready {
        plan: Box<Plan>,
        facts: Facts,
        config: crate::steps::Bytes,
    },
}

/// Check capability before downloads, then plan, acquire and ask the provider
/// for facts. No config parsing, composition, disk construction or tagging occurs.
pub fn prepare(
    request: build::Request<'_>,
    core_for: &dyn Fn(&str) -> Option<&'static dyn Core>,
    acquire: Option<&dyn Acquire>,
    log: &mut dyn FnMut(&str),
) -> Result<Prepared, String> {
    let recipe = match crate::load(request.path, request.providers).map_err(|e| e.to_string())? {
        crate::Opened::Recipe(recipe, _, _) if recipe.kind == crate::model::Kind::Machine => recipe,
        crate::Opened::Refused(report) => return Err(report.render()),
        _ => return Err("expected a machine recipe".into()),
    };
    let family = image_store::platform::canonical(
        recipe
            .identity
            .platform
            .as_ref()
            .map(|p| p.value.as_str())
            .unwrap_or_default(),
    );
    let supported = core_for(family).is_some_and(|core| {
        core.description()
            .platforms
            .iter()
            .any(|p| image_store::platform::same(p.id, family) && p.content_bindings)
    });
    if !supported {
        return Err("this core cannot compose templates".into());
    }
    let provider = request
        .providers
        .get(family)
        .ok_or("no recipe provider for template platform")?;
    let downloads = request.store.root().join(build::DOWNLOADS);
    let local_index = if request.index.is_none() {
        Some(crate::store_index::StoreIndex::load(request.store).map_err(|e| e.to_string())?)
    } else {
        None
    };
    let index = request.index.or(local_index.as_ref());
    let mut plan = crate::plan::plan(
        &recipe,
        &request.path.display().to_string(),
        request.path.parent(),
        crate::plan::Sources {
            store: index,
            downloads: Some(&downloads),
            libraries: request.libraries,
        },
    );
    for usage in &plan.uses {
        let artifact_type = usage.artifact_type()?;
        let digest: Digest = usage
            .digest
            .parse()
            .map_err(|e: image_store::Error| e.to_string())?;
        if let Some(descriptor) = descriptor(request.store, &digest).map_err(|e| e.to_string())? {
            let bytes = request
                .store
                .read_compatible_manifest(&descriptor)
                .map_err(|e| format!("use {}: {e}", usage.id))?;
            let manifest = Manifest::decode_for_summary(&bytes).map_err(|e| e.to_string())?;
            if manifest.artifact_type() != artifact_type
                || !image_store::platform::same(
                    manifest.platform().map_err(|e| e.to_string())?,
                    family,
                )
            {
                return Err(format!("use {}: wrong artifact type or platform", usage.id));
            }
        }
    }
    if !plan.complete() && request.fetcher.is_none() {
        return Ok(Prepared::Missing(Box::new(plan)));
    }
    // Refuse present but invalid packages before acquiring any missing package.
    for usage in plan.uses.iter().filter(|u| u.present) {
        provider.template_use(request.store, usage)?;
    }
    if plan
        .inputs
        .iter()
        .any(|i| matches!(i.status, Status::Missing(_)))
    {
        let Some(fetcher) = request.fetcher else {
            return Ok(Prepared::Missing(Box::new(plan)));
        };
        match build::fetch_missing(&plan, &downloads, fetcher, log) {
            Ok(()) | Err(build::FetchError::Cannot) => {}
            Err(build::FetchError::Failed(error)) => return Err(error),
        }
        plan = crate::plan::plan(
            &recipe,
            &request.path.display().to_string(),
            request.path.parent(),
            crate::plan::Sources {
                store: index,
                downloads: Some(&downloads),
                libraries: request.libraries,
            },
        );
        if plan
            .inputs
            .iter()
            .any(|i| matches!(i.status, Status::Missing(_)))
        {
            return Ok(Prepared::Missing(Box::new(plan)));
        }
    }
    let mut gathered = build::gather(&recipe, &plan, request.libraries, request.store_read)?;
    let config_id = recipe
        .template
        .as_ref()
        .and_then(|t| t.config.as_ref())
        .map(|id| id.value.as_str())
        .ok_or("missing template config input")?;
    let Some(crate::steps::Value::File(config)) = gathered.inputs.remove(config_id) else {
        return Err("template config input is not a verified file".into());
    };
    let mut facts = Facts {
        uses: Vec::new(),
        sources: Vec::new(),
    };
    for usage in &mut plan.uses {
        if !usage.present {
            let Some(acquire) = acquire else {
                return Ok(Prepared::Missing(Box::new(plan)));
            };
            log(&format!(
                "fetch: use {}: {} @ {}",
                usage.id, usage.image, usage.digest
            ));
            acquire.acquire(request.store, usage, family)?;
        }
        let verified = provider.template_use(request.store, usage)?;
        facts.uses.push(verified.facts);
        facts.sources.push(verified.content);
        usage.present = true;
    }
    for output in &mut plan.outputs {
        output.status = Status::Ready;
    }
    Ok(Prepared::Ready {
        plan: Box::new(plan),
        facts,
        config,
    })
}

/// Dispatch existing recipes unchanged and stop machine builds after verified facts.
pub fn build_path(
    request: build::Request<'_>,
    core_for: &dyn Fn(&str) -> Option<&'static dyn Core>,
    acquire: Option<&dyn Acquire>,
    log: &mut dyn FnMut(&str),
) -> std::io::Result<build::Outcome> {
    let machine = matches!(crate::load(request.path, request.providers)?, crate::Opened::Recipe(recipe, _, _) if recipe.kind == crate::model::Kind::Machine);
    if !machine {
        return build::build_path(request, log);
    }
    match prepare(request, core_for, acquire, log).map_err(std::io::Error::other)? {
        Prepared::Missing(plan) => Ok(build::Outcome::Missing(plan)),
        Prepared::Ready { plan, .. } => Ok(build::Outcome::Failed {
            plan,
            error: "machine templates are not built yet".into(),
        }),
    }
}
