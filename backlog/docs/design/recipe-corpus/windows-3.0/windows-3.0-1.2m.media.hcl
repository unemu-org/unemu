# Microsoft Windows 3.0, the 1.2M installation disks: private/media/ibm-pc/windows:3.0-1.2m-r1.
# Private: the user supplies the five disks; the documentation is an optional part.
# The disk count, names, hashes and ids are PLACEHOLDERS: nothing was measured (README).

recipe = "0.1"

media {
  name     = "private/media/ibm-pc/windows"
  platform = "ibm-pc"
  release  = "3.0-1.2m"
  revision = 1
}

title {
  title             = "Microsoft Windows"
  sort_title        = "Windows"
  version           = "3.0"
  year              = 1990
  publishers        = ["Microsoft"]
  developers        = ["Microsoft"]
  genres            = ["operating environment"]
  software_platform = "DOS"
  labels            = ["windows", "operating-system"]
  source_media      = "5 x 1.2M floppies"  # ASSUMED
  description       = "Microsoft Windows 3.0 on five 5.25-inch 1.2M installation diskettes, unmodified, in Setup's order. Setup installs Windows from DOS onto a hard disk. The printed documentation, when the user has it, is an optional part."
}

# ASSUMED: from memory of the product's stated requirements, not from a source.
needs {
  cpu    = "8088+"
  memory = "640K"
  video  = ["vga", "ega", "hercules", "cga"]
  input  = ["mouse", "none"]
}

ids {
  wikidata = "Q0"  # PLACEHOLDER: the work's item was not looked up
}

input "disk1" {
  name   = "Microsoft Windows 3.0 (5.25-1.2M) (Disk 1 of 5).img"  # ASSUMED: a catalogue's name
  size   = 1228800
  sha256 = "0000000000000000000000000000000000000000000000000000000000000001"  # PLACEHOLDER: not measured
  ids    = [
    { db = "tosec", set = "IBM PC Compatibles - Operating Systems - [IMG]", name = "PLACEHOLDER disk 1" },
  ]
}

input "disk2" {
  name   = "Microsoft Windows 3.0 (5.25-1.2M) (Disk 2 of 5).img"  # ASSUMED: a catalogue's name
  size   = 1228800
  sha256 = "0000000000000000000000000000000000000000000000000000000000000002"  # PLACEHOLDER: not measured
  ids    = [
    { db = "tosec", set = "IBM PC Compatibles - Operating Systems - [IMG]", name = "PLACEHOLDER disk 2" },
  ]
}

input "disk3" {
  name   = "Microsoft Windows 3.0 (5.25-1.2M) (Disk 3 of 5).img"  # ASSUMED: a catalogue's name
  size   = 1228800
  sha256 = "0000000000000000000000000000000000000000000000000000000000000003"  # PLACEHOLDER: not measured
  ids    = [
    { db = "tosec", set = "IBM PC Compatibles - Operating Systems - [IMG]", name = "PLACEHOLDER disk 3" },
  ]
}

input "disk4" {
  name   = "Microsoft Windows 3.0 (5.25-1.2M) (Disk 4 of 5).img"  # ASSUMED: a catalogue's name
  size   = 1228800
  sha256 = "0000000000000000000000000000000000000000000000000000000000000004"  # PLACEHOLDER: not measured
  ids    = [
    { db = "tosec", set = "IBM PC Compatibles - Operating Systems - [IMG]", name = "PLACEHOLDER disk 4" },
  ]
}

input "disk5" {
  name   = "Microsoft Windows 3.0 (5.25-1.2M) (Disk 5 of 5).img"  # ASSUMED: a catalogue's name
  size   = 1228800
  sha256 = "0000000000000000000000000000000000000000000000000000000000000005"  # PLACEHOLDER: not measured
  ids    = [
    { db = "tosec", set = "IBM PC Compatibles - Operating Systems - [IMG]", name = "PLACEHOLDER disk 5" },
  ]
}

input "users-guide" {
  name   = "Microsoft Windows 3.0 User's Guide.pdf"  # ASSUMED
  size   = 1  # PLACEHOLDER: not measured
  sha256 = "0000000000000000000000000000000000000000000000000000000000000099"  # PLACEHOLDER: not measured
}

medium "disk-1" {
  label  = "Disk 1 of 5"  # ASSUMED: the disks' own labels were not read
  format = "1.2M"
  from   = "disk1"
}

medium "disk-2" {
  label  = "Disk 2 of 5"  # ASSUMED: the disks' own labels were not read
  format = "1.2M"
  from   = "disk2"
}

medium "disk-3" {
  label  = "Disk 3 of 5"  # ASSUMED: the disks' own labels were not read
  format = "1.2M"
  from   = "disk3"
}

medium "disk-4" {
  label  = "Disk 4 of 5"  # ASSUMED: the disks' own labels were not read
  format = "1.2M"
  from   = "disk4"
}

medium "disk-5" {
  label  = "Disk 5 of 5"  # ASSUMED: the disks' own labels were not read
  format = "1.2M"
  from   = "disk5"
}

attachment "documentation" {
  title = "Printed documentation"
  kind  = "documentation"

  extra "users-guide" {
    file_name = "Microsoft Windows 3.0 User's Guide.pdf"
    kind      = "document"
    from      = "users-guide"
  }
}
