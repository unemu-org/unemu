# Deterministic device scheduler

Design spike, 2026-11-09. Status: proposed; no scheduler or CPU implementation
is included. CPU source inspected at `b0c9ae52820dffc1e2cb75c4928a670788a916d0`
(`b0c9ae5`). This document proposes an extension to
[decision-5](../../decisions/decision-5%20-%20Virtual%20clock%20in%20retired%20instructions.md),
and changes to the boundary rules in
[decision-4](../../decisions/decision-4%20-%20Hardware%20IRQ%20injection%20contract.md)
and the single-clock implementation in
[decision-14](../../decisions/decision-14%20-%20Virtual%20time%20advances%20while%20the%20CPU%20is%20halted.md).
Acceptance of the proposed decision below is required before those rules change.

## Answer and limits of the hypothesis

Replace the fixed ratio and global per-clock sweep with a rational instruction
clock, independent device clock domains, conservative next-event deadlines, and
CPU execution bounded by **retired instructions**. Service IRQs at architectural
instruction boundaries, acknowledging the PIC only when the CPU can accept the
interrupt immediately and the stopping instruction's bus effects have completed.
Keep a per-PIT-clock fallback for devices not yet migrated.

This makes timing correct **within an explicitly instruction-timed model**:
device periods do not depend on batching, and interrupt acceptance does not
depend on translated blocks. It does not model instruction cycle counts, ISA
bus cycles, DMA contention, or physical interrupt-entry latency. A faster CPU
profile means more retired instructions per unit of device time, not a claim
that every instruction takes one physical CPU cycle.

Two qualifications to the hypothesis matter:

- A deadline inside an instruction cannot stop that instruction halfway through
  in this model. Device transitions retain their scheduled timestamps; the CPU
  observes their effects at the first instruction boundary at or after the
  deadline. Execution must never pass that boundary before servicing them.
- Same effective CPU speed does **not** imply identical old goldens when IRQ
  latency and INTA ordering are corrected. An optimization-only mode must be
  identical. The corrected mode needs a separate, reviewed behavioral gate.
  Preserving block-dependent IRQ latency would defeat the purpose of the change.

