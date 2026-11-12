// SPDX-License-Identifier: AGPL-3.0-or-later
//! Creative Sound Blaster 16 DSP, 8-bit output.
//!
//! Hardware source: Creative, Sound Blaster Series Hardware Programming Guide,
//! §2 DSP I/O Ports, Resetting the DSP, Acknowledging DSP Interrupts; §3 8-bit
//! DMA Mode and Auto-initialize DMA; §6 commands 10h, 14h, 1Ch, 40h, 41h,
//! 48h, Cxh, D0h/D4h, D1h/D3h/D8h, DAh and E1h; Appendix A, SB16.
//! See `backlog/docs/design/audio.md` §SB16 8-bit output.
//!
//! | Guide section | Coverage |
//! |---|---|
//! | §2 DSP I/O, reset, interrupt acknowledgement | Byte ports, prompt reset-edge reply, latched IRQ |
//! | §3/§6 8-bit DMA | Unsigned legacy mono, signed/unsigned Cxh mono/stereo, single/auto-init |
//! | §6 rate and control | Exact rational periods, pause/resume, exit, identification |
//! | §6 speaker control | Status flag only, as specified for DSP 4.xx |
//! | §4 mixer, §5 MIDI, §6 input/16-bit/ADPCM | Not modelled |
//!
//! Creative's §2 Resetting the DSP recommends a 3 µs hold and describes typical
//! 100 µs initialization. ASSUMED compatibility policy: clear state on writing 1,
//! accept any following 1->0 edge, and expose AAh immediately (zero delay), even
//! when both writes share a PIT clock. Counted driver poll loops on fast boards
//! must not exhaust while instruction time still rounds to the same PIT clock.
//! Busy is asserted only while reset is held; other commands/replies execute
//! immediately without ISA/DSP execution latency. Command 41h is clamped to the
//! guide's 5000–45000 Hz range (§6 41h). Time constants remain unclamped,
//! including 255 (ASSUMED); the explicit-Hz limit applies only to 41h.
//! Reset defaults to 22050 Hz, unsigned mono and a 65536-byte legacy auto-init
//! block (ASSUMED).
//! ASSUMED: E0h returns the inverted argument and F2h immediately asserts the
//! 8-bit IRQ; these compatibility probes are absent from this guide. The read
//! reply queue retains at most 16 oldest bytes and drops excess replies. Empty
//! reply reads return FFh; unused status bits read zero (ASSUMED).
//! Unsupported commands consume their complete known argument payload without a
//! reply; unknown opcodes consume one byte (ASSUMED unsupported-command policy).
//! Mixer +4/+5, 16-bit acknowledge +F, and OPL ports are unclaimed, leaving FFh
//! from the empty bus rather than inventing registers.
//!
//! ASSUMED: the first frame is requested one period after the command; subsequent
//! frames use origin + ceil(index * period), never repeated rounded additions.
//! There is one frame of staging (one byte mono, two stereo), fetched exactly
//! at its playback boundary through the DMA seam. Cxh FIFO enable is accepted
//! but does not change this depth. IRQ asserts when the block's last byte arrives;
//! the last frame lasts until the following boundary. A masked/exhausted DMA
//! channel produces silence for that frame and retries at the next boundary.
//! An odd stereo block ends with a silent right channel. Pause freezes phase,
//! including the remaining fractional period; resume shifts the origin by the
//! paused duration. Chained blocks retain the sample grid; a live rate change
//! holds the current frame until its old boundary, then starts the new rate.
//! The 48h legacy auto-init size is independent of 14h/Cxh transfer counts.
//! ASSUMED compatibility policy: a new DMA command while paused discards the
//! unfinished transfer, clears pause and starts a fresh block, including an
//! auto-init-to-single replacement. Its first fetch is one period later; only
//! unpaused transfers defer a replacement to a frame/block boundary.
//!
//! PCM scaling is unsigned `(byte - 128) * 256`, signed `i8(byte) * 256`;
//! mono duplicates the result. Direct DAC holds its unsigned level until another
//! DAC write, DMA command or reset (ASSUMED analogue reconstruction). The output
//! observer is installed by assembly; hardware never depends on an audio consumer.

