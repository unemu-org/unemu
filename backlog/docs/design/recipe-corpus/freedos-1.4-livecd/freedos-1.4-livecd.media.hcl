# FreeDOS 1.4 LiveCD: private/media/ibm-pc/freedos:1.4-livecd-r1 (no licence statement, so
# user-supplied and private). The ISO, unmodified, and the
# boot floppy that ships beside it. The archive's hash, size and URLs and the ISO's hash are
# real (unemu-media build.sh); the ISO's size is from the earlier design draft. See README.

recipe = "0.1"

media {
  name     = "private/media/ibm-pc/freedos"
  platform = "ibm-pc"
  release  = "1.4-livecd"
  revision = 1
}

title {
  title             = "FreeDOS"
  version           = "1.4"
  edition           = "LiveCD"
  year              = 2025
  publishers        = ["The FreeDOS Project"]
  developers        = ["The FreeDOS Project"]
  genres            = ["operating system"]
  software_platform = "DOS"
  labels            = ["freedos", "operating-system"]
  description       = "The official FreeDOS 1.4 LiveCD, unmodified: a bootable CD-ROM that runs FreeDOS without installing it and installs the full system with all packages, and the boot diskette for machines that cannot start from a CD. Released by the FreeDOS Project on 2 April 2025."
}

# ASSUMED.
needs {
  cpu    = "386+"
  memory = "640K"
  video  = ["vga", "ega"]
}

input "livecd" {
  name   = "FD14-LiveCD.zip"
  size   = 293950337
  sha256 = "2020ff6bb681967fd6eff8f51ad2e5cd5ab4421165948cef4246e4f7fcaf6339"
  urls   = [
    "https://download.freedos.org/1.4/FD14-LiveCD.zip",
    "https://www.ibiblio.org/pub/micro/pc-stuff/freedos/files/distributions/1.4/FD14-LiveCD.zip",
  ]
}

step "unpack" "cd" {
  from     = "livecd"
  format   = "zip"
  complete = true
  members  = [
    { path = "FD14BOOT.img" },
    { path = "FD14LIVE.iso", size = 316557312, sha256 = "c48a9dcf4b8e22f44e268a9879745f0bd88c061195ac584e6ef2deb0477f81fb" },
    { path = "readme.txt" },
  ]
}

medium "cd" {
  label     = "FreeDOS 1.4 LiveCD (FD14LIVE.iso)"
  format    = "iso-2048"
  file_name = "FD14LIVE.iso"
  from      = "cd/FD14LIVE.iso"
}

medium "boot" {
  label     = "Boot diskette for the LiveCD (FD14BOOT.img)"
  format    = "1.44M"  # ASSUMED: the image's size was not measured
  file_name = "FD14BOOT.img"
  from      = "cd/FD14BOOT.img"
}

extra "readme" {
  file_name = "readme.txt"
  kind      = "document"
  from      = "cd/readme.txt"
}
