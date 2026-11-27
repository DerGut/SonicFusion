import { useContext, useEffect, useRef, useState } from "react";

import { formatDb } from "../model/signal";
import { LevelContext } from "./flow";

const FLOOR_DB = -48;
const SEGMENTS = 24;
const PEAK_HOLD_MS = 1500;

/**
 * Segmented peak meter for what the engine is playing. The ceiling is a
 * warning threshold only: the engine has no limiter yet, so nothing is
 * attenuated when the level crosses it.
 */
export function OutputMeter({ ceilingDb, onCeiling }: { ceilingDb: number; onCeiling: (db: number) => void }) {
  const level = useContext(LevelContext);
  const db = level?.peak_dbfs ?? null;
  const [hold, setHold] = useState<number | null>(null);
  const heldAt = useRef(0);

  useEffect(() => {
    const now = performance.now();
    if (db === null) {
      setHold(null);
    } else if (hold === null || db >= hold || now - heldAt.current > PEAK_HOLD_MS) {
      setHold(db);
      heldAt.current = now;
    }
    // Only a new reading moves the hold.
  }, [level]);

  const over = db !== null && db > ceilingDb;
  const step = -FLOOR_DB / SEGMENTS;
  const segments = Array.from({ length: SEGMENTS }, (_, index) => {
    const low = FLOOR_DB + step * index;
    const lit = (db !== null && db > low) || (hold !== null && hold > low && hold <= low + step);
    const zone = low >= ceilingDb ? "over" : low >= -18 ? "hot" : "ok";
    return <span key={index} className={`segment ${lit ? `lit ${zone}` : low >= ceilingDb ? "above" : ""}`} />;
  });
  const ceilingPercent = ((ceilingDb - FLOOR_DB) / -FLOOR_DB) * 100;

  return (
    <div className="meter nodrag">
      <div className="param-top">
        <span>Level</span>
        <span className="value">{level ? formatDb(db) : "not playing"}</span>
      </div>
      <div className="meter-bar" role="meter" aria-label="Output level" aria-valuemin={FLOOR_DB}
        aria-valuemax={0} aria-valuenow={db ?? FLOOR_DB}>
        {segments}
        <span className="meter-ceiling" style={{ left: `calc(${ceilingPercent}% - 1px)` }} />
      </div>
      <div className="meter-scale" aria-hidden="true">
        <span>−48</span><span>−24</span><span>−12</span><span>0</span>
      </div>
      <div className="param-top">
        <label htmlFor="output-ceiling">Warn above</label>
        <span className="value">{formatDb(ceilingDb)}</span>
      </div>
      <input id="output-ceiling" className="ceiling" type="range" min={FLOOR_DB} max={0} step={0.5}
        value={ceilingDb} onChange={(event) => onCeiling(Number(event.target.value))} />
      <p className={`meter-status${over ? " over" : ""}`}
        title="The marker only warns: the engine has no limiter, so nothing is turned down.">
        {over
          ? `Peak is over your ${formatDb(ceilingDb)} warning`
          : hold !== null
            ? `Peak ${formatDb(hold)}`
            : "Engine output, recomputed"}
      </p>
    </div>
  );
}
