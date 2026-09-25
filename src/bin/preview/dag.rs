pub(super) const DEFAULT_GRAPH: &str = "\
sine(440.0) -> $tone;
square(435.0, 0.2) -> $pulse;
mix([$tone, $pulse], [0.5, 0.5]) -> lpf(1000.0) -> gain(0.5) -> out
";
