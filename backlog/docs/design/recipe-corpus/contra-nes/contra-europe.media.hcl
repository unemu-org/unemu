# Probotector (Europe), an NES cartridge: private/media/nes/contra:europe-r1. A FUTURE platform: the id
# `nes`, the format `ines` and the needs vocabulary are ASSUMED. Private. The size, hash,
# year and ids are PLACEHOLDERS: no dump was measured (README).

recipe = "0.1"

media {
  name     = "private/media/nes/contra"
  platform = "nes"  # ASSUMED
  release  = "europe"
  revision = 1
}

title {
  title             = "Probotector"
  series            = "Contra"
  year              = 1990  # ASSUMED
  publishers        = ["Konami"]
  developers        = ["Konami"]
  genres            = ["run and gun"]
  software_platform = "NES"  # ASSUMED: not in metadata schema 1's list
  regions           = ["Europe"]
  players           = { min = 1, max = 2 }
  labels            = ["game"]
  description       = "Probotector, the Europe release of Konami's run-and-gun game for the Nintendo Entertainment System, as one cartridge dump."
}

needs {
  tv_system = ["pal"]  # ASSUMED
}

ids {
  wikidata = "Q0"  # PLACEHOLDER: one work id shared by the three regional recipes
}

input "cart" {
  name   = "Probotector (Europe).nes"
  size   = 131088  # ASSUMED: with the 16-byte iNES header
  sha256 = "0000000000000000000000000000000000000000000000000000000000000003"  # PLACEHOLDER: not measured
  sha1   = "0000000000000000000000000000000000000000"  # PLACEHOLDER: not measured
  ids    = [
    { db = "no-intro", set = "Nintendo - Nintendo Entertainment System (Headered)", name = "Probotector (Europe)" },
  ]
}

medium "cart" {
  label     = "Probotector (Europe)"
  format    = "ines"  # ASSUMED
  file_name = "Probotector (Europe).nes"
  from      = "cart"
}
