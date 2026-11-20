# FreeDOS 1.4 Floppy Edition, the 1.44M disks: unemu/media/ibm-pc/freedos:1.4-r1.
# Open. The disks are the edition's images, unmodified, in installer order.
# Real data from unemu-media's media.toml and build.sh. A draft: no digest.

recipe = "0.1"

media {
  name     = "unemu/media/ibm-pc/freedos"
  platform = "ibm-pc"
  release  = "1.4"
  revision = 1
}

title {
  title             = "FreeDOS"
  version           = "1.4"
  edition           = "Floppy Edition"
  year              = 2025
  publishers        = ["The FreeDOS Project"]
  developers        = ["The FreeDOS Project"]
  genres            = ["operating system"]
  software_platform = "DOS"
  labels            = ["freedos", "open-source", "operating-system"]
  description       = "The official FreeDOS 1.4 Floppy Edition on seven 3.5-inch 1.44M diskettes, unmodified: the boot disk with the floppy installer, then six disks of packages in a Slicer archive. It installs FreeDOS 1.4 with a set of programs suited to classic PCs, the 80286 included, on any PC that runs FreeDOS with EGA or better graphics. Released by the FreeDOS Project on 2 April 2025. GPL and other open source licences, listed per program in LICENSES.TXT; the set carries the official FreeDOS source packages of its programs."
}

license {
  expression = "GPL-1.0-only AND GPL-1.0-or-later AND GPL-2.0-only AND GPL-2.0-or-later AND GPL-3.0-or-later AND LGPL-2.1-or-later AND MIT AND BSD-2-Clause AND MPL-2.0 AND Watcom-1.0 AND Info-ZIP AND Artistic-1.0 AND GFDL-1.1-or-later AND Zlib AND LicenseRef-FreeDOS-other"
  terms      = "licenses"
  source     = "source"
}

# ASSUMED: read off the set's description; the vocabulary is the provider's.
needs {
  cpu    = "8088+"
  memory = "640K"
  video  = ["vga", "ega"]
}

input "edition" {
  name   = "FD14-FloppyEdition.zip"
  size   = 23624258
  sha256 = "45b1fa7c52dd996c3bfa5e352ffcd410781b952a6ad629f15a4c9ec4bbaefc5a"
  urls   = [
    "https://download.freedos.org/1.4/FD14-FloppyEdition.zip",
    "https://www.ibiblio.org/pub/micro/pc-stuff/freedos/files/distributions/1.4/FD14-FloppyEdition.zip",
  ]
}

input "licenses-lf" {
  name   = "licenses.txt"
  size   = 9283
  sha256 = "7467d0b4d07763a7fe160f0e4b098dee2f900cb10e830673127b19114cb61235"
  bundle = true
}

# A prepared input: built once outside the recipe (README).
# ASSUMED: the URL; the asset is not published there yet.
input "source" {
  name   = "freedos-1.4-floppy-src.tar"
  size   = 22835200
  sha256 = "dfa7006e742cb9676a0a6401ce64214089923f055f4a7ba8f36960115b5362b6"
  urls   = [
    "https://github.com/unemu-org/software/releases/download/freedos-1.4-floppy/freedos-1.4-floppy-src.tar",
  ]
}

step "unpack" "disks" {
  from    = "edition"
  format  = "zip"
  members = [
    { path = "144m/x86BOOT.img", size = 1474560, sha256 = "552f7cbb0625960c050a3e682a4d2121cc2ffdfa9dc9d592ccd49edf179333dc" },
    { path = "144m/x86DSK01.img", size = 1474560, sha256 = "b9dd5315f50a8b770b3d2ecb623b18ed067660e22f421504d199254f7f653516" },
    { path = "144m/x86DSK02.img", size = 1474560, sha256 = "4a951f15da8dd98e54ea84f07fe2a7802e914aa8e66942f646d2e11ca3943e59" },
    { path = "144m/x86DSK03.img", size = 1474560, sha256 = "fbfd172e1ac4bc4abb3f656cae94e2727d2b48b4d02cef0362564048f83f35da" },
    { path = "144m/x86DSK04.img", size = 1474560, sha256 = "894d49c4dfdd294e33878ca317e19a0322f267f639b247a6b2a669799b4720f8" },
    { path = "144m/x86DSK05.img", size = 1474560, sha256 = "0362e084497c5c995ccab1523c4e39f94b7139f988ffa3a4f7e871452c352f3c" },
    { path = "144m/x86DSK06.img", size = 1474560, sha256 = "7a93d1d4e47f5edd5d0a9f17d57c3417ae135052db9449a67aa374a3b4316d80" },
  ]
}

step "text" "licenses" {
  from = "licenses-lf"
  eol  = "crlf"
}

medium "disk-1" {
  label     = "Disk 1 of 7 - boot disk and installer (x86BOOT.img)"
  format    = "1.44M"
  file_name = "x86BOOT.img"
  from      = "disks/144m/x86BOOT.img"
}

medium "disk-2" {
  label     = "Disk 2 of 7 - packages (x86DSK01.img)"
  format    = "1.44M"
  file_name = "x86DSK01.img"
  from      = "disks/144m/x86DSK01.img"
}

medium "disk-3" {
  label     = "Disk 3 of 7 - packages (x86DSK02.img)"
  format    = "1.44M"
  file_name = "x86DSK02.img"
  from      = "disks/144m/x86DSK02.img"
}

medium "disk-4" {
  label     = "Disk 4 of 7 - packages (x86DSK03.img)"
  format    = "1.44M"
  file_name = "x86DSK03.img"
  from      = "disks/144m/x86DSK03.img"
}

medium "disk-5" {
  label     = "Disk 5 of 7 - packages (x86DSK04.img)"
  format    = "1.44M"
  file_name = "x86DSK04.img"
  from      = "disks/144m/x86DSK04.img"
}

medium "disk-6" {
  label     = "Disk 6 of 7 - packages (x86DSK05.img)"
  format    = "1.44M"
  file_name = "x86DSK05.img"
  from      = "disks/144m/x86DSK05.img"
}

medium "disk-7" {
  label     = "Disk 7 of 7 - packages (x86DSK06.img)"
  format    = "1.44M"
  file_name = "x86DSK06.img"
  from      = "disks/144m/x86DSK06.img"
}

extra "licenses" {
  file_name = "LICENSES.TXT"
  dos_name  = "LICENSES.TXT"
  kind      = "document"
  from      = "licenses"
}

extra "source" {
  file_name = "freedos-1.4-floppy-src.tar"
  dos_name  = "FD14SRC.TAR"
  kind      = "archive"
  from      = "source"
}
