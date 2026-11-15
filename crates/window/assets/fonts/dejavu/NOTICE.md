# DejaVu Sans symbol subset

Work: DejaVu Sans, from the DejaVu fonts 2.37 release
(`dejavu-fonts-ttf-2.37.tar.bz2`, file `ttf/DejaVuSans.ttf`).
Copyright: (c) 2003 Bitstream, Inc. (Bitstream Vera); (c) 2006 Tavmjong Bah
(glyphs imported from Arev fonts). DejaVu changes are in the public domain.
Source: <https://dejavu-fonts.github.io/>.
License: Bitstream Vera Fonts licence and Arev Fonts licence (SPDX
`Bitstream-Vera`); the full text is in [LICENSE.txt](LICENSE.txt), unmodified
from the release.

`DejaVuSans-UI-symbols.ttf` is a subset made by unemupc98. It keeps only the
five characters the desktop draws that egui's bundled fonts lack — U+2161 Ⅱ,
U+232B ⌫, U+2713 ✓, U+2715 ✕ and U+2717 ✗ — and drops hinting and the layout
tables. The glyph outlines are unchanged and the name table, with its
copyright and licence records, is kept whole. The family name stays
"DejaVu Sans", which the licences permit for modified fonts because it
contains neither "Bitstream", "Vera", "Tavmjong Bah" nor "Arev". The desktop
embeds the file with `include_bytes!` and adds it after egui's fonts, so it
supplies only characters those fonts do not have.

Made with fontTools 4.61.1 (the project's pinned nixpkgs):

```sh
pyftsubset DejaVuSans.ttf --unicodes=U+2161,U+232B,U+2713,U+2715,U+2717 \
  --name-IDs='*' --name-languages='*' --name-legacy --no-hinting \
  --layout-features='' --drop-tables+=GSUB,GPOS,GDEF,MATH --notdef-outline \
  --output-file=DejaVuSans-UI-symbols.ttf
```

The output is byte-for-byte reproducible.

SHA-256:

| File | SHA-256 |
| --- | --- |
| `dejavu-fonts-ttf-2.37.tar.bz2` | `fa9ca4d13871dd122f61258a80d01751d603b4d3ee14095d65453b4e846e17d7` |
| `ttf/DejaVuSans.ttf` (input) | `7da195a74c55bef988d0d48f9508bd5d849425c1770dba5d7bfc6ce9ed848954` |
| `DejaVuSans-UI-symbols.ttf` | `1a43e2ecafcbc268c105b911cb84bbbda0a60fc0417591a5f8ecab9e56752f7a` |
| `LICENSE.txt` | `7a083b136e64d064794c3419751e5c7dd10d2f64c108fe5ba161eae5e5958a93` |