Throughput is secondary. The retained
[machine-loop measurements](machine-time-profile.md#results) attribute
86–89% of real DOS workload wall time to the CPU and 11–13% to clock work;
the BIOS INT 19h spin is the outlier with roughly half in clock work. Those are
READ evidence from that experiment, not measurements repeated in this spike.
Its optimistic ceilings from removing the entire clock phase are not scheduler
speedup predictions. More frequent CPU returns can cost more than skipped ticks
save, especially while legacy devices still request every PIT boundary.

## Current contracts and gaps — READ evidence

Paths and line numbers in this section refer to the inspected source before
implementation of this proposal.

| Source | Observed behavior | Implication |
|---|---|---|
| `crates/machine/src/lib.rs:61`, `:398` | `RETIRED_PER_PIT_CLOCK = 1`; `advance_clock` carries a remainder but divides by the constant | CPU speed is fixed inside the machine |
| `crates/assembly/src/clock.rs:16`, `crates/assembly/src/assemble.rs:183` | The config declares `retired_per_pit_clock`, but assembly passes only `quantum` to the machine | Changing the ratio has no effect; zero is not validated |
| `crates/cpu/src/lib.rs:138`, `crates/cpu/src/x86jit_backend.rs:202` | `Cpu::run` forwards a **block** budget to x86jit | Neither deadline nor cap can be bounded by that number of instructions |
| `crates/machine/src/lib.rs:342` | CPU runs first; only then are elapsed PIT clocks replayed | Device edges can be latched after the CPU has passed their eligible acceptance boundaries |
| `crates/machine/src/lib.rs:347`, `:532` | Advance clocks, acknowledge/inject, then dispatch the stopping OUT/IN/MMIO | OUT-before-INTA is violated; queue emptiness substitutes for CPU acceptance readiness |
| `crates/machine/src/lib.rs:310`, `:255` | HLT advances one PIT clock at a time; virtual time includes halted clocks | Preserve the halted contribution and cap semantics when skipping idle time |
| `crates/machine/src/lib.rs:434`, `crates/bus/src/lib.rs:268` | Every PIT clock ticks every device and samples wired IRQ levels | A large `tick_all(now)` would lose short pulses |
| x86jit `x86jit-core/src/vm.rs:1889`, `:1963` | Budget tests count blocks; IRQ readiness is checked between interpreted blocks | A retirement counter alone does not provide an instruction-exact stopping point |
| x86jit `x86jit-core/src/vm.rs:1618` | `step_instruction` exists, but explicitly does not resume deferred accesses as `run` does | A machine-side loop around this function is not a complete scheduler seam |
| x86jit `x86jit-core/src/interp/mod.rs:295`, `crates/cli/tests/quantum.rs:48` | A self-jump `JMP $` is deliberately not counted as retired; the cap test avoids it | Exact retirement accounting must fix this, or that guest defeats a time cap |

The x86jit references above are to the dependency's **own** source tree at the
pinned revision; no dependency files were modified. Its `executed` counter is
not a replacement for the Real16 retired-instruction contract.

## Time model

### Units and configuration

Use a new scheduler type, provisionally `VInstant(u128)`, with reset at zero.
One PIT input period contains `S = 2^32` base units. A PIT edge at input count
`k` is exactly `k * S`; do not use a rounded nanosecond period. A 128-bit
timestamp avoids the short horizon a 64-bit count of such fine units would have.
All addition, multiplication, conversion, and deadline arithmetic is checked.
Exhausting a supported range returns a deterministic error, never wraps time.

Keep today's `VClock(u64)` meaning **PIT input counts** during migration. Do not
silently reinterpret it as the finer unit. Legacy trace comparison projects
`VInstant` to `floor(now / S)`; a later versioned trace format records full
precision. Both formats identify their units. The old wrapping `VClock::advance`
is not the scheduler's arithmetic policy.

`ClockConfig` should describe:

- `cpu_rate = A / B`, positive reduced rational **retired instructions per PIT
  period**. The compatibility default is `1 / 1`. An existing positive
  `retired_per_pit_clock = r` converts to `r / 1`; the machine must actually
  receive it. A rational field also permits slower fractional profiles.
- The modeled PIT frequency as a positive rational Hz value, initially
  `1_193_182 / 1` for conversion to existing RTC seconds. This is the current
  rounded modeling convention, not a newly asserted crystal specification.
- Per-device clock-domain configuration and phase origins, constructed by the
  assembler and passed through generic device/scheduler seams.
- An explicit temporary `LegacyBoundaries` versus `InstructionDeadlines` mode
  during migration. The former alone retains a block quantum as guest timing.

In the corrected mode, a positive finite `max_instructions_per_call` is a host
cooperation bound. It may shorten a call but cannot delay events, an IRQ window,
or a policy target. Test values including 1, 64, and 65,536 for equivalent state.
Move this cooperation bound to run policy when the legacy mode is removed;
update the contrary statements in `design/machine-config.md` and
`assembly/src/run.rs` at that step. Do not keep calling a block budget an
instruction budget, or silently change the unit of the existing field.

CPU speed is a modeling ratio. Profiles for later CPU generations can select
different ratios without changing device frequencies, but that does not enable
386/Pentium ISA behavior or give a measured MHz rating. CPU semantic profiles,
new audio devices, and wall pacing are outside this implementation.

### Conversions, rounding, and phase carry

The CPU instruction period is `S * B / A` base units. A device domain at
`f_device` Hz has period `S * f_pit / f_device`, reduced to a rational `N / D`.
For example, RTC oscillator and audio sample domains can use 32,768, 44,100, or
48,000 Hz independently of the CPU ratio. No least-common-multiple of all future
frequencies is required.

Represent a domain's exact phase by an integral floor and remainder
`0 <= r < D`. Round an event's absolute timestamp **up** to the first base unit
at or after its exact phase:

```text
event(k) = epoch + ceil(k * N / D)

x       = r + n * N
delta   = floor(x / D) + (x % D != 0) - (r != 0)
r_next  = x % D
```

The second formula advances an already upward-rounded timestamp by `n` periods.
Carry `r_next`; do not repeatedly add `ceil(N / D)`. Compute division ceilings
using quotient and nonzero remainder rather than unchecked `x + D - 1`.
Splitting 65,536 instructions into arbitrary calls then produces the same time
as one call. Rounding error is less than one base unit relative to the exact
domain phase and does not grow with event count. Equal rounded deadlines use
the stable ordering below. Validate that each configured base period is at
least one base unit and all reduced intermediates fit the checked arithmetic;
reject unsupported ratios instead of creating zero-duration CPU instructions.

PIT counter division happens in the PIT domain, not by separately rounding the
counter's frequency. RTC divider stages similarly count oscillator edges.
Domains keep their phase when unrelated devices are accessed or the CPU stops.
A register write restarts a divider only when the device's hardware model says
so. Scheduling the next period from the late **observed** CPU boundary would
introduce drift and is forbidden.

Running time advances only for completed guest instructions. IRQ entry itself
has no retired charge. During HLT, advance the scheduler instant directly to
the next event or bound, without inventing instructions; retain the CPU's
fractional instruction phase for resumption. An integer halted interval shifts
the CPU's time origin and does not consume that phase remainder.

Preserve the policy API's instruction-equivalent counter separately:

```text
virtual_time = retired + floor(halted_base_units * A / (S * B))
```

Carry the fractional halted contribution across every idle jump and runner
call; never sum rounded per-jump contributions. At the default speed with
whole-PIT idle steps this is exactly `retired + halted_pit_clocks`, as today.
Other ratios give a documented generalization, including fractional halted
instruction equivalents. Keep `retired` available as an independent CPU count.
Do not substitute `floor(now * A / (S * B))`: independent rounding of active and
halted time can disagree with the actual number retired.

Freeze a machine's timing configuration after reset. Runtime speed changes
would require explicit epochs and a new policy for cap-unit conversion; they
are not implied by configurable construction-time profiles.

### Deadlines versus instruction boundaries

Given an event deadline `d > now`, find the smallest positive instruction count
`n_event` whose rounded CPU completion time is at least `d`, using the same
phase state as forward conversion. Checked integer inequality inversion, or a
bounded monotone search, suffices; a floating-point reciprocal does not.
Run at most that count, further limited by cooperation and policy caps.

If no instruction completes exactly at `d`, the last instruction straddles it.
After its completion, process **every** device transition due through that
boundary in chronological order before executing another instruction. In
particular, a low and a subsequent high within one slow instruction must both
reach edge detection. Do not merely sample the final level. IRQ acceptance
lateness is less than one modeled instruction interval when IF and the
architectural interrupt gates are open. Masking and shadows may extend it.

RAM effects of the straddling CPU instruction precede deferred DMA memory
effects at that observation boundary. This is an explicit atomic-instruction
ordering convention, not sub-instruction DMA arbitration. Bus accesses use the
instruction-start timestamp described below; interrupt acceptance uses its
completed boundary. This convention also covers instructions that fault after
an access without charging time for a retirement that did not happen.

## Device and bus contracts

### An additive migration API

A bare `next_event() -> Option<...>` default cannot distinguish an unported
device from a device that is genuinely dormant. Use a small explicit enum:

```rust
enum NextEvent {
    EveryPitClock,
    At(VInstant),
    Dormant,
}

// Additions to Device; signatures are a design sketch.
fn next_event(&self, now: VInstant) -> NextEvent {
    NextEvent::EveryPitClock
}

fn advance_to(&mut self, now: VInstant) {
    self.tick(pit_projection(now));
}
```

`EveryPitClock` schedules the **next absolute PIT boundary**, not `now + S`.
For this variant call the existing `tick(VClock)` exactly once at each such
boundary, in existing device order. Its default is conservative, preserves
call-sensitive devices, and keeps unit-test/mock implementations working.
It deliberately prevents skipping those PIT boundaries until the device opts in.

An opted-in scheduled device overrides `advance_to` and promises:

1. `At(t)` is the earliest future autonomous transition that the machine must
   observe: IRQ level change, DMA request, output/sample, or another scheduled
   state transition. Earlier conservative deadlines are permitted; late ones
   are not. A pending PIT reload is an example of useful conservative scheduling.
2. `Dormant` means no autonomous event until a write, read side effect, external
   stimulus, reset, or coupled input changes the state. It does not mean that
   time freezes or an unlatched counter cannot be read later.
3. `advance_to(now)` advances elapsed state, is idempotent at the same instant,
   and supports lazy synchronization before reads/writes. It cannot skip a
   promised observable transition. The machine dispatches such transitions at
   their individual deadlines, including when several precede one CPU boundary.
4. After due transitions are settled, `At(t)` must satisfy `t > now`. Querying
   does not mutate state or consume an event. Immediate consequences of a bus
   operation are settled at its current timestamp, then a future deadline is
   returned. An unchanged due-now deadline is a contract error, not a busy loop.

Remove per-clock loops inside an opted-in device only after comparing its
arithmetic advancement to its original single-edge implementation. A PIT
`next_event` wrapper around today's `tick` still loops over every elapsed input
clock internally; that reduces visits to other devices but is not yet a fast
PIT implementation.

### Access synchronization and invalidation

Separate **access timestamps** from timer advancement. Add an explicit
`DeviceAccessContext { now: VInstant, pit_clock: VClock }` to bus-dispatched
port/MMIO access methods and update trace-producing implementations to use it.
The bus stamps open-bus traces from the same context. Initially their existing
`TraceEvent::at` stores the PIT projection for A/B comparison; full precision
requires a versioned format. Never call an old device's `tick` merely to update
a trace timestamp: `I8042::tick` changes state even when elapsed time is zero.

Before a scheduled device access, drain global due events, then synchronize the
target with `advance_to(access_time)` so a PIT count or RTC register reads its
current value. A legacy target has already received all PIT ticks and needs no
extra advancement. Apply the access and its completion, sample changed IRQ
levels, invalidate affected deadlines, and settle immediate work. Reads can
invalidate deadlines too: consuming an 8042 byte and clearing RTC register C
are important examples. Split byte-wide ISA accesses retain bus byte order.

Also invalidate after DMA completion, controller changes, external input,
coupled signals, reset, and restore. The existing mutable-device escape hatch
must become a time-stamped mutation operation or use a guard that invalidates
on release; otherwise `push_scancode` can leave a cached `Dormant` entry forever.
The machine still knows devices only by stable wiring identity. Assembly owns
clock-domain and signal wiring; no PIT/RTC downcast enters the machine loop.

Start with one deadline slot per device and a stable linear minimum scan: there
are only 10–11 devices in the measured machines. Requery after mutations; do not
query every device on every PIT clock. A heap with generation-tagged stale-entry
rejection is justified only by a measured larger device set. Stable device IDs
and wiring order, never hash-map iteration, define equal-time order.

### Device migration inventory

| Device and current source | Required scheduling behavior | Fidelity boundary |
|---|---|---|
| PIT, `pit8253.rs:275`, `:486` | Schedule load edges and both OUT transitions; lazily synchronize all three counters for count reads/latches; include any routed channel-1/2 outputs | Preserve currently implemented modes and count wrapping before fixing other hardware deviations |
| RTC, `cmos_rtc.rs:165`, `:669` | Schedule UIP entry and second/update/alarm boundaries; synchronize before register access, reschedule on SET and timing writes; sample register-C clear immediately | Existing second length and UIP window remain the compatibility model; 32.768 kHz/divider fidelity is a later explicit model change |
| Keyboard, `i8042.rs:341`, `:492` | Preserve hold at the first PIT boundary after data read and refill at the second when enabled/queued; reschedule on data read, command-byte write, enable, and host injection | Two modeled PIT steps are an edge-visibility policy, not a measured keyboard serial baud rate |
| FDC, `upd765.rs:65`, `:882` | `Dormant` between command/access/DMA work; service request and completion at the instruction boundary; timestamp accesses directly | Seek/read/write timing remains immediate; no rotational-latency feature is added |
| DMA, `dma8237.rs:466` | `Dormant`; channel programming invalidates affected transfers; completion can create immediate IRQ work | Whole-transfer service is retained; arbitration, per-byte clocks, and CPU stalls are deferred |
| System control, `sys_control.rs:124`, `:203` | Initially `Dormant` for autonomous work, retain per-read bit-4/5 phases and time-stamp accesses | Real refresh/PIT2 coupling is a separate hardware correction, not achieved by a scheduler alone |
| PIC/shared PIC, UART, POST, VGA | Opt into `Dormant` only after confirming tick merely stamps time; route access/cascade effects immediately | UART serialization and VGA retrace clocks remain absent; VGA's existing per-read status phase stays unchanged |

For PIT Mode 2, the one-clock low pulse and following rising edge must both be
scheduled. Mode 3 needs both half-cycle edges, including odd divisors and the
one-clock load delay. These requirements follow the Intel **8254 Programmable
Interval Timer** datasheet, “Mode 2: Rate Generator” and “Mode 3: Square Wave
Mode,” pp. 10–13 of the
[datasheet](https://www.scs.stanford.edu/10wi-cs140/pintos/specs/8254.pdf).
An implementation must not return only the next rising IRQ edge while leaving
the machine's remembered level high. Reset initializes remembered line levels
from actual reset pins without synthesizing a rising edge; subsequent
programming-induced changes are sampled immediately. Multiple edges while a
PIC request is already latched still obey its IRR coalescing rules.

Do not derive untested arithmetic for every PIT mode in one change. Compare
Modes 0, 2, and 3 against the reference stepper, including zero-as-65,536 and
odd/even divisors. Keep conservative per-clock scheduling for unsupported modes
and pathological small divisors until their present behavior is characterized.
Lazy synchronization must preserve latch and low/high-byte phase semantics.

The Motorola **MC146818 Real-Time Clock Plus RAM**, “Update Cycle,” “Registers
A–C,” and Table 6, defines oscillator/divider behavior and the UIP/update
relationship; see the [manufacturer datasheet](https://pccomponents.com/datasheets/MOTO-MC146818.PDF).
The present `1_193`-PIT-tick UIP window and frozen SET accumulation are emulator
approximations, not proof of that hardware timing. Scheduling must preserve
them during A/B; modeling real divider/reset/periodic behavior needs separate
device tests and reviewed golden changes.

For a later accurate system-control model, route PIT1 refresh requests and PIT2
OUT/GATE through declared signal wiring. IBM **Personal Computer AT Technical
Reference**, “System Timers,” p. 1-22, describes channel 1 refresh and channel 2
speaker wiring; see the [1985 reference](https://www.minuszerodegrees.net/manuals/IBM_5170_Technical_Reference_6280070_SEP85.pdf).
Do not invent a free-running toggle frequency from how often software reads
port `0x61`. The source comment claiming that an entire polling loop runs
without clock advancement is inconsistent with the inspected machine path:
each IN already causes a CPU exit. Fix that explanation when that device is
actually changed; it does not justify changing the compatibility behavior here.

## Interrupt acceptance and instruction transactions

### PIC state must follow CPU acceptance

Separate three things: device IRQ line/edge, PIC pending request (INTR), and CPU
acceptance (INTA followed by IVT entry). A PIC request can remain pending while
IF is clear. Do not move IRR to ISR, resolve a permanent vector, or populate a
CPU queue until the CPU reports an eligible architectural boundary.

The readiness predicate belongs in the CPU core. It includes IF, the STI
shadow, any modeled MOV/POP SS inhibition, no unfinished port/MMIO/retry
instruction, and no earlier acceptance in flight. `interrupts_enabled()` and
`!has_pending_irq()` together are insufficient. The hardware distinction
between IF and acknowledgement is described in Intel **80286 and 80287
Programmer's Reference Manual** (1987), §3.11.2, §5.2 and §9.2; see the
[manual](https://bitsavers.trailing-edge.com/components/intel/80286/210498-005_80286_and_80287_Programmers_Reference_Manual_1987.pdf).

At a ready boundary, poll the PIC, acknowledge its **current** winner, and enter
that vector before any next guest instruction. These operations form a
single-threaded transaction. An atomic core acceptance method must never
silently enqueue/defer a vector after readiness was established. IVT/stack
delivery faults may surface as deterministic CPU errors after INTA; do not undo
PIC history or run guest code past them. Model physical multi-cycle INTA and
spurious-vector races only if a later bus model requires them.

When PIC INTR is pending but CPU readiness is false, ask the CPU to return at
the **first** eligible instruction boundary, or use one-instruction execution
until that seam exists. Checking only at the next device deadline is too late:
`STI; NOP; long_non_io_sequence` must accept after NOP even if no timer changes
again for thousands of instructions. POPF/IRET enabling IF require the same
observation, subject to the core's architectural rules. The STI shadow is
consumed by the following completed instruction, never by one whole block,
one CPU call, an I/O retry, a clock tick, or an interrupt entry.

For `STI; HLT`, HLT itself completes that following instruction. A pending
eligible IRQ then wakes at the boundary after HLT without executing the
post-HLT instruction first. IF-clear HLT returns `Halted` immediately, as today;
NMI/reset wakeups are outside current scope. Do not import later CPU inhibition
rules by guessing them in the machine: expose the gates the Real16 core models
and test their behavior there.

### OUT-before-INTA and completion ordering

At each CPU observation boundary, use this total order:

1. Account completed instructions **before** a stopping bus instruction and
   settle device deadlines through its start instant in
   `(deadline, stable_device_order)` order. After **each** transition
   propagate/sample its IRQ or coupled output, including falling edges.
2. At that start instant synchronize the bus target, perform all OUT/IN/MMIO
   transfers of the stopping instruction, and finish its completion handshakes.
   Sample resulting line changes immediately, including the low level after
   reading an 8042 byte. Never accept an IRQ inside this transaction.
3. Once that instruction completes, charge its one retired interval and settle
   due events through its completion instant. Without a bus stop, charge the
   returned retired delta and settle events through its completion directly.
   Drain DMA work in declared wiring order through `DmaMemory`/`note_write`,
   deliver completions, propagate outputs, and settle immediate follow-on work.
   Recompute affected deadlines. This preserves immediate FDC transfers before
   another guest instruction; it does not assign a physical DMA bus latency.
4. Finish due policy inputs in stable order after the completed CPU bus effect
   and before interrupt selection. A policy input becoming due inside an
   instruction waits for this boundary. Prompt-based input is still decided
   only at the policy's specified observation target.
5. Return for a reached hard cap or explicit policy observation target after
   settling that boundary, without accepting a new IRQ. Otherwise, if the CPU
   is eligible and PIC INTR is asserted, perform INTA and immediate CPU entry.
   If halted, this acceptance clears the machine's halted state and idle span.

Thus `OUT PIC_IMR, mask` can suppress an IRQ that would otherwise win at the
same completion boundary. `OUT PIC_COMMAND, EOI` clears the old ISR before
selecting the next request. Clock events at the access's **start** timestamp
precede the write; edges during its retired interval follow the write. A timer
load is eligible on the first PIT input edge strictly after the write. These
are deterministic tie conventions, not a claim of bus-cycle fidelity.
The open ordering issue `upc-kk2.8` is covered here; its fix must not be obscured
inside a tick-performance change.

The current CPU stop kinds need explicit transaction handling. OUT advances
IP and self-retires before its side effect reaches the bus; IN also advances
IP and awaits a value. MMIO can stop **before retirement**, possibly several
times for one instruction. Use one common convention: every externally serviced
access is timestamped at the **start of its architectural instruction**, after
the preceding completed instructions. Interrupt acceptance follows completion.

For a self-retired OUT/IN, account the returned prefix excluding that final
instruction, service the bus at the prefix timestamp, then charge its one
instruction interval. For pre-retirement MMIO, account the completed prefix,
service transfers at that same start instant, and resume **only that pending
instruction** until it retires or faults. Charge one interval only on successful
retirement. Completion cannot execute the next instruction. The core must
identify the prefix, access owner, and retirement phase; a guessed
`retired_delta == 0` is not enough. Intermediate CPU state is not exposed to the
embedding policy while this transaction is unfinished.

Repeated transfers retain order and the same start timestamp. A fault has no
retirement charge; completed bus side effects remain at the start timestamp.
This avoids advancing devices to a speculative end time that cannot be rolled
back if an instruction faults after consuming a device value. Reserving a
future **time** slot and ticking devices before that fault would violate the
retired-only running-time rule. Reserve budget room for completion, not device
time. Repeated zero-retire faults/requests need the progress guard below.

An input therefore samples the device at instruction start even if its
instruction spans an autonomous transition. This is an explicit approximation,
applied equally to IN, OUT, and MMIO; physical intra-instruction access timing
needs a cycle/bus model. It changes current end-of-run access timestamps in
corrected mode and requires the reviewed golden gate. The legacy comparison
mode retains current timestamps/order. This contract preserves exactly-once
completion without adding unsupported MMIO read-modify-write behavior.

## Run loop, HLT, and bounds

The following sketch uses helpers with the ordering above. Scheduler `now` and
CPU retirement state are consistent whenever a completed boundary is exposed
to the caller. A pending instruction transaction is internal to the loop.

```text
repeat:
    settle_current_boundary()  # due events, access/completion, DMA, policy input
    if reached_cap_or_policy_target(): return CapOrPolicyBoundary
    if cpu.irq_acceptance_ready() and pic.poll() is not None:
        vector = pic.acknowledge()
        cpu.accept_irq_now(vector)  # no guest instruction, no retired charge
        halted = false
        idle_elapsed = 0

    if halted:
        if not cpu.interrupts_enabled(): return Halted
        if idle_limit is None or idle_elapsed >= idle_limit: return Halted
        d = min(next_device_event, next_policy_input,
                idle_expiry, halted_cap_instant, next_policy_observation)
        if d is infinite: return Halted
        advance_halted_time_to(d)  # retain CPU phase and idle-cap remainder
        dispatch_each_event_through(d)
        continue                  # check cap, then IRQ, before idle expiry

    d = min(next_device_event, next_policy_input, next_policy_observation)
    n_event = first_completion_at_or_after(d)  # infinite if no event
    n_cap = cap - virtual_time                # exact active instruction room
    n_policy = policy_target - virtual_time   # if an instruction-unit target exists
    n = min(n_event, n_cap, n_policy, max_instructions_per_call)
    stop = cpu.run_retired(n, stop_when_irq_ready = pic.poll().is_some())
    if stop is bus access:
        service_instruction_transaction(stop) # prefix, start-time access, completion
    else:
        advance_active_time_by(stop.retired_delta)
        dispatch_each_event_through(cpu_boundary_time)
    if stop is HLT: halted = true
    handle_fault_or_progress_yield(stop)
```

`n_cap` and a pending policy observation also limit starting an instruction
transaction; zero room returns before starting one. An IRQ-window stop can
return earlier than `n`. A non-budget exit takes precedence over budget
exhaustion at the same boundary, so an OUT or HLT that uses the final slot is
still serviced exactly once. No instruction after the chosen boundary may
execute speculatively and become guest-visible.

For a policy time target in instruction equivalents, active execution uses
the same integer room as the hard cap. For an absolute device-time target,
use the same first-boundary conversion as a device deadline. Do not let a
cooperation yield trigger console polling or stimuli earlier than the declared
policy target; extra host yields must be invisible to the guest.

With halted fractional credit `h` in units with denominator `S * B`, the first
base instant reaching `m` remaining instruction equivalents is at
`ceil((m * S * B - h) / A)` base units from now. Compute this with checked
arithmetic. Include it in the minimum instead of jumping past a cap to a timer.
When halted, idle limits remain durations in PIT periods from halt/wake state;
retain elapsed fractional time across calls. A finer event can wake before one
whole PIT period passes. `None`/zero still allows an immediately acceptable IRQ
but advances no idle time. An eligible IRQ at idle expiry wins over the idle
stop; a hard cap wins over starting new IRQ entry at that same instant.

If all devices are dormant, a halted machine with a finite bound advances only
to the earliest bound, then returns; it does not invent periodic ticks. If a
legacy device remains, its next PIT boundary is still a real scheduled event.
A future deadline is not proof that HLT will wake: it may be a falling timer
edge, a masked request, or RTC housekeeping. Continue to the next event until
acceptance, idle limit, or cap. On return and reentry preserve the halted state,
line levels, domain remainders, idle span, and deadline state.

While draining a late batch, distinguish the CPU observation endpoint from the
event cursor. Advance each due device to its own scheduled timestamp and query
its next event relative to that cursor, then continue until the endpoint is
settled. Passing the already-late CPU time to every query would hide intermediate
transitions or incorrectly report a valid next edge as an expired deadline.

Bound host cooperation and detect lack of progress without using wall time:

- Finite instruction batches bound an I/O-free running guest. Correctly count
  `JMP $`; it is a completed instruction even though IP is unchanged.
- Impose a deterministic maximum on immediate transitions/completion attempts
  at one timestamp. A device returning the same expired deadline or a retry
  that makes no progress produces a diagnostic error, not a made-up clock tick.
- The CPU needs a deterministic work-yield for an unbounded chain of fault
  deliveries with zero retirement. The instruction budget alone cannot bound
  that host work. Such a yield does not age devices or consume STI shadow.
- A time cap bounds **virtual** time, not host execution cost. Do not infer a
  host runtime guarantee, particularly for one long REP instruction.

At a hard cap, settle transitions and stopping-instruction effects through the
final instant, then return. During active execution the instruction-equivalent
cap is exact; halted rounding is less than one base unit. A physical-time
target during active execution can land on the first boundary after it. This
tightens today's quantum-sized overshoot and can itself change policy/golden
endpoints. No events later than the final observation instant are drained.

## Determinism and future consumers

The execution state consists of CPU architectural state/retirement, scheduler
instant, rational phases, retired/idle cap credit, halted/idle state, device
states, remembered IRQ levels, stable wiring order, and timestamped input
cursor. Reset/replay must reconstruct all of it. Future snapshot work must
include phases and pending instruction transactions, or restrict snapshots to
settled boundaries; implementing machine snapshots remains deferred.

All event ordering is integer and explicit. No wall clock, RNG, asynchronous
input arrival, thread scheduling, collection hash order, or host batch size
decides guest-visible behavior. Record external inputs with virtual timestamps
and an ordering index. Repeated runs with the same pin, config, model version,
fixtures, and input schedule have the same output and timing state.

Audio rendering can schedule samples in its own rational domain and buffer them
for the frontend; host playback cannot clock the machine. Wall pacing belongs
in a frontend wrapper comparing a returned virtual instant with elapsed host
time, sleeping or dropping presentation work there. It never changes the
configured ratio, skips device events, or feeds elapsed wall time back into
device advancement. Interactive live input is recorded at explicit virtual
boundaries for replay; deterministic tests supply the record directly.

## Alternatives considered

| Alternative | Where it wins | Where it loses / disposition |
|---|---|---|
| Keep single-clock device ticks, batch the outer loop | Lowest migration risk; reference oracle for device transitions; can hoist empty-device visits | Does not repair block-dependent CPU latency or eager INTA. Retain as reference and default adapter, not final timing architecture |
| Jump to next event using the existing block budget | Small apparent implementation; HLT skipping can already be correct with conservative device deadlines | Running CPU can overshoot by variable instructions, alter registers/RAM before the IRQ should enter, and lose pulses if only final levels are sampled. Cannot rewind side-effecting guest execution; reject for corrected running time |
| Set block quantum to 1 | More frequent dispatcher checks without a core API change | One block still contains variable instructions, and shadow retirement can consume a block. Not an instruction guarantee; unsuitable as the correctness gate |
| Drive the existing single-step primitive from the machine | Useful independent oracle and possible diagnostic reference after completion/IRQ gaps are closed | Existing primitive does not supply the full run/retry/acceptance contract; repeated decode/entry may be costly. Request the complete seam in x86jit instead of reimplementing it here |
| Cycle-approximate instruction/bus timing | Can model long REP, variable instruction costs, DMA steals, and calibrated CPU generations more faithfully | Requires per-op costs and hardware oracles; changes decision-5 and golden meaning. Scheduler domains can support it later, but no uncalibrated cycle table belongs in this spike |
| One integer nanosecond clock or one LCM clock for every oscillator | Familiar units or exact periods for a fixed set of frequencies | Rounded per-period nanoseconds drift without carry; a universal LCM grows with future domains. Fixed fine units plus rational phases give bounded rounding without constraining domain choice |
| Tick devices from host/audio callbacks | Convenient pacing | Makes execution depend on wall scheduling. Reject; presentation consumes virtual-time output instead |

## Migration and A/B gates

First freeze fixtures, CPU pin, config, block quantum, policies/input cadence,
and model version. Use two orthogonal comparison axes: **device dispatch**
(single-clock reference versus scheduled) and **CPU/IRQ semantics** (legacy
boundaries versus corrected instruction boundaries). Never change both axes
and then call a changed output an optimization regression.

1. Capture a complete baseline for every golden. Add the timing/IRQ diagnostics
   needed to locate a first difference: retired count, device/PIT time, virtual
   cap credit, stop reason, ordered accesses, line transitions, INTA vectors,
   PIC IRR/ISR, and relevant final device/CPU/RAM state. Keep wall timers outside
   execution. Existing output-only goldens are necessary but not sufficient.
2. Wire and validate the existing ratio into the machine, preserving `1 / 1`.
   Isolate this from deadline execution. Default-profile **all-golden equality**
   is mandatory; tests must show non-default ratios actually alter device time.
3. Add fine time/domain arithmetic, deadline slots, access context, and the
   default adapter under an experimental scheduler selection. Keep legacy CPU
   boundaries and acceptance order on the reference axis. Gate every stage on
   **all-golden equality**, including projected trace times and policy endpoints.
   In this mode replay clock events and DMA at the old service points; do not
   accidentally introduce the corrected ordering through a helper refactor.
4. Convert timestamp-only devices, then PIT, RTC, and 8042 one at a time. Compare
   each against its isolated reference stepper as well as every machine golden.
   Pending DMA remains serviced at legacy points in this comparison mode.
5. Land the requested Real16 seam in x86jit under that project's full gate,
   then re-pin its exact main revision here. Re-run **all** fast and slow
   goldens even before enabling corrected scheduling; include self-jump and
   retirement-accounting changes in the pin review.
6. Enable instruction deadlines and acceptance ordering together in an opt-in
   corrected mode. First compare the scheduled device engine with a
   single-clock **reference using the same corrected CPU/IRQ semantics**:
   all-golden equality is mandatory on that axis. Also compare corrected mode
   against the frozen old baseline for every golden. Report **identical or
   different**, first divergence, and its causal explanation individually.
   Do not regenerate goldens automatically. Review and accept intended timing,
   INTA, and tighter-cap changes with the new model identity.
7. Make corrected mode the default only after review of every golden and
   focused regressions below. Delete legacy production scheduling separately;
   retain a small single-clock test oracle. Adjust config/policy docs and
   record the decision amendments as part of that transition.

The gate inventory includes serial, timer boot, keyboard, RTC/cascade, VGA,
synthetic floppy boot, FDC write/readback, and test386, plus ignored BIOS POST,
BIOS-to-floppy boot, FreeDOS prompt, interactive DIR, and DOS file-write
sessions. Also compare every other golden found in the workspace, including
device golden tests. Fix the x86jit revision for each comparison. Exercise
cooperation sizes 1/64/65,536 in corrected mode with the **same** policy targets;
they must not change any observables. Old baseline quanta remain fixed.

Focused implementation tests, to be added with the corresponding changes:

| Test | What it must prove / expected baseline gap |
|---|---|
| Ratio validation and partitioned conversion | Zero/overflow rejected; ratios 1, 3, and 7/3 honored; arbitrary batch splits preserve phase; current assembly ignores the ratio |
| PIT isolated stepper versus deadlines | Load delay, Mode-2 low and rising edges, Mode-3 odd/even halves, latch/read sequencing, zero divisor, reprogramming; naive bulk final-level sampling loses edges |
| RTC boundaries and SET/read-C writes | UIP entry/exit and second/update/alarm sequence match reference; read-clear falls are observed even if immediately followed by other work |
| 8042 read/hold/refill and host injection | Exactly two modeled PIT boundaries after read, low sampled immediately, dormant queue rearmed on injection; an extra same-time sync must not release hold |
| Ready IRQ behind IF=0 | IRR remains pending and ISR unchanged until actual CPU acceptance; eager `service_interrupts` fails this assertion |
| `STI; NOP` and `STI; HLT` with pending INTR | Accept after the following instruction, independent of block shape and call size; no post-HLT instruction before handler |
| IMR/EOI OUT coincident with pending IRQ | Stopping OUT reaches PIC before INTA chooses winner; current run loop acknowledges first |
| MMIO/IN/OUT transaction at deadline/cap | All accesses use instruction start; value/completion and retirement precede INTA; multiple transfers charged once; post-access faults do not advance time for an unretired instruction |
| Falling and rising edges inside one slow instruction | Both reach PIC in timestamp order; final-level-only sampling is insufficient |
| HLT with cap before event, IF clear, idle expiry, repeated runner calls | No CPU retirement while idle; no jump past cap; IF-clear stops without tick/INTA; phase and idle credit preserved |
| Self-jump and zero-retire fault progress | `JMP $` consumes instruction budget/time; repeated fault/retry work yields or errors without inventing time |
| Configured slow/fast CPU and sample domains | Same physical-domain phase for a given virtual instant; CPU instruction counts differ by configured rational rate |

Device tests use `crates/testkit` mock-bus isolation; machine tests use mock CPU
and controller plus small Real16 ROMs for the instruction/IRQ seam. For each bug
fix, run its new regression against the preceding implementation and retain the
actual failure evidence. The predicted gaps above are **READ-derived test
requirements**, not claims that new tests were run in this spike.

## Spike validation — MEASURED versus unmeasured

Only this document was changed. No device/prototype tests were added because
no executable implementation was changed. No scheduler prototype was built:
the current CPU API cannot validate instruction-exact scheduling, and a PIT-only
wrapper would not test IRQ correctness. No scheduler A/B result, speedup, or
old/new golden equivalence is claimed.

The following baseline commands were run offline in the project Nix devShell
with both `cc` and `cargo` verified under `/nix/store`:

| Command | Measured result |
|---|---|
| `cargo build --offline --all-targets` | PASS, exit 0 |
| `cargo test --offline --workspace` | PASS, exit 0; 326 parent-suite tests passed, 0 failed, 5 ignored; two watchdog child tests also passed and are not counted twice |
| `cargo clippy --offline --all-targets --all-features -- -D warnings` | PASS, exit 0 |
| `cargo fmt --check` | PASS, exit 0 |

An ephemeral Python integer/Fraction check ran 193 assertions for the proposed
carry formula, batch partitioning, rational RTC/audio rounding, and first-boundary
selection. Rates checked were 1, 3, 7/3, 1/3, and 65,537/1,000 retired per PIT;
sample domains were 32,768/44,100/48,000 Hz. All passed. This validates arithmetic
examples only; it does not test Rust overflow handling or emulator behavior.

Slow ignored goldens were **not run**: this is documentation only, there is no
prototype to compare, and the worker command restrictions do not name an ignored
test to execute. All-golden A/B and performance remain proposed implementation
gates. Suitability of the API/performance tradeoff is a design inference;
hardware fidelity beyond the listed contracts remains to be tested.

## Proposed implementation beads

These are proposed titles and scopes for the supervisor, not created issues.
Order is dependency order; the existing INTA-ordering issue should be linked
to item 7 rather than duplicated.

1. **Capture scheduler timing and IRQ reference observations** — Freeze every
   golden's full behavior and add an isolated single-clock device oracle.
2. **Honor and validate the configured CPU-to-PIT ratio** — Wire positive
   ratios through assembly/machine with remainder tests; preserve default goldens.
3. **Introduce rational scheduler time and conservative device deadlines** —
   Add checked phases, access timestamps, deterministic slots and default
   per-clock adapter behind an experimental mode; preserve legacy behavior.
4. **Schedule PIT transitions without losing counter semantics** — Add PIT
   deadlines and arithmetic catch-up, tested against all modes currently modeled.
5. **Schedule RTC and keyboard events; park timestamp-only devices** — Migrate
   devices separately with isolated and all-golden A/B gates; retain model limits.
6. **Re-pin the instruction-exact Real16 CPU scheduler seam** — After the CPU
   project lands the request below, update the exact git pin and run every golden.
7. **Deliver interrupts at completed instruction boundaries** — Use exact
   budgets/readiness, transaction completion, OUT-before-INTA, bounded HLT jumps,
   and quantum-independent caps/policies; compare both golden axes explicitly.
8. **Adopt the reviewed timing model and remove legacy production scheduling** —
   Accept individually explained golden differences, update decision/config docs,
   retain reference tests, then measure CPU entry/exit and device work again.

Later hardware fidelity work (RTC divider/periodic IRQs, refresh/speaker wiring,
audio serialization, timed FDC/DMA) requires separate scope and oracles. It is
not a hidden prerequisite for dispatching the **current** device models correctly.

## Request to the CPU core project

This is a scoped request draft, not an issue filed or a message sent. Implement
and land it in x86jit first, then re-pin here. Scope: **Real16 interpreter**;
keep other modes' generated code unchanged and `CpuState` / `jit_abi`
append-only under that project's workflow.

1. **Instruction-exact execution.** Provide a new opt-in run API taking a
   maximum number `N` of **additional architecturally completed guest
   instructions**, not blocks, IR operations, decode attempts, REP iterations,
   or interrupt entries. On normal budget exhaustion retire exactly `N`,
   leave IP at the next instruction, and execute no effect of instruction
   `N+1`. For an earlier stop return the actual retired delta, `0 <= delta <= N`.
   A block containing 100 instructions with `N=7` must stop after the seventh,
   regardless of lifting/cache/block grouping. `N=0` executes nothing and has
   no guest side effects. Preserve the old block API for other users.
2. **Reliable retirement.** Count completed branches to their own IP, including
   `JMP $`, and HLT. Count each successfully completed instruction once, including
   a software INT; faulting instructions, hardware/exception delivery, MMIO
   retries, and already serviced transfer parts are not extra retirements.
   Preserve the counter across calls and expose it without lossy conversion.
   Exhaustion and a guest-visible stop at the same retirement must surface the
   guest-visible stop first, so the embedder completes it before any next run.
3. **Pending-instruction completion.** Report stop phase/ownership explicitly:
   already retired but awaiting port result/side effect, versus unretired
   MMIO/retry, with the completed prefix separated from the accessing instruction.
   Provide completion-only resumption that cannot execute the next guest
   instruction and reports final retirement (or fault). Multiple transfers and
   retries remain owned by one instruction. This allows a common start-time
   access convention and charges its elapsed time exactly once on retirement,
   including the zero-charge case of a post-access fault. It does not ask to
   add unsupported MMIO RMW operations.
4. **Immediate interrupt readiness and acceptance.** Expose the core's complete
   maskable-IRQ readiness at a boundary, including IF, STI/other modeled shadows,
   and all pending port/MMIO/retry state. Provide immediate vector acceptance
   with no guest instruction before IVT entry and no deferred queue after a
   successful readiness transaction. An equivalent callback invoked **only
   when accepting** may combine PIC acknowledgement with entry. Return delivery
   errors explicitly. Merely exposing IF or queue emptiness is insufficient.
5. **First eligible IRQ boundary.** With the embedder reporting INTR pending,
   the exact-run API must stop before the next guest instruction at the first
   boundary where acceptance becomes legal, even inside a long lifted block.
   Leave the PIC untouched until the embedder handles that stop. Readiness must
   account for POPF/IRET, `STI; NOP`, and `STI; HLT`; consume inhibition per
   completed instruction and retain it through faulting/deferred retries.
6. **Progress semantics.** Return a deterministic work-yield for zero-retirement
   exception/retry loops without pretending instructions retired. Specify REP
   explicitly: in this first instruction-time model a completed REP retires
   once, not once per element. Its internal iterations do not advance device
   time; no sub-REP IRQ latency guarantee is claimed. If hardware-correct
   between-iteration interrupt timing becomes required, that needs a separately
   accepted time/accounting model rather than covertly changing `retired` units.

CPU-project acceptance tests must cover cut points inside straight-line blocks,
batch partition equivalence, self-jumps, budget-ending OUT/IN/HLT, deferred and
multipart MMIO completion, fault/nonretirement, STI/IF/IRET acceptance windows,
and a halted wakeup before post-HLT code. A pending IRQ with IF clear must never
invoke the acknowledgement callback. Test several lifted block arrangements
with the same instruction stream and budget. Run that project's full gate and
the generated-code non-regression checks for the other modes before landing.

Until this seam exists, per-clock device optimization can be measured under
legacy boundaries, but it must not be advertised as instruction-exact IRQ timing.

## Proposed decision

### Context

Retired-instruction time and halted clock advancement provide reproducible
execution, but the configured CPU ratio is unused, device advancement sweeps
every chip on every PIT clock, and interrupt observation still depends on CPU
block budgets. Eager PIC acknowledgement can precede the stopping OUT or occur
while IF/shadows prohibit CPU acceptance. Measurements show correctness is the
main motivation; real DOS workloads are predominantly CPU-bound.

### Decision

Adopt checked 128-bit scheduler instants with `2^32` units per PIT period,
rational per-domain phases, and configurable rational retired instructions per
PIT period, defaulting to today's effective rate. Preserve retired instructions
plus carried halted instruction equivalents as the policy time measure.

Add conservative device deadlines with a default per-PIT-clock adapter and
explicit dormant state. Synchronize scheduled devices lazily on access, separate
trace timestamps from ticks, and observe every IRQ/output transition in stable
timestamp order. Keep concrete clock/signal wiring in assembly.

Require a Real16 instruction-exact CPU budget and first-eligible IRQ boundary
contract before enabling corrected scheduling. Complete stopping-instruction
bus transactions and immediate DMA/output work before PIC acknowledgement;
acknowledge only when immediate CPU acceptance is legal. HLT jumps to the
earliest event or bound without retiring instructions. Cooperation batch size
must not change guest behavior.

Extend decision-5's unit representation and decision-14's halted advancement
implementation; replace decision-4's block-quantized latency and eager queued
injection with the acceptance transaction above. Keep old and corrected
semantics explicitly distinguished until all-golden comparisons are reviewed.

### Alternatives

Retain per-clock ticking as a reference and compatibility fallback. Reject
running-CPU event skipping based on block budgets because it cannot enforce
instruction deadlines. Defer cycle/bus approximation pending calibrated oracles
and a separate decision; exclude wall-clock-driven execution.

### Consequences

Device periods and IRQ acceptance become independent of lifter block formation;
clock domains support future RTC/audio consumers without host-time coupling.
PIT pulses, I/O completion, and PIC acknowledgement gain explicit ordering.
Idle jumps and fewer device visits may reduce work, with no promised speedup.

The change requires a CPU-project seam, phase/transaction state, isolated
reference tests, and versioned timing identity. Compatibility optimizations must
preserve every golden; corrected IRQ/cap semantics may require individually
explained and accepted golden changes. Current device approximations and
atomic-REP timing remain visible fidelity limits.

### Trigger to revisit

Revisit when a demonstrated workload needs between-REP interrupt timing,
instruction-specific cycle costs, DMA arbitration, NMI/reset wakeup, calibrated
CPU frequency behavior, or finer-than-supported domain resolution. Revisit the
linear deadline scan or return strategy only when new phase measurements show
them material. Host pacing or audio playback alone is not a reason to weaken
deterministic virtual time.
