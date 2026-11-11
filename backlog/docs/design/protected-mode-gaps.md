# 386 protected-mode gaps

## Context

**Verdict: the investigated core cannot execute a DOS extender's protected-mode
entry or a guest-paged 32-bit program.** The decode groundwork is useful,
but `Compat32` is a flat application execution context, not an emulated
80386 system. Legacy protected mode needs changes to the CPU-state, mode,
memory-translation and event-delivery seams. Adding opcodes or selecting
`Compat32` alone will not work.

This is a bounded investigation for the protected-mode scope in
[decision-16](../../decisions/decision-16%20-%20Scope%20a%20period%20PC%20from%20DOS%20to%20Windows%2098%20SE%20with%203dfx%20Voodoo.md).
It proposes work; it does not create or claim implementation beads. Firmware
selection belongs to the parallel firmware investigation. No repository code,
dependency pin, guest fixture or CPU checkout was changed.

Evidence vocabulary throughout:

- **MEASURED:** executed against unmodified code in this investigation.
- **READ:** source inspection or an architectural requirement from the named
  reference; not a runtime compatibility result.
- **ASSUMED:** a proposed scope choice or inference requiring later acceptance
  tests. In particular, no complete DOS extender was run.

The current CPU pin is `8979fc95cd81d73763d50d1d627b40ded3f8f284`.
INS/OUTS are now implemented, including Real16 REP port handshakes and
per-element accounting. This resolves the opcode/handshake portion of the
historical string-I/O gap below; protected-mode privilege checks and the
broader system-mode work remain separate requirements.

Historical READ: at investigation time root `Cargo.toml:36` pinned `x86jit-core` to
`96a49a9063cdf0a2bdb2d916f01b381fbad0176e`. All CPU paths below are relative
to that revision's `x86jit-core/src/`; line numbers refer to that checkout,
whose HEAD was also verified. Machine paths are relative to this repository.
The probe used the cached core by path; the machine adapter resolved its
normal git dependency to the same revision. These are two Cargo package
instances with the same source revision, not a patched dependency.

READ: `lift/mod.rs:66` defines `CpuMode` as a decode/lift context;
`lift/mod.rs:87` has only `Long64`, `Compat32` and `Real16`.
`vm.rs:1367` explicitly makes the vCPU's mode constant for its lifetime.
`Vm::set_cpu_mode` (`vm.rs:928`) selects the mode inherited by **new** vCPUs;
it is not a guest-driven mode switch. `state.rs:349` explicitly says that
descriptor tables are not modeled. The existing selector fields and
`rflags_system` image (`state.rs:428`) must not be mistaken for implemented
descriptor or privilege state: that path assumes CPL 3.

ASSUMED scope for the first executable result: an interpreter-only 386
legacy-system path, initially CPL 0, with USE16/USE32 code, guest-owned
GDT/IDT, 4 KiB two-level paging, and transitions back to real mode for DOS
services. Then qualify an open extender on a clean DOS boot without an
existing DPMI/VCPI server. This does not assume that every extender runs its
client at CPL 0. Ring transitions and TSS support remain explicit follow-up
requirements where the selected host/client needs them. V86, hardware task
switching, debugging and an FPU are not prerequisites for the first integer
probe, and are not silently promised by it.

Architectural reference: **Intel 80386 Programmer's Reference Manual (1986)**,
abbreviated **PRM** below. Links point to a transcription of that Intel
manual, not to the behavior of a modern x86 processor. In particular, the
386 paging target has no CR4, PAE, PSE, NX, CR0.WP or INVLPG requirement.

## Probe results

### P1 — complete entry and paging attempt

MEASURED: the scratch Cargo executable loaded a NASM flat binary at
`CS:IP=0000:1000`, with DS/ES/SS zero, SP `9000h`, IF initially set, and
8 MiB of mapped RWX RAM. The interpreter was selected through
`Vm::set_cpu_mode(CpuMode::Real16)` before creating the vCPU. All writes
used `Vm::write_bytes`. The hand-written program was:

```asm
bits 16
org 0x1000
cli
lgdt [gdtr]
mov eax, cr0
or eax, 1
mov cr0, eax
jmp dword 8:protected
bits 32
protected:
mov ax, 0x10
mov ds, ax
mov es, ax
mov ss, ax
mov fs, ax
mov gs, ax
mov esp, 0x9000
mov eax, 0x3000
mov cr3, eax
mov eax, cr0
or eax, 0x80000000
mov cr0, eax
jmp short paged
paged:
mov eax, [0x00400000]
mov [0x5000], eax
hlt
align 8
gdt:
dq 0
dq 0x00cf9a000000ffff
dq 0x00cf92000000ffff
gdtr:
dw 23
dd gdt
```

The harness seeded these guest tables/data, rather than requiring the entry
stub to build them:

| Physical location | Contents / intended translation |
| --- | --- |
| `3000h` | PDE[0] = `6003h`; PDE[1] = `7003h` |
| `6000h` | 1024 PTEs: `i * 1000h + 3`, identity-map the first 4 MiB |
| `7000h` | PTE[0] = `2003h`: linear `00400000h` maps to physical `2000h` |
| `2000h` | Dword `12345678h` |
| `00400000h` | Dword `DEADBEEFh`, a sentinel for untranslated access |
| `5000h` | Initially zero; expected final marker `12345678h` |

MEASURED, `run_steps(&vm, 100, StepOptions::default())` returned:

```text
Ok(StepRun {
  steps: 1,
  stop: Exit(UnknownInstruction {
    addr: 4097, bytes: [15, 1, 22, 104, 16, ...], len: 5
  })
})
RIP=1001h CS=0000h IF=false retired_instructions=1 marker=[0,0,0,0]
```

This is `LGDT [1068h]`, encoding `0F 01 16 68 10`, immediately after CLI.
Neither CR0 write, the far jump, segment loads nor paging enable was reached.
There is **no measured paged execution** and no measured protected-mode
fault delivery. `UnknownInstruction` here is an emulator implementation
exit, not an architectural claim that LGDT is illegal on a 386.

MEASURED: the assembled image is 110 bytes; SHA-256:
`22d7b2a194a46a411f1b7426efd9b2d0896575fea942f0c633fc36372f3e0bb4`.
The GDT is at `1050h`, protected entry at `1018h`, CR3 write at `1030h`,
PG-setting CR0 write at `103Bh`, and marker load at `1040h`.

### P2 — isolated instruction coverage

MEASURED: each row was also tested from a fresh Real16 VM at `1000h`
using `Vcpu::step_instruction`, with AX initially `0010h`. Unsupported
cases kept RIP at `1000h`. The 33 cases produced 28 `UnknownInstruction`
exits and five `Continue` results. Grouped encodings below list **every**
case; `m` denotes the direct memory operand `[0800h]` used for table ops.

LAR/LSL, VERR/VERW and LLDT/SLDT/LTR/STR are protected-mode operations:
their Real16 probes measure the current emulator exit, not a legal real-mode
use or execution in protected mode. A 386 should report the specified #UD
for their real-mode use (PRM Chapter 17, named instruction entries). Their
missing protected-mode implementation is established by the source audit,
not by treating these Real16 cases as protected-mode tests.

