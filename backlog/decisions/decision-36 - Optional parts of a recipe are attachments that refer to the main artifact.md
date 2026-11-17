---
id: decision-36
title: Optional parts of a recipe are attachments that refer to the main artifact
date: 2026-11-17
status: Accepted
deciders: Mikołaj Mikołajczyk
---

# decision-36 - Optional parts of a recipe are attachments that refer to the main artifact

## Status

Accepted — 2026-11-17 by the maintainer (recipes.md Q5): attachments by OCI `subject`, in the specification now, built after the first recipes work. Design step of `upc-5pw.113`; recipes.md §3.9 and
§6.10. Uses the attachment direction of
`backlog/docs/design/registry-library-plan.md` §3.

## Context

A recipe for Windows 3.0 holds the installation disks and also the printed
documentation, because the project is an archive. The documentation is not
needed to run Windows, and few users have it. The build must need every
required input and take the optional ones it finds, and verification must
not depend on which optional files a user has.

A manifest lists its layers, so an artifact with an optional layer present
has another digest than one without. One tag cannot name both.

## Decision

1. **An optional part is an attachment:** its own OCI artifact whose
   manifest names the main artifact as its `subject` (OCI image-spec 1.1).
2. **A recipe declares attachments** as `[[attachment]]` tables, each with
   an id, a title, a kind, its own extras and its own pinned digest and
   lock.
3. **An attachment is built when every input it depends on is found**, and
   skipped otherwise. Within one attachment it is all or nothing; an author
   who wants each document on its own writes one attachment per document.
4. **The main artifact depends on required inputs only**, so its digest is
   the same for every user.
5. **The plan and Remotes show both counts:** required inputs found, and
   optional parts found.
6. **Rights are per artifact.** An attachment of user-supplied scans is
   user-supplied whatever the main artifact is.

## Consequences

- The manifest codec needs `subject`, and the store a table of referrers:
  the same work registry-library-plan phase 3 needs for covers and manuals.
  Until then attachments are specified and not built; the main artifact is
  unaffected.
- Recipe-built documents and later attachments appear in one place in the
  window.
- A registry without the Referrers API needs the tag fallback of the
  distribution specification.
- "Optional" is about what a build needs, not about what is shown: the
  licence text and the source archive of an open set stay required extras
  of the main artifact.

## Alternatives considered

- **Optional layers in the main manifest, some blobs absent.** The store
  and registries refuse a manifest with a missing layer.
- **One digest per combination.** Two to the power of the number of
  optional parts, all under one tag.
- **A second media set for the documents.** A set needs a medium.
- **No optional parts.** The archive loses its manuals.