use std::collections::VecDeque;

use device::{
    AccessDir, Device, DeviceError, DmaOp, DmaOutcome, DmaRequest, NextEvent, TraceBuffer,
    TraceEvent, TracePolicy, VClock,
};

/// Timestamped native output, consumed by assembly's device-lifetime RateSource.
/// Creative Sound Blaster Series Hardware Programming Guide, §3 PCM data format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Output {
    /// One frame held for this exact positive rational PIT-clock period.
    Frame {
        /// Absolute start of the native frame.
        at: VClock,
        /// PIT-clock period numerator.
        numerator: u64,
        /// PIT-clock period denominator.
        denominator: u64,
        /// Signed left/right levels.
        pcm: [i16; 2],
    },
    /// End the active signal; the gap before another frame is silence.
    Pause(VClock),
}

#[derive(Clone, Copy, Debug)]
struct Transfer {
    count: u32,
    auto: bool,
    mode: u8,
}

#[derive(Clone, Debug)]
struct State {
    now: u64,
    reset_start: Option<u64>,
    reset_ready: Option<u64>,
    reply: VecDeque<u8>,
    command: u8,
    args: Vec<u8>,
    needed: u8,
    irq: bool,
    speaker: bool,
    rate: u16,
    tc: Option<u8>,
    auto_block: u32,
    reload: u32,
    remaining: u32,
    active: bool,
    auto: bool,
    exit: bool,
    stereo: bool,
    signed: bool,
    paused: Option<u64>,
    origin: u64,
    index: u64,
    tail: bool,
    request: bool,
    inflight: bool,
    staging: Vec<u8>,
    level: [i16; 2],
    direct: bool,
    next_block: Option<Transfer>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            now: 0,
            reset_start: None,
            reset_ready: None,
            reply: VecDeque::new(),
            command: 0,
            args: Vec::new(),
            needed: 0,
            irq: false,
            speaker: false,
            rate: 22050,
            tc: None,
            auto_block: 65536,
            reload: 65536,
            remaining: 0,
            active: false,
            auto: false,
            exit: false,
            stereo: false,
            signed: false,
            paused: None,
            origin: 0,
            index: 1,
            tail: false,
            request: false,
            inflight: false,
            staging: Vec::new(),
            level: [0; 2],
            direct: false,
            next_block: None,
        }
    }
}

/// Optional ISA DSP with one 8-bit DMA input and one latched interrupt output.
/// Creative Sound Blaster Series Hardware Programming Guide, §2 DSP I/O Ports.
pub struct Sb16 {
    base: u16,
    dma8: u8,
    state: State,
    output: Option<Box<dyn FnMut(Output)>>,
    trace: TraceBuffer,
}

impl Sb16 {
    /// Build a DSP at the validated ISA base and byte-DMA channel.
    #[must_use]
    pub fn new(base: u16, dma8: u8) -> Self {
        Self {
            base,
            dma8,
            state: State::default(),
            output: None,
            trace: TraceBuffer::default(),
        }
    }

    /// Bind native audio once; no host clock or host audio API enters the DSP.
    pub fn observe_output(&mut self, output: impl FnMut(Output) + 'static) {
        self.output = Some(Box::new(output));
    }

    fn emit(&mut self, event: Output) {
        if let Some(output) = &mut self.output {
            output(event);
        }
    }

    /// Native stereo-frame period in exact PIT clocks (numerator, denominator).
    /// Creative Sound Blaster Series Hardware Programming Guide, §6 40h/41h:
    /// time constants count interleaved bytes; explicit rates count frames.
    #[must_use]
    pub fn sample_period(&self) -> (u64, u64) {
        if let Some(tc) = self.state.tc {
            (
                105 * u64::from(256 - u16::from(tc)) * if self.state.stereo { 2 } else { 1 },
                88,
            )
        } else {
            (105_000_000, 88 * u64::from(self.state.rate))
        }
    }

