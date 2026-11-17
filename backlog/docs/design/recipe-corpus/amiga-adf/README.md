# An Amiga disk image (a future platform), unmodified

`private/media/amiga/amdemo:r1`. Private, a user's own recipe. The
Amiga is not a platform of the family yet: the id `amiga`, the format `880K`
and the needs are ASSUMED. The size (901,120 bytes: 80 cylinders, 2 heads,
11 sectors of 512 bytes) and the hash are MEASURED. The disk was not
identified; its title is ASSUMED.

## What it exercises

- The smallest recipe: one input, one medium, no step.
- No release: nothing is known about the software, so the tag is `r1`.
- A user's own repository name under `private/`.

## Findings

1. **`release` must be optional for media too.** The first draft required
   it; this recipe had nothing honest to put there.
2. **An ADF is one of several encodings of an Amiga disk** (extended ADF,
   IPF, flux images). A recipe names the bytes of one. Finding the same
   disk in another encoding needs provider input forms (recipes.md §13).
3. **`kickstart = "1.2+"`** is a need that names firmware. It works as an
   ordered kind of the provider; whether needs should be able to point at a
   firmware package is open and not needed yet.
4. A draft like this, without a `digest`, is what a user writes for their
   own disks: it builds, and the result is tagged locally.
