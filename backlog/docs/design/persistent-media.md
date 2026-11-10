# Recoverable persistent floppy media

Status: accepted design; offline tools, immutable export and retention implemented.
The maintainer approved the defaults on 2026-11-10.
The version-1 codec and recovery scanner are implemented in `crates/media`;
the host-independent device media contract and its in-memory implementation
are implemented in `crates/device`. The durable host sink and single-writer
lifecycle are implemented in `crates/media`. Assembly now wires all three write
modes, validates journal/sidecar path aliases, replays before execution, and exposes
per-drive status/checkpoint/finish handles. CLI and window run boundaries check
host failures and explicitly finish successful sessions. Offline generation
listing, verification, explicit tail repair and backed-up rollback are implemented
in the CLI and `crates/media`, including immutable-image export (`commit`/`compact`).
The sink applies the configured positive retention count after durable checkpoints
and session closes, replacing old history with a verified anchor and retained frames.

## Context

Guest writes need to survive a restart without destroying the last usable disk.
Keep the original raw image immutable and append sector replacements to a
separate journal. Close a generation at the end of each session or at an explicit
checkpoint. A user can discard a bad session by returning to an earlier generation.
A generation means a recovery point, not a guarantee that the guest filesystem
is healthy: the user decides which version works.

Evidence from the current implementation:

- **READ (before the media contract landed; historical line numbers):**
  `crates/devices/src/upd765.rs:882–909`: WRITE DATA committed DMA bytes
  into the former `Drive.image` field. A partial final sector was zero-filled to
  512 bytes; a short isolation-test image grew. `:918–935` computed status and
  raised IRQ6 after mutation. An abnormal result could therefore accompany
  committed sectors.
- **READ:** `crates/assembly/src/image.rs:13–22` defines inline, file and blank
  sources. `assemble.rs:147–153,284–318` reads file sources once, resolves them
  to inline bytes, and validates the bytes actually mounted.
- **READ:** `crates/assembly/src/assemble.rs`, `mount_fdc_drives`, routes drives
  by named `controller: DeviceId` and controller-local `unit`. In
  `crates/assembly/src/drive.rs`, `DriveConfig.write` is `WriteMode::Volatile`
  (default), `WriteMode::ReadOnly`, or explicit `WriteMode::Journal`. Readonly
  uses `Drive::new_read_only` and preserves the existing not-writable status;
  journal mounts replay into a sector-media adapter. Volatile and readonly
  create no host output.
