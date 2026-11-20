# MS-DOS 3.30 installed from a private two-disk 360K distribution.
# ASSUMED: this exact distribution layout and media artifact; disks are supplied
# privately. No software bytes, installer text or screen captures appear here.
# FDISK creates a whole-disk active primary partition below DOS 3.30's 32 MiB
# limit. FORMAT /S transfers the system, then COPY installs the utilities.
# ASSUMED: FDISK's exact menu keys, format confirmation/label order and reboot
# transition. Each private wait is a placeholder hash of the intended screen.
# The zero-filled base's hash is calculated from its specified bytes, not a run.
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
  name = "private/machines/ibm-pc/msdos"
  platform = "ibm-pc"
  release = "3.30"
  revision = 1
  digest = "sha256:0000000000000000000000000000000000000000000000000000000000000020"
}

from {
  image = "unemu/machines/ibm-pc/open-at:blank-30m-r1"
  digest = "sha256:0000000000000000000000000000000000000000000000000000000000000001"
}

use {
  install {
    image = "private/media/ibm-pc/msdos:3.30-360k-r1"
    digest = "sha256:0000000000000000000000000000000000000000000000000000000000000002"
  }
}

output {
  disk = "system"
  file_name = "msdos-3.30.img"
  format = "raw"
}

lock {
  status = "ASSUMED-unbuilt"
  output_manifest = "sha256:0000000000000000000000000000000000000000000000000000000000000020"
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
    sha256 = "sha256:75c91b29d5522c8a97c779e50bc33f11e07ed37b2baa31c8c727016e92915c1d"
    size = 31457280
    geometry {
      cylinders = 960
      heads = 4
      sectors = 16
    }
  }
  artifact {
    id = "from"
    image = "unemu/machines/ibm-pc/open-at:blank-30m-r1"
    digest = "sha256:0000000000000000000000000000000000000000000000000000000000000001"
  }
  artifact {
    id = "install"
    image = "private/media/ibm-pc/msdos:3.30-360k-r1"
    digest = "sha256:0000000000000000000000000000000000000000000000000000000000000002"
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
    sha256 = "sha256:000000000000000000000000000000000000000000000000000000000000014c"
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
  # ASSUMED checkpoint: first DOS date input after boot.
  budget = 400000000
  wait_state {
    kind = "screen-digest"
    card = "vga"
    sha256 = "sha256:00000000000000000000000000000000000000000000000000000000000003e9"
  }
}

step {
  input {
    key = "enter"
  }
  input {
    # ASSUMED checkpoint: DOS time input.
    budget = 400000000
    wait_state {
      kind = "screen-digest"
      card = "vga"
      sha256 = "sha256:00000000000000000000000000000000000000000000000000000000000003ea"
    }
  }
  input {
    key = "enter"
  }
  input {
    # ASSUMED checkpoint: DOS shell ready on A:.
    budget = 400000000
    wait_state {
      kind = "screen-digest"
      card = "vga"
      sha256 = "sha256:00000000000000000000000000000000000000000000000000000000000003eb"
    }
  }
  input {
    type = "FDISK"
  }
  input {
    key = "enter"
  }
  input {
    # ASSUMED checkpoint: FDISK main menu.
    budget = 400000000
    wait_state {
      kind = "screen-digest"
      card = "vga"
      sha256 = "sha256:00000000000000000000000000000000000000000000000000000000000003ec"
    }
  }
  input {
    key = "1"
  }
  input {
    key = "enter"
  }
  input {
    # ASSUMED checkpoint: FDISK create-partition menu.
    budget = 400000000
    wait_state {
      kind = "screen-digest"
      card = "vga"
      sha256 = "sha256:00000000000000000000000000000000000000000000000000000000000003ed"
    }
  }
  input {
    key = "1"
  }
  input {
    key = "enter"
  }
  input {
    # ASSUMED checkpoint: FDISK primary-partition size choice.
    budget = 400000000
    wait_state {
      kind = "screen-digest"
      card = "vga"
      sha256 = "sha256:00000000000000000000000000000000000000000000000000000000000003ee"
    }
  }
  input {
    key = "y"
  }
  input {
    key = "enter"
  }
  input {
    # ASSUMED checkpoint: FDISK completion and restart boundary.
    budget = 400000000
    wait_state {
      kind = "screen-digest"
      card = "vga"
      sha256 = "sha256:00000000000000000000000000000000000000000000000000000000000003ef"
    }
  }
  input {
    key = "escape"
  }
}

step {
  reset = "warm"
}

step {
  # ASSUMED checkpoint: DOS date input after partition reboot.
  budget = 400000000
  wait_state {
    kind = "screen-digest"
    card = "vga"
    sha256 = "sha256:00000000000000000000000000000000000000000000000000000000000003f0"
  }
}