| Instruction(s) | Bytes (hex) | Measured result |
| --- | --- | --- |
| CLI | `FA` | Continue, RIP=`1001h` |
| LGDT m / LIDT m / SGDT m / SIDT m | `0F 01 16 00 08` / `0F 01 1E 00 08` / `0F 01 06 00 08` / `0F 01 0E 00 08` | UnknownInstruction, length 5 each |
| SMSW AX / LMSW AX | `0F 01 E0` / `0F 01 F0` | UnknownInstruction, length 3 each |
| MOV EAX,CR0 / MOV CR0,EAX | `0F 20 C0` / `0F 22 C0` | UnknownInstruction, length 3 each |
| MOV CR3,EAX | `0F 22 D8` | UnknownInstruction, length 3 |
| MOV EAX,CR2 / MOV CR2,EAX | `0F 20 D0` / `0F 22 D0` | UnknownInstruction, length 3 each |
| CLTS | `0F 06` | UnknownInstruction, length 2 |
| LLDT AX / LTR AX / SLDT AX / STR AX | `0F 00 D0` / `0F 00 D8` / `0F 00 C0` / `0F 00 C8` | UnknownInstruction, length 3 each |
| LAR AX,AX / LSL AX,AX | `0F 02 C0` / `0F 03 C0` | UnknownInstruction, length 3 each |
| VERR AX / VERW AX | `0F 00 E0` / `0F 00 E8` | UnknownInstruction, length 3 each |
| MOV EAX,12345678h | `66 B8 78 56 34 12` | Continue, EAX=`12345678h`, RIP=`1006h` |
| JMP 0008:1100 (16:16) | `EA 00 11 08 00` | Continue, CS=`0008h`, IP=`1100h`; real-mode selector update |
| JMP 0008:00001100 (16:32) | `66 EA 00 11 00 00 08 00` | UnknownInstruction, length 8 |
| MOV FS,AX / MOV GS,AX | `8E E0` / `8E E8` | UnknownInstruction, length 2 each |
| MOV DS,AX | `8E D8` | Continue, DS=`0010h`; no descriptor lookup |
| PUSHFD / POPFD / IRETD | `66 9C` / `66 9D` / `66 CF` | UnknownInstruction, length 2 each |
| PUSHAD / POPAD | `66 60` / `66 61` | UnknownInstruction, length 2 each |
| MOV EAX,[00400000h], address-size 32 | `67 66 A1 00 00 40 00` | Continue, EAX=`DEADBEEFh`, RIP=`1007h` |

READ: `lift/control.rs:839` accepts only `FarBranch16` for the immediate
real-mode far target; line 868 deliberately rejects the 386 16:32 form.
`lift/mod.rs:4227` implements Real16 CLI/STI, but lines 4235–4248 select
only 16-bit FLAGS/IRET forms. `lift/mod.rs:4360` selects only PUSHA/POPA,
not PUSHAD/POPAD. These are specific gaps despite successful 32-bit MOV
decoding.

The address-size-32 case is also a correctness warning, not a successful
386 memory test. READ: `interp/mod.rs:4463` truncates the limit-check
operand to 16 bits, whereas `lift/mod.rs:4980` adds the full address to
the segment base. With a normal 64 KiB real-mode DS cache, this reference
should not access 4 MiB. The probe shows that prefix acceptance does not
establish correct 386 segment-limit behavior. PRM §14.6 and §16.1 govern
the required behavior.

### P3 — Compat32 is not a protected-mode substitute

MEASURED: fresh Compat32 VMs, IF=true, same mapped RAM:

| Operation | Result |
| --- | --- |
| `run_steps(..., 1, ...)` | `Err(UnsupportedMode)` in each of four independent cases |
| `irq_ready()` | false in each case, despite IF=true |
| Step `MOV DS,AX` (`8E D8`) | UnknownInstruction at `1000h` |
| Step CLI (`FA`) | UnknownInstruction at `1000h` |
| Step `INT 31h` (`CD 31`) | `Exit::Exception`, vector 49, saved RIP=`1002h`; software trap metadata, no error code; no guest handler |
| Step `MOV EAX,[00400000h]` (`A1 00 00 40 00`) | Continue, EAX=`DEADBEEFh`; seeded PDE/PTEs have no effect |

The last row is a flat-map control, **not** a test with PG=1: there is no
CR0 state to seed. READ: `vm.rs:1860` refuses step-bounded execution
outside Real16; `vm.rs:1502` excludes other modes from readiness.
`lift/control.rs:1152` delegates user-mode segment loads as unsupported;
`lift/mod.rs:4211` emits an outward trap for INT n (and a host syscall
convention for INT 80h). These behaviors cannot serve a guest DPMI host.

### P4 — manual IRQ injection can accidentally use real-mode delivery

MEASURED: in a Compat32 VM, set IF=true, ESP=`9000h`, install IVT vector
20h at physical `80h` as `0000:1800`, put HLT at `1800h`, queue
`inject_irq(20h)`, and call `run(&vm, Some(3))`. The result was:

```text
Hlt; queued=false; RIP=1801h; ESP=8FFAh
bytes at 8FFAh = [00,10,00,00,02,02]
```

Those six bytes are a 16-bit IP/CS/FLAGS frame (`1000h`, `0000h`,
`0202h`). Three scratch assertions verified HLT, RIP and ESP. READ:
`vm.rs:2336` checks the pending IRQ and completion/shadow gate without
a mode guard, and calls `interp::deliver_interrupt`; the latter uses
`vector*4` and 16-bit pushes (`interp/control.rs:614`). Comments describe
this as real-mode-only caller usage. The machine's readiness check normally
prevents this misuse. It is neither IDT support nor a reason to enable
Compat32 IRQ readiness before protected-mode delivery exists.

### P5 — memory above 1 MiB exists as a mapping capability; A20 is unwired

MEASURED: a scratch test using the repository's `X86JitCpu` mapped 2 MiB
with `new_real16_bare`/`map_ram`, wrote `11h` at `20h` and `77h` at
`100020h`, then ran `MOV AX,FFFFh; MOV DS,AX; MOV AL,[0030h]; HLT`.
It halted after four steps with AL=`77h`. Separately, the repository's
8042 received command D1h followed by data `01h`, and reported
`a20_enabled()=false`; this did not change the CPU result.

This is an adapter/device seam test, not a full assembled chipset test.
READ: `crates/devices/src/i8042.rs:64` explicitly models only the latch;
`crates/cpu/src/x86jit_backend.rs:78` constructs a flat VM without an
A20 input. Thus the missing work is physical-address gating and its
wiring, not a new implementation of the existing D0h/D1h latch. Likewise,
1 MiB is the BIOS preset's size (`crates/assembly/src/presets.rs:72`),
not an adapter-wide address-space limit.

### P6 — open extender inspection and deliberate runtime scope cut

