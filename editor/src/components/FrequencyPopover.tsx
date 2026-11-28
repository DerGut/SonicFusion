import { useLayoutEffect, useRef, useState, type RefObject } from "react";
import { createPortal } from "react-dom";

import { DETUNE_STEP, detuneBounds, formatCents, hzToBpm, hzToMidi, noteName, parseFrequency, pitchToHz, PITCH_FLOOR_HZ, type FrequencyMode, type Pitch } from "../model/pitch";
import { ParameterRange } from "./ParameterRange";

const WHITE_KEYS_PER_OCTAVE = 7;
const BLACK_KEY_BOUNDARY: Record<number, number> = { 1: 1, 3: 2, 6: 4, 8: 5, 10: 6 };
const FREQUENCY_MODES = [["hz", "Hz"], ["note", "Note"], ["bpm", "BPM"]] as const;
const octaveOf = (midi: number) => Math.floor(midi / 12) - 1;

export function FrequencyPopover({ id, anchor, label, value, pitch, min, max, mode, onModeChange, onClose,
  onBeginContinuous, onFrequency, onPitch }: {
  id: string;
  anchor: RefObject<HTMLButtonElement | null>;
  label: string;
  value: number;
  pitch: Pitch | null;
  min: number;
  max: number;
  mode: FrequencyMode;
  onModeChange?: (mode: FrequencyMode) => void;
  onClose: () => void;
  onBeginContinuous: () => void;
  onFrequency: (hz: number) => void;
  onPitch: (pitch: Pitch, continuous: boolean) => void;
}) {
  const panel = useRef<HTMLDivElement>(null);
  const exact = useRef<HTMLInputElement>(null);
  const [text, setText] = useState<string | null>(null);
  const pendingText = useRef<string | null>(null);
  const bpm = mode === "bpm";
  const exactValue = String(bpm ? hzToBpm(value) : value);
  const [browsedOctave, setBrowsedOctave] = useState<number | null>(null);
  const floorHz = Math.max(min, PITCH_FLOOR_HZ);
  const detune = pitch && detuneBounds(pitch.midi, floorHz, max);
  const lowestOctave = octaveOf(Math.ceil(hzToMidi(floorHz)));
  const highestOctave = Math.max(lowestOctave, octaveOf(Math.floor(hzToMidi(max))) - 1);
  const octave = Math.min(highestOctave, Math.max(lowestOctave, browsedOctave ?? octaveOf(pitch?.midi ?? 0)));

  const updateText = (draft: string | null) => {
    pendingText.current = draft;
    setText(draft);
  };
  const commit = (report: boolean) => {
    const draft = pendingText.current;
    if (draft === null) return;
    const hz = draft === exactValue ? value : draft.trim() ? parseFrequency(draft, mode) : NaN;
    if (!Number.isFinite(hz) && report) {
      exact.current?.setCustomValidity(bpm ? "Enter a number in BPM." : "Enter a frequency in Hz or a note such as C#4.");
      exact.current?.reportValidity();
      return;
    }
    // Consume the draft before updating the graph. A click can trigger both blur and submit.
    updateText(null);
    exact.current?.setCustomValidity("");
    if (Number.isFinite(hz) && hz !== value) {
      onFrequency(hz);
      setBrowsedOctave(null);
    }
  };

  useLayoutEffect(() => {
    const element = panel.current;
    const button = anchor.current;
    if (!element || !button) return;
    const place = () => {
      const rect = button.getBoundingClientRect();
      const { offsetWidth: width, offsetHeight: height } = element;
      const margin = 8;
      const below = rect.bottom + margin;
      const above = rect.top - height - margin;
      const top = below + height <= window.innerHeight - margin ? below : above;
      element.style.left = Math.max(margin, Math.min(rect.left, window.innerWidth - width - margin)) + "px";
      element.style.top = Math.max(margin, Math.min(top, window.innerHeight - height - margin)) + "px";
    };
    place();
    exact.current?.focus({ preventScroll: true });
    const resize = new ResizeObserver(place);
    resize.observe(element);
    const dismissOutside = (event: Event) => {
      const target = event.target;
      if (target instanceof Node && !element.contains(target)) {
        // A pointer can dismiss the overlay before the input would lose focus naturally.
        exact.current?.blur();
        if (!button.contains(target) || event.type === "wheel") onClose();
      }
    };
    window.addEventListener("resize", place);
    window.addEventListener("pointerdown", dismissOutside, true);
    window.addEventListener("focusin", dismissOutside, true);
    window.addEventListener("wheel", dismissOutside, { capture: true, passive: true });
    return () => {
      resize.disconnect();
      window.removeEventListener("resize", place);
      window.removeEventListener("pointerdown", dismissOutside, true);
      window.removeEventListener("focusin", dismissOutside, true);
      window.removeEventListener("wheel", dismissOutside, true);
    };
  }, [anchor, onClose]);

  const closeAndFocus = () => {
    exact.current?.blur();
    onClose();
    anchor.current?.focus({ preventScroll: true });
  };

  return createPortal(
    <div className="frequency-popover nodrag nowheel nokey" ref={panel} id={id}
      role="dialog" aria-label={"Edit " + label}
      onKeyDown={(event) => {
        if (event.key === "Escape") {
          event.preventDefault();
          event.stopPropagation();
          updateText(null);
          closeAndFocus();
        }
      }}>
      <div className="frequency-popover-head">
        {pitch && !bpm ? (
          <>
            <button type="button" aria-label="Octave down" disabled={octave <= lowestOctave}
              onClick={() => setBrowsedOctave(octave - 1)}>‹</button>
            <span>C{octave} – B{octave + 1}</span>
            <button type="button" aria-label="Octave up" disabled={octave >= highestOctave}
              onClick={() => setBrowsedOctave(octave + 1)}>›</button>
          </>
        ) : <span>{label}</span>}
        <button type="button" className="frequency-popover-close" aria-label="Close frequency editor"
          onClick={closeAndFocus}>×</button>
      </div>
      {pitch && !bpm && (
        <>
          <div className="frequency-keys" role="group" aria-label="Note keyboard">
            {Array.from({ length: 24 }, (_, index) => {
              const midi = (octave + 1) * 12 + index;
              const boundary = BLACK_KEY_BOUNDARY[index % 12];
              const next = { midi, cents: pitch.cents };
              const hz = pitchToHz(next);
              return (
                <button key={midi} type="button" aria-label={noteName(midi)} title={noteName(midi)}
                  className={boundary ? "black-key" : "white-key"}
                  aria-pressed={midi === pitch.midi} disabled={hz < floorHz || hz > max}
                  style={boundary ? { left: (Math.floor(index / 12) * WHITE_KEYS_PER_OCTAVE + boundary) * 100 / (2 * WHITE_KEYS_PER_OCTAVE) + "%" } : undefined}
                  onClick={() => {
                    setBrowsedOctave(octave);
                    updateText(null);
                    if (midi !== pitch.midi) onPitch(next, false);
                  }} />
              );
            })}
          </div>
          {detune && (
            <>
              <div className="param-top">
                <label htmlFor={id + "-detune"}>Detune</label>
                <span>{formatCents(pitch.cents)}</span>
              </div>
              <ParameterRange id={id + "-detune"} min={detune.min} max={detune.max} step={DETUNE_STEP} position={pitch.cents}
                valueText={formatCents(pitch.cents)} onBeginContinuous={onBeginContinuous}
                onMove={(cents) => {
                  updateText(null);
                  onPitch({ midi: pitch.midi, cents }, true);
                }} />
            </>
          )}
        </>
      )}
      {onModeChange && (
        <div className="frequency-unit" role="radiogroup" aria-label="Frequency unit">
          <span aria-hidden="true">Frequency unit</span>
          {FREQUENCY_MODES.map(([unit, name]) => (
            <label key={unit}>
              <input type="radio" name={id + "-unit"} checked={mode === unit} disabled={unit === "note" && !pitch}
                onChange={() => onModeChange(unit)} />
              {name}
            </label>
          ))}
        </div>
      )}
      <form className="frequency-exact" onSubmit={(event) => {
        event.preventDefault();
        commit(true);
      }}>
        <label htmlFor={id + "-exact"}>{bpm ? "BPM" : "Hz or note"}</label>
        <input id={id + "-exact"} ref={exact} className="value-input nodrag" value={text ?? exactValue}
          onFocus={(event) => event.currentTarget.select()} onBlur={() => commit(false)} onChange={(event) => {
            event.currentTarget.setCustomValidity("");
            updateText(event.currentTarget.value);
          }} />
        <button type="submit" onPointerDown={(event) => {
          // A pointer submit reports syntax errors before blur can clear the draft.
          if (document.activeElement === exact.current) event.preventDefault();
        }}>Set</button>
      </form>
    </div>,
    document.body,
  );
}
