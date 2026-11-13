# LTEMM r01 local fixture

Run `vendor/ltemm/fetch.sh` to fetch and verify the Lo-tech EMS driver, or
`LTEMM_ARCHIVE=/path/to/LTEMM-r01.zip vendor/ltemm/fetch.sh` for offline use.
Requires Bash, curl (online only), unzip, and sha256sum or shasum.

The archive README says the driver is based on Alex Tsourikov's 1988 driver,
whose source was supplied under BSD-3-Clause. The modified LTEMM driver is
subject to Lo-tech's general terms: non-commercial hobbyist use, with no
redistribution right granted. It is not an open-software fixture. The terms
await maintainer confirmation (decision-23); fetching does not add redistribution rights.

LTEMM.EXE and the archive are never committed, redistributed, published or
included in a committed/published disk image. Tests add the driver only to an
in-memory copy of an open FreeDOS image. No driver source or banner is stored
here. Temporary extraction directories are removed even on failure.

| Artifact | SHA-256 |
| --- | --- |
| LTEMM-r01.zip | `0bcbcac26dd21da6125da79d74107905989c092f1639416ba90219001602383a` |
| LTEMM.EXE | `457c9a849f87749a067d4ef23477106de4abd575eb4d5cd0ff8755aeaa80956d` |

Upstream: <https://www.lo-tech.co.uk/downloads/2MB-EMS-Board/LTEMM-r01.zip>

Fallback: <https://web.archive.org/web/20260820161725/https://www.lo-tech.co.uk/downloads/2MB-EMS-Board/LTEMM-r01.zip>

It is an MZ EXE device driver: `DEVICE=LTEMM.EXE /p:D000 /n` in CONFIG.SYS
for the IBM 5170 configuration. `/p:nnnn` selects the frame segment, `/i:nnn`
selects the I/O base, `/q` is quiet, and `/3` selects EMS 3.2 only. `/n` skips
only the keyboard-polled memory test. The RAM probe still writes port 61h;
the golden uses `/n` and therefore still exercises that probe.

Online fetching verifies the archive after each download. A failed upstream
download or a mismatched upstream hash retries the HTTPS archive fallback.
The fallback must pass the same pinned hash before extraction.
