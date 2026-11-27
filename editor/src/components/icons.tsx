import type { NodeKind } from "../api/types";

const strokes: Record<NodeKind, string[]> = {
  sine: ["M1 8C3.5 2 5.5 2 8 8S12.5 14 15 8"],
  square: ["M1 11H3V5H8V11H13V5H15"],
  sawtooth: ["M1 12L7 4V12L13 4V12"],
  noise: ["M1 8L3 5L5 12L7 3L9 10L11 6L13 12L15 8"],
  low_pass: ["M1 5H8C11 5 12 8 15 13"],
  gain: ["M3 13V10", "M8 13V6.5", "M13 13V3"],
  mix: ["M1 4C6 4 7 8 10 8", "M1 12C6 12 7 8 10 8", "M10 8H15"],
  output: ["M2 6H5L9 3V13L5 10H2Z", "M12 5.5C13 6.5 13 9.5 12 10.5"],
};

export function KindIcon({ kind, size = 14 }: { kind: NodeKind; size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 16 16" fill="none" stroke="currentColor"
      strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      {strokes[kind].map((d) => <path key={d} d={d} />)}
    </svg>
  );
}

type IconName = "undo" | "redo" | "play" | "stop" | "export" | "search" | "close" | "plus" | "minus" | "check" | "alert" | "probe" | "edit";

const icons: Record<IconName, string[]> = {
  undo: ["M6 3.5L3 6.5L6 9.5", "M3 6.5H10C12 6.5 13 8 13 10S12 13.5 10 13.5H8"],
  redo: ["M10 3.5L13 6.5L10 9.5", "M13 6.5H6C4 6.5 3 8 3 10S4 13.5 6 13.5H8"],
  play: ["M4.5 3L13 8L4.5 13Z"],
  stop: ["M3.5 3.5H12.5V12.5H3.5Z"],
  export: ["M8 2V10", "M5 7L8 10L11 7", "M3 12V14H13V12"],
  search: ["M10.5 10.5L14 14", "M7 2.5A4.5 4.5 0 1 1 7 11.5A4.5 4.5 0 1 1 7 2.5Z"],
  edit: ["M3 10L10.5 2.5L13.5 5.5L6 13L2 14Z", "M9 4L12 7"],
  close: ["M3.5 3.5L12.5 12.5", "M12.5 3.5L3.5 12.5"],
  plus: ["M8 3V13", "M3 8H13"],
  minus: ["M3 8H13"],
  check: ["M3 8.5L6.5 12L13 4"],
  alert: ["M8 4.5V8.5", "M8 11V11.2", "M8 1.5A6.5 6.5 0 1 1 8 14.5A6.5 6.5 0 1 1 8 1.5Z"],
  probe: ["M8 5.5A2.5 2.5 0 1 1 8 10.5A2.5 2.5 0 1 1 8 5.5Z", "M8 1.5V4", "M8 12V14.5", "M1.5 8H4", "M12 8H14.5"],
};

export function Icon({ name, size = 14 }: { name: IconName; size?: number }) {
  const filled = name === "play" || name === "stop";
  return (
    <svg width={size} height={size} viewBox="0 0 16 16" fill={filled ? "currentColor" : "none"}
      stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      {icons[name].map((d) => <path key={d} d={d} />)}
    </svg>
  );
}
