// SPDX-License-Identifier: AGPL-3.0-or-later
//! Core-neutral planning, acquisition and verified content for machine recipes.

use crate::{
    build,
    model::Recipe,
    plan::{Plan, Status},
};
use image_store::{BlobSource, Descriptor, Digest, Manifest, Store};
use std::collections::{BTreeMap, BTreeSet};
use unemu_core_api::{
    BindingFacts, BindingKind, BindingSlot, BindingUse, ConfigDocument, Content, ContentSource,
    Core, Error, ErrorKind, PreparedConfig,
};

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
    sources: BTreeMap<String, Box<dyn ContentSource>>,
}
impl Facts {
    pub fn new(packages: Vec<VerifiedUse>) -> Self {
        let (uses, sources) = packages
            .into_iter()
            .map(|p| {
                let id = p.facts.id.clone();
                (p.facts, (id, p.content))
            })
            .unzip();
        Self { uses, sources }
    }
}

/// Discover requirements without interpreting core syntax, then compose using
/// exactly the verified package facts and their restricted content source.
pub fn compose(
    core: &dyn Core,
    platform: &str,
    config: ConfigDocument<'_>,
    disks: &[(String, u64)],
    facts: &Facts,
) -> Result<(PreparedConfig, Vec<BindingSlot>), String> {
    let slots = core
        .binding_slots(platform, config)
        .map_err(|e| e.to_string())?;
    let mut remaining: BTreeMap<_, _> = disks
        .iter()
        .map(|(id, size)| (id.as_str(), *size))
        .collect();
    let declared: BTreeSet<_> = facts.uses.iter().map(|u| u.id.as_str()).collect();
    let mut used = BTreeSet::new();
    for slot in &slots {
        match slot.kind {
            BindingKind::ZeroDisk => {
                let id = slot
                    .item_id
                    .as_deref()
                    .ok_or("zero disk slot has no item id")?;
                let size = remaining
                    .remove(id)
                    .ok_or_else(|| format!("missing disk declaration: {id}"))?;
                if Some(size) != slot.size {
                    return Err(format!("disk {id}: size differs from binding slot"));
                }
            }
            BindingKind::FirmwareRom | BindingKind::Medium => {
                let id = slot.use_id.as_deref().ok_or("package slot has no use id")?;
                if !declared.contains(id) {
                    return Err(format!("undeclared use: {id}"));
                }
                used.insert(id);
            }
            _ => return Err("unsupported binding kind".into()),
        }
    }
    if let Some(id) = remaining.keys().next() {
        return Err(format!("unused disk declaration: {id}"));
    }
    if let Some(id) = declared.difference(&used).next() {
        return Err(format!("unused use: {id}"));
    }
    let prepared = core
        .compose(platform, config, BindingFacts::new(&facts.uses, facts))
        .map_err(|e| e.to_string())?;
    Ok((prepared, slots))
}

