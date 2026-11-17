# FreeDOS 1.4 LiveCD, a CD-ROM taken unmodified

`private/media/ibm-pc/freedos:1.4-livecd-r1`. The archive's size, hash and
URLs and the ISO's hash are real (unemu-media `build.sh`); the ISO's size,
316,557,312 bytes, is from the earlier design draft, which read the
archive's directory. The boot diskette's hash and size were not measured:
the archive is 294 MB and was not downloaded for this corpus.

## What it exercises

- An optical medium (`iso-2048`) stored exactly as released.
- A medium that is a member of an archive: `unpack`, then `from =
  "cd/FD14LIVE.iso"`. No file-system step.
- `complete = true` over all three members.
- A set with two formats: a CD and a floppy.

## Findings

1. **Mixed formats in one set.** A CD with its boot diskette is one product
   and belongs in one set. Today's rule for loading a set into a drive wants
   one geometry (machine-images §5.2). The window needs a rule for such
   sets: offer each medium to the drives that take it.
2. **A member without a hash is allowed** when its archive is pinned
   (`FD14BOOT.img`). The lock pins it once the recipe is built.
3. **No licence statement, although the software is open.** An open set
   needs a licence index and the corresponding source of everything on it;
   for the LiveCD that is every package of the distribution. Until someone
   writes that, the recipe builds a user-supplied set that the push guard
   keeps private, and so its name is under `private/`. The format handles
   this without a special case.
4. **A 294 MB fetch** is the first case where consent and a size shown in
   advance matter (`can build after download (294 MB)`).
5. Insertion of optical media is disabled until CD-ROM emulation exists; the
   set can be built, stored and exported.
6. `[needs]` and the floppy's format are ASSUMED.
