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
