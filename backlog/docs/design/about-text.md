# About text for a work: design note

Bead `upc-miu`. Status: **Accepted by the maintainer on 2026-11-19 with
every recommendation** of §13, together with the mock-up
[`mockups/about.html`](mockups/about.html) and the recommendation of each of
its own questions. **Nothing here is implemented yet.** This note stays the
reference for the implementation, which lands in the steps of §15. The
format is specified where it lives, each part marked "specified, not
implemented yet": metadata schema 3 in [machine-images](machine-images.md)
§2.1 and its compatibility rows in §5.6; the work-file keys, the text files,
the text layer, the static check and the guard rules R1 to R5 in
[recipes](recipes.md) §6.1, §6.11, §6.13, §6.16 and §7.1.

Markers: **[REPO]** = read in this repository on the day of writing;
**ASSUMED** = not verified, to be checked before code.

The direction the maintainer agreed is kept: a short `synopsis`, a long
`about` per work, a source for every `about` text, languages as a map, an
About section in the details pane. This note refines it. Where it departs
from the wording of the bead it says so and why (§4.4; §13, Q7).

**Revision 2** took in the maintainer's two answers to revision 1:

- **The about text is Markdown, and the program renders it.** A small, safe
  subset of CommonMark (§2.8), a parser and a renderer of our own (§2.9),
  links that open only after a question (§9). The stored form is the
  Markdown source; the earlier "a paragraph is one stored line" is gone
  (§2.3). The synopsis stays one plain paragraph.
- **An author may keep the text in a file beside the work file**:
  `en = { file = "freedos.about.en.md" }` (§5.5).
- Changed sections: §0, §2.3, §2.8 and §2.9 (new), §4.4 (warnings), §4.6,
  §4.7, §5.1, §5.2, §5.5 (new), §6, §7, §9, §10, §11, §12, §13 (Q9, Q17 and
  Q18 replaced; Q22 to Q31 new), the test plan and the sources. The sample
  texts are English throughout, also under the `nl` tag.

## 0. The design in ten lines

1. Metadata **schema 3** adds two title fields, `about` and `about_source`,
   and gives `synopsis` a second shape: a map from a language tag to text.
2. `synopsis` stays one plain paragraph, at most 500 characters. `about` is
   **Markdown source** in a stated subset, at most 16 384 bytes per language,
   at most 16 languages. A reader without the renderer shows the source,
   which the subset keeps readable.
3. Both belong to the **work**: written once in the work file, or in a text
   file the work file names, copied whole (every language) into each
   release's title record, frozen by the lock.
4. Every `about` text has a source per language: `own`, or `licensed` with a
   title, authors, an `https` URL and one of five Creative Commons licences.
5. A schema-2 synopsis is read as English. A title with an English synopsis
   only and no `about` is still written as schema 2, byte for byte as today.
6. Release 0.0.3 shows a schema-3 record read-only without its synopsis and
   still runs the artifact. Metadata still never raises
   `unemu.min-app-version`.
7. An `own` text in an open artifact is published under `CC0-1.0` with the
   rest of the record. A `licensed` text adds its licence to the metadata
   layer's: `CC0-1.0 AND CC-BY-SA-4.0`.
8. Text without a source never leaves for a public registry and never
   travels in a recipe artifact; a private registry keeps the user's own
   copy, as it does for every other user-supplied part.
9. Only an `https` address ever becomes a link, a link opens only after the
   user confirms the host and the address, and the program fetches nothing
   by itself: no image, no preview.
10. The viewer's languages are a preference of the desktop; a missing
    language falls back to English and the pane says so. Remotes reads the
    synopsis and the about text from the title record of the selected tag,
    which it already fetches. No new manifest annotation.

## 1. What exists today

- `description` says what this release is; `synopsis` (schema 2) says what
  the work is, "String without control characters … Blank text is not
  written" (machine-images §2.1 [REPO]). The desktop draws neither the
  synopsis nor any long text.
- A recipe states `synopsis` only in a work file; the title record's
  `description` is the recipe's, or the synopsis when the recipe states none
  (recipes §6.16, decision-39 point 5 [REPO]).
- "A writer uses the oldest schema that holds every field a title sets";
  the choice is data, `image_store::compat::TITLE_FIELDS` and
  `METADATA_SCHEMAS` [REPO]. The newest published release is 0.0.3;
  `NEXT_RELEASE` is 0.0.4 [REPO].
- A reader that meets a newer schema projects it: it keeps each field it
  knows whose value has the shape it expects, drops the rest and marks the
  record read-only (`Metadata::project` [REPO]).
- The metadata blob is at most 1 MiB (`MAX_SMALL_BLOB` [REPO]).
- The generated config and metadata of an open set are marked open under
  `CC0-1.0`: "this program's own CC0 output (the title and labels the user
  typed), not the vendor's work" (machine-images §5.2 [REPO]). A recipe
  artifact states `CC0-1.0` for the recipe, its bundled texts and its lock
  (recipes §6.6, §7.1 [REPO]). **A text copied from somewhere else does not
  fit that statement**, which is why §4 is a large part of this note.
- The store's SPDX list already knows `CC0-1.0`, `CC-BY-3.0`, `CC-BY-4.0`,
  `CC-BY-SA-3.0` and `CC-BY-SA-4.0` (`image_store::license` [REPO]), so every
  published release parses a licence expression that names them.
- The summary cache (schema 8) keeps facets and search tokens per title, not
  the texts; tokens come from every string of the title record
  (`summary::title_summary` [REPO]). The cache is capped at 64 MiB; over the
  cap the largest rows lose their tokens and say "search text reduced".
- Remotes lists from the catalog, the tag lists and the newest tag's
  manifest; the details pane of a selected tag also reads that tag's title
  record for the edition and the version (machine-images §3 [REPO]).
- A work file is not pinned by the recipes that name it; a pinned recipe
  builds its locked metadata, and `check` lists where the work file now
  differs (decision-39 point 7 [REPO]). A recipe artifact carries the work
  file as a layer, every part at most 1 MiB and named by one file name in
  `org.opencontainers.image.title`; a bundled input must be text, "UTF-8
  without NUL" (`recipe::artifact` [REPO]).
- The desktop has no notion of the viewer's language, no bold face (a
  heading is drawn in the strong text colour) and no code that opens a web
  address [REPO: `ui_kit::pane`, `ui_kit::skin`].

## 2. Schema 3

### 2.1. Localized text

A **localized text** is a JSON object from a language tag (§2.5) to a
string. It has 1 to 16 entries; an empty object is not written (the field is
left out). Keys are written in byte order, as every object of the canonical
writer is. No entry is blank.

### 2.2. `synopsis`

| | Schema 2 (unchanged) | Schema 3 |
| --- | --- | --- |
| Shape | string | localized text |
| Content | no control characters; blank is not written | each entry: no control characters, not blank, at most **500 characters** (Unicode scalar values) |
| Meaning | what the work is, shared by its releases | the same, per language |

One paragraph of **plain text**: a line break is a control character, so a
synopsis cannot hold one, and no markup is read in it. It is drawn in list
hovers and cards and copied into `description` and an OCI annotation (§5.3),
places that show characters as they are, so a `*` in a synopsis is a `*`.
The 500-character limit is a writer's rule and a rule of the schema-3 codec.
A schema-2 record with a longer synopsis still reads, and is rewritten as it
was read; it cannot move to schema 3 until it is shortened.

Control characters are C0 and DEL, as today (`metadata::has_control` [REPO]).

### 2.3. `about`

Localized text. Each entry is the **Markdown source** of the text, in the
subset of §2.8, in **canonical form**:

- lines separated by one `LF`; no `CR`; no line break after the last line;
- no tab, and no other control character: C0 except `LF`, DEL, and U+0085,
  U+2028 and U+2029 (line breaks Markdown does not know);
- no space at the end of a line;
- no empty line at the start or the end, and never two empty lines in a row;
- at most **16 384 bytes** of UTF-8, and at least one character.

Spaces at the start of a line are kept (they carry the nesting of a list),
and so are the author's line breaks inside a paragraph: the source is stored
as the author wrapped it. Two sources that differ only in where they wrap
draw the same text and have different bytes, as two source files do.

With 16 languages a title's about text is at most 256 KiB; the 1 MiB cap on
the blob bounds an image with many titles, as it does today.

**Normalisation at the door.** Text reaches a record through a writer's
entry point: the recipe engine reading a work file or a file it names (§5),
an editor later. That entry point brings the text to canonical form, in this
order, and the codec then accepts canonical text only:

