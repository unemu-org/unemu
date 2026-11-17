# Contra (Japan), an NES cartridge: private/media/nes/contra:japan-r1. A FUTURE platform: the id
# `nes`, the format `ines` and the needs vocabulary are ASSUMED. Private. The size, hash,
# year and ids are PLACEHOLDERS: no dump was measured (README).

recipe = "0.1"

media {
  name     = "private/media/nes/contra"
  platform = "nes"  # ASSUMED
  release  = "japan"
  revision = 1
}

title {
  title             = "Contra"
  series            = "Contra"
  year              = 1988  # ASSUMED
  publishers        = ["Konami"]
  developers        = ["Konami"]
  genres            = ["run and gun"]
  software_platform = "NES"  # ASSUMED: not in metadata schema 1's list
  regions           = ["Japan"]
  players           = { min = 1, max = 2 }
  labels            = ["game"]
  description       = "Contra, the Japan release of Konami's run-and-gun game for the Nintendo Entertainment System, as one cartridge dump."
}

needs {
  tv_system = ["ntsc"]  # ASSUMED
}

ids {
  wikidata = "Q0"  # PLACEHOLDER: one work id shared by the three regional recipes
}

input "cart" {
  name   = "Contra (Japan).nes"
  size   = 262160  # ASSUMED: with the 16-byte iNES header
  sha256 = "0000000000000000000000000000000000000000000000000000000000000002"  # PLACEHOLDER: not measured
  sha1   = "0000000000000000000000000000000000000000"  # PLACEHOLDER: not measured
  ids    = [
    { db = "no-intro", set = "Nintendo - Nintendo Entertainment System (Headered)", name = "Contra (Japan)" },
  ]
}

medium "cart" {
  label     = "Contra (Japan)"
  format    = "ines"  # ASSUMED
  file_name = "Contra (Japan).nes"
  from      = "cart"
}
