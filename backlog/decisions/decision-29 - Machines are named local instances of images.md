---
id: decision-29
title: Machines are named local instances of images
date: 2026-11-15
status: Accepted
deciders: Mikołaj Mikołajczyk
---

# decision-29 - Machines are named local instances of images

## Status

Accepted — 2026-11-15. The maintainer asked for the image vs container split as
key MVP functionality (an exception to the post-MVP feature freeze) and approved
the mock-up `backlog/docs/design/mockups/machines.html` with every "please
confirm" item. Builds on [decision-25](decision-25%20-%20Machine%20images%20are%20self-contained%20OCI%20artifacts%20in%20a%20local%20store.md) (machine images, workspaces) and
[decision-26](decision-26%20-%20Installation%20media%20sets%20are%20their%20own%20OCI%20artifact%20type.md) (installation media). Tracked as `upc-5pw.63`.

## Context

An image is an immutable OCI artifact: firmware, configuration and disk layers.
Running it needs writable state: disk and floppy journals, CMOS, settings and
inserted media. Until now each image had one implicit regular workspace named
from its digest, plus disposable Gaming-mode workspaces. The user could not see
that state, could not keep two machines from one image (two DOS installations
from one template), and the library did not show the split that makes images
shareable and machines personal.

## Decision

1. **Machine.** A machine is a named, local, mutable instance of one image
   manifest (pinned by digest, remembered with the tag it was made from). It
   owns its journals, CMOS, machine settings, pending inserts, labels,
   favourite flag and last-run time. Machines never leave the local store and
   are never pushed; images stay immutable and are what moves between stores
   and registries.
2. **Several machines per image.** Names are free text, 1–64 characters,
   unique ignoring case; the default is the image title, then "title (2)". Each
   machine also has a fixed workspace id for the command line; renaming changes
   only the name.
3. **Library.** One Library group: Machines, Images, Installation media,
   Firmware, Machine files. Recently played and Favourites list machines. The
   library opens on Machines. A Remotes group follows after the MVP.
4. **Start.** On a machine it runs that machine. On an image with no machine it
   creates one named after the image and starts it; with machines it asks which
   (the last run preselected) or offers a new one.
5. **Settings.** Settings… on a machine changes that machine only, from its next
   start; Settings… on an image publishes a new image revision, as before.
6. **Commit and lifecycle.** Save as new image… writes the machine's changes
   and settings into a new image and, by default, moves the machine onto it
   with fresh journals. Duplicate… copies a machine with its changes. Reset to
   image… drops its changes and pending inserts and keeps its identity.
   Delete machine… never deletes the image; an image whose last tag is deleted
   while machines use it stays as "detached".
7. **Gaming mode** plays a disposable copy of a machine as it is (or of an
   image) and on exit offers Discard, Keep as new machine… or Keep as new
   image….
8. **New machine…** (File menu) keeps its name and builds both an image and a
   machine from it, unless the user opts out of the machine.
9. **Migration.** Existing regular workspaces become machines named after their
   image title; named command-line workspaces keep their names; unfinished
   Gaming-mode workspaces stay as such with Keep and Discard. Nothing is lost.

## Consequences

- The store gains machine records next to workspaces; the command line gains
  machine commands that mirror the desktop actions.
- Rebasing a machine onto a newer tag, machine export/import and machines on a
  registry are out of scope; images remain the unit of exchange.