    fn boundary(&self) -> u64 {
        let (p, q) = self.sample_period();
        let delta = (u128::from(self.state.index) * u128::from(p)).div_ceil(u128::from(q));
        u64::try_from(u128::from(self.state.origin) + delta).unwrap_or(u64::MAX)
    }

    fn frame(&mut self) {
        let (numerator, denominator) = self.sample_period();
        self.emit(Output::Frame {
            at: VClock(self.state.now),
            numerator,
            denominator,
            pcm: self.state.level,
        });
    }

    fn clear(&mut self) {
        let now = self.state.now;
        self.state = State {
            now,
            ..State::default()
        };
        self.emit(Output::Pause(VClock(now)));
    }

    fn start(&mut self, count: u32, auto: bool, mode: u8) {
        // Unpaused playback drains the current frame before a chained block.
        // Auto-init also defers a single-cycle replacement until the block ends
        // (Creative Sound Blaster Series Hardware Programming Guide, §6 1Ch).
        if self.state.active
            && self.state.paused.is_none()
            && (self.state.tail || (self.state.auto && !auto))
        {
            self.state.next_block = Some(Transfer { count, auto, mode });
            self.state.exit = true;
            return;
        }
        self.emit(Output::Pause(VClock(self.state.now)));
        let s = &mut self.state;
        s.reload = count;
        s.remaining = count;
        s.auto = auto;
        s.exit = false;
        s.stereo = mode & 0x20 != 0;
        s.signed = mode & 0x10 != 0;
        s.active = true;
        s.paused = None;
        s.origin = s.now;
        s.index = 1;
        s.tail = false;
        s.request = false;
        s.inflight = false;
        s.staging.clear();
        s.direct = false;
        s.level = [0; 2];
        s.next_block = None;
    }

    fn set_rate(&mut self, next: u64) {
        if self.state.active {
            let paused = self.state.paused.map_or(0, |at| self.state.now - at);
            if self.state.paused.is_some() {
                self.state.paused = Some(self.state.now);
            }
            // Finish the held frame on its old boundary. The new rational grid
            // starts there, so rate changes cannot truncate it or insert a gap.
            self.state.origin = next.saturating_add(paused);
            self.state.index = 0;
        }
    }

    fn hold_until_boundary(&mut self) {
        self.emit(Output::Frame {
            at: VClock(self.state.now),
            numerator: self.boundary().saturating_sub(self.state.now).max(1),
            denominator: 1,
            pcm: self.state.level,
        });
    }

    /// Argument bytes in the Creative Sound Blaster Series Hardware Programming
    /// Guide, §6 DSP Commands, including input, ADPCM, MIDI and word DMA commands.
    /// ASSUMED: argument lengths for 04h/05h/08h/0Eh/0Fh/F9h/FAh follow
    /// DOSBox-X's SB16 DSP command-length table. Consuming these unsupported
    /// payloads prevents them becoming output commands.
    fn argument_count(command: u8) -> u8 {
        match command {
            0x04 | 0x08 | 0x0F | 0x10 | 0x38 | 0x40 | 0xE0 | 0xE2 | 0xE4 | 0xF9 => 1,
            0x05 | 0x0E | 0x14..=0x17 | 0x24 | 0x41 | 0x42 | 0x48 | 0x74..=0x77 | 0x80 | 0xFA => 2,
            0xB0..=0xCF => 3,
            _ => 0,
        }
    }

