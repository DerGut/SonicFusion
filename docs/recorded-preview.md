# Recorded preview

Run `scripts/preview` from the repository to build the `preview` binary,
render its sine → gain graph for one second at 48 kHz, write a fresh float WAV,
and play it. Edit `src/bin/preview/dag.rs` to change the graph. The player is
`afplay` on macOS or `ffplay -nodisp -autoexit` on Linux. If neither is suitable,
pass `--player COMMAND` and repeat `--player-arg ARG` for its options; the WAV
path is appended as the final argument.

For example:

```sh
scripts/preview --seconds 0.5 --waveform
scripts/preview --seconds 2 --player ffplay --player-arg -nodisp --player-arg -autoexit
```

`--output-dir DIR` changes where results are written. Each invocation creates
a new `preview-*.wav` path, so a failed run cannot play an earlier recording.
The optional SVG has the same basename. The command reports the WAV path before
starting the player. Render, sample validation, file, and player errors have
distinct messages; a nonzero player exit makes the command fail.

The shell entry point reports build time separately. The binary reports graph
construction, time from binary start to the first decoded batch, total render
and validation time, output preparation, player launch, and time from binary
start to player launch. Player launch is measured at process spawn; confirming
time to audible output still requires listening. The WAV is the same graph and format
as `examples/render.rs` when both use their default one-second configuration.

The binary can also be run directly with `cargo run --bin preview -- --seconds 0.5`.
