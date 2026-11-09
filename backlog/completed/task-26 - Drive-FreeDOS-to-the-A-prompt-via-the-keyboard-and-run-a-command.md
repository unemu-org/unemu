---
id: TASK-26
title: Drive FreeDOS to the A prompt via the keyboard and run a command
status: Done
assignee: []
created_date: '2026-07-18 11:16'
updated_date: '2026-07-18 12:16'
labels:
  - dos
  - devices
dependencies: []
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
FreeDOS boots and idles at 'Enter new date' waiting on INT 16h. Feed set-1 scancodes through the 8042 (self-pacing input queue): Enter (date), Enter (time) -> A: prompt, then DIR + Enter -> capture the directory listing. Proves the full input path scancode->8042->IRQ1->INT09h->BDA->INT16h->DOS under real DOS. #[ignore] golden asserting the A: prompt + command output. Validate the set-1 encoding fast with a short INT 16h echo harness before the ~9min full boot.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Scancode script pre-loaded into the 8042; FreeCOM accepts the date/time prompts and reaches the A: prompt
- [ ] #2 A command (DIR) runs and its output is captured from the MDA console
- [ ] #3 #[ignore] golden pins the prompt + command output; fast gates green
<!-- AC:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Done 2026-07-18. run_freedos_interactive drives FreeDOS to A: and runs DIR (KERNEL.SYS + COMMAND.COM listed). Findings: (1) power-on preloaded keys eaten by POST -> inject after FreeCOM owns kbd; (2) IRQ0 starves IRQ1 under coarse quantum (one ack/boundary) -> fine quantum workaround; (3) 8042 refills output buffer instantly so IRQ1 level never falls -> edge lost -> inject one-at-a-time. (2)(3) are interrupt-model fidelity limits, worked around in the driver; faithful fixes deferred (task-27).
<!-- SECTION:NOTES:END -->
