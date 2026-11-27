# Graph editor designs

Prototypes for the interactive patch editor, exported from the design canvas:
https://claude.ai/artifact/JDb1iPEKm6w1gPYGtceiAX (private).

## Build target

`Studio.dc.html` (v2) is the chosen direction:

- A free canvas with nodes, and a node palette on the left.
- Every node has inline parameter sliders.
- The Output node has a live segmented level meter with a draggable ceiling.
- Selecting a node opens a Scope/Spectrum probe for that node's output.

`Anatomy.dc.html` defines the shared vocabulary, which is approved:

- Audio: round ports and solid amber wires.
- Modulation: diamond ports and dashed blue wires.
- Snapping, rejected and replace states.
- Parameter states.
- The menu that opens when a wire is dropped on empty canvas.

## Explorations (for reference)

- `Main.dc.html`: A · studio graph (base of v2)
- `Rack.dc.html`: B · modular rack (source of the level meter)
- `Code.dc.html`: C · code + auto-layout. Its Graph/Split/Code view is deferred to #28.

## Caveats

- Parameter modulation (diamond ports, depth, the DSL syntax shown in C) is **proposed**; the engine doesn't support it yet.
- The signals in v2 are simulated in the browser; nothing is connected to the engine.
- The `.dc.html` files need the design-canvas runtime (`support.js`) to render. Use them as a visual and behavioural spec, not as code to ship.
