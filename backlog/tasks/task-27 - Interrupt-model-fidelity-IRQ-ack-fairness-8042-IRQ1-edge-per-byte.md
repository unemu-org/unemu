---
id: TASK-27
title: 'Interrupt-model fidelity: IRQ ack fairness + 8042 IRQ1 edge per byte'
status: Done
assignee: []
created_date: '2026-07-18 12:16'
updated_date: '2026-07-18 13:46'
labels:
  - machine
  - devices
dependencies: []
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Two real interrupt-model limitations surfaced while driving FreeDOS via the keyboard (task-26), currently worked around in the cli driver rather than the model: (1) The machine acknowledges only ONE PIC vector per run() boundary; under a coarse quantum a busy-polling guest retires many timer periods per quantum, so a fresh higher-priority IRQ0 wins every priority race and starves lower lines (IRQ1). A faithful fix services pending interrupts more finely (e.g. drain serviceable vectors at a boundary, or cap the retired delta per boundary independent of quantum). (2) The 8042 refills its output buffer the instant a byte is read, so irq_pending (a level) never falls between queued bytes; the edge-triggered PIC sees no new edge and only the first key is delivered. A faithful fix pulses IRQ1 per byte (like real hardware where each keyboard byte is a discrete event), matching how the PIT OUT is edge-detected. Decide the right model change; the workarounds (fine quantum, one-scancode-at-a-time) can then be removed from the driver.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 IRQ ack fairness: a lower-priority IRQ is not indefinitely starved by a higher one under a coarse quantum
- [ ] #2 8042 delivers a clean IRQ1 edge per queued byte (or the machine handles the level correctly)
- [ ] #3 The task-26 driver workarounds (fine quantum / one-at-a-time) can be simplified; interactive golden still passes
<!-- AC:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Done 2026-07-18. Issue 2 (8042 IRQ1 edge per byte) FIXED in the model: serial refill on tick + one-tick refill_hold so edge_detect sees the line fall -> clean edge per byte; interactive driver simplified (whole script queued, dropped one-at-a-time). Issue 1 (IRQ0 starves IRQ1 under coarse quantum) DOCUMENTED not fixed - a quantum-granularity tradeoff per decision-4; faithful fix would shift boot goldens, kept the fine DOS_INTERACTIVE_QUANTUM knob. All goldens green, no re-baseline. Machine core + x86jit untouched.
<!-- SECTION:NOTES:END -->
