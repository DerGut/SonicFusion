# SonicFusion

DataFusion, but for sound.

The preview binary can build a synth graph from a text file:

```sh
scripts/preview --graph examples/graphs/two-tone.sf --seconds 1
```

After the first build, run `target/debug/preview --graph examples/graphs/two-tone.sf`
directly to try edits without invoking Cargo.

The graph language supports `sine(frequency)`, `square(frequency, pulse_width)`, `sawtooth(frequency)`,
`noise()`, `lpf(cutoff)`, `gain(factor)`, `mix(inputs, weights)`, and `out`.
`->` passes the previous node to a filter or gain. A `$name` at the end of a
statement saves its output for later statements; references must be defined
before use. For example:

```text
sine(440.0) -> $high;
sine(220.0) -> $low;
mix([$high, $low], [0.4, 0.6]) -> out
```

`mix($high, $low)` uses unit weights. `gain($high, 0.5)` is equivalent to
`$high -> gain(0.5)`. `out` applies the final frame limit from `RenderConfig`,
which the preview command derives from `--seconds`.

The Low-pass node's Dry/Wet slider blends the dry input with the filtered output.
Well above the cutoff the filter increasingly attenuates the wet signal, so
the blend approaches the dry share. Spectrum shows this asymptotic floor
relative to the input below 100% wet. At 70% wet, the dry share of 0.3 gives a
floor of about −10.5 dB.

In the DSL, `lpf(cutoff, wet)` takes a wet ratio from 0 to 1 and defaults to
1 (fully wet), as do saved patches without a ratio. For example,
`sine(440) -> lpf(500, 0.25) -> out` uses 25% filtered signal. The ratio also
works with a wired cutoff, as in `lpf($audio, $cutoff, 0.25)`.

`sawtooth(frequency)` generates a rising bipolar ramp from -1 toward +1,
wrapping to -1 each cycle. Frequency is in hertz, must be nonnegative and below
Nyquist, and zero holds -1. The Rust API is
`FrameSawToothOscExec::try_new(&config, frequency_hz)` in `physical::frame`.
Like the square oscillator, the sawtooth is not band-limited.

The Noise node and `noise()` generate mono white noise in [-1, 1]. Output is
deterministic per source and absolute frame, independent of render batch size.
An optional unsigned 32-bit seed, `noise(seed)`, fixes a particular sequence.
The editor assigns a seed from the node ID when lowering or exporting a patch.

## Direct playback

```sh
cargo run --features audio-device --example live_play
# Or pass a graph file:
cargo run --features audio-device --example live_play -- examples/graphs/two-tone.sf
```

The first command plays a quiet test tone until Ctrl-C. The example selects a
native `f32` mono or stereo format on the default device before building the
circuit. It prefers the device's default rate, so a 44.1 kHz-only speaker runs
at 44.1 kHz. Stereo output duplicates mono. Processing uses 1,024-frame batches
regardless of the device's callback buffer size. There is no resampling; to use
a different output, change the system default and restart the example. Linux
builds with this feature require ALSA development libraries (e.g. `libasound2-dev`
on Debian). Offline preview builds without the feature.

In Rust, use `device::DeviceOutput::default_device()` to select an output, build
at its rate, then call `output.play(runtime)`. For a prebuilt `SynthRuntime`,
`device::play_default_device` still requires an exact sample-rate match.
Playback starts after at least two complete batches are queued. The queue holds
up to three batches; the worker checks for space for an entire batch before
polling the graph again.
The callback fills whatever buffer the host requests, using silence only where
ready audio runs out. On stop, the example reports frames admitted to the queue,
graph frames consumed, and device frames submitted (including inserted silence).
These are not measurements of what has already reached the listener. Stopping
discards any queued audio; `live_play` does not reload or replace a circuit.

### Live-editing a running session

```sh
cargo run --locked --features audio-device --example live_runtime
# Or start from a file instead of the quiet 220 Hz tone:
cargo run --locked --features audio-device --example live_runtime -- examples/graphs/two-tone.sf
```

`live_runtime` uses the same startup format selection, 1,024-frame batches, and
bounded queue as `live_play`. Open an editor while it runs,
save a complete graph such as `sine(440) -> gain(0.08) -> out` to `edit.sf`, and
type `load edit.sf` in the running example's terminal. Paths are relative to the
example's working directory and must refer to regular files, not pipes or
devices. File reads run on the input thread, so a stalled read can delay terminal
commands. Ctrl-C still stops playback.
`load` reads the file only on command and keeps the same device session open.
Each submission gets a revision number. If two edits are already in flight,
wait for one to finish and retry.

After hearing the 440 Hz tone, save `sine(999999) -> out` and run `load edit.sf`
again. The frequency is above Nyquist (half the session rate), so the edit is
rejected and the tone continues. Then save `sine(330) -> gain(0.08) -> out`,
reload, and listen for the lower tone. A file read error also leaves the
current graph alone.

Use `quit`, EOF, or Ctrl-C to stop. Ctrl-C does not require another terminal
input line. A stdin read error stops playback and exits with an error. On a
normal stop, the example discards queued audio and reports prefilled and
admitted graph frames, plus the callback's submitted frames split into graph
audio and starvation silence. These are counts, not audible positions.

There is no live recording in this example; use the preview command for
offline WAV output. The hardware-free `runtime_end_to_end` test writes and
reads back a temporary bounded WAV from its fake output's admitted frames.

### Replacing a circuit from Rust

Use `runtime.control().submit_replacement(dsl)` from a Tokio runtime to submit a
complete live circuit without reopening the device. Preparation runs on Tokio's
blocking pool with the session's existing sample rate and batch size. The returned
request has a session-local revision and reports events via `next_event().await`.
`Accepted` means preparation passed and the edit entered the pending slot, not
that it is active. A valid request can supersede the one pending replacement;
invalid requests do not displace it. Future UI or API adapters can use this same
control handle and feedback without changing the worker or callback.

The producer reports `Scheduled { frame }` when it selects the next root window
and `Applied { frame }` once the new stream is installed. The frame is an absolute
graph frame, not a playback timestamp: previously queued audio is left in order,
and the new stream may not have produced audio yet. The old window (including any
retained remainder) finishes first. Starvation silence can add device time without
advancing graph frames. A rejected replacement keeps the old circuit; a later
stream or output failure stops the run. New processors start with fresh state.
Clicks and discarded old filter tails are possible; there is no crossfade.
