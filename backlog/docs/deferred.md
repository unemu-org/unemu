# Deferred

Things **deliberately not implemented**. If something seems missing and is listed here, don't add it unprompted — there's a reason. Each entry: what, why deferred, when to revisit.

## Format

```markdown
### <Feature / behavior>

- **Why deferred:** <one paragraph>
- **Revisit when:** <trigger condition>
- **Tracked in:** <task-N, if any>
```

## Entries

### Protected mode, paging (386+), APIC, PCI

- **Why deferred:** Target is IBM PC/AT-class (80286). The 286 has no paging;
  protected mode is not needed for real-mode boot. APIC/PCI are 386+/later-era.
- **Revisit when:** A later target machine generation is chosen.
- **Tracked in:** decision-1.

### Full A20 gate (via 8042)

- **Why deferred:** A20 modelled as always-on for M1–M3 to unblock early boot.
- **Revisit when:** A boot flow requires A20 toggling.
- **Tracked in:** decision-8.

### `RegionKind::Rom` in x86jit

- **Why deferred:** BIOS stub can load into `Ram` by convention initially.
- **Revisit when:** Faithful ROM write-protection matters (real BIOS blob).
- **Tracked in:** decision-9.

### Graphics beyond serial (VGA text, framebuffer)

- **Why deferred:** First E2E target is headless serial (deterministic goldens).
- **Revisit when:** M4 — VGA text as the first `FrameSink`.
- **Tracked in:** task-5.

### Real-mode segment-register loads (`mov Sreg, r/m`) and far transfers

- **Why deferred:** x86jit's real mode does not yet lift `mov Sreg, r16`/`pop Sreg`
  or far `jmp`/`call`/`retf` (CS reloads other than `INT`/`IRET`). The emulator's
  boot programs work around this: the machine seeds CS/DS/ES/SS before running.
- **Revisit when:** running a real BIOS/DOS/bootloader, which loads segment
  registers itself — needs these lifted in x86jit (a further real-mode extension).
- **Tracked in:** decision-3 (real-mode scope), task-7 (test386.asm / real BIOS).

### Real (unmodified) BIOS

- **Why deferred:** Start from a minimal BIOS stub; x86jit models a modern CPU, so
  286-detection quirks would break a period BIOS.
- **Revisit when:** A real BIOS blob is a goal.
- **Tracked in:** decision-10.
