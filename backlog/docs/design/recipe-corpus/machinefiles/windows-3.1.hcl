# Windows 3.1 installed from six private 1.44M floppies, standard mode on a 286.
# READ: Microsoft KB Q113227..Q113233 list this six-disk distribution; see README.
# ASSUMED: the MS-DOS 3.30 output of msdos-3.30.toml is the parent;
# lock.base.sha256 equals that document's lock.capture.sha256 placeholder.
# No title bytes or title-file text is included. Commands and example identity
# are ours; screen hashes stand for private checkpoints, not copied dialog text.
# ASSUMED: custom Setup with VGA, US keyboard, no mouse, printer or network;
# install to C:\WINDOWS, skip application search/tutorial, return to DOS.
# Exact dialog order, focused controls, disk requests and keys are NOT measured.
# A DOS sentinel proves return to the shell only; it does not prove WIN /S boots.
# Design document, not accepted by recipe check/build or machine check.
# ASSUMED: the Machinefile extensions, templates and lock shape described in
# ../README.md, section "Machinefile corpus (2026-11-20)". No guest run was made.
# ASSUMED: every key sequence, wait, budget and private screen digest below.
# Zero-padded digests are PLACEHOLDERS, not measured pins; never publish them.
# READ: CPU revision and the three firmware artifact pins come from this tree.
# ASSUMED: from resolves an AT/80286 template with 640 KiB conventional RAM,
# 2048 KiB extended RAM, VGA, US keyboard, one 1.44M floppy drive and primary
# IDE master named system, 960/4/16 sectors of 512 bytes (30 MiB).
# The disk base is immutable and fully SHA-256 pinned; all writes use a separate
# sector overlay. commit captures a NEW base, never overwrites the parent.
# ASSUMED: inline lock is a readable draft of run/input/output pins, not the
# emitted manifest/config/metadata lock directory specified by recipes.md 7.2.
# The engine revision and output hashes must be measured before a real pin.

recipe = "0.1"

machine {
  name = "private/machines/ibm-pc/windows"
  platform = "ibm-pc"
  release = "3.1"
  revision = 1
  digest = "sha256:0000000000000000000000000000000000000000000000000000000000000021"
}

from {
  image = "private/machines/ibm-pc/msdos:3.30-r1"
  digest = "sha256:0000000000000000000000000000000000000000000000000000000000000020"
}

use {
  install {
    image = "private/media/ibm-pc/windows:3.1-1.44m-r1"
    digest = "sha256:0000000000000000000000000000000000000000000000000000000000000003"
  }
}

output {
  disk = "system"
  file_name = "windows-3.1.img"
  format = "raw"
}

lock {
  status = "ASSUMED-unbuilt"
  output_manifest = "sha256:0000000000000000000000000000000000000000000000000000000000000021"
  engine {
    id = "unemupc"
    revision = "0000000000000000000000000000000000000000"
    cpu_revision = "6cb817b86c1fbf463f7aab8c0da1ce0eec3db0b7"
  }
  run {
    rtc_start = "1992-04-06T00:00:00"
    keyboard_layout = "us"
    cpu_rate = "8/13"
  }
  base {
    disk = "system"
    sha256 = "sha256:000000000000000000000000000000000000000000000000000000000000014c"
    size = 31457280
    geometry {
      cylinders = 960
      heads = 4
      sectors = 16
    }
  }
  artifact {
    id = "from"
    image = "private/machines/ibm-pc/msdos:3.30-r1"
    digest = "sha256:0000000000000000000000000000000000000000000000000000000000000020"
  }
  artifact {
    id = "install"
    image = "private/media/ibm-pc/windows:3.1-1.44m-r1"
    digest = "sha256:0000000000000000000000000000000000000000000000000000000000000003"
  }
  artifact {
    id = "system-bios"
    image = "unemu/firmware/ibm-pc/unemu-at-bios-1.0.2-unemu.3:r1"
    digest = "sha256:c4c53a6a5c0727d8cccb26b22a92dcbf68e906aed7d9424be80763ea30533ff2"
  }
  artifact {
    id = "video-bios"
    image = "unemu/firmware/ibm-pc/vgabios-0.9d-unemu.1-std:r1"
    digest = "sha256:c1972247ba5fc03dee5bc011b137e49c547ef1f4fc32dc3f74fa09b205c235e7"
  }
  artifact {
    id = "fixed-disk-bios"
    image = "unemu/firmware/ibm-pc/xub-r638-xtp-ata1f0:r1"
    digest = "sha256:4dc25f1c00342e44b2476e84befc9dbbb1817d478cdda9b1fd0a5d8e026026a1"
  }
  capture {
    disk = "system"
    sha256 = "sha256:000000000000000000000000000000000000000000000000000000000000014d"
    size = 31457280
  }
}

step {
  insert = "install/disk-1"
  drive = "a"
  write = "readonly"
}

step {
  start = true
}

step {
  # ASSUMED checkpoint: DOS shell before Windows Setup.
  budget = 400000000
  wait_state {
    kind = "screen-digest"
    card = "vga"
    sha256 = "sha256:0000000000000000000000000000000000000000000000000000000000000406"
  }
}