    fn write(&mut self, byte: u8) {
        if self.state.reset_start.is_some() || self.state.reset_ready.is_some() {
            return;
        }
        if self.state.needed != 0 {
            self.state.args.push(byte);
            self.state.needed -= 1;
            if self.state.needed == 0 {
                self.execute();
            }
            return;
        }
        self.state.command = byte;
        self.state.args.clear();
        self.state.needed = Self::argument_count(byte);
        if self.state.needed == 0 {
            self.execute();
        }
    }

    fn execute(&mut self) {
        let args = &self.state.args;
        match self.state.command {
            0x10 => {
                let level = (i16::from(args[0]) - 128) * 256;
                self.state.active = false;
                self.state.origin = self.state.now;
                self.state.index = 1;
                self.state.paused = None;
                self.state.inflight = false;
                self.state.staging.clear();
                self.state.next_block = None;
                self.state.request = false;
                self.state.tail = false;
                self.state.direct = true;
                self.state.level = [level; 2];
                self.emit(Output::Frame {
                    at: VClock(self.state.now),
                    numerator: u64::MAX - self.state.now,
                    denominator: 1,
                    pcm: [level; 2],
                });
            }
            0x14 => self.start(
                u32::from(u16::from_le_bytes([args[0], args[1]])) + 1,
                false,
                0,
            ),
            0x1C => self.start(self.state.auto_block, true, 0),
            0x40 => {
                let next = self.boundary();
                self.state.tc = Some(args[0]);
                self.set_rate(next);
            }
            0x41 => {
                let next = self.boundary();
                self.state.rate = u16::from_be_bytes([args[0], args[1]]).clamp(5000, 45000);
                self.state.tc = None;
                self.set_rate(next);
            }
            0x48 => self.state.auto_block = u32::from(u16::from_le_bytes([args[0], args[1]])) + 1,
            0xC0..=0xCF => {
                if self.state.command & 9 == 0 {
                    self.start(
                        u32::from(u16::from_le_bytes([args[1], args[2]])) + 1,
                        self.state.command & 4 != 0,
                        args[0],
                    );
                }
            }
            0xD0 if self.state.active && self.state.paused.is_none() => {
                self.state.paused = Some(self.state.now);
                self.emit(Output::Pause(VClock(self.state.now)));
            }
            0xD4 if self.state.active => {
                if let Some(at) = self.state.paused.take() {
                    self.state.origin = self.state.origin.saturating_add(self.state.now - at);
                    self.hold_until_boundary();
                }
            }
            0xD1 => self.state.speaker = true,
            0xD3 => self.state.speaker = false,
            0xD8 => self
                .state
                .reply
                .push_back(if self.state.speaker { 0xFF } else { 0 }),
            0xDA => {
                self.state.exit = true;
                self.state.next_block = None;
            }
            0xE0 => self.state.reply.push_back(!args[0]),
            // Epson Product Support Bulletin S-0172, Software, reports DSP 4.05
            // on a physical SB16; Creative's guide identifies SB16 as 4.xx.
            0xE1 => self.state.reply.extend([4, 5]),
            0xF2 => self.state.irq = true,
            _ => {}
        }
        self.state.reply.truncate(16);
    }

    fn record(&mut self, port: u16, size: u8, dir: AccessDir, value: u32) {
        self.trace.push(TraceEvent::Port {
            at: VClock(self.state.now),
            port,
            size,
            dir,
            value,
        });
    }
}

