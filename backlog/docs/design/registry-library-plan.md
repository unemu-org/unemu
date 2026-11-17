# upc-5pw.65 — The registry as a game library: catalogue index and attachments

Plan only. Nothing here is implemented. Date: 2026-11-15.
Markers: **[SRC n]** = external fact with a source in §11; **[REPO]** = read in
this repository; **ASSUMED** = not verified, must be checked by the named bead;
**UNVERIFIED** = an external claim found only in secondary sources.

## 0. Verdict on the discussed direction

| Direction | Verdict | Main changes proposed |
|---|---|---|
| 1. Catalogue index as an OCI artifact per namespace | **Accept, with changes** | Tag is the schema major (`v1`), not `latest`. Full rebuild is deterministic and is the source of truth; the publish hook is only an optimisation. Entries are sharded by repository-name hash. No wall-clock field; a monotonic `generation` plus `parent` digest instead. The index goes through the existing push guard like any artifact. |
| 2. Covers, manuals, scans as OCI 1.1 referrers | **Accept, with changes** | One artifact type per kind (the Referrers API filters only on `artifactType`). Listing data lives in manifest annotations so one list call renders the pane. In-manifest artwork (decision-25 point 7) stays the portable default; referrers are additive. Referrers bind to a digest, so a new version needs an explicit "carry forward". The catalogue records attachment digests, so normal browsing never needs the Referrers API. |
| zot GraphQL search as an optional extra | **Reject for now** | It indexes only the standard `org.opencontainers.image.*` annotations (title, description, vendor, licences, authors, labels) [SRC 5], not genres, years, platforms or series. It would be a second code path with weaker results. Keep the local index only. |

Three findings in the code that change the scope:

1. **The client speaks Basic authentication only.** A Bearer challenge ends in
   `Error::UnsupportedAuth` (`crates/registry/src/client.rs`, lines 139 and
   355) [REPO]. Docker Hub and GHCR use token services [SRC 7], so "works on
   any OCI registry incl. GHCR/Docker Hub" is not true today, with or without a
   catalogue. Token authentication is its own bead (P4-1) and is needed only
   when a non-zot host is actually wanted.
2. **`Manifest` has no `subject` and denies unknown fields**
   (`crates/image-store/src/artifact.rs`, line 253) [REPO]. A referrer cannot
   be decoded or stored today. Attachments need a codec change and a local
   referrers table.
3. **Pull validates every closure through a provider chosen by artifact type
   and platform** (`image_registry::select_provider`) [REPO]. The catalogue and
   attachments have no platform provider, so they need their own fetch paths
   that never enter the tagged library.

## 1. Registry capability facts

| Capability | OCI Distribution 1.1 | zot | Docker Hub | GHCR | Harbor |
|---|---|---|---|---|---|
| List repositories (`/v2/_catalog`) | **Not in the spec at all**; only `tags/list` is defined [SRC 1] | Yes; lists what the caller may read, paged with `n`/`Link` [REPO §5.1 spike, zot v2.1.23] | No on `registry-1.docker.io`; Hub has its own non-OCI web API. ASSUMED (well known, not re-verified) | No; listing goes through the GitHub REST packages API. ASSUMED | Present but restricted to administrators; listing normally goes through Harbor's own `/api/v2.0`. ASSUMED |
| Referrers API `GET /v2/<name>/referrers/<digest>` | Defined; returns an image index; `?artifactType=` filter SHOULD be supported, signalled by `OCI-Filters-Applied`; a registry that supports it MUST NOT answer 404; paged with `Link` [SRC 1] | Native [REPO spike] | UNVERIFIED: a third-party matrix says "partial, fallback tags" [SRC 8] | UNVERIFIED: a February 2026 third-party advisory says no endpoint, fallback needed [SRC 8] | UNVERIFIED: sources disagree (full since 2.x vs partial) [SRC 8] |
| Referrers tag fallback | On 404 the client MUST fall back to the tag `<alg>-<hex>` (for example `sha256-<64 hex>`), which holds an image index the client maintains: read, append, push back. Deleting a manifest MAY also clean the tag [SRC 1] | Not needed | Works: it is only a tag | Works: it is only a tag | Works: it is only a tag |
| Push with `subject` | A registry with the API MUST answer `OCI-Subject: <digest>`; without that header the client MUST update the fallback tag [SRC 1] | Sends the header. ASSUMED, verify in P0-2 | — | — | — |
| `artifactType` | Field of the image manifest and of descriptors in indexes [SRC 2] | Round-trips custom artifact and media types [REPO spike] | Stores OCI artifacts [SRC 6] | OCI and Docker v2 formats; 10 GB per layer; 10 minute upload timeout [SRC 7] | Stores OCI artifacts [SRC 8] |
| Manifest size | No hard limit; clients and registries SHOULD support at least 4 MiB; `413` when refused [SRC 1] | No default limit [REPO spike] | ASSUMED ≥ 4 MiB | ASSUMED ≥ 4 MiB | ASSUMED ≥ 4 MiB |
| Tags | `[a-zA-Z0-9_][a-zA-Z0-9._-]{0,127}` [SRC 1] | same | same | same | same |
| Authentication | Not mandated by the spec | htpasswd Basic (used here) | Bearer token service | Bearer token; personal access token (classic); public packages readable anonymously [SRC 7] | Bearer token service; Basic on its own API. ASSUMED |
| Search | None | GraphQL at `/v2/_zot/ext/search`: `GlobalSearch`, `ImageList`, `Referrers`, `RepoListWithNewestImage`, …; image fields Title, Description, Vendor, Licenses, Labels, Authors, IsSigned, Referrers [SRC 5] | Own web API | GitHub API | Own API |
| Retention / GC | Not specified | `deleteUntagged` (default true), `deleteReferrers` (default false: "delete manifests with a missing Subject"), `delay` (24h in the documented example) [SRC 4] | Hosted | Hosted | Own GC and retention rules |

Consequences for the design:

- A listing that works everywhere cannot rely on `_catalog`. The index artifact
  is the only portable answer; this confirms direction 1. The design note
  already predicted it: "A registry without a catalog would need a small
  well-known index image instead" (machine-images §5.5) [REPO].
- Referrer support must be **probed per registry at run time**, never looked
  up in a table: try the API, treat 404 as "use the tag schema" [SRC 1]. The
  uncertain cells above then do not matter to correctness.