step {
  input {
    type = "A:\\SETUP"
  }
  input {
    key = "enter"
  }
  input {
    # ASSUMED checkpoint: Setup initial confirmation.
    budget = 400000000
    wait_state {
      kind = "screen-digest"
      card = "vga"
      sha256 = "sha256:0000000000000000000000000000000000000000000000000000000000000407"
    }
  }
  input {
    key = "enter"
  }
  input {
    # ASSUMED checkpoint: Setup destination-directory input.
    budget = 400000000
    wait_state {
      kind = "screen-digest"
      card = "vga"
      sha256 = "sha256:0000000000000000000000000000000000000000000000000000000000000408"
    }
  }
  input {
    type = "C:\\WINDOWS"
  }
  input {
    key = "enter"
  }
  input {
    # ASSUMED checkpoint: Setup hardware selection.
    budget = 400000000
    wait_state {
      kind = "screen-digest"
      card = "vga"
      sha256 = "sha256:0000000000000000000000000000000000000000000000000000000000000409"
    }
  }
  input {
    key = "enter"
  }
  input {
    # ASSUMED checkpoint: Setup user and organization input.
    budget = 400000000
    wait_state {
      kind = "screen-digest"
      card = "vga"
      sha256 = "sha256:000000000000000000000000000000000000000000000000000000000000040a"
    }
  }
  input {
    type = "Corpus Example"
  }
  input {
    key = "tab"
  }
  input {
    type = "Example Organization"
  }
  input {
    key = "enter"
  }
  input {
    # ASSUMED checkpoint: Setup identity confirmation.
    budget = 400000000
    wait_state {
      kind = "screen-digest"
      card = "vga"
      sha256 = "sha256:000000000000000000000000000000000000000000000000000000000000040b"
    }
  }
  input {
    key = "enter"
  }
}

step {
  # ASSUMED checkpoint: Setup requests installation disk 2.
  budget = 400000000
  wait_state {
    kind = "screen-digest"
    card = "vga"
    sha256 = "sha256:000000000000000000000000000000000000000000000000000000000000040c"
  }
}

step {
  insert = "install/disk-2"
  drive = "a"
  write = "readonly"
}

step {
  key = "enter"
}

step {
  # ASSUMED checkpoint: Setup requests installation disk 3.
  budget = 400000000
  wait_state {
    kind = "screen-digest"
    card = "vga"
    sha256 = "sha256:000000000000000000000000000000000000000000000000000000000000040d"
  }
}

step {
  insert = "install/disk-3"
  drive = "a"
  write = "readonly"
}

step {
  key = "enter"
}

step {
  # ASSUMED checkpoint: Setup requests installation disk 4.
  budget = 400000000
  wait_state {
    kind = "screen-digest"
    card = "vga"
    sha256 = "sha256:000000000000000000000000000000000000000000000000000000000000040e"
  }
}

step {
  insert = "install/disk-4"
  drive = "a"
  write = "readonly"
}

step {
  key = "enter"
}

step {
  # ASSUMED checkpoint: Setup requests installation disk 5.
  budget = 400000000
  wait_state {
    kind = "screen-digest"
    card = "vga"
    sha256 = "sha256:000000000000000000000000000000000000000000000000000000000000040f"
  }
}

step {
  insert = "install/disk-5"
  drive = "a"
  write = "readonly"
}

step {
  key = "enter"
}

step {
  # ASSUMED checkpoint: Setup requests installation disk 6.
  budget = 400000000
  wait_state {
    kind = "screen-digest"
    card = "vga"
    sha256 = "sha256:0000000000000000000000000000000000000000000000000000000000000410"
  }
}

step {
  insert = "install/disk-6"
  drive = "a"
  write = "readonly"
}

step {
  key = "enter"
}

step {
  # ASSUMED checkpoint: Setup printer selection.
  budget = 400000000
  wait_state {
    kind = "screen-digest"
    card = "vga"
    sha256 = "sha256:0000000000000000000000000000000000000000000000000000000000000411"
  }
}

step {
  input {
    key = "escape"
  }
  input {
    # ASSUMED checkpoint: Setup application-search choice.
    budget = 400000000
    wait_state {
      kind = "screen-digest"
      card = "vga"
      sha256 = "sha256:0000000000000000000000000000000000000000000000000000000000000412"
    }
  }
  input {
    key = "escape"
  }
  input {
    # ASSUMED checkpoint: Setup tutorial choice.
    budget = 400000000
    wait_state {
      kind = "screen-digest"
      card = "vga"
      sha256 = "sha256:0000000000000000000000000000000000000000000000000000000000000413"
    }
  }
  input {
    key = "escape"
  }
  input {
    # ASSUMED checkpoint: Setup exit to DOS.
    budget = 400000000
    wait_state {
      kind = "screen-digest"
      card = "vga"
      sha256 = "sha256:0000000000000000000000000000000000000000000000000000000000000414"
    }
  }
  input {
    key = "enter"
  }
}

step {
  eject = "a"
}

step {
  reset = "warm"
}

step {
  # ASSUMED checkpoint: DOS shell after Windows installation reboot.
  budget = 400000000
  wait_state {
    kind = "screen-digest"
    card = "vga"
    sha256 = "sha256:0000000000000000000000000000000000000000000000000000000000000415"
  }
}

step {
  input {
    type = "COPY CON C:\\VERIFY.BAT"
  }
  input {
    key = "enter"
  }
  input {
    type = <<-TEXT
@ECHO OFF
ECHO CORPUS-READY
ECHO SYSTEM=WINDOWS-3.1
ECHO ROOT=C:\WINDOWS
TEXT
  }
  input {
    key = "ctrl+z"
  }
  input {
    key = "enter"
  }
  input {
    type = "C:\\VERIFY.BAT"
  }
  input {
    key = "enter"
  }
}

step {
  wait_text = <<-TEXT
CORPUS-READY
SYSTEM=WINDOWS-3.1
ROOT=C:\WINDOWS
TEXT
  card = "vga"
  match = "contains"
  budget = 400000000
}

step {
  commit = "system"
  label = "Windows 3.1 standard-mode installation on MS-DOS 3.30"
}
