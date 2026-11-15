# SPDX-License-Identifier: AGPL-3.0-or-later
"""Copy named files out of an ISO 9660 image, reading only the primary
volume descriptor and its directory records (ECMA-119, sections 8.4 and 9.1).
Names compare case-insensitively without the ";1" version suffix.
Usage: iso-extract.py IMAGE OUTDIR PATH...

The caller checks the image against a pinned SHA-256 before running this, so
the input is known bytes; the bounds below only stop a damaged or unexpected
image early. Output paths come from the arguments, never from names in the
image, and nothing in the image is executed."""
import os
import sys

SECTOR = 2048
# A directory record is at least 33 bytes plus a one-byte name.
MIN_RECORD = 34
# Directory extents of the release LiveCD are a few sectors; more means a
# damaged image.
MAX_DIRECTORY = 4 * 1024 * 1024


def main():
    image, out, wanted = sys.argv[1], sys.argv[2], sys.argv[3:]
    image_size = os.path.getsize(image)
    with open(image, "rb") as iso:
        def read(lba, length):
            if lba * SECTOR + length > image_size:
                sys.exit("extent past the end of the image")
            iso.seek(lba * SECTOR)
            return iso.read(length)

        pvd = read(16, SECTOR)
        if pvd[0] != 1 or pvd[1:6] != b"CD001":
            sys.exit("not an ISO 9660 primary volume descriptor")
        root = pvd[156:190]

        def entries(record):
            lba = int.from_bytes(record[2:6], "little")
            size = int.from_bytes(record[10:14], "little")
            if size > MAX_DIRECTORY:
                sys.exit("directory extent too large")
            data = read(lba, size)
            at = 0
            while at < len(data):
                length = data[at]
                if length == 0:
                    at = (at // SECTOR + 1) * SECTOR
                    continue
                if length < MIN_RECORD or at + length > len(data):
                    sys.exit("damaged directory record")
                entry = data[at:at + length]
                name_length = entry[32]
                name = entry[33:33 + name_length]
                if name not in (b"\x00", b"\x01"):
                    text = name.decode("ascii").split(";")[0].rstrip(".").lower()
                    yield text, entry
                at += length

        for path in wanted:
            record = root
            parts = path.lower().split("/")
            for depth, part in enumerate(parts):
                if depth > 0 and not record[25] & 2:
                    sys.exit(f"{path}: {parts[depth - 1]} is not a directory")
                found = dict(entries(record)).get(part)
                if found is None:
                    sys.exit(f"{path}: not in the image")
                record = found
            if record[25] & 2:
                sys.exit(f"{path}: is a directory")
            lba = int.from_bytes(record[2:6], "little")
            size = int.from_bytes(record[10:14], "little")
            target = os.path.join(out, path)
            os.makedirs(os.path.dirname(target), exist_ok=True)
            with open(target, "wb") as copy:
                copy.write(read(lba, size))


main()