step {
  input {
    key = "enter"
  }
  input {
    # ASSUMED checkpoint: DOS time input after partition reboot.
    budget = 400000000
    wait_state {
      kind = "screen-digest"
      card = "vga"
      sha256 = "sha256:00000000000000000000000000000000000000000000000000000000000003f1"
    }
  }
  input {
    key = "enter"
  }
  input {
    # ASSUMED checkpoint: DOS shell ready to format C:.
    budget = 400000000
    wait_state {
      kind = "screen-digest"
      card = "vga"
      sha256 = "sha256:00000000000000000000000000000000000000000000000000000000000003f2"
    }
  }
  input {
    type = "FORMAT C: /S"
  }
  input {
    key = "enter"
  }
  input {
    # ASSUMED checkpoint: FORMAT destructive-operation confirmation.
    budget = 400000000
    wait_state {
      kind = "screen-digest"
      card = "vga"
      sha256 = "sha256:00000000000000000000000000000000000000000000000000000000000003f3"
    }
  }
  input {
    key = "y"
  }
  input {
    key = "enter"
  }
  input {
    # ASSUMED checkpoint: FORMAT volume-label input.
    budget = 400000000
    wait_state {
      kind = "screen-digest"
      card = "vga"
      sha256 = "sha256:00000000000000000000000000000000000000000000000000000000000003f4"
    }
  }
  input {
    key = "enter"
  }
  input {
    # ASSUMED checkpoint: DOS shell after system transfer.
    budget = 400000000
    wait_state {
      kind = "screen-digest"
      card = "vga"
      sha256 = "sha256:00000000000000000000000000000000000000000000000000000000000003f5"
    }
  }
}

step {
  input {
    type = "MD C:\\DOS"
  }
  input {
    key = "enter"
  }
  input {
    type = "COPY A:\\*.* C:\\DOS\\"
  }
  input {
    key = "enter"
  }
}

step {
  # ASSUMED checkpoint: DOS shell after boot-disk utility copy.
  budget = 400000000
  wait_state {
    kind = "screen-digest"
    card = "vga"
    sha256 = "sha256:00000000000000000000000000000000000000000000000000000000000003f6"
  }
}

step {
  insert = "install/disk-2"
  drive = "a"
  write = "readonly"
}

step {
  input {
    type = "COPY A:\\*.* C:\\DOS\\"
  }
  input {
    key = "enter"
  }
}

step {
  # ASSUMED checkpoint: DOS shell after utility-disk copy.
  budget = 400000000
  wait_state {
    kind = "screen-digest"
    card = "vga"
    sha256 = "sha256:00000000000000000000000000000000000000000000000000000000000003f7"
  }
}

step {
  input {
    type = "COPY CON C:\\CONFIG.SYS"
  }
  input {
    key = "enter"
  }
  input {
    type = <<-TEXT
FILES=20
BUFFERS=20
TEXT
  }
  input {
    key = "ctrl+z"
  }
  input {
    key = "enter"
  }
}

step {
  # ASSUMED checkpoint: DOS shell after CONFIG.SYS entry.
  budget = 400000000
  wait_state {
    kind = "screen-digest"
    card = "vga"
    sha256 = "sha256:00000000000000000000000000000000000000000000000000000000000003f8"
  }
}

step {
  input {
    type = "COPY CON C:\\AUTOEXEC.BAT"
  }
  input {
    key = "enter"
  }
  input {
    type = <<-TEXT
@ECHO OFF
PATH C:\DOS
PROMPT $P$G
TEXT
  }
  input {
    key = "ctrl+z"
  }
  input {
    key = "enter"
  }
}

step {
  # ASSUMED checkpoint: DOS shell after AUTOEXEC.BAT entry.
  budget = 400000000
  wait_state {
    kind = "screen-digest"
    card = "vga"
    sha256 = "sha256:00000000000000000000000000000000000000000000000000000000000003f9"
  }
}

step {
  eject = "a"
}

step {
  reset = "warm"
}

step {
  # ASSUMED checkpoint: DOS shell booted from C:.
  budget = 400000000
  wait_state {
    kind = "screen-digest"
    card = "vga"
    sha256 = "sha256:00000000000000000000000000000000000000000000000000000000000003fa"
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
ECHO SYSTEM=MSDOS-3.30
ECHO ROOT=C:\DOS
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
SYSTEM=MSDOS-3.30
ROOT=C:\DOS
TEXT
  card = "vga"
  match = "contains"
  budget = 400000000
}

step {
  commit = "system"
  label = "MS-DOS 3.30 bootable system and utilities"
}