impl Device for Sb16 {
    fn name(&self) -> &'static str {
        "sb16-dsp"
    }
    fn port_read(&mut self, port: u16, size: u8) -> Result<u32, DeviceError> {
        let value = match port.wrapping_sub(self.base) {
            0x0A => self.state.reply.pop_front().unwrap_or(0xFF),
            0x0C => {
                if self.state.reset_start.is_some() || self.state.reset_ready.is_some() {
                    0x80
                } else {
                    0
                }
            }
            0x0E => {
                self.state.irq = false;
                if self.state.reply.is_empty() {
                    0
                } else {
                    0x80
                }
            }
            _ => 0xFF,
        };
        self.record(port, size, AccessDir::Read, value.into());
        Ok(value.into())
    }
    fn port_write(&mut self, port: u16, size: u8, value: u32) -> Result<(), DeviceError> {
        self.record(port, size, AccessDir::Write, value);
        match port.wrapping_sub(self.base) {
            6 if value as u8 & 1 != 0 => {
                self.clear();
                self.state.reset_start = Some(self.state.now);
            }
            6 => {
                if self.state.reset_start.take().is_some() {
                    self.state.reset_ready = Some(self.state.now);
                    self.tick(VClock(self.state.now));
                }
            }
            0x0C => self.write(value as u8),
            _ => {}
        }
        Ok(())
    }
    fn mmio_read(&mut self, addr: u64, _: u8) -> Result<u64, DeviceError> {
        Err(DeviceError::UnmappedRegister {
            kind: "mmio",
            offset: addr,
        })
    }
    fn mmio_write(&mut self, addr: u64, _: u8, _: u64) -> Result<(), DeviceError> {
        Err(DeviceError::UnmappedRegister {
            kind: "mmio",
            offset: addr,
        })
    }
    fn tick(&mut self, now: VClock) {
        if now.0 < self.state.now {
            return;
        }
        self.state.now = now.0;
        if self.state.reset_ready.is_some_and(|at| at <= now.0) {
            self.state.reset_ready = None;
            self.state.reply.push_back(0xAA);
        }
        if self.state.active
            && self.state.paused.is_none()
            && !self.state.request
            && !self.state.inflight
            && self.boundary() <= now.0
        {
            if self.state.tail {
                self.state.tail = false;
                if let Some(transfer) = self.state.next_block.take() {
                    let old_period = self.sample_period();
                    self.state.remaining = transfer.count;
                    self.state.reload = transfer.count;
                    self.state.auto = transfer.auto;
                    self.state.exit = false;
                    self.state.stereo = transfer.mode & 0x20 != 0;
                    self.state.signed = transfer.mode & 0x10 != 0;
                    if self.sample_period() != old_period {
                        self.state.origin = now.0;
                        self.state.index = 0;
                    }
                    self.state.request = true;
                } else {
                    self.state.active = false;
                    self.state.origin = now.0;
                    self.state.index = 1;
                    self.emit(Output::Pause(now));
                }
            } else {
                self.state.request = true;
            }
        }
    }
    fn next_event(&self, now: VClock) -> NextEvent {
        let mut at = self.state.reset_ready;
        if self.state.active
            && self.state.paused.is_none()
            && !self.state.request
            && !self.state.inflight
        {
            at = Some(at.map_or(self.boundary(), |at| at.min(self.boundary())));
        }
        at.map_or(NextEvent::Dormant, |at| {
            NextEvent::At(VClock(at.max(now.0.saturating_add(1))))
        })
    }
    fn irq_pending(&self) -> bool {
        self.state.irq
    }
    fn reset(&mut self) {
        self.clear();
        self.trace.clear();
    }
    fn take_dma_request(&mut self) -> Option<DmaRequest> {
        if !self.state.request || self.state.paused.is_some() {
            return None;
        }
        self.state.request = false;
        self.state.inflight = true;
        Some(DmaRequest {
            channel: self.dma8,
            op: DmaOp::FromMemory {
                len: (if self.state.stereo { 2 } else { 1 } - self.state.staging.len())
                    .min(self.state.remaining as usize),
            },
        })
    }
    fn dma_complete(&mut self, outcome: DmaOutcome) {
        if !self.state.inflight {
            return;
        }
        self.state.inflight = false;
        let DmaOutcome::FromMemory { bytes, .. } = outcome else {
            return;
        };
        let expected = (if self.state.stereo { 2 } else { 1 } - self.state.staging.len())
            .min(self.state.remaining as usize);
        if bytes.is_empty() {
            self.state.level = [0; 2];
            self.state.staging.clear();
            self.frame();
            self.state.index = self.state.index.saturating_add(1);
            return;
        }
        let count = bytes.len().min(expected);
        self.state.staging.extend_from_slice(&bytes[..count]);
        self.state.remaining -= count as u32;
        let end = self.state.remaining == 0;
        if end {
            self.state.irq = true;
            if self.state.auto && !self.state.exit {
                self.state.remaining = self.state.reload;
            } else {
                self.state.tail = true;
            }
        }
        if self.state.stereo && self.state.staging.len() == 1 && !end {
            self.state.request = true;
            return;
        }
        let sample = |byte| {
            if self.state.signed {
                i16::from(byte as i8) * 256
            } else {
                (i16::from(byte) - 128) * 256
            }
        };
        let left = sample(self.state.staging[0]);
        self.state.level = [
            left,
            if self.state.stereo {
                self.state.staging.get(1).map_or(0, |&b| sample(b))
            } else {
                left
            },
        ];
        self.frame();
        self.state.staging.clear();
        self.state.index = self.state.index.saturating_add(1);
    }
    fn save(&self) -> Vec<u8> {
        self.save_state()
    }
    fn restore(&mut self, bytes: &[u8]) -> Result<(), DeviceError> {
        self.restore_state(bytes)
    }
    fn set_trace_policy(&mut self, policy: TracePolicy) {
        self.trace.set_policy(policy);
    }
    fn trace_dropped(&self) -> u64 {
        self.trace.dropped()
    }
    fn take_trace(&mut self) -> Vec<TraceEvent> {
        self.trace.take()
    }
}

