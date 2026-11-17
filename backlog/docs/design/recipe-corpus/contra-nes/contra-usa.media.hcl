# Contra (USA), an NES cartridge: private/media/nes/contra:usa-r1. A FUTURE platform: the id
# `nes`, the format `ines` and the needs vocabulary are ASSUMED. Private. The size, hash,
# year and ids are PLACEHOLDERS: no dump was measured (README).

recipe = "0.1"

media {
  name     = "private/media/nes/contra"
  platform = "nes"  # ASSUMED
  release  = "usa"
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
  regions           = ["USA"]
  players           = { min = 1, max = 2 }
  labels            = ["game"]
  description       = "Contra, the USA release of Konami's run-and-gun game for the Nintendo Entertainment System, as one cartridge dump."
}

needs {
  tv_system = ["ntsc"]  # ASSUMED
}

ids {
  wikidata = "Q0"  # PLACEHOLDER: one work id shared by the three regional recipes
}

input "cart" {
  name   = "Contra (USA).nes"
  size   = 131088  # ASSUMED: with the 16-byte iNES header
  sha256 = "0000000000000000000000000000000000000000000000000000000000000001"  # PLACEHOLDER: not measured
  sha1   = "0000000000000000000000000000000000000000"  # PLACEHOLDER: not measured
  ids    = [
    { db = "no-intro", set = "Nintendo - Nintendo Entertainment System (Headered)", name = "Contra (USA)" },
  ]
}

medium "cart" {
  label     = "Contra (USA)"
  format    = "ines"  # ASSUMED
  file_name = "Contra (USA).nes"
  from      = "cart"
}