- **READ:** [Deferred journal tools and retention](../deferred.md#offline-floppy-journal-tools-and-retention)
  records implemented tools and the remaining frontend/configuration work.
  Append/replay persistence uses the assembly session API below.
- **READ:** [Machine configuration](machine-config.md#single-homes-no-triplication)
  assigns media to top-level drives and composition to assembly. It deliberately
  defines Rust data, without a serialized config format.
- **READ:** `crates/cli/tests/fdc_write_golden.rs:42–48` proves same-run write/read
  behavior. `dos_write_golden.rs:132–136` inspects final in-memory FAT12 bytes;
  it does not establish persistence across assemblies.

Scope: the existing 512-byte-sector floppy formats, a single writer per journal,
offline recovery tools, and deterministic headless execution. No CPU change,
fixed-disk controller, whole-machine snapshot, guest-filesystem transaction,
live recovery service, compression, encryption or qcow2 compatibility is needed.
Default drives and all goldens remain volatile. Fixture paths must never become
journal outputs implicitly.

**ASSUMED storage contract:** local storage preserves previously synchronized
bytes when an append is interrupted, supports exclusive locking and atomic
same-directory replacement, and honors the platform durability operations
specified below. A crash may tear, lose or reorder unsynchronized tail writes.
Checksums detect damage; they cannot repair a damaged base, malicious edits,
bit rot in closed history, or hardware that violates synchronization guarantees.
Those cases require a separately retained base/journal backup. This is not a
claim of measured power-loss resilience.

## Options compared

Let `B` be image bytes, `W` sector replacements, `C` completed commands, and `G`
closed generations since the last retention rewrite. Costs below are analytical,
not filesystem benchmarks. Copies mean ordinary full copies, without assuming
filesystem reflinks.

| Design | Crash safety, including torn writes | Disk use | Simplicity and testability | Rollback cost |
| --- | --- | --- | --- | --- |
| Append-only sector journal | Check each frame; replay only complete command batches. Preserve synchronized history and discard an incomplete tail. Generation markers identify recovery points. No live base overwrite. | This format: `128 + 640 × (W + C + G)` bytes. Repeated writes accumulate until retention/compaction. | One forward parser and a bounded command buffer; exhaustive byte-cut and injected-I/O tests are practical. No persistent block index. | Scan to find the generation, then truncate; remount replays the retained prefix. |
| Full-image copy-on-write generations | Write a new full image, synchronize it, publish it atomically and synchronize its directory. A torn unpublished copy is ignored. In-place copying over the current version is unsafe. | About `B × G`, plus a working image and temporary copy. Persisting each command needs a full copy per command; copying only at session end loses that session on a crash. | Simplest recovery representation; whole-image comparisons and publication-failure tests are straightforward. A defensible fallback for tiny disks. | Select a prior full image, effectively constant metadata work plus loading `B` bytes. |
| Block-mapped overlay, qcow-like | Data, allocation maps and generation roots must be published in a consistent order. Torn mapping/root changes need redundant roots or their own journal. | Changed blocks plus maps, allocation metadata and retained old blocks; repeated writes within a generation can reuse space. | More invariants: allocation, map updates, reference ownership and reclamation. Much larger fault-injection surface for these floppy sizes. | Select an old root; reclaiming unreachable blocks is separate work. |

Choose the append-only journal: incremental command persistence is the requested
behavior, and a floppy does not need an on-disk block lookup tree. Reconstruct
the entire working image in memory at mount. For example, a one-sector command
adds two 640-byte frames (1,280 bytes); a full 1.44 MB image copy is 1,474,560
bytes. Heavy repeated overwrites can make the journal larger than full copies;
retention and export are explicit parts of the design.

The qcow comparison is an inference from QEMU's documented allocation tables,
refcounts and snapshots, not a proposal to implement a subset of its format.
See [QEMU, Qcow2 Image File Format](https://www.qemu.org/docs/master/interop/qcow2.html).
The publication and storage assumptions follow the issues described in
[SQLite, Atomic Commit, §§2, 3.7, 3.11 and 9](https://www.sqlite.org/atomiccommit.html).

## Chosen format (byte layout)

Version 1 is a header followed by fixed-size frames. All integers are unsigned
little-endian. Offsets in the tables are inclusive. There is no native Rust
struct serialization, alignment padding, timestamp, pathname, host identifier
or random identifier. SHA-256 fields contain the raw 32-byte digest.

### Header: exactly 128 bytes

| Bytes | Size | Field and required value |
| --- | ---: | --- |
| 0–7 | 8 | Magic: ASCII `UPCFJNL` followed by `00` hex |
| 8–9 | 2 | Version: `1` |
| 10–11 | 2 | Header length: `128` |
| 12–15 | 4 | Sector length: `512` |
| 16–23 | 8 | Exact base-image length in bytes; positive and divisible by 512 |
| 24–55 | 32 | SHA-256 of every original base-image byte, in file order |
| 56–63 | 8 | Anchor generation: `0` initially; positive after retention rewrite |
| 64–67 | 4 | Flags: `0` |
| 68–95 | 28 | Reserved: all zero |
| 96–127 | 32 | SHA-256 of header bytes 0–95 |

Before applying any frame, check the header, image length, configured geometry
length and base digest against the exact bytes loaded for this assembly. A
different base, even one byte different at the same size, is a hard mount error.
Never create a fresh journal over an existing invalid file or silently mount it
as volatile. An incomplete header is not a recoverable tail.

### Every frame: exactly 640 bytes

Frame `i`, starting at zero, begins at file offset `128 + 640 × i`. A fixed
stride permits inspection beyond a damaged frame without searching inside
sector data for a magic string. The extra padding trades space for a simpler
parser; this is not a claim that a 640-byte host write is atomic.

| Bytes within frame | Size | Field and required value |
| --- | ---: | --- |
| 0–7 | 8 | Magic: ASCII `UPCFREC` followed by `00` hex |
| 8 | 1 | Kind: `1` SECTOR, `2` COMMAND_END, `3` GENERATION_END |
| 9 | 1 | Flags: `0` |
| 10–11 | 2 | Frame header length: `64` |
| 12–15 | 4 | Payload length: `512` for SECTOR, otherwise `0` |
| 16–23 | 8 | Sequence: starts at `1`, increases by exactly one per frame |
| 24–31 | 8 | Generation id; positive, governed by the grammar below |
| 32–39 | 8 | Target: LBA, sector count, or closing reason, according to kind |
| 40–63 | 24 | Reserved: all zero |
| 64–575 | 512 | Complete replacement sector; all zero for marker frames |
| 576–607 | 32 | Reserved: all zero |
| 608–639 | 32 | SHA-256 of frame bytes 0–607 |

SECTOR's target is a zero-based LBA, strictly below `base_length / 512`.
Its bytes replace that whole sector. COMMAND_END's target is the exact number
of immediately preceding SECTOR frames in that command batch. GENERATION_END's
target is a closing reason: `1` session end, `2` explicit checkpoint, `3`
recovered interrupted session, `4` retention anchor. All other encodings,
nonzero reserved bytes and integer overflow are errors. No wraparound is allowed.

### Grammar and generation identity

An ordinary command is one or more SECTOR frames followed by COMMAND_END. All
frames have the same generation id. LBAs within a command are unique and occur
in the controller's transfer order; across commands an LBA may repeat, with the
last replacement winning. The parser buffers at most one image's sector count,
checks each LBA before indexing, and applies replacements only after validating
COMMAND_END. A zero-byte DMA completion emits no command batch. A write of
unchanged bytes still emits the touched sectors.

A generation contains zero or more commands followed by GENERATION_END. The
marker is legal only between commands. After it, the next generation id is
exactly one greater. Empty sessions/checkpoints may close empty generations.
No BEGIN frame is needed: the header and preceding generation end determine
the current generation. A complete command batch without GENERATION_END is
an interrupted generation, not an invalid journal.

For a new journal the immutable base is logical generation `0`, the first run
uses `1`, and sequences start at `1`. A checkpoint or session end closes the
current generation. Each independently mounted media session has its own ids;
a call to `run_bounded`, a cap, a device reset, or a prompt poll does not close
one. Multiple drives have independent journals; there is no atomic multi-drive
checkpoint promise.

After rollback, discarded generation ids and sequence numbers may be reused.
They are addresses within the current journal lineage, not permanent global
identifiers. Tools print the base digest and warn that later generations will
be removed. Retention preserves retained generation ids but renumbers frames.

### Retention anchor

If header `anchor_generation = A > 0`, the file must begin with exactly one
synthetic command describing generation A: list, in increasing LBA order, every
sector whose bytes at A differ from the original base; then COMMAND_END and
GENERATION_END with reason `4`. This one synthetic command may have zero
SECTOR frames. No other zero-sector command or reason-4 marker is legal.
Subsequent generations are A+1, A+2, and so on, using the ordinary grammar.
The anchor must be complete before any part of this file can be mounted.

This retains the full state at the oldest retained recovery point without
changing the base or its hash. An incomplete anchor is a hard error, not an
invitation to mount the original base and lose retained changes. Generation 0
is available to rollback only while the header anchor is zero; after pruning,
the original base remains independently usable with a new journal path.

### Open and tail recovery

1. Acquire the journal's lock, validate the header and base binding, and scan
   forward at fixed frame offsets. Check hashes, encodings, sequences, generation
   grammar, counts and bounds. Remember the offsets after every validated
   COMMAND_END and GENERATION_END. Do not expose pending SECTOR bytes.
2. At EOF, a partial frame or complete SECTOR frames without COMMAND_END are
   an uncommitted tail. A checksum/grammar failure can also be a torn tail.
   At the first failure, inspect later complete frames at the known stride:
   if any independently checksum-valid GENERATION_END frame is present, refuse
   ordinary mount as interior corruption. Do not skip the bad frame and replay
   later data. This check is conservative, including markers with inconsistent ids.
3. With no later intact generation marker and a complete anchor, retain only
   the validated prefix through the last command/generation end (or the initial
   header). Discard the entire pending command and every subsequent byte.
   Report the discarded byte count and last closed generation. Automatic
   tail classification assumes the crash fault model: a damaged final marker
   and a torn append cannot always be distinguished. Verification reports this
   uncertainty; it never labels recovered data as known good.
4. A writable mount truncates to that safe offset and durably synchronizes
   before appending. If complete commands remain after the last generation end,
   replay them and append/synchronize a reason-3 end marker. They become a
   visible recovered generation. Start the new session in the next generation.
   If no complete command remains in the interrupted generation, reuse its id.
5. A read-only verification scan reports the same proposed actions without
   truncating or sealing. A writable recovery failure aborts mount before any
   guest execution. Closed history with detected interior corruption requires
   explicit salvage or restoration, never automatic truncation.

A crash after a fully valid but not yet acknowledged COMMAND_END may preserve
that entire command. Recovery promises a valid prefix, not knowledge of which
sync calls returned before the crash. Checksums provide accidental-damage
detection, not authentication. A checksum-valid marker after corrupted data
does not make that generation usable.

## Integration (layers, flush points, determinism argument)

### Media behind the drive

Introduce a small hardware-neutral sector-media contract in `device`, an
in-memory implementation, and a host journal adapter owned by `assembly`.
The uPD765's drive owns media through that contract. Its operations provide
sector reads and a batch of complete replacement sectors for one completed
WRITE DATA. The in-memory implementation retains today's zero-read and growth
semantics for short isolation-test media. Normal assembly still validates a
full geometry-sized floppy image.

The journal adapter contains an ordinary in-memory image plus an output sink.
It first applies the batch to memory exactly as today, then mirrors the final
sector bytes and COMMAND_END. The device sees no file, pathname, flush policy,
generation, lock, or host error. Write protection remains a controller-visible
medium property and rejects writes before calling this contract.

`Upd765::complete_write` supplies complete 512-byte replacements, including
the zero-filled partial sector. It submits all sectors actually committed,
including those preceding an overrun/no-data/end-of-cylinder result. Persisting
only ST1-successful commands or the unpadded DMA buffer would change behavior.
Reset keeps the mounted media and does not create a generation; device snapshots
continue to exclude media. This does not implement machine snapshot/replay.

The assembly media loader must retain the original `ImageSource::File` metadata
while resolving bytes: today's conversion to `Inline` otherwise loses the path
needed for validation. Hash the single read already used for mounting, replay
the journal into those bytes, then construct the drive. Do not reread the base
between hashing and mounting. All host paths, journal ownership and finalization
belong to assembly/frontend code, never the generic machine core.

### Persist at command completion

Choose synchronous mirroring at each WRITE DATA completion, rather than at a
run boundary. A long session interrupted before the next frontend boundary
must not lose every earlier command. One call through the media contract
groups all sectors of a command; this also bounds replay buffering. No new
device deadlines, wall-clock timers, worker thread or machine scheduling hook
is needed.

For every nonempty completed command, in order:

1. Apply all replacement bytes to the in-memory image.
2. Encode and `write_all` the SECTOR frames, followed by COMMAND_END, retrying
   interrupted writes and handling short writes without duplicating bytes.
3. Flush any userspace buffer, then perform a durability synchronization of
   the journal before returning from the adapter. One barrier covers the batch
   and marker. Reordered incomplete batches fail validation on recovery.

Version 1 has a single, fixed durability policy: **every completed write
command**, plus each generation end and each recovery truncation. `flush()`
alone is insufficient. On Unix use a file-data-and-metadata sync (`fsync`, via
`File::sync_all`). On Apple targets, std's `File::sync_all` already issues
`fcntl(F_FULLFSYNC)` and retries interrupted calls; rely on that implementation
rather than issuing a second device-cache flush. Any remaining direct platform
syscalls must retry `Interrupted`. An unsupported durability primitive is a host
error, not a silent downgrade. Apple documents the distinction in
[fsync(2), Description](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/fsync.2.html).

Before the first guest instruction, creation writes the header into an unused
same-directory temporary file and synchronizes it. Publish with a hard link to
the final path (which refuses an existing destination), unlink the temporary,
then synchronize the directory. A failure before publication leaves no journal
at the final path; an unpublished temporary is never mounted automatically.
A failure after publication can leave a complete header whose directory entry
is synchronized again on reopen. Under the stable lock, mounts remove the
journal's `<journal>.create-*.tmp` leftovers before synchronizing the directory.
This handles both unpublished orphans and the extra hard link left by a crash
between publication and unlink; journal contents remain authoritative.
Each replacement file is written to an unused temporary name in the same
directory, fully verified and synchronized before atomic publication;
synchronize the containing directory after publication.
Use the corresponding tested namespace/durability operations per supported
platform. Unsupported filesystems/platforms are refused for journal mode.

An explicit `checkpoint()` is accepted only at an existing completed machine
service boundary: drain any completed command, append GENERATION_END, flush
and synchronize, then increment the generation id. The caller may continue.
`finish()` does the same when the embedding frontend explicitly ends the media
session, including a policy-limited but otherwise successful session. It is
fallible and mandatory before reporting persistence success; `Drop` only
releases handles. On execution errors do not label the session clean: leave
its completed commands for recovery. A clean label does not certify filesystem
consistency, even when the guest is at a prompt.

### Implemented host sink boundary

`media::JournalMedia` owns the reconstructed image and host journal. Assembly
wraps its `(LBA, complete sector)` command batches in `device::SectorMedia`;
this keeps host I/O out of device crates without adding a dependency between the
format crate and device contracts. `JournalMedia::open` takes the exact loaded
base bytes, geometry length and a frontend-owned `SinkStatus`. Each media session
uses its own status handle. Duplicate LBAs are collapsed to their final bytes,
ordered by their last occurrence, before encoding unique SECTOR frames; memory
applies the original batch in order, matching volatile last-wins behavior.
`checkpoint` closes a generation and continues;
`finish` consumes the session. Dropping without finish releases the journal and
stable sidecar lock without writing a marker. The sink prunes only after a
successfully synchronized checkpoint or session-end marker.

Both scanner entry points share validation and classification. `scan_reader`
uses `Read + Seek`, retaining the base, reconstructed image, one bounded command,
one frame and generation summaries; it does not retain journal payload history.
Memory is O(image bytes + number of closed generations), not O(journal bytes).
Offsets must fit the host's `usize`; larger files are refused before replay.
Read/seek errors abort mount rather than authorizing tail truncation.

The supported implementation is Unix: `sync_all` for file and directory, with
std supplying `F_FULLFSYNC` on Apple targets for both. A rejected barrier aborts
creation/mount or poisons an active sink, with no fallback. New headers are
synchronized before hard-link publication; existing destinations are preserved.
After locking and opening (or publishing a new header), mount re-resolves the
canonical journal path and refuses a mismatch before cleanup, scan or recovery.
Cleanup considers only `<journal-file-name>.create-<decimal digits>.tmp`, with
at least one ASCII digit. Journal filenames containing `.create-`, `.retain-` or `.export-` anywhere are
refused before locking in both assembly and the sink, including through aliases;
nested journal names cannot own overlapping temporary namespaces.

A candidate must be a regular file whose bytes are empty, a prefix of the exact
base-bound creation header, or that header alone. On Unix it is opened with
`O_NOFOLLOW | O_NONBLOCK`: a symlink swap cannot be followed and a FIFO swap
cannot block. Metadata from the open handle must still describe a regular file
with the same device/inode as the initial `symlink_metadata`. Cleanup reads that
handle, then rechecks the entry's device/inode immediately before unlinking.
There remains a small path-replacement window between the final check and the
unlink, so the journal directory must be trusted. Off Unix cleanup retains the
initial type/content checks without the Unix open flags or identity checks;
durable mounts remain unsupported there.

Creation writes no other bytes to its temporary, so commands, foreign data,
symlinks and directories remain untouched. Unknown entries are ignored: mount
never replays them, and `create_new` skips occupied names. An entry disappearing
during metadata lookup, open or unlink is already gone and does not fail mount.
The directory is synchronized even when no tail needs repair. Other cleanup or
directory-sync errors abort mount.
Existing validated history is synchronized on mount before confirming its last
command sequence. The status records the last confirmed COMMAND_END sequence;
generation markers do not advance that command receipt. A writable recovery
failure aborts before a media session can be returned.

**MEASURED on macOS:** Rust tests exercise real-file creation, reopen/replay,
lock exclusion through normal paths and cross-directory symlinks, torn-frame
repair/sealing, checkpoints, finish and Drop. Real header I/O tests fail after
each of the 128 incomplete write prefixes and on header sync, then verify no
journal was published and a fresh mount can succeed. Another real-file test
checks that hard-link publication refuses an existing destination. Each real
file test uses a unique subdirectory of `std::env::temp_dir()` and removes it.
A dangling-symlink test checks refusal before lock creation. A filesystem hook
stages alias creation and a competing target mount between initial resolution
and open, without threads or timing; post-open verification refuses the stale
alias path without changing the active target's bytes or sink status. Real-file
tests reconstruct both temporary-file crash shapes, check orphan removal and
hard-link count reduction, preserve unrelated names, and replay the journal.

The injected model checks header publication ordering, torn-tail repair/seal
ordering, and exact successful traces for clean-history and command-complete
unsealed reopens. For each of those four fixtures it injects one error at every
operation in the successful trace, checking abort, lock release and the
first-error status; creation also checks publication state and allows
best-effort temporary cleanup. This
covers the model boundary (lock/open, post-open path verification, temporary
creation/publication/unlink and leftover cleanup, read/seek, write/flush/sync,
directory sync and recovery truncation), rather than every syscall, filesystem
failure combination or per-entry cleanup failure. A separate model check
requires leftover cleanup under the lock followed by directory sync, and aborts
on either boundary's injected failure. Both reopen fixtures explicitly require
directory sync and the existing-history barrier,
with recovery sealing only after that barrier for an unsealed tip.

Command-output tests cover all 1,920 incomplete prefixes of a two-sector batch,
short/interrupted/zero writes and flush/sync errors. Duplicate-sector tests
check last-wins memory and replay, unique frame counts and last-occurrence order,
including memory writes after poisoning. Streaming tests compare both scanners
at every truncation of a generated journal and reject oversized read requests
and injected read errors. Targeted mutations check the changed behaviors; this
is not a claim that every assertion or every platform syscall is mutation-tested.
These process/software fault tests establish neither a physical power-cut result
nor, by themselves, status/IRQ equivalence through an assembled journal drive.
Assembly integration is measured separately below.

**ASSUMED:** the filesystem/device honors successful synchronization and retains
previously synchronized bytes under the tail-only crash model stated above.

### Host errors and determinism

The media write operation has no host-I/O error result visible to the controller.
Its adapter latches the first sink failure in an assembly-owned status handle,
preserves all in-memory writes, and disables further appends in that session.
The frontend checks that handle at every existing host cooperation return and
before final success. It reports a host persistence error, stops further host
advancement, and does not seal or prune the poisoned generation. The current
chunk may finish before this check; a strict stop before the next instruction
would require a separate execution-loop change and is not promised here.
Report the last confirmed durable command sequence; the failed command's
durability is uncertain. Never translate ENOSPC/EIO into ST1, write protection,
an IRQ, a retry of the guest command, or replacement read data.

For fixed mounted bytes and fixed guest input, memory mutations, status bytes,
DMA results, IRQ edges and virtual-time advancement are identical across
volatile and journal adapters. Sink latency contributes no virtual time;
hashing, sequencing and generation ids use only deterministic bytes/counters.
There are no reads of the journal or base during guest execution; explicit
closed-boundary retention reads the journal and uses the original base bytes
retained in memory. Opening/replaying
media is an explicit initial-state input, just like today's base-image load.
Host failure can stop a run at a frontend boundary; its completed guest prefix
must equal the corresponding volatile prefix. This is the precise limit of
the determinism claim, not a promise that failing and healthy hosts finish
the same amount of execution. Interactive-input timing remains a frontend
concern; persistence adds no new timing input.

## Tooling and config

### Config contract

Keep the agreed per-drive `write` modes: `volatile` (default), `readonly`,
`journal`. The `Eq`/`Debug` Rust enum, `WriteMode`, implements `Volatile`, `ReadOnly` and
`Journal { path, keep_generations }`, with the existing not-writable status
preserved. The journal variant carries its own path and retention count rather
than independent, potentially conflicting options.
The following is field notation only; it does not introduce TOML parsing:

```toml
write = "journal"
journal = "media/work.img.upfj"
keep_generations = 10
```

- A journal path is explicit and required; suggest an adjacent `<base>.upfj`
  path in help, but never create one merely because an image is file-backed.
  Relative paths follow the existing explicit-image convention: process cwd;
  a machine description file resolves its own relative paths against the
  file before it builds the `MachineConfig`.
- `keep_generations` defaults to 10 and must be positive. It counts closed
  positive generations, including recovered and empty ones; the active
  generation does not consume a slot. No age-based pruning or wall clock.
- Journal mode initially requires `ImageSource::File`, a floppy, and 512-byte
  sectors. Reject blank/inline sources and the unserved HardDisk seam for this
  mode. The same sources remain valid for volatile/readonly.
- Reject journal-only fields with other modes and any journal/lock/output path
  aliasing a base, mounted image or another journal. Check existing file identity
  as well as normalized paths, including hard links. Open the base read-only.
- Volatile/readonly must not create a journal, lock, backup or temporary output.
  Presets, tests and goldens select volatile unless explicitly testing readonly.

### Assembly session API

`WriteMode::journal(path)` constructs `Journal { path, keep_generations: 10 }`;
the explicit variant can select a different positive count. Journal-only fields
cannot be represented on `Volatile` or `ReadOnly`. Assembly rejects non-file
sources, non-floppies and non-512-byte geometry for journal mode. Before any
journal or lock is opened, it compares canonical paths and existing file
identity (including hard links) against every drive/load image and other journal
or lock output. It also reserves each journal's same-directory
`<journal-file-name>.create-<decimal digits>.tmp` namespace against mounted
inputs, checking the canonical parent plus unresolved filename as well as the
fully canonical path. Other journal filenames containing `.create-` are refused
independently. These checks reserve names more broadly than cleanup, which also
checks file type and contents. Dangling journal symlinks are refused
before mounting any drive. Journal normalization errors identify the drive and
path; every failed input normalization is ignored for alias comparison so its
own image read reports the real error, without attribution to another journal. On case-insensitive filesystems,
not-yet-existing journal names differing only in case can pass validation; the
second mount fails safely at lock acquisition and names its drive, without case
folding in validation. Observation policy errors (missing roles, unsupported
outputs and duplicate routes) are checked before device construction and journal
mounts, so these failures neither create outputs nor recover an existing tip.
Mounting multiple drives is not transactional: if a later mount fails (including
a case-only lock collision), earlier journals and lock sidecars may already
exist, and earlier recovery may already have repaired or sealed a tip. Failure
drops their handles without a session-end seal; durable command history remains
recoverable, and stable lock sidecars are retained.
Base files are read once, read-only; replay uses those exact
loaded bytes. Volatile and readonly sessions create no host output.

`AssembledMachine::persistence` contains one `PersistenceHandle` per journal
with its drive index, configured retention count, mount recovery report, first
latched host error, `checkpoint()` and `finish()`. The assembly-owned sector
adapter shares the session with this handle. `run_session(policy)` checks errors
before and after every existing natural/prompt/stimulus run boundary;
`check_persistence()` supports an embedding frontend's own boundaries. A failed
run must be dropped without `finish_persistence()`. A latched error during a run
skips every drive's finish; the other drives' unsealed sessions are recovered
on their next mount. A successful cap, halt or
explicit normal window close calls `finish_persistence()`; repeated finish calls
are harmless, and the session run API refuses further advancement after finish.
Drop never writes a clean marker. Final display/disk capture can still read the
finished media bytes. There is no atomic close across multiple drives.

There is no new frontend flag or serialized journal configuration in this step.
The existing presets and goldens remain volatile; journal selection is explicit
Rust data. Assembly passes `keep_generations` into the sink. The anchor counts
as one retained closed generation: keep=2 over ids 1–5 retains anchor 4 and id 5.

### Offline commands

The existing binary provides the following `media` commands, preserving `--boot`.
Parsing is dependency-free; the command layer and journal operations are library
APIs tested without spawning the binary:

```text
unemupc98 media generations --base disk.img --journal disk.img.upfj
unemupc98 media verify --base disk.img --journal disk.img.upfj
unemupc98 media verify --base disk.img --journal disk.img.upfj --repair-tail --backup before-repair.upfj
unemupc98 media rollback --base disk.img --journal disk.img.upfj --to 7 --backup before-rollback.upfj
```

`generations` lists ids, closing reasons, end offsets, command/sector counts,
the retained anchor and any unsealed tip. Include the base digest. Do not call
an interrupted generation clean or claim that clean means bootable.

`verify` checks binding and every frame/grammar invariant, replaying without
mutating media by default. It reports closed history, recoverable-tail bytes,
interior damage and the latest reconstructible state. Exit codes: 0 fully
valid/closed, 1 corruption/binding or I/O failure, 2 usage/config error, 3
recoverable tail or unsealed generation. `--repair-tail` explicitly applies
the normal open procedure, after a synchronized full backup to an unused path.
It still refuses interior corruption and incomplete anchors; arbitrary salvage
is a later, separately reviewed tool.

`rollback --to N` requires a retained closed generation (or available generation
0), backs up the entire journal to an unused path, synchronizes the backup and
its directory, then truncates just after N's GENERATION_END (128 bytes for 0)
and synchronizes. It removes later generations and any open tip. No guest runs
concurrently. A successful return means the truncated prefix is durable;
interruption may leave the old or truncated file, with the backup available.
An unavailable, pruned, nonexistent or unsealed target is a configuration error
(exit 2); a missing journal is an I/O failure (exit 1), and is never created by
a tool. Existing backup destinations, including symlink/hard-link aliases of
the base or journal, are refused (exit 2). Backup filenames containing the
reserved `.create-`, `.retain-` or `.export-` markers are also refused so mount cleanup cannot delete a
header-only backup. Backups are never deleted by these tools. On Unix they are
created with the journal's permission bits, then those bits are set exactly
before copying, so the umask cannot expose data or silently narrow permissions.
A failed copy or backup barrier leaves a retained file and reports it as a
partial, invalid backup with its path. Every I/O diagnostic names the role and
path of the affected base, journal, lock or backup. Tools preflight both inputs before
creating lock metadata, and open them without blocking on Unix; non-regular
inputs such as FIFOs are refused. Missing inputs leave no new files. Dangling
journal symlinks and canonical-path changes
after locking are I/O failures (exit 1). Tail diagnostics give the safe truncation
point and discarded bytes, separately from the failure/EOF offset. Rollback
validates its target before printing the removal warning.

Immutable export:

```text
unemupc98 media commit --base disk.img --journal disk.img.upfj --to 7 --output restored.img
```

`commit` (alias `compact`) reconstructs a selected closed generation into a
**new raw image** at an unused output path. Write a same-directory temporary,
check its size and digest against reconstruction, synchronize it, publish
without overwriting an existing destination, then synchronize its directory.
The old base and journal remain untouched; never update them as a two-file
transaction. To continue from this new base, point config at it and an unused
journal path; its new lineage starts at generation 0. No automatic config edit,
in-place commit, or export of an unsealed tip. A user repairs/seals the tip first.
Export requires the shared stable lock (it refuses inspection's lockless fallback).
A recoverable tail or unsealed tip is exit 2; format/binding or I/O failures are
exit 1, and an unavailable generation or occupied output is exit 2.
On supported Unix hosts, publication uses `hard_link(temporary, output)`, which
refuses an existing destination, including symlinks/hard links to inputs. This
also protects against a destination appearing after the unused-path preflight.
The temporary is `<output>.export-<decimal>.tmp`, created exclusively with mode
0600. Read back and compare its length and SHA-256 before `sync_all`, hard-link
publication and directory `sync_all`. Cleanup unlinks only this export's temporary;
an interrupted orphan is never mounted or automatically imported. Errors before the hard-link attempt say "nothing was published"; errors at or
after that attempt report uncertain publication. EPERM, ENOTSUP and EXDEV from the
hard-link operation instead name the output-filesystem hard-link requirement and
say "nothing was published". An error from the directory barrier remains uncertain
even with those codes. Existing-destination errors keep their configuration exit.
A publication or directory-sync failure may leave a complete output; inputs remain authoritative.
Other platforms are refused by the durability barrier before publication.

### Locking and retention

Resolve an existing journal's canonical path before deriving the stable
`<journal>.lock` sidecar, so symlink aliases share the same nonblocking OS
advisory lock. Refuse a dangling journal symlink before creating a lock. For a
missing journal, canonicalize its parent only to establish the expected name
for the post-open comparison; locking itself already follows parent aliases.
After acquiring the lock and opening the journal, re-resolve its canonical path
and refuse any mismatch with the name from which the lock was derived. This
also catches a symlink introduced after initial resolution. Under that lock,
remove only decimal-named creation leftovers passing the type, identity and
header-prefix checks described above, then synchronize the directory before
scan/recovery. Never mount a leftover temporary as a journal.
Mounts and mutating tools take it exclusively; inspections take it shared.
Inspections open an existing lock read-only. If it is absent and creation fails
with EACCES, EPERM or EROFS, they may inspect without a lock and print one line
saying no lock was taken. A writing session must have created a lock in that
directory; an existing entry is never bypassed, even when unreadable. Mutating
tools still require an exclusive lock with write access.
Creation of lock metadata is permitted for inspections, but image/journal
contents remain read-only. The lock file is never replaced/unlinked by normal
tools; locks release on process exit, without stale-PID or time heuristics.
Never delete `<journal>.lock`: it is not a stale-PID file; deletion during a
session defeats the single-writer guarantee for sessions and tools alike.
Two drives/sessions cannot share a writable journal. The same lock protects
rename-based pruning; locking only the journal inode would not. External tools
that ignore this protocol are outside the contract.

After a successful checkpoint/session close exceeds `keep_generations`, rewrite
one journal atomically. Select the earliest of the newest N closed generations,
reconstruct it, and encode it as the anchor described above. Copy/re-encode
later retained generations, preserving ids/reasons while renumbering sequences.
Verify the complete replacement against the old states before synchronization
and publication. The original immutable base remains the hash binding.

Pruning runs only at a closed boundary, never on a poisoned/open generation.
A failed pre-publication rewrite leaves the original journal intact; report
maintenance failure separately from already durable guest commands. After a
publication/sync error, report uncertain maintenance completion and verify on
next open. The visible journal must be either the old valid version or the new
valid version under the storage contract. Temporary files are never mounted
automatically. No automatic deletion of offline backups.

The implementation uses `create_new` for `<journal>.retain-<decimal>.tmp`,
requests the journal's mode bits at creation, then applies those bits exactly with
`fchmod` before writing, to restore permissions filtered by the umask. Ownership
is not preserved: the replacement is owned by its creator; preserving another
owner would require privileges that this tool does not request. It streams later
frames from the old descriptor, re-encodes their sequence numbers, and compares
full images from each retained old/new closed prefix. A separate whole-replacement
scan checks status, generation count, ids and later closing reasons before sync.
The oldest point becomes reason 4;
subsequent reasons, including recovered reason 3, are preserved. Unix `rename`
replaces only the journal path under the unchanged sidecar lock, followed by
directory `sync_all`. The live sink then appends through the replacement descriptor.
The base and offline backups are never rewritten or removed. Creation cleanup
considers only `.create-` decimal names and requires the creation-header prefix.
Under the exclusive mount lock, cleanup also removes this journal's exact
`.retain-<decimal>.tmp` names, accepting any content/length because a crash can
leave any prefix of a full replacement. Both paths require a regular entry,
`O_NOFOLLOW | O_NONBLOCK` open, matching device/inode after open and just before
unlink; a disappearing entry is already gone. Symlinks, directories, nondecimal
names and other journals' leftovers survive. The subsequent directory barrier
covers removal. No leftover is mounted. Retention/export operations unlink only
their own temporary and never sweep another operation's active files.
Validation rejects input and journal filenames containing `.retain-`, including
through resolved aliases. This applies to volatile/read-only inputs too; errors
name the owning drive or the loadable image's role and path. Offline tools also
refuse reserved `.create-` and `.retain-` names, as given or resolved, for every
input/output including the base, journal, lock, backup and export destination
(exit 2), before any destructive operation. Independently, cleanup skips any
opened entry with the base file's device/inode. The identity comes from the same
file read that supplied the mounted bytes; offline repair carries it into sink
recovery. Thus even a reserved-name or hard-link alias cannot make cleanup own
the mounted base.

`SinkStatus::maintenance_failure()` and the corresponding assembly handle method
report maintenance separately from `failure()`/`status()`'s command-error latch.
Checkpoint/finish return success once the generation marker is durable, even when
retention fails. Assembly attempts every drive's finish, then returns the first
real command/close error. CLI and window frontends print one warning per drive
with a maintenance outcome; maintenance alone does not make the run fail.
A pre-publication failure leaves the old journal intact and permits retry at a
later close. A publication attempt or directory-barrier failure is uncertain:
verify on the next open before further persistent work. The old session preserves in-memory guest behavior but appends no more host
records. At a checkpoint this disabled output latches a host persistence failure,
so `check()` and the next frontend boundary stop further advancement; it cannot
durably close another generation without reopening. A session already ending
still reports its maintenance outcome without failing the durable close.
No controller-visible error/protection/IRQ is synthesized; Drop adds no marker.

Retention cost scales with journal size because retained states are reconstructed
by scanning history; this is acceptable for floppy images.
The anchor costs at most one frame per base sector plus two markers. Retained
write history and the current session can still grow without bound if a guest
rewrites sectors indefinitely before closing a generation. A generation count
is a history bound, not a byte quota. Explicit checkpoints or export bound
ordinary use; do not invent a hidden time/size checkpoint policy in version 1.

## Failure modes and recovery

| Failure | Required outcome |
| --- | --- |
| Different/modified base, short header, unknown header version | Refuse before replay and guest execution; preserve files. A path match is not a binding. |
| Process kill/power loss midway through SECTOR or COMMAND_END | Recover only through the last complete validated command; truncate pending/torn suffix under lock before further append. |
| Valid COMMAND_END but no generation end | Replay that command, seal the interrupted generation with reason 3, and retain the prior closed generation for rollback. |
| Torn GENERATION_END | Preserve completed commands; recover/seal the generation. Do not infer filesystem health from the marker. |
| Damaged history with a later intact generation marker | Refuse ordinary mount/repair; use the original base plus a valid backup or explicitly designed salvage. No magic-string resynchronization. |
| Corrupt final suffix without any later intact generation marker | Treat as tail under the documented fault assumption, report discarded bytes and uncertainty. Offline repair preserves a full backup first. |
| Guest logically damages a disk during an otherwise clean session | Checksums can remain valid. List generations, select a working one, back up and roll back; this loses later guest writes deliberately. |
| ENOSPC, short writes, flush/sync failure | Latch a host error; guest memory/status remain unchanged by it, appends stop, no clean end/prune, frontend reports failure. Recovery may preserve more or less than the last attempted batch. |
| Failure during pruning/export | Before publication, old inputs remain authoritative. After publication, verify old/new outcome; never pair a new base with an old journal automatically. |
| Failure during rollback | A retained validated prefix and synchronized pre-rollback backup are the recovery sources. Reopen/verify before continuing. |
| Second writer or mutating tool against live media | Fail lock acquisition before modifying journal data; no concurrent rollback/commit. |
| Torn storage writes that damage previously synchronized bytes, lost files, broken flush/rename guarantees | Outside the tail-only guarantee. Detect where possible; restore separately stored backups. Software fault injection is not a power-cut test. |

Always keep the base when copying/backing up a journal: a journal alone may
depend on untouched base sectors. A checkpoint cannot make a multi-command FAT
update atomic or capture CPU/RAM state. Rollback is a disk operation performed
before assembling a new machine, not a rewind of a live guest.

## Tests

### Spike evidence

**MEASURED:** a disposable Python standard-library probe outside the repository
encoded the specified 128-byte header and 640-byte frames and replayed a
two-generation, two-command journal. Seven tests passed, zero failed:

- Every one of 3,969 truncation boundaries of a 3,968-byte journal: incomplete
  headers refuse; complete headers recover precisely the command-complete prefix.
- Single-byte corruption at all 3,200 positions in the first five frames of
  sealed history: refuse rather than skipping to a later intact generation end.
- Exact layout/size, bad final-frame checksum, different-base rejection,
  rollback followed by append, and anchor reconstruction. The anchor test also
  rejects all 1,920 incomplete anchor-prefix lengths with a complete header.

**MEASURED mutation checks:** removing base-hash comparison makes the base-binding
test fail (one failure). Applying SECTOR bytes before COMMAND_END makes the
exhaustive cut test fail at 1,280 subcases. Both mutations were detected.
These are parser-model checks, not Rust integration tests, lock/fsync tests,
physical power cuts or performance measurements. No repository test was added
or changed by this documentation-only spike.

### Implementation acceptance tests

1. **Format units:** fixed known byte vectors for header and all frame kinds;
   endian/checksum coverage, reserved/unknown values, bad sizes/hash/base,
   sequence gaps/duplicates, LBA overflow/out of bounds, marker counts,
   zero-sector special case, malformed generation transitions and integer
   exhaustion. Generate every truncation of headers, multi-sector commands,
   markers and anchors. Ignore incomplete batches; never half-replay a command.
   Corrupt sealed history and prove refusal rather than loss of later history.
2. **Writer and filesystem fault injection:** an injectable output/filesystem
   boundary records write/flush/sync/publish/directory-sync/lock operations.
   Fail every operation, every partial write position, interrupted write and
   recovery truncation; model lost/reordered unsynchronized writes. Check the
   parser's valid-prefix result, complete-anchor requirement, error latch,
   last-confirmed sequence and refusal to append after failure. This validates
   ordering under a model; platform synchronization still needs host validation.
3. **Device isolation on `testkit::MockBus`:** a recording in-memory media
   adapter observes actual committed sectors for normal, multi-track,
   partial-terminal-count, zero-byte, overrun and invalid-address/protected
   commands. Reads, result bytes, IRQ and zero-filled tails match the current
   model. A failing output sink never changes guest-visible device state;
   rejected commands emit no sector batch. Reset retains media and output state.
4. **Two-run assembly integration:** copy a generated full floppy into a test
   temp directory. Run a small deterministic guest through actual uPD765/8237
   WRITE DATA, explicitly finish/drop the entire assembled machine, then freshly
   assemble using the original file plus journal. A second guest that only issues
   READ DATA asserts the written bytes through DMA. Assert the base hash is unchanged
   and no state is shared between runs. Repeat with a zero-filled partial sector
   and with process-style termination after a synchronized command but before
   GENERATION_END; recovery must list reason 3 and expose the committed bytes.
5. **Rollback and tools:** create A/B/C generations that overwrite the same LBA,
   roll back to A, remount and read A through the controller; append a new B and
   verify no discarded bytes return. Exercise generation 0, unavailable/pruned
   ids, open tips, required backups, lock exclusion and output-path collisions.
   Export every retained generation and compare full bytes with its replay.
6. **Retention/publication:** with keep=2 and at least four generations, compare
   every surviving generation before/after rewrite, including sectors last
   changed only in a pruned generation and an anchor equal to the base. Verify
   stable ids, renumbered sequences, original-base binding and pruned-id errors.
   Inject failure at every temporary write/sync/replace/directory-sync: only
   the old or complete new journal is authoritative; base bytes never change.
7. **Volatile, readonly and deterministic regression:** compare full memory,
   sector reads, status/IRQ traces, CPU state and virtual time for identical
   inputs under volatile and journal adapters, including delayed/failing sinks
   up to the host error boundary. Volatile starts from the unchanged file on
   run two and creates no output; readonly rejects writes with the existing
   ST1 not-writable result, also without output. Run existing fast floppy-write
   and boot goldens unchanged. At implementation merge, run the fixture-backed
   slow suite with volatile explicitly selected and compare fixture hashes and
   directory entries before/after; no sidecars may appear beside fixtures.

For the implementation, record red/green evidence or a targeted mutation for
each new behavioral test. The two-run persistence check is expected to fail
with today's load-once/no-output behavior (**READ**, not run against the current
Rust baseline in this spike). Suppressing command mirroring/replay must fail
the cross-run test; bypassing the sink latch must fail fault tests; skipping
the anchor snapshot must fail retention tests. Existing goldens are regression
evidence, not red tests for a feature that defaults off. Do not claim the
proposed tests already prove the Rust integration.

### Session wiring evidence

**MEASURED:** eighteen assembly integration tests cover two fully independent guest
WRITE/READ DMA sessions, 17-byte terminal-count padding, explicit finish and
interrupted-tip recovery, immutable bases, volatile/readonly output absence,
config defaults/refusals, canonical and hard-link aliases (including locks and
load images), reserved temporary names (including unresolved symlink inputs),
dangling journal refusal, missing-input error attribution, observation-policy
preflight without output or recovery, damaged-header refusal, checkpoint/finish
and error boundaries.
A journal/volatile comparison checks all RAM, exposed CPU registers, interrupt
readiness, device snapshots, IRQ levels, traces and virtual time at successive
run boundaries, for full and partial-sector commands.

**MEASURED:** an assembly unit test wraps a real mounted journal in the optional
`media/test-support` failing-output adapter. Actual WRITE DATA updates memory,
then its output fails. A second WRITE DATA in the same bounded run still replaces
memory; read-back DMA and guest status match the volatile run without write
protection. The frontend boundary reports the latched error, refuses further advancement and cannot seal the session. A
separate exhausted-generation test covers a latched checkpoint error. CLI and
headless presenter tests check successful cap/halt finalization, persistence-error
classification, stopping at the first failed window boundary, and preservation
of unsealed execution errors, without opening a window. A matrix compares the
observation pre-check with actual `Observation::bind` for every configured device
kind and all output kinds, including missing and duplicate routes. Real-file
sink tests retain symlinks, commands and foreign matching entries while cleaning
all creation-header prefixes, including empty and full-header files.

**MEASURED:** forty-five recorded mutation variants establish command mirroring,
output-free volatile mode, config/alias validation, guest-state preservation,
checkpoint/error latching and frontend finalization. They also reject both
post-error write regressions, omitted window checks, dangling journal symlinks,
wrong CLI persistence classifiers, unconditional temporary deletion, symlink
following, omitted content checks, disabled genuine-header cleanup, reserved
journal names, canonical-only input checks and observation pre-check drift.
Thirteen variants specifically cover decimal-only temporary names, nested
journal refusal in both layers, disappearance at each I/O boundary, both Unix
open flags, blocking FIFO swaps, opened/final entry identity and input error
attribution for not-a-directory paths and symlink loops. They produce fourteen
expected targeted test failures. Positive integration runs cover independent
sessions, immutable bases, byte replay and host failure boundaries. Deterministic
I/O hooks stage the disappearance and identity races; FIFO tests run in isolated
child processes with a timeout so a blocking regression fails safely.
These are software checks, not a physical power-loss measurement. Retention
acceptance is recorded below; offline command
acceptance is covered by the CLI media-tool integration suite and the backup
barrier fault model.

### Offline command evidence

The initial sixteen CLI integration tests cover rollback/remount/reappend,
generation 0, retained-anchor and unavailable/open targets, read-only scans,
all four exit codes, wrong bases, torn tails, interior damage, incomplete anchors,
required backups and their aliases, shared/exclusive locks (including journal
symlinks), deterministic listing output, usage errors and the existing boot demo.
The initial twenty-nine targeted mutations produced failing tests. Their backup
barrier checks omitted calls in the abstract backup model; they did not establish
that HostBackup invoked either production barrier or that rollback synchronized
the truncated journal. They also did not test the post-lock canonical recheck.

Recording tests now drive the production HostBackup and rollback paths against
real files. They record backup creation, copy, backup-file sync, backup-directory
sync, journal truncate and journal sync in that order. Each operation also
records its target kind; sync/copy/truncate classify the actual descriptor using
metadata. File and directory barrier targets are asserted separately. Repair
records the same backup prefix before transferring the held lock to normal sink
recovery; the sink's existing fault model separately tests recovery truncation/seal
barriers.
Failure injection at copy and both backup barriers checks that no journal
mutation follows and that the retained invalid backup is identified. A path-swap
hook exercises the actual post-lock canonical comparison. Permission and FIFO
checks cover read-only locks/storage, private backups and nonblocking refusal.
The FIFO test has its own one-test binary so process creation cannot transiently
inherit locks held by sibling tests. Permission checks use a file-access probe
without spawning, and skip when the host bypasses permission bits (such as root).
Tests require an unreadable existing lock to fail with its lock path, rather than
falling back to lockless inspection; dangling lock symlinks name the lock too.
**MEASURED:** each of `media_tools` and `media_fifo` passed sixty consecutive
runs with the default parallel harness, with zero failed runs in either series.
Twenty-four targeted mutations fail their tests, including removal of the
existing-lock guard, incorrect lock error paths/roles and swapping file/directory
sync descriptors while preserving their operation labels.
These checks do not establish physical power-loss durability or implement
retention/export acceptance by themselves.

### Export and retention evidence

**MEASURED:** real-file tests export base generation 0 and every retained closed
generation, compare complete output bytes, and compare both input files byte for
byte. They reject occupied paths, input hard-link/symlink aliases, unavailable or
pruned ids, torn/unsealed tips, bad bases and corruption. Shared inspections can
coexist with export, while a live writer excludes it. The recording I/O seam drives
production writes and syncs, asserting temporary write → file sync → no-clobber
publication → directory sync and the descriptor kinds. Corrupted or shortened
export temporaries never reach a barrier or publication; a destination created
at publication is preserved.

**MEASURED:** retention tests compare each retained image, ids, original-base
binding and frame sequences. They cover keep=2 over five generations, remount,
continued writes after pruning, empty anchors, sectors last changed in
pruned history, recovered closing reasons, unavailable/pruned export targets and
`media verify` exit 0. An altered anchor whose sector is overwritten later is
rejected before sync, proving verification checks intermediate states too.
Failures at every replacement write (including partial writes), file sync, rename
and directory sync leave the old valid journal or the complete valid replacement.
A rename-then-error hook exercises an uncertain successful publication. Reports
distinguish pre-publication failure from uncertainty. Write-protection status
stays unchanged; pre-publication maintenance leaves the command-error latch clear,
while uncertainty at a live checkpoint latches disabled appends for the next
frontend boundary. A real assembled controller test checks
RAM/result bytes, device snapshots/IRQ levels and virtual time across all four
failure boundaries. Open and poisoned generations never prune. Namespace tests
keep creation and retention operations from sweeping each other's active files;
the next exclusive mount removes its own regular retention orphans, including a
header-only orphan. Symlink orphans survive.

**MEASURED (initial export/retention implementation):** the 21 new behavioral tests have matching targeted mutation
failures. Across 34 mutation variants and 40 targeted executions, 39 executions
fail their tests. Removing only the explicit export length comparison survives
because SHA-256 still rejects a shortened file; bypassing the complete
size/digest guard fails. Other detected variants remove export writes/barriers,
change no-clobber publication to replacement, permit unsealed export, use an
exclusive export lock, disable pruning or assembly wiring, omit anchor sectors,
reuse old sequences, change later reasons, skip retained-state verification,
latch maintenance as a command error, suppress uncertainty, prune open/poisoned
history, mix cleanup namespaces, or truncate occupied temporary aliases.
All mutation source changes and runs used a disposable copy outside the repository.

**MEASURED (initial export/retention implementation):** gates passed: workspace build, Clippy with warnings denied,
format check, the fast workspace suite (zero failed, slow tests ignored), and the
required fixture-backed ignored run (14 passed, zero failed). Counts use top-level test
harnesses and exclude duplicate watchdog child executions. Before/after checks
find identical SHA-256 values for all 14 vendor files and identical directory
entries, including the four resolved fixture target directories; no sidecars
appear beside fixtures. No new test spawns a process.

### Export and retention review evidence

**MEASURED:** thirteen additional tests cover maintenance-only close success and
warnings for either or both of two drives, uncertain reporting, finishing later
drives after a real error (including a previously latched error), exact 0664 mode
before writes/sync/publication under an emulated restrictive umask, and the orphan
and export-diagnostic rules above. An otherwise valid later closing reason is
changed without changing any retained image: the whole-history check refuses it
before sync. A close reaching exactly `keep_generations` preserves the expected
closed journal byte for byte and records no replacement operation. A generation
sealed during mount consumes a retention slot. Failed rewrites remove their own
temporary while preserving unrelated entries. A command written and synchronized
after pruning survives Drop without close and is recovered on remount; changing
rename to copy fails this test directly.

**MEASURED:** twenty-five valid targeted mutation variants fail their tests,
including all four previously surviving mutations, copy-instead-of-rename,
maintenance-as-error, warning suppression/duplication, lost mode restoration,
unsafe orphan selection/identity checks and incorrect publication diagnostics.
Two initial mutation invocations omitted the optional assembly test-support
feature and are excluded as compilation failures, not counted as detection.
Mutation checks leave the tested source tree unchanged.

**MEASURED:** the two pre-existing spawning tests now have their own
`media/tests/temporary_fifo.rs` binary; the library test binary spawns no process.
Both use the same production temporary-entry implementation. Each binary passes
thirty consecutive default-parallel runs (zero failures in either binary).
Removing nonblocking open still fails the isolated watchdog test rather than
blocking the test runner indefinitely.

**MEASURED:** review gates pass: build, warning-free Clippy and format check;
the fast workspace suite passes with slow tests ignored, then the required
fixture-backed slow set passes once (14 passed, zero failed). All 14 vendor-file hashes and directory
entries, including four resolved fixture directories, are unchanged. These
checks remain software fault/reopen tests, not physical power-cut evidence.

**MEASURED:** reserved-base repair and rollback probes, along with the read-only
and export tools, refuse both reserved name families as given and through
symlinks without changing base/journal bytes or creating backups. Output and lock
aliases receive the same configuration refusal. Direct cleanup tests protect the
base inode in both namespaces while deleting a different inode with identical
bytes; a sink mount and assembly mount independently check identity propagation.
An uncertain checkpoint followed by an actual guest WRITE DATA retains guest
memory/status behavior, reports disabled appends through the persistence latch,
and prevents the next checked run boundary from advancing. Subject tests pin the
second drive and the firmware/RAM-load image roles. Fourteen targeted mutations
cover the path guards, inode protection and propagation, disabled-output latch,
and input error subjects; every mutation fails its targeted test.

**ASSUMED:** successful platform synchronization and atomic namespace operations
obey the storage contract above. These checks measure software ordering, real-file
reopen behavior and injected failures on the test host, not physical power cuts,
other operating systems/filesystems, storage latency or guest filesystem health.

## Implementation beads

These are proposed narrow work items for the supervisor to file/link, not a
second task board. No tracker state was changed by this spike. Each item needs
its own tests and the normal project gates; dependencies refer to this list.

1. **Re-scope upc-sk3.1: Recoverable journal-backed floppy persistence.** Make
   the existing write-back bead the umbrella for this contract, with volatile
   default, readonly mapping, immutable bases and the explicit durability/error
   behavior. Acceptance: maintainer reviews the format and defaults; link child
   beads below and update the deferred write-back entry when implementation
   lands. Dependencies: this design review; completion waits for items 2–8.
2. **Implement the floppy journal codec and recovery scanner.** Acceptance:
   byte-exact version-1 encoder/parser, base binding, command/generation grammar,
   anchor validation, exhaustive truncations, interior-corruption refusal and
   read-only verification report. No device/filesystem coupling. Dependency: 1's
   approved contract, not its umbrella closure.
3. **Route floppy writes through a host-independent media contract.** Acceptance:
   replace direct image mutation with command-batched media operations, retain
   existing in-memory behavior and write protection, and pass MockBus and fast
   goldens unchanged, including partial/abnormal completions and reset. No host
   persistence enabled yet. Dependency: 1's approved contract.
4. **Implement the durable journal sink and single-writer lifecycle.** Acceptance:
   append/flush/platform-sync ordering, stable locks, synchronized creation,
   tail repair/sealing, checkpoints/finish and latched output errors; injected
   short-write/sync/lock failures cannot alter device bytes/status. Dependencies:
   2 and 3.
5. **Wire drive write modes and replay into assembly sessions.** Acceptance:
   validated Rust config enum/path/retention count, preserved source metadata,
   replay before boot, explicit finalization/error checks in consuming frontends,
   no silently discarded persistence handle, and two-run guest-write integration.
   Volatile/readonly remain free of host output; no serialization feature.
   Dependencies: 3 and 4.
6. **Add offline media verification, generation listing and rollback.** Acceptance:
   proposed CLI/exit contracts, backups and lock exclusion, correct truncation
   through a selected retained generation, bad-base/interior-damage refusal,
   and rollback/remount/reappend integration. Dependencies: 2 and 4.
7. **Add immutable-image export and atomic journal retention.** Acceptance:
   commit/compact produces a new verified raw image without changing inputs;
   keep=N rewrites an independently valid anchor plus retained history;
   publication-failure tests show old/new valid outcomes and no input aliases.
   Dependencies: 2, 4 and 5; reuse the command group from 6.
8. **Validate persistence failures and unchanged volatile goldens.** Acceptance:
   full tests above, targeted mutation/red-green evidence, platform sync/locking
   validation, fixture hashes/absence of sidecars and full-result deterministic
   comparisons; document actual supported host guarantees and command examples.
   Run slow fixture goldens in the implementation gate, not this spike.
   Dependencies: 5–7.

Open maintainer choices, with defaults selected so implementation is actionable:
confirm ten retained generations, synchronous per-command durability (including
its latency), mandatory backups for destructive offline tools, and replay/seal
of an interrupted command-complete tip on normal mount. The alternative for
the last choice is to require explicit repair before mounting; do not silently
change this behavior during implementation. Host/filesystem support and real
power-cut validation remain work to establish, not assumptions to advertise as
measured guarantees. The one-day estimate applies to this design spike, not
to implementing all eight work items.