impl Sb16 {
    fn save_state(&self) -> Vec<u8> {
        let s = &self.state;
        let optional = |v: Option<u64>| v.unwrap_or(u64::MAX);
        let values = [
            u64::from(self.base),
            u64::from(self.dma8),
            s.now,
            optional(s.reset_start),
            optional(s.reset_ready),
            u64::from(s.command),
            u64::from(s.needed),
            u64::from(s.irq),
            u64::from(s.speaker),
            u64::from(s.rate),
            s.tc.map_or(256, u64::from),
            u64::from(s.auto_block),
            u64::from(s.remaining),
            u64::from(s.active),
            u64::from(s.auto),
            u64::from(s.exit),
            u64::from(s.stereo),
            u64::from(s.signed),
            optional(s.paused),
            s.origin,
            s.index,
            u64::from(s.tail),
            u64::from(s.request),
            u64::from(s.inflight),
            u64::from(s.level[0] as u16),
            u64::from(s.level[1] as u16),
            u64::from(s.direct),
            s.next_block.map_or(0, |transfer| u64::from(transfer.count)),
            s.next_block.map_or(0, |transfer| u64::from(transfer.mode)),
            u64::from(s.reload),
            s.next_block.map_or(0, |transfer| u64::from(transfer.auto)),
        ];
        let mut bytes = b"SB16-8V2".to_vec();
        for value in values {
            bytes.extend(value.to_le_bytes());
        }
        for values in [
            s.reply.iter().copied().collect::<Vec<_>>(),
            s.args.clone(),
            s.staging.clone(),
        ] {
            bytes.push(values.len() as u8);
            bytes.extend(values);
        }
        bytes
    }

