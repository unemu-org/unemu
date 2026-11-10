# Recoverable persistent floppy media

Status: accepted design (the maintainer approved the defaults on 2026-11-10).
The version-1 codec and recovery scanner are implemented in `crates/media`;
the device media contract, the file sink, the write modes and the offline
tools are still to come.

## Context

Guest writes need to survive a restart without destroying the last usable disk.
Keep the original raw image immutable and append sector replacements to a
separate journal. Close a generation at the end of each session or at an explicit
checkpoint. A user can discard a bad session by returning to an earlier generation.
A generation means a recovery point, not a guarantee that the guest filesystem
is healthy: the user decides which version works.

Evidence from the current implementation:

- **READ:** `crates/devices/src/upd765.rs:882–909`: WRITE DATA commits DMA bytes
  into `Drive.image`. A partial final sector is zero-filled to 512 bytes; a short
  isolation-test image grows. `:918–935` computes status and raises IRQ6 after
  mutation. An abnormal result can therefore accompany committed sectors.
- **READ:** `crates/assembly/src/image.rs:13–22` defines inline, file and blank
  sources. `assemble.rs:147–153,284–318` reads file sources once, resolves them
  to inline bytes, and validates the bytes actually mounted.
- **READ:** `crates/assembly/src/assemble.rs`, `build_fdc_drives`, routes drives
  by named `controller: DeviceId` and controller-local `unit`. In
  `crates/assembly/src/drive.rs`, `DriveConfig.write` is `WriteMode::Volatile`
  (default) or `WriteMode::ReadOnly`; the latter uses `Drive::new_read_only`
  and preserves the existing not-writable status. Neither mode persists to the
  host image.
- **READ:** [Deferred, Host-file write-back for a floppy WRITE](../deferred.md#host-file-write-back-for-a-floppy-write)
  records the absence of host persistence. The brief identifies its follow-up
  as `upc-sk3.1`; the tracker was not queried during this worker spike.
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
`File::sync_all`) and, on macOS, a full device-cache flush through `F_FULLFSYNC`
where required by the platform implementation; do not assume Rust's wrapper
already supplies the stronger guarantee. An unsupported durability primitive
is a host error, not a silent downgrade. Apple documents the distinction in
[fsync(2), Description](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/fsync.2.html).

Before the first guest instruction, creation must synchronize the header and
the containing directory entry. Each replacement file is written to an unused
temporary name in the same directory, fully verified and synchronized before
atomic publication; synchronize the containing directory after publication.
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
There are no reads of the journal or base during execution. Opening/replaying
media is an explicit initial-state input, just like today's base-image load.
Host failure can stop a run at a frontend boundary; its completed guest prefix
must equal the corresponding volatile prefix. This is the precise limit of
the determinism claim, not a promise that failing and healthy hosts finish
the same amount of execution. Interactive-input timing remains a frontend
concern; persistence adds no new timing input.

## Tooling and config

### Config contract

Keep the agreed per-drive `write` modes: `volatile` (default), `readonly`,
`journal`. The current `Eq`/`Debug` Rust enum, `WriteMode`, implements `Volatile`
and `ReadOnly`, with the existing not-writable status preserved. Extend it with
a journal variant carrying a path and retention count, rather than adding an
independent boolean and potentially conflicting options.
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

### Offline commands

Add a `media` command group to the existing binary, preserving `--boot`.
Names and options below are proposed CLI contracts, not commands available now:

```text
unemupc98 media generations --base disk.img --journal disk.img.upfj
unemupc98 media verify --base disk.img --journal disk.img.upfj
unemupc98 media verify --base disk.img --journal disk.img.upfj --repair-tail --backup before-repair.upfj
unemupc98 media rollback --base disk.img --journal disk.img.upfj --to 7 --backup before-rollback.upfj
unemupc98 media commit --base disk.img --journal disk.img.upfj --to 7 --output restored.img
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

`commit` (alias `compact`) reconstructs a selected closed generation into a
**new raw image** at an unused output path. Write a same-directory temporary,
check its size and digest against reconstruction, synchronize it, publish
without overwriting an existing destination, then synchronize its directory.
The old base and journal remain untouched; never update them as a two-file
transaction. To continue from this new base, point config at it and an unused
journal path; its new lineage starts at generation 0. No automatic config edit,
in-place commit, or export of an unsealed tip. A user repairs/seals the tip first.

### Locking and retention

Use a stable `<journal>.lock` sidecar with a nonblocking OS advisory lock.
Mounts and mutating tools take it exclusively; inspections take it shared.
Creation of lock metadata is permitted for inspections, but image/journal
contents remain read-only. The lock file is never replaced/unlinked by normal
tools; locks release on process exit, without stale-PID or time heuristics.
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