- zot's `deleteUntagged: true` meets untagged referrer manifests and untagged
  old catalogue generations. ASSUMED: zot keeps a referrer whose subject is
  alive. P0-2 must prove it on the maintainer's zot before any attachment is
  published.

## 2. Catalogue index design

### 2.1 Name, tag, types

- **Repository:** `<namespace>/catalog`, for example `unemu/catalog` and
  `private/catalog`. One per namespace. The name `catalog` becomes
  **reserved**: the client refuses to push a machine image, media set or
  firmware package to `<ns>/catalog`. (A leading underscore is impossible: the
  repository grammar is `[a-z0-9]+(...)` [REPO §5.5].)
- **Tag:** `v1` = the schema major, a moving alias. Not `latest`. A later
  schema 2 is published under `v2` next to `v1`, so old clients keep working.
  `v1` is not semver-shaped, so the existing tag rules let it move
  (decision-25 amendment "free tag names") [REPO].
- **Shape:** an ordinary OCI image manifest, like every other unemu artifact.

| Part | Media type | Notes |
|---|---|---|
| `artifactType` | `application/vnd.unemu.catalog.v1` | |
| config | `application/vnd.unemu.catalog.config.v1+json` | small header, ≤ 64 KiB |
| entry shard layer | `application/vnd.unemu.catalog.entries.v1+json+zstd` | canonical JSON, compressed with the store's pinned codec; carries `unemu.decoded.digest` and `unemu.decoded.size` as other `+zstd` payloads do (`codec.rs`) [REPO] |
| manifest annotations | `unemu.catalog.namespace`, `unemu.catalog.generation`, `org.opencontainers.image.title` | so `oras`/zot UIs show what it is |

Rejected alternative: an **OCI image index** whose descriptors carry the
metadata as annotations (the "Collections" direction of §5.4). Annotations are
flat string maps, array facets become encoded strings, and at about 1 KB per
entry the single manifest passes the 4 MiB guidance [SRC 1] near 4 000 entries
with no way to shard. Collections stay a separate, later feature.

### 2.2 Config (header)

```json
{
  "schema": 1,
  "namespace": "unemu",
  "generation": 42,
  "parent": "sha256:…",          // manifest digest of generation 41, absent for 1
  "entries": 1234,
  "shard_bits": 0,               // 0 = one shard, 4 = 16 shards, 8 = 256 shards
  "shards": [ { "index": 0, "entries": 1234, "digest": "sha256:…" } ],
  "withdrawn": [ { "repository": "…", "manifest": "sha256:…", "reason": "…" } ],
  "built_by": "unemu 0.x"
}
```

- **No timestamp.** Manifests in this project carry no creation time
  (§5.1) [REPO]. `generation` + `parent` order generations instead. The client
  records locally when it first saw a digest and shows "last checked" from
  that; this is UI state, outside the emulation path, so hard rule 4 holds.
- **Schema versioning** follows the metadata precedent (`Metadata::decode` /
  `project`) [REPO]: a reader that meets a newer minor field ignores it; a
  newer `schema` is shown read-only with the fields it understands, and the
  client says the catalogue was built by a newer version.

### 2.3 Deterministic encoding

- Canonical JSON: UTF-8, no insignificant whitespace, object keys in the fixed
  order of the struct, absent rather than null or empty, integers only.
- Entries sorted by `repository` (byte order); titles in their metadata order.
- Shard of an entry: the top `shard_bits` bits of `SHA-256(repository)`. This
  is stable when entries are added, unlike alphabetical ranges, so one publish
  changes one shard and the registry deduplicates the rest.
- Layers listed in shard index order. `shard_bits` is a pure function of the
  entry count: 0 below 2 000 entries, 4 up to 50 000, 8 above.
- Compression uses the pinned zstd crates and the fixed level and window the
  store already uses (`zstd =0.13.5`, `LEVEL 3`, `WINDOW_LOG_MAX 23`) [REPO].
  Identity is checked on the **decoded** digest, so a different zstd version
  cannot break verification, only deduplication.
- Result: the same registry state gives the same bytes and the same manifest
  digest on any host. Two publishers who rebuild from the same state converge
  on the same digest. A golden test pins the bytes.

### 2.4 One entry

One entry per repository. It describes the **offered version** (the release
`latest` names, else the newest release: the rule Get templates already uses,
§5.5 [REPO]) and lists the other tags.

```json
{
  "repository": "unemu/keen4-shareware",
  "kind": "media-set",                    // machine | media-set | firmware
  "platform": "ibm-pc",
  "tag": "1.0.0",
  "manifest": "sha256:…",
  "size": 1474560,                        // manifest + distinct blobs, as Get templates shows
  "tags": [ { "name": "1.0.0", "manifest": "sha256:…" }, { "name": "latest", "manifest": "sha256:…" } ],
  "labels": ["shareware"],
  "reach": "open",                        // open | restricted, from guard::reach
  "licenses": ["LicenseRef-Apogee-Shareware"],
  "titles": [ { "id": "…", "title": "…", "sort_title": "…", "series": "…",
                "genres": [], "developers": [], "publishers": [], "platform": "DOS",
                "release_year": 1991, "players": {"min":1,"max":1},
                "languages": [], "regions": [], "description": "… (≤ 600 chars)",
                "version": "…", "edition": "…", "source_media": "…", "tags": [] } ],
  "media": { "count": 1, "formats": ["floppy-1440"], "extras": 2 },     // media sets
  "firmware": { "roles": [], "boards": [], "machine_classes": [],
                "catalogue_ids": [], "vendor": "…", "version": "…" },   // firmware
  "art": { "thumbnail": { "digest": "sha256:…", "size": 9000, "mediaType": "image/png" },
           "cover":     { "digest": "sha256:…", "size": 180000, "mediaType": "image/png" } },
  "attachments": { "manual": 1, "box-scan": 4, "screenshot": 6 }
}
```

The projection reuses what `image_store::summary` already extracts for the
local library (`TitleSummary`, `FirmwareSummary`, `MediaSetSummary`) [REPO],
so local and remote facets come from one code path.

**Goes in:** the published metadata blob's title fields, image labels, artifact
kind and platform, tags and digests, sizes, licence set and reach, media and
firmware summaries, descriptors (digest, size, type) of thumbnail and cover,
attachment counts by kind.