    fn restore_state(&mut self, bytes: &[u8]) -> Result<(), DeviceError> {
        let bad = || DeviceError::InvalidSnapshot {
            device: "sb16-dsp",
            reason: "invalid SB16 byte-DMA state",
        };
        if bytes.get(..8) != Some(b"SB16-8V2") {
            return Err(bad());
        }
        let mut rest = &bytes[8..];
        let mut values = [0u64; 31];
        for value in &mut values {
            let raw: [u8; 8] = rest
                .get(..8)
                .ok_or_else(bad)?
                .try_into()
                .map_err(|_| bad())?;
            *value = u64::from_le_bytes(raw);
            rest = &rest[8..];
        }
        let v = values;
        let optional = |value| if value == u64::MAX { None } else { Some(value) };
        let mut lists = Vec::new();
        for limit in [16, 3, 1] {
            let len = usize::from(*rest.first().ok_or_else(bad)?);
            rest = &rest[1..];
            if len > limit {
                return Err(bad());
            }
            lists.push(rest.get(..len).ok_or_else(bad)?.to_vec());
            rest = &rest[len..];
        }
        if !rest.is_empty()
            || v[0] != u64::from(self.base)
            || v[1] != u64::from(self.dma8)
            || v[5] > 255
            || v[6] > 3
            || !(5000..=45000).contains(&v[9])
            || v[10] > 256
            || !(1..=65536).contains(&v[11])
            || v[12] > 65536
            || v[12] > v[29]
            || (v[19] > v[2] && (v[13] == 0 || v[20] != 0))
            || (v[20] == 0 && v[13] == 0)
            || !(1..=65536).contains(&v[29])
            || v[30] > 1
            || (v[30] != 0 && v[27] == 0)
            || v[27] > 65536
            || v[28] > 255
            || (v[27] != 0 && (v[13] == 0 || (v[21] == 0 && (v[14] == 0 || v[15] == 0))))
            || v[24] > 65535
            || v[25] > 65535
            || [7, 8, 13, 14, 15, 16, 17, 21, 22, 23, 26]
                .iter()
                .any(|&i| v[i] > 1)
            || [3, 18]
                .iter()
                .any(|&i| optional(v[i]).is_some_and(|at| at > v[2]))
            || v[4] != u64::MAX
            || (v[13] != 0 && v[12] == 0 && v[21] == 0)
            || (v[22] != 0 && v[23] != 0)
        {
            return Err(bad());
        }
        let expected_args = usize::from(Self::argument_count(v[5] as u8));
        if lists[1].len() + v[6] as usize != expected_args
            || (v[13] == 0 && (v[21] != 0 || v[22] != 0 || v[23] != 0 || v[18] != u64::MAX))
            || (v[16] == 0 && !lists[2].is_empty())
        {
            return Err(bad());
        }
        let s = State {
            now: v[2],
            reset_start: optional(v[3]),
            reset_ready: optional(v[4]),
            reply: lists[0].iter().copied().collect(),
            command: v[5] as u8,
            args: lists[1].clone(),
            needed: v[6] as u8,
            irq: v[7] != 0,
            speaker: v[8] != 0,
            rate: v[9] as u16,
            tc: (v[10] < 256).then_some(v[10] as u8),
            auto_block: v[11] as u32,
            reload: v[29] as u32,
            remaining: v[12] as u32,
            active: v[13] != 0,
            auto: v[14] != 0,
            exit: v[15] != 0,
            stereo: v[16] != 0,
            signed: v[17] != 0,
            paused: optional(v[18]),
            origin: v[19],
            index: v[20],
            tail: v[21] != 0,
            request: v[22] != 0,
            inflight: v[23] != 0,
            staging: lists[2].clone(),
            level: [v[24] as u16 as i16, v[25] as u16 as i16],
            direct: v[26] != 0,
            next_block: (v[27] != 0).then_some(Transfer {
                count: v[27] as u32,
                auto: v[30] != 0,
                mode: v[28] as u8,
            }),
        };
        self.state = s;
        // Presentation history is not hardware state. Restore resumes this source
        // at the restored clock; it never replays previously captured PCM.
        if self.state.direct {
            self.emit(Output::Frame {
                at: VClock(self.state.now),
                numerator: u64::MAX - self.state.now,
                denominator: 1,
                pcm: self.state.level,
            });
        } else if self.state.active && self.state.paused.is_none() {
            self.hold_until_boundary();
        } else {
            self.emit(Output::Pause(VClock(self.state.now)));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
