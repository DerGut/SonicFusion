# SonicFusion

DataFusion, but for sound.

The preview binary can build a synth graph from a text file:

```sh
scripts/preview --graph examples/graphs/two-tone.sf --seconds 1
```

After the first build, run `target/debug/preview --graph examples/graphs/two-tone.sf`
directly to try edits without invoking Cargo.

The graph language supports `sine(frequency)`, `square(frequency, pulse_width)`, `sawtooth(frequency)`,
`lpf(cutoff)`, `gain(factor)`, `mix(inputs, weights)`, and `out`.
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

`sawtooth(frequency)` generates a rising bipolar ramp from -1 toward +1,
wrapping to -1 each cycle. Frequency is in hertz, must be nonnegative and below
Nyquist, and zero holds -1. The Rust API is
`FrameSawToothOscExec::try_new(&config, frequency_hz)` in `physical::frame`.
Like the square oscillator, the sawtooth is not band-limited.

## Direct playback

```sh
cargo run --features audio-device --example live_play
# Or pass a graph file:
cargo run --features audio-device --example live_play -- examples/graphs/two-tone.sf
```

The first command plays a quiet test tone until Ctrl-C. The example uses 48 kHz
and 1,024-frame processing batches. It requires native `f32` mono or two-channel
output at that rate on the default device; two-channel output duplicates mono.
No WAV, resampling, or device switching is involved. Linux builds with this
feature require ALSA development libraries (e.g. `libasound2-dev` on Debian).
Offline preview builds without the feature.

`device::play_default_device` takes a `SynthRuntime` and starts playback after
at least two complete batches are queued. The queue holds up to three batches;
the worker checks for space for an entire batch before polling the graph again.
The callback fills whatever buffer the host requests, using silence only where
ready audio runs out. On stop, the example reports frames admitted to the queue,
graph frames consumed, and device frames submitted (including inserted silence).
These are not measurements of what has already reached the listener. Stopping
discards any queued audio; this example does not reload or replace a circuit.
