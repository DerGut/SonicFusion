import type { ConstantDestination } from "../model/constant";
import { formatHz } from "../model/signal";

export function ConstantReadout({ destinations }: { destinations: ConstantDestination[] }) {
  return (
    <div className="constant-readout">
      {destinations.length === 0 && <p className="param-note">Connect to FM or cutoff to see Hz</p>}
      {destinations.map((destination) => (
        <div key={`${destination.node}:${destination.port}`} className="constant-destination">
          <p className="constant-heading">
            <span className="param-note" title={`${destination.label} ${destination.port}`}>{destination.label} {destination.port}</span>
            <strong>{formatHz(destination.hz)}</strong>
          </p>
          <p className="param-note">
            Range {formatHz(destination.minHz)}–{formatHz(destination.maxHz)}
          </p>
          {destination.capped && <p className="param-note">Capped below Nyquist</p>}
        </div>
      ))}
    </div>
  );
}