1. `CR LF` and a lone `CR` become `LF`.
2. A tab becomes spaces up to the next multiple of four columns, the tab
   stop CommonMark gives it [CM 2.2].
3. Spaces at the end of every line go.
4. Empty lines at the start and the end go, and a run of empty lines becomes
   one.
5. Then the checks: another control character is refused with its line
   number; a text over the limit is refused with its size; a text with
   nothing left is refused ("leave the key out").

The steps need no parser, change nothing a Markdown reader would show, and
give the same bytes whatever line ends the author's editor wrote. They are
needed, not a convenience: TOML lets a parser choose how it reads a line
break inside a multi-line string [TOML 1.0.0, String], so without step 1 the
same work file could build two digests.

There is **no format marker** in the record: `about` is Markdown from its
first release. No Unicode normalisation is applied; the search tokenizer
already folds case and Latin accents [REPO].

The synopsis is not normalised: it is stored as given, as today.

### 2.4. `about_source`

An object from a language tag to a **source**. Its keys are a subset of the
keys of `about`: a source for a language that has no text is refused by the
codec. A text without a source is valid data; §4.4 says where it may go.

| Key | Shape | `own` | `licensed` |
| --- | --- | --- | --- |
| `kind` | lowercase ASCII letters, digits and `_`, starting with a letter, at most 32 characters (the shape of a need kind) | required | required |
| `authors` | array of 1 to 16 strings, each at most 128 characters, not blank, without control characters | optional: a credit | required: the creators, or the source's own collective name ("Wikipedia contributors") |
| `title` | string, as one author | refused | required: the name of the source work ("FreeDOS") |
| `url` | `https` URL, ASCII, without white space, at most 512 bytes | refused | required: where the text was taken from; a permanent link when the source has one |
| `license` | one SPDX licence identifier of the store's list | refused | required: one of `CC0-1.0`, `CC-BY-3.0`, `CC-BY-4.0`, `CC-BY-SA-3.0`, `CC-BY-SA-4.0` |
| `changes` | string, as one author | refused | optional: what was changed against the source ("abridged", "translated from English"). Absent means the text is the source's own words |

Every string here is plain text; no markup is read in a source.

As for needs and outside catalogues (machine-images §2.1 [REPO]), the
**decoder checks shapes** and the **writer checks the vocabulary**: a reader
keeps a `kind` or a `license` of a valid shape that a later release adds,
shows it as written and refuses only to rewrite it; a writer states only the
two kinds and the five licences above. An unknown key is refused, as
everywhere.