READ: inspected the public DOS/32A 9.12 source at
[amindlost/dos32a](https://github.com/amindlost/dos32a), specifically:

- `src/dos32a/text/kernel/mode.asm`, blob
  `a444de6344974d74e86fe5ede971e31445295a7d`, lines 132–157:
  the raw/XMS entry saves EFLAGS with PUSHFD, uses CLI, LIDT, LGDT,
  MOV CR0, a 16:16 far jump into protected USE16 code, protected
  segment reloads including FS/GS, an ESP load, then IRETD to its target.
  Lines 170–199 clear PE and return to real mode. This source requires
  mixed USE16/USE32 execution; a direct Real16-to-flat-Compat32 switch
  would miss its actual sequence.
- `detect.asm`, blob `766593291e5c78712019e992b482dbb5ce0d1a5f`,
  line 122 uses SMSW and lines 328–361 use PUSHFD/POPFD for CPU
  identification. Lines 167–205 probe optional DPMI/VCPI hosts.
- `init.asm`, blob `358c31368458020fa9d15f6cb8ef44aa3ea2b204`,
  lines 536–613 enable A20 and try E820h/E801h before AH=88h.
  Lines 775–797 touch CR3, CR0, CR2 and CLTS after entry. The CR4/FXSR
  branch at 784–795 is conditional on a newer CPU being reported.
  Line 149 uses LAR to inspect CS.

MEASURED evidence applicable to this source: P2's PUSHFD, SMSW, LIDT,
LGDT, MOV CR0/CR2/CR3 and CLTS forms all exit as unsupported at this
pin. These are isolated instruction measurements, not a replay of the
extender's assembled entry. ASSUMED: its raw path should be the first
extender qualification target, avoiding an EMS/V86 host and its extra
contracts. Rebuilding its DOS loader and creating a bootable client fixture
would not resolve the earlier core blockers, so no binary extender run was
attempted within this spike. Its complete dynamic instruction footprint and
client privilege requirements remain unmeasured.

### Reproduction and validation record

MEASURED: all harness sources, NASM listing, binary, Cargo outputs and
logs are confined to the assigned `o0j1-probe` scratch directory. The
executable uses `Vm::new(VmConfig::flat(...))`, `set_cpu_mode`, `map`,
`write_bytes`, `new_vcpu`, `set_reg`, `run_steps` and `step_instruction`;
it injects no CPUID, paging or mode behavior. Fresh VMs isolate instruction
cases; it uses no guest execution clock, randomness or threads.
The scratch artifacts are `Cargo.toml`, `Cargo.lock`, `src/main.rs`,
`entry.asm`, `entry.bin`, `entry.lst`, `probe.log`, `build.log`, `test.log`,
`clippy.log` and `fmt.log`, with build products under `target` and
`workspace-target`.

To reproduce, supply the assigned scratch directory as `PROBE_DIR` and
the provided environment file as `DEVSHELL_ENV`, then run from this
worktree (the environment is sourced before every Cargo invocation):

```sh
source "$DEVSHELL_ENV" && which cc cargo
source "$DEVSHELL_ENV" && nasm -f bin "$PROBE_DIR/entry.asm" -o "$PROBE_DIR/entry.bin" -l "$PROBE_DIR/entry.lst"
source "$DEVSHELL_ENV" && CARGO_TARGET_DIR="$PROBE_DIR/target" cargo run --offline --manifest-path "$PROBE_DIR/Cargo.toml"
source "$DEVSHELL_ENV" && CARGO_TARGET_DIR="$PROBE_DIR/workspace-target" cargo build --offline --all-targets
source "$DEVSHELL_ENV" && CARGO_TARGET_DIR="$PROBE_DIR/workspace-target" cargo test --offline --workspace
source "$DEVSHELL_ENV" && CARGO_TARGET_DIR="$PROBE_DIR/workspace-target" cargo clippy --offline --all-targets --all-features -- -D warnings
source "$DEVSHELL_ENV" && cargo fmt --check
```

Both `cc` and `cargo` resolved under `/nix/store`. Final command results:

| Command | Measured result |
| --- | --- |
| NASM assembly | Exit 0; binary/hash recorded in P1 |
| Scratch `cargo run --offline --manifest-path …` | Exit 0; 33 Real16 cases, four Compat32 cases, full-entry attempt, IRQ frame and adapter/A20 probe; three IRQ assertions pass |
| `cargo build --offline --all-targets` | Exit 0 |
| `cargo test --offline --workspace` | Exit 0; 48 result groups, 739 passed, 0 failed, 14 ignored, 0 measured, 2 filtered out |
| `cargo clippy --offline --all-targets --all-features -- -D warnings` | Exit 0 |
| `cargo fmt --check` | Exit 0 |

An initial scratch-only build failed with two E0277 diagnostics because
`StepResult` has no Debug implementation. The harness was corrected to
match Continue/Exit explicitly; the final run above passes. No Cargo
sandbox/registry permission failure occurred. The ignored/slow set was
not run: the brief did not authorize it. No repository tests were added,
because this change is an investigation document; P1–P5 record behavior
of the unmodified implementation, rather than a claimed fix with a
red/green regression test.

## CPU-core gap list

Each entry names the current implementation seam it would touch, not a
claim that a future feature can fit entirely within one file. All entries
are READ unless a P-number additionally supplies MEASURED evidence.

### Segmentation

| Gap | Current seam / evidence | Intel 80386 PRM requirement |
| --- | --- | --- |
| GDTR, LDTR, selector decoding and cached descriptors for all six segment registers | `state.rs:296`, `state.rs:349`, `state.rs:433`: selectors and FS/GS bases exist, no table registers or hidden caches | [§5.1.1–§5.1.4, Segment Translation](https://pdos.csail.mit.edu/6.828/2018/readings/i386/s05_01.htm): bases, effective limits, access rights, GDT/LDT selection, null selectors, hidden state |
| Protected data/stack segment loads and validation | `lift/mod.rs:2233`, `lift/control.rs:1147`; P2/P3. Real16 loads derive base from selector; user-context loads are unsupported | [§6.3.1–§6.3.2, Segment-Level Protection](https://pdos.csail.mit.edu/6.828/2018/readings/i386/s06_03.htm): type, P, DPL/RPL/CPL checks; readable/writable and expand-down semantics; accessed-bit update |
| Segment checks on fetches, stack, scalar and string accesses | `lift/mod.rs:4938`, `interp/mod.rs:4451`, `interp/control.rs:146`; fixed real-mode check and fixed 16-bit string indexing are insufficient | PRM §6.3.1.2 (Limit Checking), §6.3.2 (Restricting Access to Data); P2 exposes the 32-bit-offset limit-check hole |
| Independent CS.D and SS.B; protected USE16 code and mixed-width transfers | `lift/mod.rs:94`, `lift/mod.rs:5233`, `lift/control.rs:77`, `lift/control.rs:839`: one CpuMode currently couples decode, PC and stack rules | [§16.1, How the 80386 Implements 16-Bit and 32-Bit Features](https://pdos.csail.mit.edu/6.828/2018/readings/i386/s16_01.htm); CS.D selects defaults, SS.B selects SP/ESP, gate width selects frame width |
| Protected far JMP/CALL/RETF/IRET and call gates, including changed CPL | `lift/mod.rs:4014`, `lift/control.rs:1217`, `interp/mod.rs:4359`: existing user IRET only accepts configured CS/SS; P2's far16 merely changes a real selector | PRM §6.3.3 (Restricting Control Transfers), §6.3.4 (Gate Descriptors Guard Procedure Entry Points), §16.4 (Transferring Control Among Mixed Code Segments), §17 CALL/JMP/RET/IRET |

Hidden descriptor caches must survive guest edits to their backing table
until an architectural reload. Recomputing a base from the current GDT on
every access would also be incorrect. Existing fixed real-mode limit
checks and same-selector user IRET are useful code, not descriptor support.

### Mode switch

| Gap | Current seam / evidence | Intel 80386 PRM requirement |
| --- | --- | --- |
| Architectural CR0 and mutable execution regime separate from decoder width | `state.rs:296`, `vm.rs:1367`, `vm.rs:932`, `lift/mod.rs:2254`; P1/P2 stop before PE can change | [§4.1.3, Control Registers](https://pdos.csail.mit.edu/6.828/2018/readings/i386/s04_01.htm), [§10.3, Switching to Protected Mode](https://pdos.csail.mit.edu/6.828/2018/readings/i386/s10_03.htm): PE starts protected execution at CPL 0 while old segment caches initially remain |
| PE transition, far CS reload, PG transition and return to real mode | `lift/mod.rs:4014`, `vm.rs:2376`, `lift/mod.rs:276`: fetch formation knows only real selector-shift or flat RIP; P1 | [§14.5, Switching Back to Real-Address Mode](https://pdos.csail.mit.edu/6.828/2018/readings/i386/s14_05.htm), §10.3; CR0 writes and far transfers must end execution under stale context |
| Context-sensitive cache identity and invalidation | `vm.rs:3047`, `lift/mod.rs:241`: fetch physical address and CpuMode are currently sufficient only for existing regimes | PRM §5.1, §5.2.5, §16.1. ASSUMED implementation: include/validate segment context, decode defaults and translated code-page identity; invalidate relevant cached IR after descriptor-context, CR3, PG or A20 changes |
| 386 model and reset state distinct from the 286 real-mode contract | `lift/mod.rs:284`, `state.rs:428`, `vm.rs:1367`; Real16 has 286-specific policies and no CPU-generation state governing the transition | [§10.1–§10.2, Initialization](https://pdos.csail.mit.edu/6.828/2018/readings/i386/s10_02.htm), §14.8; do not change existing 286 goldens by enabling all 386 semantics indiscriminately |

### Paging

| Gap | Current seam / evidence | Intel 80386 PRM requirement |
| --- | --- | --- |
| CPU-local linear-to-physical translation using CR3 and CR0.PG | `memory.rs:84`, `memory.rs:2152`, `memory.rs:3732`, `jit_abi.rs:89`: current accesses resolve guest address directly into backing; neither CR state nor access privilege is supplied to a page walk | [§5.2.1–§5.2.4, Page Translation](https://pdos.csail.mit.edu/6.828/2018/readings/i386/s05_02.htm): 10/10/12-bit split, 1024-entry directory/table, 4 KiB frames; P3 is a flat-access control |
| Translation for instruction fetch and implicit system accesses as well as data | `lift/mod.rs:276`, `memory.rs:2174`, `interp/control.rs:614`; currently no page walk before decode, IVT fetch or stack access | PRM §5.3 (Combining Segment and Page Translation), [§6.4.3, Overrides to Page Protection](https://pdos.csail.mit.edu/6.828/2018/readings/i386/s06_04.htm): descriptor/TSS/IDT and inner-stack accesses have supervisor rules |
| PDE/PTE P/U/S/R/W checks and A/D side effects | `memory.rs:2157`, `memory.rs:2265`; host map Prot/RegionKind is not a guest PTE | PRM §5.2.4.2–§5.2.4.4, §6.4.1–§6.4.2. Preserve 386 supervisor-write behavior; do not import later CR0.WP semantics |
| Guest #PF with CR2, complete architectural error code and restart state | `fault.rs:260`, `fault.rs:497`, `fault.rs:572`, `memory.rs:28`; existing flat-map projection deliberately cannot claim a complete architectural #PF | PRM §9.8.14 (Page Fault): record faulting linear address, P/W/U code and restart EIP; distinguish guest paging faults from host physical-map/MMIO failures |
| CR3/TLB semantics and physical alias coherence | `vm.rs:3047`, `memory.rs:1379`, `memory.rs:3732`; no architectural guest TLB exists | PRM §5.2.5 (Page Translation Cache). A software TLB is optional initially; CR3 reload must invalidate any cached translation, and stores through aliases must invalidate translated code by physical backing |

READ: the enum name `MemoryModel::SoftMmu` is not evidence of guest
paging. Its `map` arm is a `todo!` at `memory.rs:1761`. Existing SMC
code-page bitmaps, fetch-policy caches and host alias bookkeeping are
also not PDE/PTE walkers or architectural TLBs.

The initial page walker can be uncached. It must still split page-crossing
fetches and operands, preserve defined fault/restart state, route translated
MMIO to physical device addresses, and distinguish a guest not-present
PTE from an unmapped physical bus address. Do not emulate paging by
changing the shared Vm map per address space: CR3 belongs to each CPU.

### Interrupts/exceptions

| Gap | Current seam / evidence | Intel 80386 PRM requirement |
| --- | --- | --- |
| IDTR, 286/386 interrupt/trap gates and IDT limit/type/presence checks | `state.rs:296`, `interp/control.rs:614`, `vm.rs:2336`; P2/P4 | [§9.4, Interrupt Descriptor Table](https://pdos.csail.mit.edu/6.828/2018/readings/i386/s09_04.htm), [§9.5, IDT Descriptors](https://pdos.csail.mit.edu/6.828/2018/readings/i386/s09_05.htm): eight-byte entries, descriptor-selected target and gate width |
| Protected INT/INTR delivery and IRET, including IF/TF rules and software gate DPL | `lift/mod.rs:4158`, `lift/mod.rs:4211`, `lift/mod.rs:4260`, `vm.rs:1502`; P3/P4 | [§9.6.1, Interrupt Procedures](https://pdos.csail.mit.edu/6.828/2018/readings/i386/s09_06.htm), §9.2: interrupt gate clears IF, trap gate preserves it; hardware IRQs do not use software-INT gate-DPL admission |
| Privilege-changing event entry and outer-level IRET | `lift/control.rs:1217`, `interp/mod.rs:4359`; configured-selector user return cannot restore a real protected privilege frame | PRM §9.6.1.1–§9.6.1.2, §6.3.4; stack switch through the TSS, correct saved SS:ESP and return validation |
| Precise fault/trap/abort delivery, architectural error codes and fault-on-delivery escalation | `vm.rs:1791`, `interp/control.rs:621`, `fault.rs:497`; Real16 re-vectors faults through the IVT, while Compat32 exits outward | [§9.7, Error Code](https://pdos.csail.mit.edu/6.828/2018/readings/i386/s09_07.htm), [§9.8, Exception Conditions](https://pdos.csail.mit.edu/6.828/2018/readings/i386/s09_08.htm): selector EXT/IDT/TI fields, restart RIP, #DF(0), shutdown if #DF delivery fails |

Required fault coverage, distinguished from an enum containing a vector:

- **#GP(13), #SS(12), #NP(11):** new descriptor, limit, privilege and
  gate checks, with the correct selector code or zero; PRM §9.8.11–§9.8.13.
- **#PF(14):** genuine guest paging violations, CR2 and code; §9.8.14.
- **#TS(10):** invalid task/stack-transition state when TSS/task machinery
  is enabled; §9.8.10. A flat CPL 0 probe does not establish this coverage.
- **#DF(8) and shutdown:** faults during event delivery require the
  exception-class combination rules, not unconditional recursive delivery;
  §9.8.8. A machine reset policy follows a reported shutdown.
- **#DE(0), #BP(3), #OF(4), #BR(5), #UD(6):** several producers already
  exist, but need protected delivery and applicable 32-bit forms. READ:
  `lift/mod.rs:4143` and `lift/mod.rs:4175` provide Real16-specialized
  producers; this is not a claim that all exception generation is absent.
- **#DB(1):** TF is stored without single-step delivery (`state.rs:428`);
  DR0–DR3/DR6/DR7 and precise debug conditions are absent from CpuState.
  PRM [§12.3, Debug Exceptions](https://pdos.csail.mit.edu/6.828/2018/readings/i386/s12_03.htm).
  Defer debugger qualification unless the chosen extender requires it.
- **#NM(7) / coprocessor integration:** CR0.EM/MP/TS and CLTS need a
  coherent policy before claiming 386/387 software support; PRM §9.8.7
  and §11.1. Existing x87 execution does not establish those control bits
  or PC coprocessor error wiring. The first probe is integer-only.

### Task state

| Gap | Current seam / evidence | Intel 80386 PRM requirement |
| --- | --- | --- |
| TR, cached TSS descriptor, LTR/STR; TSS privilege stacks | `state.rs:296`, `lift/mod.rs:2221`, `interp/control.rs:614`; P2's LTR/STR unsupported | [§7.1, Task State Segment](https://pdos.csail.mit.edu/6.828/2018/readings/i386/s07_01.htm), [§7.3, Task Register](https://pdos.csail.mit.edu/6.828/2018/readings/i386/s07_03.htm): TR cache, descriptor validation/busy bit, SS0–SS2/ESP0–ESP2 for inward transitions |
| Dynamic CPL/IOPL and TSS I/O permission bitmap | `state.rs:607` implements fixed CPL 3 flag restore; `lift/mod.rs:4227` only provides real-mode CLI/STI | [§8.3.1–§8.3.2, Protection and I/O](https://pdos.csail.mit.edu/6.828/2018/readings/i386/s08_03.htm): check permission before any port side effect; POPF/IRET IF and IOPL changes are privilege-sensitive |
| Hardware task switch, task gates, backlink, busy/NT/TS behavior | `state.rs:428`, `interp/mod.rs:4350`: NT is stored/rejected for the user-context IRET, not used for a task return | [§7.5–§7.6, Task Switching / Task Linking](https://pdos.csail.mit.edu/6.828/2018/readings/i386/s07_05.htm), §9.6.2. Conditional scope; no evidence that the initial raw extender path needs task switching |
| V86 execution and transitions | `lift/mod.rs:87`, `state.rs:428`: a stored VM flag does not select V86 semantics | [§15.3, Entering and Leaving V86 Mode](https://pdos.csail.mit.edu/6.828/2018/readings/i386/s15_03.htm), §15.4–§15.5. Needed for qualifying V86 memory-manager paths, not implied by running raw DOS/32A |

Do not equate loading TR with implementing a hardware task scheduler.
TSS privilege stacks and port permissions can be needed with software
task management, while a single-CPL entry/paging probe needs neither.

### Instructions

The following is a bounded minimum/audit list, not a complete 80386 opcode
conformance claim. PRM [§4.2, Systems Instructions](https://pdos.csail.mit.edu/6.828/2018/readings/i386/s04_02.htm)
and the named entries in **PRM Chapter 17, 80386 Instruction Set** define
the encodings, privilege rules and faults.

| Family | Gap and current source seam | Evidence / PRM sections |
| --- | --- | --- |
| MOV CR0/CR2/CR3, SMSW/LMSW, CLTS | Add system-register operands and effects, not generic GPR MOV lowering (`lift/mod.rs:2254`, `state.rs:296`) | P2 MEASURED for listed forms; §4.1.3, §4.2, Chapter 17 MOV/SMSW/LMSW/CLTS |
| LGDT/SGDT, LIDT/SIDT, LLDT/SLDT | Add table-register state and operand-size-aware loads/stores (`lift/mod.rs:2221`, `state.rs:296`) | P2 MEASURED; §5.1.2, §9.4, Chapter 17 LGDT/LIDT/LLDT/SGDT/SIDT/SLDT |
| LTR/STR, LAR/LSL, VERR/VERW, ARPL | Descriptor/TSS queries and privilege semantics (`lift/mod.rs:2221`, `lift/control.rs:1147`) | P2 MEASURED except ARPL; ARPL absence READ from dispatch; §4.2, §6.3, §7.3, Chapter 17 named entries |
| MOV/POP Sreg, LDS/LES/LSS/LFS/LGS; PUSH/POP FS/GS | Complete real386 and protected forms, with caches/limits and SS inhibition (`lift/mod.rs:2233`, `lift/mod.rs:4381`) | P2/P3 measure MOV DS/FS/GS differences. Remaining forms READ: LSS/LFS/LGS absent, LDS/LES specialized to real mode; §3.10, §5.1.4, Chapter 17 MOV/POP/LDS/LSS |
| Far JMP/CALL/RETF; IRET/IRETD | 16:32 targets, mixed descriptor sizes, same/inter-level returns (`lift/control.rs:839`, `lift/control.rs:1217`) | P2 MEASURED far32 and Real16 IRETD rejection; §6.3.4, §9.6, §16.4, Chapter 17 named entries |
| PUSHFD/POPFD, PUSHAD/POPAD; CPU identification flags | Add 386 real/USE16/USE32 forms without fixed CPL 3 restore rules (`lift/mod.rs:4235`, `lift/mod.rs:4252`, `lift/mod.rs:4360`, `state.rs:607`) | P2 MEASURED Real16 rejection; §3.1, §3.8, §4.1.1, §16.1, Chapter 17 PUSHF/POPF/PUSHA/POPA |
| CLI/STI, HLT, IN/OUT/INS/OUTS | Correct CPL/IOPL/TSS gating as applicable (HLT requires CPL 0) and 16/32-bit string port handshakes (`lift/mod.rs:4110`, `lift/mod.rs:4227`, `interp/control.rs:104`) | P3 measures missing Compat32 CLI. HLT and scalar ports already exist; INS/OUTS have no dispatch arms (READ). §6.3.5.1, §8.2–§8.3, §9.2, Chapter 17 named entries |
| 32-bit addressing, REP and stack/loop forms | Audit all implicit registers and segments; accepting 66h/67h on MOV is insufficient (`interp/control.rs:141`, `lift/mod.rs:4316`, `lift/mod.rs:5233`) | P2 MEASURED partial support and limit-check problem; §16.1–§16.3, Chapter 17 address/operand-size attributes |
| MOV DRn and debug behavior | New debug state/operands (`state.rs:296`, `lift/mod.rs:2254`); outside first integer qualification | READ; §12.1–§12.3, Chapter 17 MOV |

ASSUMED: configure the selected 386 personality so extender CPU-detection
code does not discover modern capabilities and enter a CR4/FXSR branch.
The legacy 286 path and existing Long64/Compat32 application contracts
must retain their existing behavior. Reusing common integer IR is sensible;
reusing fixed user-mode privilege assumptions is not.

## Machine gap list

1. **Extended RAM and its discovery.** READ:
   `crates/assembly/src/memory.rs:68` already has arbitrary address-space
   size and RAM regions; `assemble.rs:250` maps those regions; P5 measures
   a larger adapter map. The current BIOS preset is 1 MiB. Add an extended
   RAM region starting at `100000h`, preserve the conventional-memory/MMIO/
   ROM holes, and make CMOS and firmware reports agree. `assemble.rs:448`
   already derives extended KB, so avoid inventing a second memory-size
   authority. ASSUMED first profile: 16 MiB total physical extent, with
   15 MiB contiguous extended RAM, to keep AH=88h qualification bounded.

2. **A20 physical-address effect and board wiring.** READ:
   `crates/devices/src/i8042.rs:393`, `:505`, `:557`, `:703` already expose,
   change and snapshot the D0h/D1h output-port latch. No CPU-side gate input
   or port-92h device/configuration exists in the inspected assembly/device
   path (`crates/assembly/src/device.rs:224`, `assemble.rs:166`). Add port
   92h bit 1, preserve its distinct bit-0 reset semantics, and settle changes
   before the next guest instruction. A disabled gate clears **physical
   address bit 20**, not every high address bit and not the linear address
   before paging. Cover instruction fetch, CPU data and implicit page-table
   accesses, crossing transfers, and code-cache invalidation through aliases.
   Firmware/loader physical access and DMA need explicitly documented paths:
   ISA DMA is a separate bus master and must not accidentally go through
   CPU segmentation/paging. ASSUMED profile policy: OR the 8042 and fast-A20
   enable sources; choose and test reset latch values in that profile while
   preserving the existing profile's behavior. This wiring is board policy,
   not an Intel PRM CPU feature. Reference: **Ralf Brown's Interrupt List,
   Release 61**, PORT 0064h output-port commands and PORT 0092h, table
   P0415 ([port reference](https://github.com/cirosantilli/ralf-brown-interrupt-list/blob/master/inter61d/PORTS.A)).

3. **Firmware INT 15h contract.** READ:
   `vendor/kiselev-bios/README.md:1` identifies the current 8088/Xi8088
   firmware. Its absence of these AT services is an input from the worker
   brief, not a new runtime firmware measurement. Select/validate an
   AT/386-capable open BIOS through the parallel firmware work. Keep BIOS
   instructions running in the guest; do not add host INT 15h shims to the
   CPU adapter. Required service expectations, from **Ralf Brown's Interrupt
   List, Release 61**, INT 15/AH=87h/88h/89h, tables 00498–00501
   ([service reference](https://github.com/cirosantilli/ralf-brown-interrupt-list/blob/master/inter61a/INTERRUP.C)):

   | Service | Contract to qualify |
   | --- | --- |
   | AH=88h | AX reports contiguous extended KB from physical 1 MiB; CF/status indicate support/error. The initial profile reports its actual contiguous region, excluding holes; test zero and configured extended RAM |
   | AH=87h | ES:SI describes source/destination through a BIOS GDT work area; CX counts words (up to 8000h); test real copies across 1 MiB, CF/AH success/failure and A20 handling. This is a BIOS operation with protected-mode machinery, not merely a high-memory host memcpy |
   | AH=89h | ES:SI supplies the switch GDT including IDT/data/stack/code descriptors; BL/BH select PIC vector bases in multiples of eight. Success continues after the INT in protected mode with changed segments; test entry state and PIC/IDT consistency, plus A20 failure. It is not itself a DPMI implementation |

   ASSUMED: raw DOS/32A qualification uses its own PE switch; it does not
   require AH=89h merely because it is a DOS extender. Its inspected source
   needs memory discovery (AH=88h fallback) and A20. E820h/E801h can remain
   unsupported with a proper failure result if that fallback is verified.
   Qualify AH=87h/89h separately for the advertised BIOS compatibility
   contract; do not conflate those tests with the first extender milestone.

4. **A real 386 profile and reset choice.** READ:
   `crates/assembly/src/cpu.rs:13` only has an I80286 marker; `assemble.rs:251`
   always constructs Real16; `cpu.rs:47` selects the low `F000:FFF0` reset
   convention. Add a genuine core capability/model choice and a declarative
   profile with suitable RAM, firmware, cascaded PIC, PIT, keyboard/A20 and
   CMOS. [Intel 80386 PRM §10.2.3, First Instructions](https://pdos.csail.mit.edu/6.828/2018/readings/i386/s10_02.htm)
   starts reset fetch at physical `FFFFFFF0h` until the first far transfer.
   Proper reset needs the hidden CS/reset state and board ROM alias at the
   high address, or a clearly documented diagnostic-entry profile that
   bypasses hardware reset. `MemoryModel::Flat` would otherwise reserve a
   dense 4 GiB span; SoftMmu is unfinished. The first probe uses direct
   entry; it proves nothing about 386 reset/BIOS boot fidelity. Resolve sparse
   physical backing/high ROM decode with the CPU project before claiming a
   reset-accurate 386 profile.

5. **Protected interrupts through existing INTA.** READ:
   `crates/machine/src/lib.rs:440` already polls readiness, acknowledges the
   PIC and injects one vector, and `crates/cpu/src/x86jit_backend.rs:336`
   delegates readiness. Preserve that seam. The core must use IDTR/gates,
   not the machine decode an IDT itself. Add tests for guest PIC remapping,
   a timer IRQ into USE32 code, IRETD, IF and STI/SS inhibition, pending
   completion blocking and one acknowledgement per accepted request.
   A handler must explicitly EOI the actual PIC. Shutdown from failed #DF
   delivery needs a defined typed stop/reset policy, not a host panic.

6. **Deadline engine and 32-bit step accounting.** READ:
   `crates/assembly/src/assemble.rs:238` probes step support even for a
   zero-step run. `crates/machine/src/lib.rs:489` budgets to deadlines and
   charges completed steps, including error prefixes. Core Compat32 cannot
   currently pass this contract (P3). Extend the legacy-system interpreter's
   step API across mode switches, including 32-bit REP iteration boundaries,
   port/MMIO retry handoffs and IRQ readiness. Do not charge JIT blocks as
   instructions or substitute its optional execution counter. Preserve the
   existing distinction: whole retired instructions for diagnostics; completed
   instructions/REP iterations for scheduling; event delivery itself retires
   no instruction. Device time and halted PIT clocks stay under the existing
   deterministic engine. No new machine scheduler is needed.

7. **Snapshot of the new CPU state and replay boundaries.** READ:
   `CpuState` is Clone (`state.rs:295`), but vCPU mode, pending IRQ,
   retirement and shadows live outside it (`vm.rs:1367`, `:1415`, `:1432`).
   `crates/cpu/src/lib.rs:179` has no complete CPU snapshot API;
   `backlog/docs/architecture.md:151` explicitly describes a future machine
   snapshot. Add a versioned CPU image including CR0/2/3, GDTR/IDTR,
   LDTR/TR and their caches, six segment caches/selectors, CPL/EFLAGS and
   execution defaults, plus pending vector, inhibition, completion/retry
   state and counters. Include CPU model and effective A20 input; latch
   sources remain in device state. Page tables are guest RAM. Flush derived
   translations/IR caches on restore instead of serializing host pointers.
   Specify whether snapshots are allowed with an outstanding I/O completion;
   an initial explicit quiescent-boundary restriction is acceptable. Full
   machine replay also needs RAM, devices/PIC, last sampled IRQ levels,
   global/fractional clock, halted/idle state and config/firmware/core identity.
   This spike does not introduce a general disk-persistence framework.

## Proposed order

1. **Agree the CPU extension contract first.** Keep the pinned dependency
   model and append-only CpuState/jit_abi offsets. Introduce a 386 personality
   and mutable legacy architectural regime without changing the existing
   Long64/Compat32 application semantics. Plan a CPU-aware memory-access path;
   do not try to force protected USE16 through `Code16 == real-mode` checks.
2. **Make unpaged entry and return real.** Implement the needed 386 real
   forms, table registers, descriptor cache/validation, PE switching and
   same-CPL USE16/USE32 transfers. Extend exact step accounting at this stage,
   before enabling the machine profile. Acceptance: P1 without its paging
   suffix writes an expected marker and returns to a real-mode stub.
3. **Deliver protected faults and interrupts at one CPL.** Build IDT/gate/
   IRET handling and fault escalation before claiming restartable paging.
   Keep unsupported-feature exits distinct from architectural #UD.
4. **Add the guest page walker and precise #PF.** Qualify the complete P1
   remap, non-present and permission cases, cross-page fetch/data accesses,
   CR3 reload and physical alias SMC/MMIO. A TLB can wait.
5. **Integrate machine RAM/A20/profile and firmware in parallel as their
   dependencies arrive.** The larger RAM map, CMOS derivation and isolated
   port-92h tests can be prepared earlier. Hardware-reset/high-ROM qualification
   and firmware INT 15h tests have their own gate. Keep 286 goldens pinned.
6. **Qualify an open raw extender/client end to end.** First use a clean
   DOS boot, no EMS/V86 host, integer client, deterministic marker/exit back
   to DOS. Add TSS/ring/IOPL support where the selected client requires it;
   do not represent that choice as general DPMI compatibility. Test timer
   delivery and a real guest paging case as separate explicit obligations.
7. **Round-trip protected CPU state and replay.** Agree state extraction
   earlier, then test at protected and paged boundaries. Defer V86, task-gate
   switches, hardware debug and FPU qualification unless a measured guest
   dependency promotes them into the current scope.

ASSUMED engineering judgment: this is a substantial extension of the mode
and memory architecture, not a small instruction patch. The current design
**cannot host legacy protected mode without redesigning those seams**:
immutable vCPU mode, CPU-independent memory addressing, decode-width-based
segmentation, coupled stack width and user-only privilege semantics are
concrete evidence. There is no evidence that the entire core must be
replaced. An interpreter-first system path can reuse decoding, integer IR,
memory-region routing, SMC tracking and the existing machine traits while
keeping current JIT-generated application code unchanged. Performance of
that future path is unmeasured; this spike makes no schedule or throughput
promise for implementing it.

## x86jit requests

These are proposed narrow requests for the supervisor to submit, not
changes filed by this worker. **Every request** requires isolation tests,
full x86jit gates, append-only existing CpuState/jit_abi layout, unchanged
existing-mode generated code, and a tested seam merged whole before this
repository re-pins. Dependencies refer to this numbered list.

1. **Define 386 legacy-system state and execution-context contract.** Separate
   CPU generation, architectural real/protected regime, CS.D and SS.B from
   fixed application decode contexts; append system/descriptor state. Test
   context derivation and old ABI offsets. Dependency: none; establish the
   CPU-aware translation boundary for request 7 without adding paging yet.
2. **Complete 386 real-mode entry instruction forms.** Add PUSHFD/POPFD,
   PUSHAD/POPAD, FS/GS and 16:32 far forms; correct 67h limit/index behavior
   and CPU identification flags. Test the P2 forms against a 386 contract
   while retaining the 286 personality. Dependency: 1.
3. **Implement descriptor-table and control-register primitives.** Add
   LGDT/SGDT/LIDT/SIDT, SMSW/LMSW, MOV CR0/CR2/CR3 and CLTS with correct
   operand sizes and legal/illegal uses. Test state round trips and rejected
   PG-without-PE writes. Dependencies: 1–2; PE/PG execution effects must land
   with their consuming seams (4/7), not as successful no-ops.
4. **Execute same-CPL protected segmentation and PE transitions.** Add
   descriptor-backed segment loads, limits/types/presence and accessed bits,
   USE16/USE32 far JMP/CALL/RET/IRET, entry and return to real mode. Test
   hidden-cache persistence and DOS/32A-style mixed-width entry. Dependencies:
   1–3; fault generation included, guest IDT routing follows in 6.
5. **Extend exact step execution across legacy 16/32-bit modes.** Preserve
   counters, 32-bit REP iteration boundaries, fault prefixes, I/O completion
   and STI/SS inhibition across PE/CS changes. Test exact caps and no duplicated
   port/MMIO effects. Dependency: 4; keep the new system path interpreted.
6. **Deliver same-CPL protected IDT events and fault escalation.** Implement
   LIDT-based 16/32-bit interrupt/trap gates, INT/INTR/IRET, architectural
   error codes, #DF and typed shutdown. Remove the possibility of accidental
   IVT delivery in a legacy protected context. Dependencies: 4–5; test P3/P4
   counterparts and faults during gate/stack delivery.
7. **Implement 386 two-level guest paging and restartable page faults.** Add
   CPU-local CR3 translation for fetch/data/system accesses, P/U/S/R/W and A/D,
   CR2/#PF code, CR3 invalidation and physical alias/MMIO/SMC correctness.
   Dependencies: 1, 3–6; verify P1 remaps to `12345678h` and a handler can
   repair a non-present PTE then retry. No software TLB optimization required.
8. **Implement LDT and descriptor-query instructions.** Add LLDT/SLDT and
   LAR/LSL/VERR/VERW/ARPL, with table/privilege rules and descriptor faults.
   Dependencies: 4, 6; test an LDT-based client segment and DOS/32A's LAR use.
9. **Implement TSS privilege stacks and protected privilege transitions.**
   Add LTR/STR, cached TR/TSS and busy-bit handling, inward interrupt/call-gate
   stacks and outer IRET. Dependencies: 6, 8; test ring 3 to ring 0 and return,
   malformed TSS/stack descriptors and #TS/#SS/#NP. Hardware task switching is
   outside this request.
10. **Enforce legacy CPL/IOPL and TSS port permissions.** Add protected
    CLI/STI/HLT/POPF/IRET policies and scalar/string I/O permission checks
    before exits/side effects. Dependencies: 5, 9; test allowed/denied byte
    and wider ports and 32-bit INS/OUTS iteration accounting.
11. **Expose coherent physical A20 gating to the embedder.** Supply a
    core-facing gate control applied to CPU physical fetch/data/page-walk
    cycles, with split-access and cache/alias coherence; keep 8042/92h board
    policy in the machine. Dependencies: 1, 7; test both real and paged aliases.
12. **Support 386 reset fetch and sparse high-ROM mapping.** Represent
    reset CS hidden state through the first far transfer and offer a bounded
    physical backing/alias strategy for `FFFFFFF0h`. Dependency: 1; test
    high reset fetch and low-ROM continuation without allocating 4 GiB of RAM.
13. **Export and restore complete legacy CPU checkpoint state.** Include
    architectural caches and vCPU event/shadow/counter state, define completion
    boundary restrictions and flush derived caches on restore. Dependencies:
    5–11 for their final fields; test continuation equivalence across PE/PG
    boundaries. Persistent machine-file format remains the embedder's concern.

Conditional follow-up requests, **not initial-entry blockers**: hardware
task switches/task gates/NT returns (PRM §7.5–§7.6), V86 transitions and
sensitive operations (§15), DR/TF debugging (§12), and 386/387 control/error
integration (§11.1). Promote each separately only when guest evidence or an
explicit compatibility target requires it; do not bundle them into request 4.

## Implementation beads

Proposed machine-side beads only; no tracker was accessed or changed.
All implementation starts after the foundation/consolidation gates required
by the accepted project workflow. Each CPU dependency below means **landed
upstream, pinned here**, not a local patch under the Cargo cache. Firmware
selection/configuration remains owned by the parallel firmware/config work.

1. **Add a protected-mode CPU integration fixture and re-pin the core.**
   Dependencies: x86jit requests 1–7 and their upstream gates. Acceptance:
   an in-tree, reproducibly assembled entry fixture performs PE entry,
   protected segment loads, USE32 execution, P1's non-identity page mapping,
   a recoverable #PF and a return to real mode. Assert marker, CR2/error code,
   state and exact completed steps through an appropriate test API. Record
   CPU revision in golden identity; rerun existing slow goldens when the pin
   moves under supervisor authorization. Before evidence: P1/P2/P3 fail these
   requirements on the current pin.
2. **Add extended RAM to the 386 diagnostic configuration.** Dependencies:
   none for the data/model work; 1 for protected execution tests. Acceptance:
   configurable contiguous RAM above 1 MiB, preserved VGA/ROM holes, matching
   CMOS extended KB, no change to the existing 286 preset. An integration
   guest writes/reads extended RAM and a paging alias; host loader and DMA
   retain physical semantics. Before evidence: P5 proves mapping already
   works, while the current BIOS preset still provides no extended region.
3. **Add port 92h and a profile-owned A20 source policy.** Dependencies:
   none for isolated latch/device work; existing 8042 remains reused.
   Acceptance: MockBus tests cover bit-1 enable/readback, bit-0 reset request
   semantics, reset values and snapshots. Test all combinations of 8042 and
   fast-A20 sources under the chosen OR policy; controls use declarative
   device/config wiring. No CPU masking is claimed by latch tests alone.
4. **Wire effective A20 into CPU physical accesses.** Dependencies: 2–3,
   x86jit request 11. Acceptance: assembled guest toggles D1h and 92h;
   `FFFF:0030` distinguishes/aliases `100020h` and `20h` correctly; repeat
   with paging mapping a linear page onto an A20-sensitive physical page.
   Test split fetch/read/write, code-alias invalidation, gate changes at
   instruction boundaries and documented DMA/reset behavior. Before evidence:
   P5 reads `77h` after clearing the 8042 latch.
5. **Expose the 386 model and reset profile through assembly.** Dependencies:
   1–4, x86jit request 12, firmware/config selection result. Acceptance:
   validated CPU/model choice rather than an I80286 label, 16 MiB reference
   map and expected device roles, reset at the correct high ROM address then
   far transfer to low ROM. Missing/mismatched firmware is rejected clearly.
   Diagnostic direct entry remains explicit. Existing 286 reset and goldens
   remain unchanged.
6. **Qualify the selected BIOS INT 15h services.** Dependencies: 4–5,
   parallel firmware result. Acceptance: guest integration tests for AH=88h
   memory size/error reporting, AH=87h descriptor-based high-memory copy and
   AH=89h entry/PIC/segment state; capture CF/AH and A20 outcomes. Unsupported
   E801h/E820h return errors that permit the tested AH=88h fallback. These
   tests execute firmware and produce pinned golden evidence; no adapter
   interrupt shim.
7. **Qualify protected timer IRQs through PIC INTA.** Dependencies: 1,
   x86jit request 6; request 9 if the chosen fixture changes CPL. Acceptance:
   guest remaps PICs, loads IDT, handles a PIT IRQ in USE32 code, EOIs and
   IRETDs. Assert one acknowledgement, correct frame, resume IP, IF/shadows,
   no premature interrupt during incomplete port/MMIO access, and a defined
   shutdown result when delivery fails irrecoverably. Before evidence:
   P3 readiness refuses the mode and P4 injects a real-mode frame.
8. **Extend scheduler equivalence fixtures to mixed and paged 32-bit code.**
   Dependencies: 1, 7, x86jit request 5; request 10 for INS/OUTS fixtures.
   Acceptance: existing Deadline and test-only PerClock engines agree on
   guest registers/RAM/device state, steps, PIT clocks and IRQ order across
   PE/PG changes, 32-bit REP, deferred I/O, HLT and bounded caps. Vary run
   chunk sizes; completed stopping accesses are charged once and faults
   preserve the completed prefix. No wall clock or block-count timing.
9. **Add protected CPU checkpoint capture and restore qualification.**
   Dependencies: 1, 4, 7–8, x86jit request 13. Acceptance: versioned CPU
   image round-trips hidden caches, CR/table/task state, mode/default sizes,
   flags, A20 and pending/shadow/counter state. At the documented legal
   boundary, snapshot/restore plus the test-owned RAM/device/clock image
   reproduces continuation exactly; changed GDT bytes do not overwrite a
   cached segment during restore. Reject incompatible core/profile versions
   and illegal in-flight completion boundaries atomically. Do not expand
   this bead into frontend snapshot UI or disk persistence.
10. **Run an open DOS extender integer client end to end.** Dependencies:
    5–8, firmware memory-discovery qualification from 6, and x86jit requests
    8–10 if required by the selected raw host/client. Acceptance: reproducible,
    license-documented extender/client fixture, clean DOS boot without an
    EMS/V86 host, measured PE/USE32 entry, extended-RAM operation, timer IRQ,
    deterministic output/exit back to DOS and golden identity pinned to CPU,
    firmware, media and config. Explicitly verify whether this extender path
    enables paging; if not, the separate paged integration fixture from 1
    remains mandatory. List any remaining guest-triggered unsupported
    instructions/faults rather than calling one successful client general
    DPMI/386 compatibility. Full V86/task/FPU qualification is separate work.
