pub(super) const DEFAULT_GRAPH: &str = "\
sine(440.0) -> $tone;
square(435.0, 0.2) -> $pulse;
sine(1.0) -> $cutoff;
mix([$tone, $pulse], [0.5, 0.5]) -> lpf($cutoff) -> gain(0.5) -> out
";