`license` is one identifier, not an expression. A source offered under
several licences (Wikipedia's older text is also under the GFDL) is taken
under the one the author names.

Not included, on purpose: a `quoted` or `unlicensed` kind (§4.6), a
public-domain kind (a later kind, when a first text needs it), the date the
text was fetched (a permanent link says more).

### 2.5. Language tags

A subset of BCP 47 [RFC 5646], written in its canonical case:

```
tag = language [ "-" script ] [ "-" region ]
language = 2 or 3 lowercase letters      (ISO 639: en, pl, nl, yue)
script   = 4 letters, the first capital  (ISO 15924: Hans, Latn)
region   = 2 capitals or 3 digits        (ISO 3166-1 or UN M.49: BR, 419)
```

`en`, `nl`, `pt-BR`, `zh-Hans`, `sr-Latn`. No variants, extensions or
private-use parts. The decoder checks the shape only, so it needs no
registry; a writer also refuses `und`, `mul`, `mis` and `zxx`, which name no
language. Another case (`EN`, `pt-br`) is refused with the canonical
spelling in the message, so one language has one key.

### 2.6. Which schema a writer uses, and the migration

A schema-2 record never stated the language of its synopsis. **This design
reads it as `en`.** Every synopsis the project has written is English, as
everything it commits is; a private record in another language is
mislabelled until its author states the language, which changes nothing but
the name the language switch shows (§13, Q1).

In memory the synopsis is a localized text in every schema. The writer still
uses the oldest schema that holds what a title sets:

| A title sets | Envelope | `synopsis` is written as |
| --- | --- | --- |
| none of the schema-2 or schema-3 fields | schema 1 | — |
| a synopsis in `en` only, no `about` | schema 2 | the string, **the bytes of today** |
| a synopsis in another language, or in several; or `about` | schema 3 | the map |

The envelope takes the newest schema any of its titles needs, and in a
schema-3 envelope every title's synopsis is a map: one schema, one shape. A
schema-3 blob with a string synopsis, and a schema-2 blob with a map, are
refused as a field of the wrong schema is today.

So the migration rewrites nothing:

- every published artifact keeps its digest, and an identical re-import is
  still "already imported";
- a work file that changes `synopsis = "…"` into `synopsis = { en = "…" }`
  builds the same metadata bytes, so `recipe check` reports no difference
  against any lock and no release needs a new revision;
- a release moves to schema 3 on the day its work gains an about text or a
  second language, with a new revision, as recipes §8.4 publishes any
  metadata change.

**What older releases show** for a schema-3 record:

| Release | Shows |
| --- | --- |
| 0.0.1, 0.0.2 (read schema 1) | The record read-only, without the fields of schemas 2 and 3. Runs the artifact. |
| 0.0.3 (reads schema 2) | The record read-only. `about` and `about_source` are unknown and left out. `synopsis` is an object where it expects a string, "a schema-2 field of an invalid shape left out": **no synopsis**. Everything else as before: `description`, needs, work id. Runs the artifact. [REPO: `Metadata::project`] |
| 0.0.4 | Everything. |

Nothing the user of 0.0.3 sees today disappears, because the desktop of
0.0.3 never drew the synopsis; `upcli image media show` of 0.0.3 loses its
`synopsis` line for such a record. The release's `description` still reads,
and when the recipe states none it is the English synopsis (§5.3).

### 2.7. Example

```json
{"schema":3,"platform":"ibm-pc","labels":["freedos","open-source","operating-system"],
 "titles":[{"id":"freedos","title":"FreeDOS",
   "description":"FreeDOS 1.4 on four 1.44M diskettes.",
   "version":"1.4","work_id":"freedos",
   "synopsis":{"en":"A free, open source operating system compatible with MS-DOS, by the FreeDOS Project.",
               "nl":"(the synopsis in Dutch)"},
   "about":{"en":"FreeDOS is a complete, free operating system …\n\n## What is on the disks\n\n- a package manager, `FDIMPLES`;\n- several text editors and compilers.\n\nThe project's site is [freedos.org](https://www.freedos.org/).",
            "nl":"(the text in Dutch)"},
   "about_source":{
     "en":{"kind":"own","authors":["unemu contributors"]},
     "nl":{"kind":"licensed","authors":["Wikipedia contributors"],"title":"FreeDOS",
           "url":"https://nl.wikipedia.org/w/index.php?title=FreeDOS&oldid=00000000",
           "license":"CC-BY-SA-4.0","changes":"abridged; converted to Markdown"}}}]}
```

`about` and `about_source` follow `work_ids` in the record, so the fixed
order of the canonical writer only grows at its end. (The entries under `nl`
are placeholders: this repository is written in English. They stand for an
abridged article and its made-up revision number.)

### 2.8. The Markdown subset

The reference is the CommonMark specification, version 0.31.2 [CM]; section
numbers below are its. The subset is chosen by three rules:

1. **Readable as source.** Whatever is accepted reads well as plain text,
   because the command line, an export, a registry's web page and any reader
   without the renderer show the source.
2. **Agreement.** A text that `recipe check` passes without a warning is
   read by this renderer as a CommonMark renderer reads it. Nothing accepted
   means something else here.
3. **No error at display time.** The parser is total: every canonical text
   gives something to draw. What is outside the subset is **shown as
   typed**, its source characters in place, and `recipe check` warns (§4.4,
   W4). It is never hidden, never guessed at and never a reason to refuse a
   record.

**Accepted blocks.**

| Construct | [CM] | Accepted form | Drawn as |
| --- | --- | --- | --- |
| Paragraph | 4.8 | In full, with lazy continuation lines. A line break inside it is a soft break [CM 6.8]: a space | Wrapped text |
| Heading | 4.2, 4.3 | ATX (`#` to `######`) and setext (a line of `=` or `-` under a paragraph). **Every level is drawn alike**: the text has one level of headings under the pane's own "About" | A line in the heading style of the pane's sections, with room above it |
| Thematic break | 4.1 | In full (`---`, `***`, `___`) | A thin rule |
| Block quote | 5.1 | Lines that start with `>`; inside it paragraphs only, with their inlines. No quote, list or heading inside a quote | Indented text beside a bar |
| List | 5.2, 5.3 | Bullet (`-`, `+`, `*`) and ordered (`1.` or `1)`, starting where the first item says). An item is one paragraph and, under it, at most one nested list: **two levels**. An ordered list interrupts a paragraph only when it starts at 1, and a changed marker starts a new list, as in [CM]. Empty lines between items change nothing: every list is drawn tight | A marker and a hanging indent; `•` at the first level, `–` at the second, the number and a full stop for an ordered item |

**Accepted inlines.**

| Construct | [CM] | Accepted form | Drawn as |
| --- | --- | --- | --- |
| Backslash escape | 2.4 | In full: a backslash before ASCII punctuation | The character |
| Code span | 6.1 | In full | The monospace face |
| Emphasis, strong emphasis | 6.2 | **In full**, the delimiter-run rules and the algorithm of the specification's appendix, for `*` and `_`. A shorter rule would read `2*3*4` and `snake_case_name` differently from every other reader | Emphasis in italics (egui slants the face). Strong emphasis in the strong text colour, as headings are, until a bold face is bundled (§13, Q26) |
| Link | 6.3 | The inline form only, `[text](destination)`, without a title. The text may hold emphasis, strong emphasis and code, and no link. The destination is a **link address** (below) | Underlined text in the link colour; a click asks first (§9) |
| Autolink | 6.5 | `<https://…>` when what is inside is a link address | The address as a link |
| Bare address | [GFM 6.9] | `https://…` at the start of a line or after white space or `(`, to the next white space or `<`, without the punctuation that ends a sentence after it, by the extended-autolink rule of GitHub's specification. This is the one construct taken from outside CommonMark: an address written out is what reads best as source | The address as a link |

**A link address** is what may become a link, here and in `about_source`:
`https://`, then a host of ASCII letters, digits, `-` and `.`, an optional
port, and an optional path, query and fragment of printable ASCII without
space, `<`, `>` or backslash, with parentheses balanced; at most 512 bytes.
No other scheme, no user name or password before the host, no character
outside ASCII (a host in another script is written in its `xn--` form, which
the confirmation then shows as it is). A destination that is not a link
address makes no link: the construct is shown as typed, so the reader sees
the address and nothing can be clicked.

**Shown as typed**, with a warning from `recipe check`:

| Construct | [CM] | Why it is left out |
| --- | --- | --- |
| Raw HTML, blocks and inline | 4.6, 6.6 | A second language with its own escapes and its own risks. `<b>` is three characters |
| Image | 6.4 | The program fetches nothing by itself, and an image inside the record has no place to come from. `![alt](address)` is shown whole and is not a link |
| Link reference definition, reference link | 4.7, 6.3 | The definition would vanish from the drawn text while every source reader shows it |
| Link title, a destination in `<…>` or with a space | 6.3 | One spelling of a link |
| Code block, fenced or indented | 4.4, 4.5 | Left out of the first subset (§13, Q23). Its lines are read as paragraphs |
| Hard line break | 6.7 | Two spaces at a line's end are invisible, and normalisation removes them; a backslash there is shown |
| Entity and numeric character reference | 2.5 | `&amp;` is five characters. The text is Unicode already |
| A list or a quote nested deeper than the table above says | 5.1 to 5.3 | A third list level is drawn at the second; a list, quote or heading inside a quote is shown as the quote's text |
| Tables, footnotes, strike-through, task lists | not [CM] | Extensions |

**The subset only grows.** A later release may accept a construct of the
second table; none may change what an accepted construct means. An older
release then shows the newer construct as typed, which rule 1 keeps
readable, so widening the subset needs no schema and no compatibility row.

### 2.9. Parser, renderer and tests

- **The parser** is our own code in a workspace crate without dependencies
  (proposed name `about-text`): the normalisation of §2.3, the check of the
  canonical form, the parser to a small tree of the blocks and inlines of
  §2.8, the list of findings for `recipe check`, and the text without
  markup for search (§7). It has no egui in it because `recipe check`, the
  metadata codec, the command line and the summary cache need it, and they
  must not depend on a user interface.
- **The renderer** is `ui_kit::about`: it draws the tree with egui widgets
  the kit already has (wrapped labels, the monospace face, the strong
  colour, a separator). The kit gains one dependency, the parser crate. **No
  external crate is added.**
- **Bounded.** The input is at most 16 384 bytes. Block nesting is fixed by
  the grammar. Inline nesting deeper than 16 levels is shown as typed. The
  emphasis algorithm is the specification's, linear in the length of the
  text. There is no recursion on input the grammar does not bound, and no
  back-tracking search.
- **Deterministic.** The tree is a function of the canonical text. Nothing
  depends on the host, the locale or the pane's width; only line wrapping
  does, in egui.
- **Tests.**
  1. The examples of the CommonMark specification (`spec.json`, 652 examples
     in 0.31.2), as a fixture. A table in the test names, per section, the
     examples that stay inside the subset; for each of them the tree,
     written out as the HTML the specification expects, must equal the
     expected HTML. Every other example must parse without a panic, give at
     least one finding, and draw every character of its source that is not
     markup of an accepted construct.
  2. The bare-address rule against the examples of [GFM 6.9] that use
     `https`.
  3. The link-address grammar: each refusal of §2.8 (another scheme, a user
     name, a character outside ASCII, unbalanced parentheses, 513 bytes).
  4. A property test over random text and random nesting: the parser ends,
     never panics, and normalisation is idempotent.
  5. Layout tests of the renderer in both skins and both themes, as the
     kit's other widgets have.

  The specification and its examples are under CC BY-SA 4.0 (ASSUMED from
  the specification's front page; to be confirmed before the fixture is
  committed). The fixture is test data with its licence and notice beside
  it, as the bundled font has; it is not part of the program.

## 3. The viewer's languages and the fallback

The desktop gets one preference, **Catalogue text languages**: an ordered
list of language tags (Tools › Preferences… › General). Its default is the
language of the host's user interface, read once when the preference is
first needed, then `en`. It is a display setting of the shell: it reaches no
artifact byte and nothing in the execution path (hard rule 4).

`synopsis` and `about` are looked up **separately**, since a title may have
a synopsis in more languages than it has about texts:

1. For each preferred tag in order: the entry with that tag; else the entry
   the tag gives after its last part is cut off, repeatedly (`pt-BR`, then
   `pt`); else an entry with the same language part (`pt` finds `pt-PT`),
   the first in byte order.
2. Else `en`, matched the same way.
3. Else the first entry in byte order.

When the entry shown is not the first preference, the pane says so in small
print ("Not available in Italian; shown in English"). When a title has
several languages the About heading carries a switch; a choice there lasts
while the title is selected and changes no preference. The source line
always belongs to the language shown. The program never translates.

Languages are named in English ("Dutch", "Italian") from a small table in
the shell, and by their tag when the table has no name: the interface is
English, and native names would bring characters the bundled fonts lack
(§13, Q15).

**What the fonts can draw.** The desktop's fonts draw Latin, Greek and
Cyrillic text; they have no CJK glyphs, and egui does no right-to-left or
complex shaping (ASSUMED from the font list of `backlog/docs/design/icons.md`;
to be measured). The schema takes any language. The desktop skips, in steps
1 to 3, an entry it cannot draw, lists it in the switch as "cannot be shown
by this release", and falls through to the next one (§13, Q16).

## 4. Rights: the source and the guard

### 4.1. Why a source at all

The text on a box, in a manual, in a review and on a store page is somebody's
work, and a registry is a publication. Wikipedia's text may be reused, under
CC BY-SA 4.0: with the authors credited, the licence named and linked, a
link to the source, a note of whether it was changed, and any adaptation
under the same licence [CC BY-SA 4.0, sections 3(a) and 3(b); Wikimedia
Terms of Use, section 7, which accepts a link to the page as the credit].
The record therefore carries, beside each text, what a reader needs to
credit it, and the guard refuses to publish a text that carries nothing.

### 4.2. What the two kinds state

- **`own`**: the person who wrote the record wrote this text. In an **open**
  artifact it is published with the record, under `CC0-1.0`, the licence the
  project gives everything a recipe's author types (recipes §6.6 [REPO]). An
  author who wants credit or another licence publishes the text somewhere
  and names it as a `licensed` source (§13, Q4). `authors` on an `own` text
  is a courtesy line, not a condition.
- **`licensed`**: the text is somebody else's, or the author's own under a
  licence, taken from `url` under `license`. An abridgement, a translation
  and a conversion to Markdown leave it a `licensed` text: `changes` says
  what was done, `authors` keeps the original credit (and may add the
  translator), and for a share-alike licence `license` is the source's
  licence.

Both are **claims by the author**, like `[license]` in a recipe ("A recipe's
licence statement is a claim by its author", recipes §6.6 [REPO]).

### 4.3. The licence of the metadata layer

Today the metadata layer of an open artifact is marked
`unemu.redistribution = open`, `unemu.license = CC0-1.0`. With an about
text:

| The record's about texts | Metadata layer of an open artifact | Manifest `org.opencontainers.image.licenses` |
| --- | --- | --- |
| none, or all `own`, or `licensed` under `CC0-1.0` | `CC0-1.0`, as today | as today |
| some `licensed` | `CC0-1.0 AND <each other licence once, in byte order>`, for example `CC0-1.0 AND CC-BY-SA-4.0` | gains the same licences, by the existing rule (the conjunction of the layer licences) |
| some without a source | **the artifact cannot be open**: the writer refuses the open statement (§4.4, R2) | — |

The expression says what is true of the blob: the record is `CC0-1.0`, the
text inside it is under its own licence. Releases 0.0.1 to 0.0.3 parse it
and count the layer as open [REPO]. A user-supplied artifact carries no
licence marks, as today, whatever its record holds.

This amends one sentence of machine-images §5.2 ("the generated metadata
layer is this program's own CC0 output") and the lock comparison of
`recipe::lock::marks`, which accepts a generated layer "open under CC0-1.0
at most" [REPO]: the metadata layer's licence becomes "`CC0-1.0` and the
licences its about sources state, no more".

Share-alike is read here as binding the text and anything made from it, not
the disks beside it in the same artifact: the record and the text sit in one
blob as a collection. This is the project's reading, not legal advice; the
maintainer confirms it before the first CC BY-SA text is published under
`unemu/` (§13, Q6).

### 4.4. The rule

**Refused.**

| # | Where | What |
| --- | --- | --- |
| R1 | The codec, always | A source for a language without a text; a source of the wrong shape; an about text that is not in canonical form (§2.3). Writers also: a kind that is not `own` or `licensed`; a `licensed` source without `title`, `authors`, `url` or `license`; a `url` that is not a link address (§2.8); a licence outside the five; a key the kind does not take; a `url` on a Wikimedia host (`wikipedia.org`, `wikimedia.org`, `wikibooks.org` and their subdomains) whose `license` is not `CC-BY-SA-4.0` or `CC-BY-SA-3.0`. |
| R2 | `recipe check`, `recipe build`, and any other writer that marks an artifact open | An about text without a source in an artifact stated open. The metadata layer of an open artifact is marked open, and a text nobody vouches for cannot be. |
| R3 | The push guard, as **rule 6** | Into `unemu/` on any host, or into a namespace or registry not marked private: every about text of every title in the pushed metadata needs a source of a kind this release knows, and the licence of each `licensed` source must be named by the metadata layer's own `unemu.license`. The refusal names the title, the language and what is missing, and says "There is no override for this rule." A namespace marked private outside `unemu/` is not checked: it keeps the user's own copy, as rules 4 and 5 leave it. |
| R4 | The recipe guard, on every `recipe push` | A bundled work file, a text file it names (§5.5), or locked metadata, with an about text that has no source. A recipe artifact is public text whatever its output is ("recipes for private media are public, their outputs are not", recipes §7.3 [REPO]), so it has no private case. With a `licensed` text aboard, the layer that holds it says so: a text file's layer is marked with its source's licence alone, the work-file layer (for a text written inline) and the lock's metadata layer `CC0-1.0 AND <licences>`, and the artifact's `org.opencontainers.image.licenses` names them all, where today it is `CC0-1.0`. |
| R5 | The push guard, into `unemu/` only | A `synopsis` or an `about` without an `en` entry. The public catalogue's common language is English, and English is where the fallback of §3 ends (§13, Q8). |

**Links add no refusal.** Only a link address becomes a link (§2.8), by the
way the text is read and in every reader, so there is nothing for a guard to
let through or stop: a destination with another scheme is text. The record
holds **no list of its links**; they are read from the text when they are
wanted, and a list beside the text would be a second statement that could
disagree with the first (§13, Q28).

**Warned** (`recipe check` prints them and still passes; the push dialog
shows them as note lines, with the mark it uses for a note):

| # | What |
| --- | --- |
| W1 | An about text without a source in a recipe without `[license]`: "builds, and stays in this library and on private registries; the recipe cannot be pushed". |
| W2 | An `own` text that reads as pasted: a bracketed reference mark (`[12]`, `[citation needed]`), "All rights reserved", a copyright sign or `(c)` before a year. The warning names the line and the pattern. |
| W3 | A `licensed` source on a Wikimedia host whose URL names no revision (`oldid=`): "the article will change; link the revision the text was taken from". |
| W4 | Markdown outside the subset, each with its line: "shown as typed". Also a line indented by four spaces or more, which other readers show as a code block. |
| W5 | `[text](destination)` or `<…>` whose destination is not a link address: "not a link; shown as typed". |
| W6 | A block quote in an `own` text: "a quoted passage; name its source in the text" (§4.6). |
| Note | The hosts the text links to, one line: "links to en.wikipedia.org, www.freedos.org". Not a warning: it puts the addresses in front of whoever reviews the work file. |

A link from an `own` text **to** a Wikimedia page is not warned: pointing at
an article is what an own text should do in place of copying it. For that
reason the name of a Wikimedia project is no longer one of W2's patterns, as
it was in revision 1.

W4 and W5 stay warnings under `unemu/` too. A later release may accept more
of Markdown; a refusal would tie the public catalogue to the subset of the
release that pushed (§13, Q27).

**A departure from the bead's wording.** The bead says the guard "refuses
publishing about without a source". R3 refuses it for every public
destination and R4 for every recipe, but a registry marked private still
takes it. That is the existing model: a private registry holds the user's
own copies of user-supplied disks and ROMs, and a text pasted for one's own
library is the same kind of thing. Refusing it there would only make people
delete text before a backup.

### 4.5. What the guard can and cannot check

It can check, without any network request: that a source is stated, its
shape, the licence vocabulary, the address grammar and the host, that the
layer's licence names the source's, that English is present, the patterns of
W2, and what of the text is outside the subset.

It cannot check: that an `own` text is the author's; that a `licensed` text
is what the URL holds, or was under the stated licence on the day it was
taken; that the authors are the right ones; that a translation was made from
the stated source; that a quotation is a fair one; what a linked page holds
or will hold. The guard never fetches an address: a push has no reason to
contact a third party, and what a page says today proves nothing about the
text.

So the guard closes the mechanical gap, "somebody pasted a text and said
nothing", and the rest is the publisher's review. For `unemu/` that is the
maintainer's review of each work file, as for every licence statement.

### 4.6. Quotation

There is no kind of source for quoted text. Markdown gives a quotation a
form, and that is all it gives it.

- A **short quotation inside an own text** is the author's decision: in
  quotation marks or as a block quote (`>`), with the source named in the
  text ("the box called it …"). Most laws allow it when it is brief and
  serves the author's own text; the guard cannot judge it and does not try.
  It reminds: W6 lists the block quotes of an own text.
- A text that **is** a quotation, a whole box blurb or a review's paragraph,
  is not an own text, and it has no licence to state. It does not go into
  `about` of anything published.
- The style proposed for `unemu/`: own text or Creative Commons text, and no
  quotation longer than a sentence.

### 4.7. What a recipe author writes

Own text, in a file beside the work file (§5.5):

```toml
[title.about]
en = { file = "freedos.about.en.md" }

[title.about_source.en]
kind = "own"
authors = ["unemu contributors"]
```

A short text may stay in the work file, as a multi-line string:
`en = """…"""`.

An abridged Wikipedia article:

```toml
[title.about_source.nl]
kind = "licensed"
title = "FreeDOS"
authors = ["Wikipedia contributors"]
url = "https://nl.wikipedia.org/w/index.php?title=FreeDOS&oldid=00000000"
license = "CC-BY-SA-4.0"
changes = "abridged; converted to Markdown"
```

The author's own translation of that article into another language is the
same table with the translation's language as its key,
`changes = "translated from Dutch and abridged"` and the same licence. A
text from a box or a manual is not written at all; the author writes two
paragraphs of their own and links what they do not want to retell.

## 5. Recipes

### 5.1. Keys of a work file

`[title]` of a work file gains two keys and one key changes its type. All
three are refused in a recipe, as `synopsis` is today, so each text has one
home (recipes §6.16 [REPO]).

| Key | Type | Content |
| --- | --- | --- |
| `synopsis` | table: language tag to string. A plain string is still read, as `{ en = "…" }` | §2.2. One line per language, written in the work file |
| `about` | table: language tag to **either** a string **or** a table with the one key `file` | §2.3. The string is the Markdown source itself; `{ file = "NAME" }` names a file that holds it (§5.5). Either way the text goes through the normalisation of §2.3 and has its limits |
| `about_source` | table: language tag to a table with the keys of §2.4 | §2.4 |

Rows for the table "Which field lives where":

| Field | Work file | Recipe | When both state it |
| --- | --- | --- | --- |
| `synopsis` | yes | no | — |
| `about`, `about_source` | yes | no | — |

A work whose only release needs an about text gets a work file; the text is
the work's even then.

The maintainer's example reads `[about.en]`, a table of the work file's top
level. This note keeps `about` under `[title]`, where `synopsis` is and
where the merge, `recipe show` and the lock's differences name it
(`title.about.en`). Moving it to the top level is a change of one line of
the grammar (§13, Q25).

`synopsis` does **not** get the `file` form (§13, Q24): it is one line of at
most 500 characters and belongs beside the title it describes; a file per
language for a sentence would scatter a work over a dozen files.

Limits of a recipe file stay: the work file is at most 1 MiB (recipes §6.1
[REPO]).

### 5.2. Static checks

`recipe check` (recipes §6.11) gains a rule 11, on the merged record:

- the shapes and limits of §2;
- R1 and R2 of §4.4;
- for each `file`: the rules of §5.5, each refusal naming the work file's
  key and the file;
- the warnings W1 to W6 and the note line, each with the file (the work file
  or the text file) and the line.

`recipe-schema-0.1.json` gains the keys; `about`'s value is "a string, or an
object with exactly `file`".

### 5.3. How a release gets the work's text

The merge (decision-39 point 4) copies `synopsis`, `about` and
`about_source` whole, every language, from the work file into the one title
record of each release, a `file` replaced by the text it holds. Nothing is
filtered by the release's `languages` field: that field says which languages
the software speaks, not which the catalogue describes it in.

`description` (decision-39 point 5): the recipe's; when the recipe states
none, the synopsis in `en`; when there is no `en`, the first entry in byte
order. It stays one plain string, and so does the
`org.opencontainers.image.description` annotation written from it.

The lock holds (decision-39 point 7): a pinned recipe builds its locked
metadata, so a corrected about text reaches a release when its author pins a
new revision. `recipe check` lists the differences per language key
(`title.about.nl`, `title.about_source.nl.url`). Each release then carries
its own copy of the text in its own metadata blob: a few KiB per release,
and the price of "the record travels inside the artifact" (recipes §8.4
[REPO]).

### 5.4. Recipe version

The keys are added to recipe version `0.1`, with compatibility rows (§6).
"`0.x` promises nothing … and no `0.x` recipe is published" (recipes §9
[REPO]). From 1.0 the same change would be a new minor version (§13, Q12).

### 5.5. A text file beside the work file

```toml
[title.about]
en = { file = "freedos.about.en.md" }
nl = { file = "freedos.about.nl.md" }
```

**The name.** `file` is a file name, not a path: the checker's existing
`FileName` form, "one line of at most 128 characters, without `/` or `\`,
not `.` or `..`" (`recipe::spec::Text::FileName` [REPO]). The file is in
the work file's own folder. That is stricter than "a relative path inside
the recipe folder without `..`", on purpose (§13, Q22):

- a work file names its recipes' folder and nothing else today (`work =
  "contra"` "names it by id; there is no path to write and none to escape
  through", recipes §6.16 [REPO]); a bare name keeps that true;
- every part of a recipe artifact but the lock is one file name, and a pull
  writes each part at its name into one folder (`recipe::artifact` [REPO]).
  A bare name needs no new code that turns text from a registry into a path.

The name must differ from the recipe's and the work file's own names, from
every bundled input's, and between languages. The ending is the author's
choice (`.md`, `.txt`, none) and is never read. Suggested:
`<work id>.about.<tag>.md`, so several works share a folder.

**The file.** A regular file, not a symbolic link (a link would leave the
folder by another door). At most 65 536 bytes as it is on disk. UTF-8
without a byte-order mark and without NUL: the rule of a recipe file
(recipes §6.1 [REPO]) and of a bundled text. Then the normalisation and the
limits of §2.3, exactly as for a text written inline: the two forms of the
key give the same metadata bytes for the same text.

**Pinned by the lock, not by a hash in the work file.** The lock holds the
output's metadata blob, the canonical text is inside that blob, the locked
manifest names the blob's digest and the recipe's `digest` names the
manifest. So the text a release was pinned with is fixed by digest, with no
new field. A `sha256` beside `file` is not added (§13, Q29):

- it would pin the bytes on disk, and those are not the identity: the same
  text with other line ends is the same text;
- the work file is not pinned by its recipes, so that a correction does not
  mean editing every recipe (decision-39, alternatives [REPO]); a hash here
  would make every correction two edits and protect nothing the lock does
  not;
- in a recipe artifact the file's bytes are pinned anyway, by the digest of
  their layer (below).

For the same reason the text file is not an `[input]` of the recipes: an
input's hash is stated in each recipe, and five releases of one work would
each restate the hash of one text.

**What each command does with it.**

| Command | With a `file` |
| --- | --- |
| `recipe check` | Rule 11: the name; the file is there, a regular file, within the size, UTF-8; the normalised text within its limits; the warnings with the file's own line numbers. A missing file is refused whether the recipe is pinned or a draft, as a missing work file is. For a pinned recipe it lists `title.about.en` among the differences when the file's text is no longer the lock's. |
| `recipe show` | The `about` line says where the text is: "from the work file, file freedos.about.en.md". `recipe show FILE --about [TAG]` prints the normalised source, the bytes a build would store. |
| `recipe build` (a draft) | Reads the file as it is and writes its text into the metadata. |
| `recipe build` (pinned) | Builds the lock's metadata. The file is read for the report of differences only. |
| `recipe pin` | The author's build: reads the file, writes the metadata with the text, and the lock keeps that blob. It writes nothing into the work file and copies no text file into the lock's folder. |
| `recipe scaffold` | Unchanged. It writes the inputs an author has; "the author adds the description" (recipes §6.15 [REPO]). It writes no about key and no text. |
| `recipe push` | The artifact carries each text file its work file names, as a layer (below). |
| `recipe pull` | Writes each text file at its name beside the work file, after the checks. |

**In a recipe artifact.** Decided: **the file travels**, as a layer of its
own, titled with its file name like every other part. A pulled recipe must
"plan and build with no other file" (recipes §7.1 [REPO]), and its folder
must pass `recipe check`, which wants the files a work file names.

- Media type: a new one, `application/vnd.unemu.recipe.text.v1`, "a text
  file a work file names". It is not a bundled input: an input's size and
  hash are stated in the recipe and checked against the part, and a text
  file has neither there. The reader's rule is the one it has for the other
  parts, nothing more and nothing less: the artifact holds exactly the text
  files its work file names.
- Limits and checks: at most 65 536 bytes; text, by the rule bundled inputs
  have; the layer's digest in the manifest pins its bytes.
- Order of the layers: the work file, its text files in the order of their
  language tags, the bundled inputs, the lock.
- Licence: R4 of §4.4.
- As the work file itself, the copy is the one beside the recipe when it was
  bundled; if it has moved on since the pin, `check` in the pulled folder
  lists the difference and the build still writes the lock's text.

## 6. Compatibility rows (machine-images §5.6)

| Table | Row | First read by | What it decides |
| --- | --- | --- | --- |
| `image_store::compat::METADATA_SCHEMAS` | `3` | 0.0.4 | Which releases read the record in full. Never raises `unemu.min-app-version` (decision-30 point 5). |
| `image_store::compat::TITLE_FIELDS` | `about` | 0.0.4 | The title needs schema 3. |
| `image_store::compat::TITLE_FIELDS` | `about_source` | 0.0.4 | The title needs schema 3. |
| `image_store::compat::TITLE_FIELDS` | `synopsis:localized` | 0.0.4 | A row for a shape, not a key: a synopsis that is not the one `en` text. The writer adds it to the keys it rates when a title's synopsis is such. `synopsis` itself keeps its 0.0.3 row. |
| `recipe::compat` (a new table of work-file keys) | `title.about`, `title.about_source`, `title.synopsis:localized` | 0.0.4 | Raise a **recipe artifact's** marker, the oldest release that can build it, because the parser of 0.0.3 refuses the keys. The output's marker is untouched. |
| `image_store::compat::RECIPE_TYPES` | `application/vnd.unemu.recipe.text.v1` | 0.0.4 | The text-file part of a recipe artifact (§5.5). Raises that artifact's marker, as the keys do. |
| `image_store::compat::NEWEST_ROW` | — | 0.0.4 | So a development build reads what it writes. |

`metadata::SCHEMA` becomes 3. The existing tests then demand the rest: a
title field without a row cannot be written, and every row needs a test
image [REPO].

No artifact's `unemu.min-app-version` changes: an image, a set or a package
with an about text runs on every release that ran it without one.

The Markdown subset has no row: a text is stored as source, and a release
that knows fewer constructs shows the others as typed (§2.8).

## 7. Summary cache and search

- **Cache schema 9.** A title's summary gains `synopsis`, the localized
  text, so a list can show the synopsis as a row's hover text and the pane
  can draw its head without decoding the blob. At most 500 characters per
  language. The about text is **not** copied into the cache; the pane reads
  it from the metadata blob of the selected item, which it decodes anyway.
  A cache of another schema is rebuilt, as today.
- **Search stays all-text, without the markup.** A title's tokens gain the
  synopsis and, for the about text in **every** language, the text the
  parser gives without markup: the words of paragraphs, headings, quotes and
  list items, the text of a link, the content of a code span. A link's
  address is left out, and so are a source's `url`, `license`, `kind` and
  `changes`, as the artwork digests are, so "https" and "wikipedia" do not
  match half the library. A source's `title` and `authors` are tokens.
  Markers (`#`, `*`, `>`, a list's number) make no token.
- **Size.** Tokens are a set, so a long text adds less than its length.
  Measured on English prose from this repository with the cache's own
  splitting rule: a full 16 384-byte text gives 885 distinct tokens, 8.0 KiB
  of cache; a 4 KiB text 386 tokens, 3.3 KiB; a 2 KiB text 216 tokens,
  1.8 KiB. A library of 1 000 titles with a 4 KiB text in two languages
  grows its cache by about 7 MiB. The 64 MiB cap and its "search text
  reduced" fallback stay as they are (§13, Q14).
- A search finds a title by a word of a language the viewer does not read.
  That is wanted: the row found is the same title.

## 8. Remotes: what is read when

| Moment | Read from the registry | Shows |
| --- | --- | --- |
| A node is listed | The catalog, the tag lists, the newest tag's manifest (today's requests) | The annotations: title, vendor, licences and `org.opencontainers.image.description`, which is the release's description or the English synopsis. No synopsis map, no about text. |
| A row is hovered | Nothing | The description annotation; the synopsis instead once this session has read the tag's record. |
| A tag is selected | That tag's **title record** (the metadata layer), which the pane already reads for the edition and the version | The card: the synopsis in the viewer's language, then the release's description when it says something else. The About section, with its language switch and source line. |
| A link in the text is clicked | Nothing by the program. After the confirmation the host's browser opens the address (§9) | — |
| Pull | Everything, as today | — |

The record is read once per digest in a session. Until it arrives the card
shows the manifest's description and the pane says the record is being read;
if it cannot be read the card keeps the description and says so. The pane's
last line names what was read: "the tag list, this manifest and its title
record".

A text from a registry is somebody else's text: it is drawn by the same
parser with the same limits, its links ask before they open, and nothing in
it makes a request.

**No new annotation.** A compact copy of the synopsis on the manifest
(`unemu.synopsis`, as `unemu.needs` copies the needs) would let a list show
it without the record. It is not added now: an annotation holds one
language; it would change the manifest bytes of every set that states a
synopsis today, so an identical re-import would become a tag conflict; and
the pane has the record a moment after selection. The catalogue index of
`upc-5pw.65` is the planned home of per-repository texts for lists
(decision-39 point 9), derived from the newest revision's record (§13, Q13).

**Recipes group** (recipes §7.4). A recipe row reads the manifest and the
config only. When a recipe is selected, its pane may read the lock's
metadata layer, one small blob, and draw the same card and About section
from it.

The Licence column and the Licence line of a tag with a `licensed` text show
the longer conjunction (`… AND CC0-1.0 AND CC-BY-SA-4.0`), which is what the
manifest says.

## 9. The desktop

Drawn in [`mockups/about.html`](mockups/about.html); this is the behaviour.

- **The head.** Under the title and the tag line: the synopsis in the
  viewer's language, plain, then the release's description in the muted
  style when it differs from every entry of the synopsis.
- **About** is a section of the details pane of a machine image, an
  installation set, a machine made from an image with titles, and a Remotes
  tag: the same widget everywhere. Per title when an image holds several.
  Without an about text there is no section and no placeholder.
- **The text** is drawn from the tree of §2.9 as §2.8's tables say. What is
  outside the subset is drawn as its characters. Lines are at most about 72
  characters long, whatever the pane's width. The text can be selected and
  copied; a copy gives the text as drawn, without markers.
- **Collapsed** is counted in **drawn blocks**, never in characters:
  - the section shows the text's leading blocks up to and with its second
    *body block*: a paragraph, a list or a quote. A heading and a rule are
    not body blocks; they are drawn where they stand, and neither is left as
    the last thing shown;
  - a list of more than six items at its first level is drawn with its first
    five, each with its nested items, and a line with “…”;
  - **More…** follows when anything is left out and opens the rest in place;
    the pane scrolls. **Less** closes it. A text that fits has neither.
- **A link** is underlined in the link colour. A click, or Enter on a
  focused link, opens **Open link?** and nothing else:
  - the host, alone on a line, as the address spells it;
  - the whole address in a field that can be selected, wrapped, never cut;
  - "unemu does not contact this address. Your web browser opens it.";
  - **Open in browser**, **Copy address**, and **Cancel**, which is the
    default button and what Escape does.

  The question is asked every time, for every host: there is no "do not ask
  again" and no list of trusted hosts (§13, Q30). The text of a link may say
  anything; the dialog shows where it goes. Hovering a link shows its
  address in the status bar. The two links of a source line (the source and
  the licence) open the same dialog.
- **Nothing is fetched by the program**: no image, no preview of a linked
  page, no icon of a site, no check that an address answers.
- **The source line**, in the pane's small muted style, under the text:
  - `own`: "Own text" and the authors when stated ("Own text · unemu
    contributors");
  - `licensed`: "From “FreeDOS” by Wikipedia contributors · CC BY-SA 4.0 ·
    changed: abridged". The title is a link to the source and the licence a
    link to its page at creativecommons.org. The line is the credit CC BY
    and CC BY-SA ask for, so it is always drawn when the text is, also while
    the text is collapsed;
  - no source: "⚠ No source recorded: this text stays in this library and on
    private registries";
  - a kind this release does not know: the kind as written and "this release
    does not know this kind of source".
- **Language:** §3. The switch is in the section's heading line.
- **Rows.** A list row's hover text is the synopsis in the viewer's
  language, from the cache.
- **Not in this change:** editing. Edit metadata… and the local overlay keep
  editing the fields of schema 1 only, and publishing an overlay keeps a
  record's schema-3 fields and schema, as it keeps schema 2's today
  (machine-images §2.1 [REPO]). An editor for the about text is its own bead
  and its own mock-up (§13, Q20).

## 10. Command line

The command line draws no Markdown. It prints the source, which §2.8 keeps
readable.

- **`upcli image media show REF`** prints, after `work`:

  ```
  synopsis	en	"A free, open source operating system …"
  synopsis	nl	"…"
  about	en	31 lines	1766 bytes
  about-source	en	own	"unemu contributors"
  about-link	en	"https://www.freedos.org/"
  about	nl	12 lines	602 bytes
  about-source	nl	licensed	CC-BY-SA-4.0	"https://nl.wikipedia.org/…"	"FreeDOS"	"Wikipedia contributors"	changed "abridged; converted to Markdown"
  ```

  The `synopsis` line gains a column, the language, like `need` and
  `work-id`; a schema-2 record prints `en`. An about text without a source
  prints `about-source	nl	none`, and the guard's verdict line names it.
  `about-link` lists each address the text links to, once, in the order of
  the text: read from the text, not stored.
- **`upcli image media show REF --about [TAG]`** prints the stored source,
  then the source line; with a tag one language, without one every language
  under a heading line. No language is chosen from the host: the command
  line prints what the record holds, the same on every machine.
- **`upcli recipe show FILE`** prints the same lines for the merged record,
  each marked as coming from the work file, with the text file's name when
  there is one; `--about [TAG]` prints the normalised source (§5.5).
- **`upcli recipe check FILE`** prints rule 11's refusals, warnings and the
  note of linked hosts.
- **`upcli image media import`**: no new flags. `--synopsis TEXT` keeps its
  meaning and states English. An about text, and a synopsis in another
  language, are written in a work file and built by a recipe (§13, Q19).
- **`upcli image export`** (decision-37): `metadata.json` is the record, so
  the texts and their sources are exported with it and nothing is added.
- **`upcli image push`, `recipe push`**: the refusals of R3, R4 and R5, each
  naming the title, the language and the missing part.

## 11. What changes in the documents

Done with the acceptance on 2026-11-19, marked "specified, not implemented
yet": machine-images §2.1 (schema 3, the writer's rule, the migration) and
§5.6 (the rows); recipes §6.1, §6.11 (rule 11), §6.13 (R1 to R5 and the
warnings), §6.16 (the keys, the text files, synopsis and description) and
§7.1 (the text layer and the artifact's licences); the mock-up in
`backlog/docs/design/mockups/` with its icon inventory entries. The rest
lands with the implementation step that changes the behaviour (§15):


- machine-images §2.1: the schema-3 table, the writer's rule of §2.6, the
  Markdown subset (or a document of its own that §2.1 points to), the
  cache's synopsis and the tokens of §7; §2 annotations: nothing.
- machine-images §3 (Remotes): the card and the About section of a selected
  tag, the hover text.
- machine-images §5.1: guard rule 6 and the English rule of `unemu/`;
  `crates/registry/src/guard.rs` carries the same list in its module
  comment.
- machine-images §5.2: the sentence on the generated metadata layer's
  licence (§4.3 here).
- machine-images §5.6: the rows of §6.
- machine-images §7 (desktop behaviour): links and the Open link? dialog,
  the first place where the program hands an address to the host's browser.
- recipes §6.1 (a work file may name text files), §6.4, §6.11 (rule 11),
  §6.13 (the recipe guard; a work file's `file` is a name, not a path),
  §6.16 (the keys, the table row, synopsis and description), §7.1 (the
  text-file layer, the artifact's licence list), `recipe-schema-0.1.json`.
- A new decision record, "Title metadata carries an about text with its
  source, in several languages", which extends decision-39 (the work's
  fields) and records the change to the licence of generated layers
  (machine-images §5.2 and §5.3). The Markdown subset and "links ask first"
  deserve a record of their own, since other front ends will follow them.
- `backlog/docs/commands.md`: §10.
- `backlog/docs/design/icons.md` and the icon inventory: the mock-up's
  entries (the list is at the foot of the mock-up).
- The mock-up joins `backlog/docs/design/mockups/`.

## 12. Considered and not chosen

- **The about text as its own layer**, referenced from the record by digest
  like a cover. It would deduplicate the text across the releases of a work,
  keep the record small, and let the layer carry its own licence mark, so
  the metadata layer stayed `CC0-1.0`. But releases 0.0.1 to 0.0.3 refuse an
  artifact with a layer media type they do not know (machine-images §5.6,
  "New layer media type: Refused" [REPO]), so an about text would raise
  `unemu.min-app-version` and stop an older release from running the
  artifact. A catalogue text must not do that.
- **An OCI referrer** carrying the text. The record must travel inside the
  artifact (recipes §8.4, decision-25 point 7 [REPO]).
- **`about` as `{ en = { text, source } }`**, one object per language. It
  ties the source to its text, but it gives `about` another shape than
  `synopsis`, and the codec's one check (the keys of `about_source` are keys
  of `about`) does the tying.
- **Reading a schema-2 synopsis as `und`** (BCP 47, undetermined). Honest
  about what schema 2 knew, but then changing a work file to
  `{ en = "…" }` changes the metadata bytes, every pinned recipe reports a
  difference, and every published set with a synopsis needs a new revision
  to say what everybody knows.
- **All of CommonMark, through a Markdown crate.** It brings raw HTML,
  images and reference definitions, which then have to be switched off or
  filtered after parsing, and a dependency that decides how a registry's
  text is read. The subset is small enough to own, and owning it is what
  lets the checker, the search index and the renderer agree.
- **Storing the parsed tree**, or HTML, in place of the source. Every reader
  without this renderer would show markup or nothing, and the stored form
  would be tied to one renderer's idea of a tree.
- **One stored line per paragraph** (revision 1). Markdown needs its line
  breaks, and re-flowing Markdown safely needs a parser at the door.
- **A list of forbidden hosts** (store pages, databases with their own
  terms), for sources or for links. It cannot be complete and reads as a
  promise that every other host is fine.

## 13. Decided

Each question of revision 2 was asked with a recommendation; the maintainer
accepted every recommendation on 2026-11-19. Q1 to Q21 are revision 1's
(Q9, Q17 and Q18 as revision 2 replaced them); Q22 to Q31 are revision 2's.

1. **What language is a schema-2 synopsis?** Decided: `en`. The migration
   then rewrites nothing (§2.6); a private synopsis in another language is
   mislabelled until its author says so.
2. **Does a title with an English synopsis only stay schema 2?** Decided: it
   stays schema 2. Digests and re-imports are unchanged and 0.0.3 keeps
   showing those records in full; the cost is one sentence in the writer's
   rule.
3. **`about_source` as a parallel map, or the source inside each language's
   entry?** Decided: the parallel map (§12).
4. **Is an own text in an open artifact `CC0-1.0`, like the rest of the
   record?** Decided: yes. One rule for everything a recipe's author types.
   An author who wants a credit that binds uses `licensed` with a URL of
   their own.
5. **Which licences may a `licensed` source state?** Decided: the five
   Creative Commons identifiers the store already knows (`CC0-1.0`, `CC-BY`
   and `CC-BY-SA` in 3.0 and 4.0). No NonCommercial or NoDerivatives licence
   (the catalogue is for anyone, and an abridgement is a derivative), no GFDL
   (it asks for the whole licence text beside the work), no `LicenseRef-`.
6. **A `licensed` text makes the metadata layer `CC0-1.0 AND <licence>`.**
   This amends "the generated metadata layer is this program's own CC0
   output". Decided: the mixed expression, not a layer of its own (§12); the
   maintainer confirms the reading of share-alike in §4.3 before the first
   CC BY-SA text is published under `unemu/`.
7. **May a text without a source go to a registry marked private?** Decided:
   private registries take it; public destinations and every recipe artifact
   do not (§4.4). It is the rule user-supplied disks already follow.
8. **Must `unemu/` have English?** Decided: yes, for `synopsis` and for
   `about` when present, checked by the guard for `unemu/` only (R5).
9. **The about text is Markdown** (answered by the maintainer before
   revision 2). In Markdown a single line break is a soft break by definition
   (§2.8), and the stored form keeps the author's lines (§2.3).
10. **The limits.** Decided: synopsis 500 characters; about 16 384 bytes of
    Markdown source per language; 16 languages per field; a text file at
    most 65 536 bytes before normalisation. Bytes for the long text because
    it is a storage bound, characters for the short one because it is a
    display bound.
11. **How far do the mechanical checks go?** Decided: the Wikimedia host rule
    of R1 and the warnings W1 to W6, and no list of forbidden hosts (§12).
12. **New keys in recipe version `0.1`, or a version `0.2`?** Decided:
    `0.1`, with compatibility rows for the keys. No `0.x` recipe is
    published; after the freeze such a change is a minor version.
13. **A manifest copy of the synopsis for Remotes lists?** Decided: not now
    (§8). The record is read on selection; lists wait for the catalogue
    index.
14. **Does search index the about text in every language?** Decided: yes,
    without markup and without addresses, measured at about 8 KiB of cache
    for a full text (§7).
15. **Where does the viewer's language come from, and how are languages
    named?** Decided: one ordered preference, defaulting to the host's
    language then English; English names of languages.
16. **A language the fonts cannot draw** (CJK, right-to-left). Decided: the
    schema takes it, the desktop skips it in the fallback and says so; fonts
    and shaping are separate work with its own bead.
17. **More… opens in place.** Decided: in place, as drawn in the mock-up. A
    reading window can follow if a long text in the pane proves awkward.
18. **The collapsed size.** Decided: counted in drawn blocks (§9): up to the
    second body block, a long list cut to five items, no count of characters.
19. **Flags on `image media import`?** Decided: none. The work file is where
    a long text is written and reviewed.
20. **An editor in the desktop?** Decided: later, its own bead and mock-up.
    This change reads and shows; recipes write.
21. **Does `description` get languages too?** Decided: no. It is a short
    statement about one release and feeds a plain-string annotation.
22. **`file` as a bare file name beside the work file, or a relative path?**
    Decided: a bare name (§5.5): no path enters the work file, and a pull
    writes one more flat file with no new path handling. Folders can be
    allowed later; the reverse cannot.
23. **How much Markdown?** Decided: the subset of §2.8 as stated. Drawn and
    specified: paragraphs, headings of one drawn level (written with any `#`
    or underlined), rules, block quotes, bullet and ordered lists of two
    levels, emphasis, strong emphasis, code spans, links, `<https://…>` and
    bare `https://` addresses, backslash escapes. Left out and shown as
    typed: code blocks, hard line breaks, HTML, images, reference links,
    entities, tables.
24. **Does `synopsis` get `{ file = … }`, and Markdown?** Decided: no to
    both. One plain line lives beside the title; it is drawn in hovers, cards
    and an OCI annotation, which show characters as they are.
25. **`[title.about]` or a top-level `[about]`** in the work file? Decided:
    under `[title]`, beside `synopsis`, so the work file's tables stay
    `[work]`, `[title]`, `[ids]` and `[needs]` and a difference against the
    lock reads `title.about.en`.
26. **Strong emphasis without a bold face.** Decided: strong emphasis in the
    strong text colour now, as headings are drawn, emphasis in egui's
    slanted face, and a bold and an italic face as font work of their own
    (the licence, the size and the glyph tests of a new font file). The
    mock-up draws bold, as every mock-up does for headings.
27. **Do Markdown outside the subset and a non-`https` destination stay
    warnings under `unemu/`?** Decided: yes (§4.4): a refusal would tie the
    catalogue to one release's subset. The maintainer's review reads the
    warnings.
28. **A list of links in the record, and a warning for a link to a Wikimedia
    page from an own text?** Decided: no list and no warning. The links are
    read from the text when wanted (`about-link`, the note of hosts); a link
    to an article is the right thing in an own text.
29. **A `sha256` beside `file`?** Decided: no (§5.5). The lock pins the text
    through the metadata blob and the recipe artifact pins the file's bytes
    through its layer.
30. **Is the link question asked every time?** Decided: yes, with Cancel as
    the default button and no "do not ask again".
31. **Where does the parser live?** Decided: the renderer in `ui-kit`, the
    parser in a workspace crate without dependencies that `ui-kit`,
    `image-store` and `recipe` use (§2.9), because the checker and the search
    index must read the text as the renderer does and cannot depend on egui.
    No external crate.

The mock-up's own questions were decided the same day with their
recommendations: the synopsis in the head with the release's description
under it; the subset above; strong emphasis in the strong colour; text
outside the subset drawn in the body style with no mark; the collapsed size
in drawn blocks; More… in place; the link question every time; the words of
the source line as drawn, always visible; a language switch in the section's
heading; English names of languages; an undrawable language listed and
skipped; Remotes showing the manifest's description until the title record
arrives; a text without a source shown with a warning line.

## 14. Test plan

Testability-first (hard rule 1); every input is synthetic.

1. **Codec.** Round trips of schema 3; a title with an English synopsis only
   encodes to the bytes a pinned schema-2 fixture has; a schema-2 blob reads
   as `{en}`; a map in schema 2 and a string in schema 3 are refused; each
   limit at its edge; each refusal of R1; a non-canonical about text is
   refused by the codec and accepted, normalised, by the entry point; the
   normalisation is idempotent and gives the same bytes for `LF`, `CR LF`
   and `CR` input.
2. **Markdown.** The tests of §2.9: the specification's examples, the
   bare-address examples, the link-address grammar, the property test.
3. **Older releases.** The projection a 0.0.3 decoder gives for a schema-3
   blob (the decoder is in this repository's history): read-only, no
   synopsis, everything else intact.
4. **Compatibility rows.** The existing tests: every title field has a row,
   every row a test image; an artifact with an about text carries the marker
   it carried without one; a recipe artifact that uses the new keys or
   carries a text file declares 0.0.4.
5. **Recipes.** Merge of the three keys; the inline form and the `file` form
   of one text give one digest; each refusal of §5.5 (a path, `..`, a link,
   a missing file, a byte-order mark, NUL, 65 537 bytes, a name used twice);
   `check` rule 11 with the file and line of each refusal and warning; a
   string synopsis and the `en` map give one digest; an edit to a text file
   shows as a difference against the lock and does not change a pinned
   build; the corpus digests do not move.
6. **Recipe artifacts.** A recipe with text files bundles, pushes to the
   mock registry, pulls into an empty folder and builds with no other file;
   an artifact with a text file its work file does not name, or without one
   it names, is refused; the layer licences of R4.
7. **Licence marks.** The metadata layer's expression for each row of §4.3;
   the manifest's licence list; the lock comparison accepts exactly that.
8. **Guard.** R3 for `unemu/`, a public namespace and a private one; R4 for
   a recipe artifact; R5; the refusal texts.
9. **Cache and search.** Schema 9 rebuilds a schema-8 cache; a word of the
   second language's about text finds the title; a word that is only in a
   link's address, and a marker, do not.
10. **Desktop.** Layout tests of the About section: each block and inline of
    §2.8, text outside the subset drawn as typed, short text, long text
    collapsed and open (the block rule, the cut list), each source line, the
    fallback note, two languages, an undrawable language, no about text;
    Open link? with a long address, its default button and Escape; no
    request is made by drawing or by hovering; both skins and both themes.
11. **Command line.** The pinned output of `image media show` and
    `recipe show`, with `--about`.

## 15. Implementation

In order, each step its own bead, each landing with its tests (§14) and its
part of the documents (§11). A step does not start the next one's work.

1. **Decision records.** "Title metadata carries an about text with its
   source, in several languages" (extends decision-39, records the change to
   the licence of generated metadata layers), and one for the Markdown
   subset and "links ask first", which other front ends follow.
2. **The parser crate** (§2.9): normalisation and the canonical-form check
   of §2.3, the language-tag and link-address grammars, the parser of the
   subset of §2.8 to a tree, the findings `recipe check` prints (W4, W5, the
   note of hosts), and the text without markup for search. The CommonMark
   example fixture lands with it once its licence is confirmed.
3. **Metadata schema 3** in the store (§2, §6): the localized synopsis,
   `about` and `about_source`, the codec's rules (R1), the writer's choice
   of schema, the compatibility rows and their test images, the licence of
   the metadata layer and the lock comparison (§4.3), and the cache's
   schema 9 with the synopsis and the search tokens (§7).
4. **Recipes** (§5.1 to §5.5): the work-file keys, the `file` form and its
   checks, the merge and the description rule, rule 11 of `recipe check`
   with R2 and the warnings, `recipe show --about`, the work-file key rows
   of `recipe::compat`, and `recipe-schema-0.1.json`.
5. **Recipe artifacts** (§5.5): the text layer and its row, push and pull
   of text files, and the layer licences of R4.
6. **The push guard** (§4.4): rule 6 (R3) and the English rule (R5), with
   machine-images §5.1 and the guard's module comment.
7. **The command line** (§10): the `image media show` lines, `--about` on
   `image media show` and `recipe show`, the refusals of push, and
   `backlog/docs/commands.md`.
8. **The renderer and Open link?** (§2.9, §9): `ui_kit::about`, the link
   question and the hand-over of an address to the host's browser, with
   layout tests in both skins and both themes.
9. **The desktop** (§3, §8, §9): the Catalogue text languages preference and
   the fallback, the synopsis in the pane's head, the About section in the
   library and in Remotes (the title record read on selection), the hover
   text of rows, and machine-images §3 and §7.

Not in this order, each its own later bead: a bold and an italic face
(Q26), fonts and shaping for scripts the bundled fonts lack (Q16), and an
editor for the about text in the desktop with its own mock-up (Q20).

## 16. Sources

- [CM] CommonMark Spec, version 0.31.2 (2024-01-28), and its appendix "A
  parsing strategy" for emphasis and links.
- [GFM] GitHub Flavored Markdown Spec, section 6.9, "Autolinks (extension)",
  for the bare-address rule only.
- RFC 5646, Tags for Identifying Languages (BCP 47); RFC 4647, Matching of
  Language Tags, section 3.4 (lookup).
- RFC 3986, Uniform Resource Identifier: Generic Syntax, for the parts of a
  link address.
- Creative Commons Attribution-ShareAlike 4.0 International, legal code,
  sections 1(a), 3(a), 3(b).
- Wikimedia Foundation Terms of Use, section 7 (licensing of content and
  re-use of text).
- TOML 1.0.0, String (multi-line basic strings and line endings).
- SPDX Specification 2.3, Annex D (licence expressions), as
  `image_store::license` reads them.