**Never goes in:**

- personal ratings and notes, and unpublished local overlay edits (they are
  local only by decision-25 point 7) [REPO];
- `launch` records (provider-owned host input; of no use for search);
- host paths, source-folder hints, user names, credentials, provenance text
  that names a private host;
- any byte of a payload: no inline cover data, no ROM, no document text. The
  description is cut to 600 characters; the full text stays in the artifact;
- an entry for a repository of **another namespace**. A public index never
  names a private repository, not even its title: that would leak the
  existence of private content.

### 2.5 The guard applies to the index

The index is pushed through the same `guard::check` as any closure [REPO]:

- Each shard layer and the config carry `unemu.redistribution` and
  `unemu.license`. The generated index data is stated `open` with `CC0-1.0`
  **only when every entry's `reach` is `open`**; otherwise the builder marks
  the layers `user-supplied`.
- A `user-supplied` index can therefore go only to a namespace marked private
  that refuses anonymous reads (guard rules 1 and 3), and never to `unemu/`
  (rule 2). No new guard rule is needed for the index itself.
- The builder adds one check of its own before it calls the guard: every entry
  must be in the target namespace, and for a namespace not marked private
  every entry must be `open`. A violation stops the build; there is no
  override, as elsewhere.
- Thumbnails are derived content: a thumbnail inherits the class of the cover
  it was made from (§5: "restricted inputs propagate that classification to
  derived content") [REPO].

### 2.6 Size budget, sharding, thumbnails

| Catalogue | Entries | Raw JSON (≈1.2 KB/entry) | zstd (≈4:1, ASSUMED) | Shards | Manifest |
|---|---|---|---|---|---|
| First public generation | < 50 | 60 KB | 15 KB | 1 | < 2 KB |
| A personal private library | 2 000 | 2.4 MB | 0.6 MB | 1 → 16 | < 6 KB |
| eXoDOS scale | 10 000 | 12 MB | 3 MB | 16 | < 6 KB |
| Upper design bound | 100 000 | 120 MB | 30 MB | 256 | < 80 KB |

- Hard caps (refuse to build or to load): 256 shards, 16 MiB decoded per
  shard, 600 characters of description, 64 titles per entry, 64 tags listed
  per entry. The manifest stays far below 4 MiB [SRC 1].
- **No pagination protocol.** The client always fetches the header, then all
  shards it does not already have by digest. At the design bound that is 30 MB
  once and one shard (about 120 KB) after a typical publish.
- **Thumbnails are referenced, never inline.** A thumbnail is a small blob
  (PNG or JPEG, longest side ≤ 256 px, ≤ 32 KiB) stored **in the artifact's own
  repository** as a referrer attachment of kind `thumbnail` (§3). The entry
  records its digest, so the client fetches it with one
  `GET /v2/<repo>/blobs/<digest>` for visible rows only and caches it by
  digest. The Referrers API is not involved. Inline base64 would multiply the
  index size by ten and defeat shard deduplication; a thumbnail pack blob per
  shard is a later option (P4-5) if per-row requests prove slow over the
  internet.
- Until thumbnails exist (phase 3), the entry carries only the in-manifest
  cover descriptor and the client fetches covers lazily with the same request,
  limited to a few parallel reads.

### 2.7 Building and updating

Three ways, one algorithm (`catalog::build(namespace, repositories) -> index`):

1. **Explicit full rebuild** — `image catalog build <host>/<ns>`. Enumerates
   repositories, reads each offered manifest, its metadata blob and its
   referrers, builds every entry, writes generation N+1. This is the source
   of truth. It is idempotent: when nothing changed, the bytes equal the
   current generation and nothing is pushed.
   - Enumeration uses `_catalog` where the registry has one (zot). Where it
     has none, the list is the previous generation's repositories plus
     `--add <repository>` arguments.
2. **Publish hook** — after a successful `image push` / Push… into a namespace
   whose registry entry has `catalog = true`, the client fetches the current
   index, replaces the one entry, and publishes generation N+1. A failed hook
   never fails the push: the push result says "pushed; catalogue not updated,
   run `image catalog build`".
3. **CI or cron** — the same full rebuild run on a schedule next to the
   registry. Recommended for `unemu/` once it is on the internet; it repairs
   anything the hook lost.

New preference key (per namespace, in the existing `namespaces` list):
`{ name = "private", private = true, catalog = true }`. Default: off, except
the built-in default registry's `unemu/`, which is read-only for everyone but
the maintainer anyway.

**Concurrency.** OCI has no compare-and-swap on tags (§5.1 already says so)
[REPO]. Two hooks that run together can lose one entry. Measures, in order:

- Publish shards first, the manifest last; before the manifest `PUT`, read the
  tag again; if it moved, merge onto the new generation and retry (at most
  three times). This narrows the window; it does not close it.
- After the `PUT`, read the tag once more. When it names neither this
  generation nor a descendant (`parent` chain), merge and publish again.
- Loss is repairable, because every entry is derived from registry content: a
  full rebuild restores it. On registries without `_catalog` the lost
  repository must be named again with `--add`; the publisher's client
  remembers what it pushed (pull/push provenance in the local library,
  ASSUMED to be recorded or easy to record) and re-adds it on its next hook.
- `unemu/` has one writer (the maintainer), so the race is a `private/`
  matter only.

**Staleness detection (client).**

- Cheap check: `HEAD /v2/<ns>/catalog/manifests/v1`, compare the digest with
  the cached one. Done when Remotes is opened and on Refresh, never on a timer
  in the background of a running machine.
- Rollback rule: a fetched `generation` lower than the cached one is refused
  with "the catalogue went backwards", with an explicit "Use it anyway". This
  also surfaces a lost-update accident.
- Per entry: Pull always uses the entry's **digest** (as Get templates does).
  A 404 on it means the entry is stale: the row says so and offers Refresh.
- `image catalog verify <host>/<ns>` compares the index with the registry
  (tags and digests) and prints the difference without writing.

**Signing (later, P4-2).** Sign the catalogue manifest digest with cosign or
notation. The signature is itself a referrer (or a `sha256-<hex>.sig` tag),
so the client must already ignore referrers and tags it does not know. A
signed index pins the digest of every listed artifact, which gives one
signature for the whole namespace. `generation` gives rollback protection.
Nothing in schema 1 needs to change for this.

### 2.8 Local cache and search

**Cache.** A directory in the platform cache location, outside the OCI store:
`remotes/<host>/<namespace>/` holding the manifest, the config, the shards by
digest and a small `state.toml` (cached digest, generation, first-seen and
last-checked times). It is disposable and never a GC root of the library.
Every load verifies each shard's digest; a bad file is deleted and refetched.
Thumbnails and fetched attachments sit in `remotes/blobs/sha256/…`, shared
across registries, with a size cap (default 512 MiB, least recently used
evicted first).

**Engine.** In memory, built from the shards on load; no SQLite, no external
index crate. At 100 000 entries a linear scan over pre-tokenised titles is a
few milliseconds, which is below one frame. The work runs on the library
worker, never on the frame loop, like every registry operation today [REPO].

- **Tokenisation and diacritics:** move `summary::tokenize` (lower-casing, the
  Latin diacritic table that already covers Polish, combining-mark removal,
  CJK character and bigram tokens) [REPO] into a shared `image_store::search`
  module and use it for both the local library and Remotes. One behaviour, one
  test suite.
- **Matching:** every query term must prefix-match some token of the entry
  (the local library's rule today) [REPO].
- **Fields and facets:** the same facet set as `summary::Facets` (genre,
  platform, year, developer, publisher, series, label, firmware role, board,
  machine class, catalogue id, licence) plus `kind`, `reach` and `namespace`.
  Facet values match on the same title record, then group under the
  repository (§2.1 rule) [REPO]. Facet counts are computed over the current
  result.
- **Query syntax (CLI and search box):** free words plus `field:value`:
  `publisher:apogee year:1990..1993 genre:platformer kind:media license:open`.
- **Ranking**, a fixed tier order so results are deterministic: exact title;
  title prefix; any title word; series, developer, publisher; genre, tags,
  labels; description. Ties: `sort_title` (or title) folded, then repository.
  No fuzzy matching in the first version; trigram typo tolerance is a possible
  later bead.
- **Offline:** search and facets work entirely from the cache. Rows show a
  cached thumbnail when present and a placeholder otherwise.

## 3. Attachment (referrer) design

### 3.1 Types

An attachment is an OCI image manifest with a `subject` descriptor naming the
manifest of a machine image, media set or firmware package **in the same
repository** (the Referrers API is per repository [SRC 1]).

| Kind | `artifactType` | Layer formats | Per-file cap | Per-subject cap |
|---|---|---|---|---|
| Thumbnail (generated) | `application/vnd.unemu.attachment.thumbnail.v1` | PNG, JPEG, ≤ 256 px | 32 KiB | 1 per title |
| Cover | `application/vnd.unemu.attachment.cover.v1` | PNG, JPEG | 2 MiB | 8 |
| Box scan | `application/vnd.unemu.attachment.box-scan.v1` | PNG, JPEG; one layer per part | 16 MiB | 16 parts |
| Screenshot | `application/vnd.unemu.attachment.screenshot.v1` | PNG | 1 MiB | 32 |
| Manual | `application/vnd.unemu.attachment.manual.v1` | PDF, plain text, Markdown | 256 MiB | 16 |
| Other document | `application/vnd.unemu.attachment.document.v1` | PDF, PNG, JPEG, text, Markdown | 256 MiB | 32 |

- **Why one artifact type per kind:** the Referrers API can filter only on
  `artifactType` [SRC 1]. With separate types the details pane asks for
  covers alone and never downloads the list of 40 scans.
- **Config:** `application/vnd.unemu.attachment.config.v1+json`, versioned:
  `schema`, `kind`, `title_id` (which title of a multi-title image it belongs
  to), `title`, `language` (BCP 47), `region`, `edition`, `parts` (for a box
  scan: `front`, `back`, `spine`, `media-label`, `inlay`, in order), `pages`,
  `provenance` (free text, publish-safe), `replaces` (digest of an attachment
  this one supersedes).
- **Annotations mirror the listing fields**, because the Referrers response
  carries each manifest's `artifactType` and annotations in its descriptor
  [SRC 1]: `org.opencontainers.image.title`, `unemu.attachment.title-id`,
  `unemu.attachment.language`, `unemu.attachment.edition`,
  `unemu.attachment.sequence`, `unemu.attachment.replaces`, plus the summed
  size and the `unemu.redistribution` class. One list call is enough to draw
  the whole Attachments list; no attachment manifest is fetched until the
  user opens one.
- **Formats** follow decision-27's long-term readability rule (open, plain
  formats) [REPO]: PNG, JPEG, PDF, UTF-8 text. No TIFF, DjVu or comic archives
  in the first version. The import checks the file's magic bytes against the
  stated type and image dimensions (≤ 16 384 px a side) before anything is
  stored.
- **Viewing:** images are decoded by the window's `image` crate, which today
  has only `png` and `gif` enabled [REPO], so JPEG needs the feature added.
  PDFs and text are never rendered in the program: they open on the host, as
  media-set extras already do (decision-26 amendment, point 2) [REPO].

### 3.2 Licences and guard rules

The existing guard already decides on a manifest closure; an attachment's
closure is its config and layers, so `guard::check` applies unchanged [REPO].
Rules for marking:

| Kind | Default | May be stated `open` when |
|---|---|---|
| Thumbnail | inherits the class of its source image | never stated separately |
| Cover, box scan | `user-supplied` | the rights holder's licence covers the artwork, stated with `--open --license <SPDX>` |
| Manual, document | `user-supplied` | as above; a vendor's shareware terms usually cover the distributed archive, **not** the printed manual, so the terms of the media set do not carry over automatically |
| Screenshot | `user-supplied` | the subject's whole closure is `open` and the shot was produced from it; then it takes the subject's licence |

- The program never guesses `open` (decision-27 amendment, point 2) [REPO]. A
  scan of a commercial manual or box is proprietary and stays in a private
  namespace.
- An attachment stated open under a vendor's terms (`LicenseRef-…`) links the
  terms text with `unemu.terms` and **carries that text as its own layer**, so
  guard rule 5 ("terms text in the same closure") holds without a new rule.
  The blob is shared with the media set in the same repository, so it costs
  nothing.
- Because a referrer lives in its subject's repository, a proprietary scan
  **cannot be attached to a public artifact**: the guard refuses the push into
  the public namespace. This is the intended result. Whether private scans for
  public games get a home is open question 6.
- The dry run of an attachment push shows the same table as Push… (blob,
  effective class, where recorded) and has no override.

### 3.3 Languages and editions

- Several attachments of one kind may exist; `language`, `region` and
  `edition` distinguish them ("Manual — German", "Box — US big box").
- Display choice for the single cover shown in a row: local overlay cover
  first, then the in-manifest cover, then referrer covers ordered by: UI
  language match, then the title's first listed language, then `sequence`,
  then digest. The order is total, so the choice is deterministic.
- An attachment names a `title_id`, so a multi-title image shows the right
  manual under each title; without one it belongs to the whole artifact.

### 3.4 Listing and caching in the client

- **Normal path (catalogue present):** the entry already holds the thumbnail
  and cover descriptors and the attachment counts. The details pane lists
  attachments with one Referrers call for the selected row, on selection, not
  for every row.
- **Fetch policy:** thumbnails and one cover are fetched automatically for
  visible rows; everything else only on a click ("Fetch", with the size
  shown). `image pull --attachments[=cover,manual,…]` and a checkbox in Pull…
  fetch them with the artifact.
- **Local store:** attachments are stored in the OCI layout as untagged
  manifests, reachable from a local referrers table keyed by subject digest.
  That table is a GC root for them, the same way overlay artwork is a GC root
  today (§2.1) [REPO]. Deleting the last tag of a subject releases its
  attachments. Attachments never appear as library rows.
- **Lazy cache:** attachment blobs the user only looked at (not pulled) live in
  the Remotes blob cache (§2.8), not in the store.

### 3.5 Lifecycle

- **A new version loses its attachments,** because a referrer binds to a
  manifest digest. Publishing a new version of a repository therefore offers
  "Carry attachments forward" (default on): the client writes new referrer
  manifests with the same layers for the new digest. Only small manifests are
  uploaded. `image attach copy --from <ref> --to <ref>` does the same by hand.
- **Changing an attachment** = adding a new one whose `replaces` names the old
  digest. Clients hide a replaced attachment. Where the account may delete,
  `image attach remove` also deletes the old manifest; where it may not, the
  `replaces` chain alone gives the right view.
- **Deleting a subject:** the client deletes its referrers first, then the
  subject, then (fallback registries) the referrers tag [SRC 1].

### 3.6 Registries without the Referrers API

- **Probe once per registry and session:** `GET /v2/<repo>/referrers/<digest>`;
  `200` = native; `404` = fallback [SRC 1]. The answer is remembered in the
  session, not in preferences.
- **Read fallback:** fetch the tag `sha256-<hex>` as an image index; a missing
  tag means "no attachments". Filter by `artifactType` on the client. Also
  filter on the client when a native answer lacks `OCI-Filters-Applied`.
- **Write fallback:** after pushing a manifest with `subject`, when the
  response has no `OCI-Subject` header, read the fallback index, append the
  descriptor (with `artifactType` and annotations), push it back [SRC 1]. This
  read-modify-write has the same race as the catalogue; the same re-read and
  merge applies, and `image attach repair <ref>` rebuilds the index from the
  attachments the catalogue or the local store knows.
- **Fallback tags must stay invisible:** tags of the shape
  `<alg>-<hex>` (and cosign's `.sig`/`.att` suffixes) are filtered out of every
  tag listing the user sees: Remotes, Pull…, the catalogue's `tags`, the Get
  templates version choice.
- Neither path is needed for browsing when a catalogue exists, because the
  entry carries the digests. A registry with neither a catalogue nor
  referrers still shows in-manifest covers.

## 4. Remotes (upc-5pw.64) and the details pane

**Sidebar.** Library = the local store; Remotes = one node per configured
registry, one child per namespace that has a catalogue (or is listed in the
entry). Under a namespace, the same facet tree as the Library (Genre,
Platform, Year, Developer, Publisher, Series, plus Kind and Licence). The same
search box searches the selected scope.

**Rows.** Name, title, year, publisher, kind, size, a thumbnail, and the local
state: not in library / in library (same digest) / local tag names another
image (as `templates::Local`) [REPO]. Actions: Pull (by digest, through the
existing `PullRun`), Show details.

**Details pane for a remote row.** Title fields from the entry; cover; the
blob list with sizes and classes after one manifest fetch; an Attachments
list from one Referrers call; Fetch / Open for each. For a **local** row the
pane shows stored attachments from the local referrers table and, when the
artifact has a known origin registry, a "Check for attachments" action.

**States** (each needs a mock-up frame and a model test):

| State | What the user sees | Offers |
|---|---|---|
| Never fetched | "Not loaded yet" | Load |
| Loading | progress by shard; Cancel stops at the next read | Cancel |
| Fresh | rows | Refresh |
| Offline / unreachable | cached rows, banner "Offline — catalogue generation N, last checked <date>"; Pull disabled with a reason | Try again |
| No cache and offline | one message | Try again |
| Login required (`401`), private namespace | no rows, **no cached rows either** (see question 4) | Log in… (opens Preferences › Registries on that entry, as Get templates does) |
| Login refused / `403` | one message naming the namespace | Log in… |
| No catalogue artifact | "This namespace has no catalogue." On a registry with `_catalog`, a plain repository list (name, tag, title from annotations), as Get templates lists today | Build catalogue… (only when the account may push) |
| Unsupported authentication (Bearer) | "This registry uses token authentication, which is not supported yet" | — |
| Catalogue schema newer | rows with the understood fields, read-only banner | — |
| Partial failure (some shards) | loaded rows, banner "N of M parts could not be loaded" | Try again (fetches only the missing shards) |
| Stale entry (Pull answers 404) | row marked "no longer on the registry" | Refresh |
| Catalogue went backwards | message with both generations | Use it anyway / Keep cached |
| Attachment list failed | pane shows entry data and cover; "Attachments could not be listed" | Try again |
| Withdrawn artifact present locally | Library row badge "withdrawn by the publisher: <reason>" | — |

Timeouts are the short login-check ones Get templates uses (10 s connect,
15 s per read) [REPO]. Every request runs on the library worker. Nothing in
Remotes runs while no one looks at it: no background polling.

## 5. Interaction with existing pieces

| Piece | Effect |
|---|---|
| **Get templates…** | Becomes a filtered view of `unemu/catalog` (kind = machine, label `template`): two or three requests instead of a catalog walk plus tags and a manifest per repository. The current `_catalog` path stays as the fallback when no catalogue exists. `templates.rs` already skips repositories that are not PC machine images, so `unemu/catalog` does not disturb it before the switch [REPO]. |
| **Push…** | Unchanged for the image. After success, the hook (§2.7) when the namespace has `catalog = true`; an "Update catalogue" line in the result. Refuses the reserved repository name `catalog`. A new-version push offers "Carry attachments forward". |
| **Pull…** | Unchanged by default. Optional attachments checkbox. Pull by a catalogue entry uses the digest. |
| **Guard** | No rule changes. Called for the index and for every attachment. One new builder-side check (entries only from the target namespace; open only in a non-private one). `LocalMarks::scan` reads tagged manifests [REPO]; it must also read locally stored attachments so a restricted mark on a scan blob is honoured. |
| **Firmware packages** | Listed with their fit facets. Enables a later "find this firmware on a remote" by catalogue id. Their documents stay extras inside the package (decision-27 point 4); attachments are for material added after publication. |
| **Templates build** | `image template build` never pushes [REPO]; likewise `image catalog build --dry-run` prints the index and its digest without pushing. Publishing `unemu/catalog` is the maintainer's step. |
| **Immutable version tags** | Untouched: an attachment or a catalogue update never changes the subject's digest. The catalogue's own tag `v1` and the fallback tags are deliberately non-version tags. Where zot enforces tag immutability by pattern, the pattern must exclude `catalog:v*` and `sha256-*`. |
| **Tombstones** | The existing tombstones are local (discarded workspaces; machine-file card removal) [REPO] and unrelated. The catalogue adds `withdrawn` records: repository, manifest digest, reason. A client that holds a withdrawn digest shows a badge; nothing is deleted locally. Withdrawals are written by `image catalog withdraw` and kept across rebuilds until removed by hand. |
| **Local overlay** | Stays local. Only published metadata reaches the index. |
| **Archives** | OCI archive export may later include a subject's attachments (the layout can hold manifests with `subject`); not in this work. |
| **Collections (§5.4)** | Separate. The catalogue lists everything in a namespace; a collection is a curated subset. A collection could later be one more entry kind. |

## 6. Code map (where the work lands)

| Crate | New / changed |
|---|---|
| `image-store` | `catalog.rs` (schema, canonical encoding, sharding, entry projection from summaries); `search.rs` (tokeniser moved from `summary.rs`, query parser, ranking); `attachment.rs` (config codec, kinds, limits); `Manifest` gains an optional `subject`; store gains the local referrers table and its GC roots; `media_type` constants. |
| `registry` | `catalog.rs` (fetch with cache, build from a registry, publish with merge and re-read); `referrers.rs` (API, tag-schema fallback, `OCI-Subject`); `Client::referrers_page`, `put_manifest` returning response headers; reserved-name check in `push`; `test_support` gains the referrers endpoint and switches (no referrers API, no filter support, losing a concurrent write). |
| `pc-image` | Nothing structural; provider-declared values already flow through summaries. |
| `cli` | `image catalog build | update | verify | show | search | withdraw`; `image attach add | list | fetch | copy | remove | repair`; `--attachments` on `image pull`. |
| `window` | `remotes.rs` headless model (like `templates.rs`); sidebar section; details-pane attachments; Get templates switch. All after approved mock-ups. |

Hard-rule check: no emulator crate is touched; determinism of the execution
path is unaffected; every new module gets a doc-comment citing the OCI
Distribution Specification 1.1 by section; design text goes to
`machine-images.md` as new §5.6 (Catalogue) and §5.7 (Attachments).

## 7. Risks

1. **Lost catalogue update** when two clients publish together (no tag CAS).
   Mitigated, not closed: §2.7. Residual risk only in shared private
   namespaces.
2. **zot retention deletes untagged referrers or needed old generations.**
   Must be proven by the spike before publishing anything (P0-2).
3. **Public hosts need Bearer authentication.** Without P4-1 the feature works
   on zot (and other Basic-auth registries) only.
4. **Metadata leak.** A private title's name in a public index. Covered by the
   namespace rule in the builder and by the guard on the index; needs
   dedicated negative tests.
5. **Private catalogue at rest.** The cache holds private titles on disk.
   See question 4.
6. **Attachment loss on re-versioning** if carry-forward is skipped. The
   catalogue could show attachments of earlier versions as a hint.
7. **Fallback tag pollution** in tag listings; one filter function used
   everywhere, with a test per listing.
8. **Untrusted bytes.** Catalogue JSON, images and PDFs come from a registry.
   Size caps before decode, dimension caps, no in-app PDF rendering, digest
   verification of every blob.
9. **Scope growth.** This is post-MVP during a feature freeze; phases 1 and 3
   are independent and each ends in something usable.
10. **zstd byte stability** across crate upgrades changes shard digests
    (not correctness). Pinned versions already exist; a golden test catches it.

## 8. Open questions for the maintainer

1. **Catalogue repository and tag.** Recommended: `<ns>/catalog` with the tag
   `v1` (schema major), and `catalog` reserved as a repository name.
2. **Who updates the catalogue.** Recommended: explicit `image catalog build`
   is authoritative; the publish hook is opt-in per namespace
   (`catalog = true`); `unemu/` is rebuilt by the maintainer (later by CI).
3. **Is a title's name private?** Recommended: yes. A public index lists only
   its own namespace, and private repositories appear only in the private
   namespace's index, which needs a login.
4. **Private catalogue cache after logout or offline.** Recommended: keep the
   cache while logged in (so offline browsing works), purge the private
   namespace's cache and its cached attachments on Log out.
5. **Thumbnails.** Recommended: small referrer blobs referenced by digest from
   the entry (phase 3); before that, lazy in-manifest covers. No inline
   images.
6. **Proprietary scans for public games.** A referrer must live in the
   subject's repository, so a private scan cannot hang on a public shareware
   set. Recommended: not in this work. Later option: "detached" attachments in
   a private repository, linked by subject digest through the private
   catalogue.
7. **Bearer token authentication.** Recommended: schedule it only when the
   public namespace is about to move to a host that needs it; keep the
   catalogue work independent of it.
8. **zot search extension.** Recommended: do not use it; it cannot see the
   game fields.
9. **Attachment artifact types.** Recommended: one per kind (cover, box scan,
   screenshot, manual, document, thumbnail), because the Referrers API filters
   only on `artifactType`.
10. **Carry attachments forward on a new version.** Recommended: offered by
    default in Push… and in the CLI.
11. **In-manifest artwork.** Recommended: keep it as the portable default
    (decision-25 point 7 stays); referrers add to it and never replace it in
    archives.
12. **Withdrawals.** Recommended: yes, a `withdrawn` list in the catalogue
    header with a reason, shown as a badge on local copies; never an
    automatic local delete.
13. **Search syntax and fuzzy matching.** Recommended: words plus
    `field:value`, prefix matching, fixed ranking tiers; no typo tolerance in
    the first version.
14. **eXoDOS / LaunchBox metadata import** (already "future work only" in
    §2.1). Recommended: keep it out; the catalogue only publishes what the
    artifacts already carry.

## 9. Phases and beads

Sizes: S ≤ 1 day, M 2–3 days, L about a week. "Mock-up first" = no
implementation bead starts before the maintainer approves the mock-up.

### Phase 0 — decision and proof

| Id | Title | Scope | Depends | Tests / exit |
|---|---|---|---|---|
| P0-1 | Decision record: registry catalogue and attachments | Draft of §10; design text §5.6/§5.7 in `machine-images.md`; glossary entries | answers to §8 | maintainer accepts |
| P0-2 | Spike: referrers and catalogue churn on zot; probes of other registries | On a loopback zot with the production retention settings: push a subject and referrers with `oras`, run GC past `delay`, confirm referrers of a live subject survive and those of a deleted one follow `deleteReferrers`; `OCI-Subject` header; `artifactType` filter; 100 catalogue generations then GC. Read-only anonymous probes of the Referrers endpoint on a public Docker Hub and GHCR repository to replace the UNVERIFIED cells | — | a "Registry verification" paragraph like the 2026-11-14 one; synthetic data only |

### Phase 1 — catalogue, command line only

| Id | Title | Scope | Depends | Tests |
|---|---|---|---|---|
| P1-1 | Catalogue schema and canonical encoding | `image_store::catalog`: header, entry, shard function, encode/decode, caps, newer-schema projection | P0-1 | golden bytes and digest of a fixed catalogue; round trip; shard stability when entries are added; cap refusals; property test: encode is independent of input order |
| P1-2 | Entry projection and exclusion rules | Build an entry from a manifest, metadata blob and summaries; offered-version rule shared with Get templates; never-goes-in list | P1-1 | per-kind fixtures (machine, media set, firmware); negative tests: overlay notes, launch, foreign namespace, restricted entry in a public index |
| P1-3 | Shared search module | Move the tokeniser; query parser; facets; ranking tiers; used by the local library unchanged | P1-1 | existing summary tests still pass; ranking table tests; Polish and CJK cases; 100 000-entry timing bound in release mode |
| P1-4 | Registry: fetch, cache, verify | `image_registry::catalog` fetch by `HEAD` + shards, disk cache, rollback rule, partial failure result | P1-1 | mock registry: fresh, unchanged (one `HEAD` only, asserted on the request log), one shard changed (one blob fetched), corrupt shard, 401, no catalogue, newer schema, generation going backwards, cancel |
| P1-5 | Registry: build and publish | Full rebuild from a registry (with and without `_catalog`), guard on the index, reserved name, publish with re-read and merge | P1-2, P1-4 | determinism: two builds of the same mock registry give the same digest; idempotent rebuild pushes nothing; guard refusals; mock switch "another writer wins" exercises merge and the retry bound |
| P1-6 | CLI: `image catalog …` and the publish hook | `build`, `update`, `verify`, `show`, `search`, `withdraw`; `catalog = true` namespace key; hook after push | P1-3, P1-5 | CLI tests against the mock registry; hook failure does not fail the push; docs in `commands.md` and `status.md` |

Exit of phase 1: `image catalog search unemu "keen year:1991"` works offline
after one fetch, against the maintainer's zot.

### Phase 2 — Remotes in the window (with upc-5pw.64)

| Id | Title | Scope | Depends | Tests |
|---|---|---|---|---|
| P2-1 | **Mock-up: Remotes section and its states** | HTML mock-up under `backlog/docs/design/mockups/`: sidebar, rows, facets, every state of §4 | upc-5pw.64 direction, P0-1 | maintainer approval |
| P2-2 | Headless Remotes model | `window/src/remotes.rs` in the style of `templates.rs`: load, refresh, search, states, Pull by digest through `PullRun` | P2-1, P1-4 | model tests per state with the mock registry; cancel; login round trip |
| P2-3 | Remotes in the sidebar | egui wiring, thumbnails placeholder, lazy cover fetch | P2-2 | window-level tests as the library view has |
| P2-4 | Get templates through the catalogue | Use `unemu/catalog` when present; keep the `_catalog` fallback | P2-2 | existing Get templates tests on both paths; request-count assertion |

### Phase 3 — attachments

| Id | Title | Scope | Depends | Tests |
|---|---|---|---|---|
| P3-1 | Attachment codec and `subject` | `Manifest` optional `subject`; `attachment.rs`; type and size validation; magic-byte checks | P0-1, P0-2 | golden manifest bytes; existing manifests decode unchanged and keep their digests; refusals per cap |
| P3-2 | Local referrers table | Store API attach / list / release; GC roots; `LocalMarks` reads attachments | P3-1 | store tests incl. interruption and GC; guard honours a restricted mark recorded only on an attachment |
| P3-3 | Registry referrers client | Native API with paging and filter; tag-schema fallback read and write; `OCI-Subject`; fallback-tag filter in all listings | P3-1 | mock registry in three modes (native, native without filter, no API); concurrent fallback write; delete order |
| P3-4 | CLI: `image attach …`, guard, carry forward | add / list / fetch / copy / remove / repair; `--open --license`; terms-text layer rule; `pull --attachments` | P3-2, P3-3 | CLI tests; negative guard tests (commercial scan to `unemu/`; open under `LicenseRef-` without terms text) |
| P3-5 | **Mock-up: attachments in the details pane, and the import review** | list by kind, language/edition, fetch state, licence marks, dry run | P3-1 | maintainer approval |
| P3-6 | Details pane attachments | UI for local and remote rows; open on host; JPEG decoding feature | P3-4, P3-5, P2-3 | model tests per state |
| P3-7 | Thumbnails | generate at attach time (pure-Rust scaler, fixed filter), `thumbnail` kind, descriptors in catalogue entries, Remotes rows use them | P3-4, P1-5 | pinned thumbnail bytes for a fixture image; index size test |

### Phase 4 — optional, each needs its own go-ahead

| Id | Title | Note |
|---|---|---|
| P4-1 | Bearer token authentication in the client | Prerequisite for Docker Hub, GHCR, Harbor. The guard keeps refusing restricted pushes behind a Bearer challenge unless that rule is revisited. |
| P4-2 | Signature verification of the catalogue | cosign or notation; trust roots in preferences. |
| P4-3 | Scheduled rebuild next to the registry | A documented CI job; no code beyond P1-6. |
| P4-4 | Detached private attachments | Question 6. |
| P4-5 | Thumbnail packs per shard | Only if per-row blob requests are too slow. |

Test strategy across phases:

- **Determinism:** golden bytes and digests for the catalogue, attachment
  manifests and thumbnails; no wall-clock input to any encoder (a test builds
  twice under different fake times and paths and compares digests).
- **Mock registry:** every network behaviour is tested in-process on loopback
  with `test_support::MockRegistry`, extended in P1-4/P3-3; the request log
  asserts laziness (what was *not* fetched). No test needs a real registry.
- **Guard:** each new publish path has a refusal test for every rule.
- **Release-mode gate** as usual; the optional live-registry test stays
  opt-in, in the `unemu/test-…` repositories.

## 10. Draft decision record (outline)

**decision-29 — The registry catalogue is an OCI artifact per namespace;
artwork and documents attach as OCI referrers**

- Status: Proposed. Builds on decision-25 (point 7 and its amendments),
  decision-26, decision-27.
- Context: the registry is to be browsed like a game library; `_catalog` is
  not part of the OCI specification; version tags are immutable, so artwork
  added later cannot go into the manifest.
- Decision points:
  1. **A catalogue per namespace.** `<ns>/catalog:v1`, artifact type
     `application/vnd.unemu.catalog.v1`, schema versioned, the tag is the
     schema major; the repository name is reserved.
  2. **Content.** Published metadata of every machine image, media set and
     firmware package of that namespace; never local overlay data, launch
     records, payload bytes or another namespace's repositories.
  3. **Deterministic.** Canonical JSON, hash-sharded, no timestamp;
     `generation` and `parent` order generations; the same registry state
     gives the same digest.
  4. **Rights.** The index goes through the push guard; an index with any
     restricted entry is user-supplied and lives only in a private namespace.
     A title's name is treated as private content.
  5. **Building.** A full rebuild is authoritative; the publish hook is opt-in
     per namespace; concurrent updates are merged best-effort and repaired by
     a rebuild.
  6. **Client.** The index is fetched, verified, cached and searched locally;
     search works offline; zot's search extension is not used.
  7. **Attachments.** Covers, box scans, screenshots, manuals and documents
     are referrer artifacts with one artifact type per kind, bound to the
     subject's manifest digest in the same repository; they never change the
     subject. In-manifest artwork remains.
  8. **Attachment rights.** Every layer carries a licence and a redistribution
     mark; default user-supplied; open only by explicit statement; a scan of
     commercial material is proprietary.
  9. **Registries without referrers.** The client probes and falls back to the
     referrers tag schema of the OCI Distribution Specification 1.1; fallback
     tags are hidden from users.
  10. **New versions.** Attachments are carried forward explicitly.
  11. **Withdrawals.** The catalogue can mark an artifact withdrawn; clients
      show it and delete nothing.
  12. **Deferred.** Token authentication, signing, detached private
      attachments, thumbnail packs.
- Consequences: `Manifest` gains `subject`; the store gains a local referrers
  table; Get templates reads the catalogue; Remotes (upc-5pw.64) is built on
  it; GUI work starts from two mock-ups.

## 11. Sources

1. OCI Distribution Specification v1.1.1 — Endpoints table; "Listing Tags";
   "Listing Referrers"; "Pushing Manifests with Subject"; "Referrers Tag
   Schema"; "Deleting Manifests"; "Pushing Manifests" (size guidance); tag
   grammar under "Pulling manifests".
   https://github.com/opencontainers/distribution-spec/blob/v1.1.1/spec.md
2. OCI Image Specification v1.1 — image manifest (`artifactType`, `subject`),
   descriptor and image index. https://github.com/opencontainers/image-spec/blob/v1.1.0/manifest.md
   (cited from knowledge of the spec; not re-fetched in this session)
3. OCI announcement of the 1.1 releases (client responsibility for the
   fallback tag). https://opencontainers.org/posts/blog/2024-03-13-image-and-distribution-1-1/
4. zot — retention article (`deleteUntagged`, `deleteReferrers`, `delay`).
   https://zotregistry.dev/v2.1.5/articles/retention/
5. zot — GraphQL / search extension article (endpoint, queries, image fields).
   https://zotregistry.dev/v2.1.23/articles/graphql/
6. Docker Hub — OCI artifacts documentation.
   https://docs.docker.com/docker-hub/repos/manage/hub-images/oci-artifacts/
7. GitHub — Working with the Container registry (formats, 10 GB layers,
   10 minute uploads, personal access token, anonymous public pulls).
   https://docs.github.com/en/packages/working-with-a-github-packages-registry/working-with-the-container-registry
8. Third-party registry compatibility notes used only for the UNVERIFIED
   cells (GHCR, Docker Hub, Harbor referrers support); they disagree with each
   other and must be replaced by the P0-2 probes.
   https://git.stella-ops.org/stella-ops.org/git.stella-ops.org/src/branch/main/docs-archived/product/advisories/2026-02-19-oci-referrers-api-registry-compatibility.md
9. This repository: `backlog/decisions/decision-25…27`, 
   `backlog/docs/design/machine-images.md` §§2.1, 5, 5.1, 5.2, 5.4, 5.5,
   `crates/image-store/src/{artifact,metadata,summary,license,codec}.rs`,
   `crates/registry/src/{lib,client,guard,config,test_support}.rs`,
   `crates/pc-image/src/media_import.rs`, `crates/window/src/templates.rs`.