impl ContentSource for Facts {
    fn open(&self, sha256: &[u8; 32], size: u64) -> Result<Content, Error> {
        for usage in &self.uses {
            let Some(source) = self.sources.get(&usage.id) else {
                continue;
            };
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
    let levels = preparation_levels(request.store, family, core_for)?;
    let mut required = crate::compat::requirements(
        &recipe,
        &image_store::compat::Requirements {
            core: image_store::compat::Requirement::None,
            frontend: image_store::compat::Requirement::None,
        },
        &crate::compat::TABLE,
    )?;
    if let Some(stated) = &recipe.identity.min_app_version {
        required.core = stated
            .value
            .parse::<image_store::compat::AppVersion>()
            .map_err(|e| e.to_string())?;
    }
    image_store::compat::Requirements {
        core: image_store::compat::Requirement::Release(required.core),
        frontend: image_store::compat::Requirement::Release(required.frontend),
    }
    .check("the machine recipe", levels)
    .map_err(|e| e.to_string())?;
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
        sources: BTreeMap::new(),
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
        facts
            .sources
            .insert(verified.facts.id.clone(), verified.content);
        facts.uses.push(verified.facts);
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

fn preparation_levels(
    store: &Store,
    family: &str,
    core_for: &dyn Fn(&str) -> Option<&'static dyn Core>,
) -> Result<image_store::compat::Levels, String> {
    use image_store::compat::{Levels, BASELINE, EXTRACTION_BASELINE};
    if store.compatibility_configured() {
        return Ok(store.compatibility_levels(family));
    }
    let core = core_for(family)
        .map(|core| core.description().version.parse().map(|v| (v, BASELINE)))
        .transpose()
        .map_err(|e: image_store::Error| e.to_string())?;
    Ok(Levels::new(
        core,
        EXTRACTION_BASELINE,
        crate::compat::NEWEST_ROW,
    ))
}

/// Compose templates and verify their locks before publishing through the provider.
pub fn build_path(
    request: build::Request<'_>,
    core_for: &dyn Fn(&str) -> Option<&'static dyn Core>,
    acquire: Option<&dyn Acquire>,
    log: &mut dyn FnMut(&str),
) -> std::io::Result<build::Outcome> {
    build_path_with(request, core_for, acquire, log, false)
}

/// Pin a template only after its lock and pinned recipe text pass their checks.
pub fn pin_path(
    request: build::Request<'_>,
    core_for: &dyn Fn(&str) -> Option<&'static dyn Core>,
    acquire: Option<&dyn Acquire>,
    log: &mut dyn FnMut(&str),
) -> std::io::Result<crate::pin::Pinned> {
    crate::pin::pin_with(request, log, |request, log| {
        build_path_with(request, core_for, acquire, log, true)
    })
}

fn build_path_with(
    request: build::Request<'_>,
    core_for: &dyn Fn(&str) -> Option<&'static dyn Core>,
    acquire: Option<&dyn Acquire>,
    log: &mut dyn FnMut(&str),
    defer_tag: bool,
) -> std::io::Result<build::Outcome> {
    let recipe = match crate::load(request.path, request.providers)? {
        crate::Opened::Recipe(recipe, _, _) if recipe.kind == crate::model::Kind::Machine => recipe,
        _ => return build::build_path(request, log),
    };
    let lost = recipe.identity.digest.is_some() && crate::lock_lost(request.path);
    let locked = recipe
        .identity
        .digest
        .as_ref()
        .filter(|_| !lost)
        .map(|digest| crate::lock::open(&digest.value, &crate::lock::dir(request.path)))
        .transpose()
        .map_err(std::io::Error::other)?;
    if let Some(locked) = &locked {
        let family = recipe
            .identity
            .platform
            .as_ref()
            .map(|p| p.value.as_str())
            .unwrap_or_default();
        let levels =
            preparation_levels(request.store, family, core_for).map_err(std::io::Error::other)?;
        image_store::compat::Requirements::read(&locked.manifest_bytes)
            .check(&crate::artifact::output(&recipe), levels)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
    }
    let own = crate::artifact::output(&recipe);
    let (repository, own_tag) = own.rsplit_once(':').unwrap_or((own.as_str(), ""));
    let reference = match request.keep_as {
        None => own.clone(),
        Some(kept) if kept.is_empty() || kept.contains([':', '/', '@']) => {
            return Err(std::io::Error::other(format!(
                "--keep-as {kept}: give a tag of the recipe's repository, without the repository"
            )));
        }
        Some(kept) if kept == own_tag => {
            return Err(std::io::Error::other(format!(
                "--keep-as {kept}: that is the recipe's own tag; a kept result takes another"
            )));
        }
        Some(kept) => format!("{repository}:{kept}"),
    };
    let recipe_digest = Digest::of(&std::fs::read(request.path)?);
    let family = image_store::platform::canonical(
        recipe
            .identity
            .platform
            .as_ref()
            .map(|p| p.value.as_str())
            .unwrap_or_default(),
    );
    let provider = request
        .providers
        .get(family)
        .ok_or_else(|| std::io::Error::other("no template provider"))?;
    let store = request.store;
    match prepare(request, core_for, acquire, log).map_err(std::io::Error::other)? {
        Prepared::Missing(plan) => Ok(build::Outcome::Missing(plan)),
        Prepared::Ready {
            plan,
            facts,
            config,
        } => {
            // A composed manifest the lock disagrees with is a mismatch, as
            // a package's payloads that disagree with its lock are.
            let mut differs = false;
            let result = (|| {
                let core = core_for(family).ok_or("no template core")?;
                let document = ConfigDocument::new(provider.template_config_type()?, &config);
                let (prepared, slots) = compose(core, family, document, &plan.disks, &facts)?;
                let (current, descriptor) = provider.template_manifest(
                    store,
                    &recipe,
                    &prepared,
                    &slots,
                    &facts,
                    locked.as_ref(),
                )?;
                let descriptor = match &locked {
                    Some(locked) => {
                        crate::lock::verify_machine(locked, &current)
                            .inspect_err(|_| differs = true)?;
                        crate::lock::store(store, locked, &[])?
                    }
                    None => {
                        if lost {
                            let digest = &descriptor.digest;
                            let pinned = recipe
                                .identity
                                .digest
                                .as_ref()
                                .map(|d| d.value.as_str())
                                .unwrap_or_default();
                            if digest.to_string() != pinned {
                                return Err(format!("the lock is lost; this build makes {digest}, the recipe pins {pinned}: the recipe needs its lock; restore it from where the recipe was published"));
                            }
                        }
                        descriptor
                    }
                };
                if defer_tag {
                    Ok(build::Published {
                        descriptor,
                        created: false,
                    })
                } else {
                    provider
                        .publisher()
                        .ok_or("no template publisher")?
                        .tag_locked(store, &reference, &descriptor)
                }
            })();
            match result {
                Err(error) if differs => Ok(build::Outcome::Mismatch {
                    plan,
                    mismatches: vec![error],
                    kept: None,
                }),
                Err(error) => Ok(build::Outcome::Failed { plan, error }),
                Ok(published) => {
                    build::record(store, &reference, &published, &recipe_digest, log);
                    Ok(build::Outcome::Built(Box::new(build::Report {
                        plan: *plan,
                        reference,
                        published,
                        kept: recipe
                            .template
                            .as_ref()
                            .and_then(|t| t.config.as_ref())
                            .map(|id| build::Kept {
                                input: id.value.clone(),
                                how: "composed into the stored machine config".into(),
                            })
                            .into_iter()
                            .collect(),
                        attachments: Vec::new(),
                        locked: recipe.identity.digest.is_some(),
                        drift: Vec::new(),
                        outputs: BTreeSet::new(),
                        lock_rebuilt: lost,
                    })))
                }
            }
        }
    }
}
