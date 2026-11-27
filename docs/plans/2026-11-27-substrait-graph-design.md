---
title: "Substrait as a SonicFusion plan format"
status: exploratory
date: 2026-11-27
---

# Substrait as a SonicFusion plan format

**Scope:** [#27 — Graph format and local adapter](https://github.com/DerGut/SonicFusion/issues/27). The frontend prototype and [#6 — API/server](https://github.com/DerGut/SonicFusion/issues/6) can proceed separately. This note recommends what to test; it does not select a public format, transport, or endpoint contract.

## Recommendation

Use a typed SonicFusion graph as the common model for validation and lowering. Keep open whether it needs a public native encoding. **Probe Substrait as an interchange format for feed-forward plans, not as the sole definition of a patch.** Use one extension relation for the oscillator and a standard `ProjectRel` for constant gain as the initial experiment. Then try a two-input filter and a graph with real fan-out before calling the format viable. A little native graph notation alongside the Substrait examples makes it possible to see what the envelope actually buys.

A successful sine/gain demo does not settle the format for feedback, stateful fan-out, or channel routing. If those are central to the frontend's first patches and the important semantics live almost entirely in SonicFusion protobuf messages, prefer a native graph payload for the app. Substrait can still be useful for exporting or composing acyclic sections where another producer or consumer can use the relational structure.

## What Substrait buys us

- **A typed, cross-language representation for relational plans.** Substrait serializes higher-level operations and their inputs, column mappings, types, and function bindings rather than a DataFusion `ExecutionPlan`. This is useful for inspecting plans, composing known operations, and exchanging them between independently written producers and consumers. Orderedness and distribution can be described; they do not imply a dense audio timeline, sample accuracy, or deadlines. [Relation basics](https://substrait.io/relations/basics/) · [Binary serialization](https://substrait.io/serialization/binary_serialization/)
- **Reusable, standard expressions where the operation fits.** The pinned schema has an arithmetic `multiply` and `sin`. Constant gain is multiplication over a sample field, with the frame field preserved. A sine expression *over an existing frame source* is possible, but that is not an oscillator source. It doesn't create frames, and making it equivalent to SonicFusion would still require specifying sample-rate binding, phase and numerical behavior. An extension function declaration names a signature; it neither supplies code nor requires a one-for-one DataFusion UDF. [Scalar functions](https://substrait.io/expressions/scalar_functions/) · [Pinned arithmetic definitions](https://github.com/substrait-io/substrait/blob/2aaae7c0d369a23b5b469badce0644097f1769a1/extensions/functions_arithmetic.yaml)
- **Composition with domain-specific operators.** `ExtensionLeafRel`, `ExtensionSingleRel`, and `ExtensionMultiRel` carry a protobuf `Any` description and zero, one, or multiple relational children respectively. The extension defines how the output schema and properties follow from its inputs and parameters. Generic tooling can see the relation spine, but a consumer without our extension definitions can't execute the DSP. Each relation has at most one relational output; multiple logical ports would need a defined mapping to fields or an extension-owned model. [Custom relations](https://substrait.io/extensions/#custom-relations)
- **References, not a scheduling policy.** `ReferenceRel` points to another relation by a plan-local ordinal and expresses shared subtrees in a DAG. It isn't a stable editor node ID, a guarantee of one live execution with multiple subscribers, or a causal feedback edge. Detached ordinals in the rolling spec solve nesting, not computation sharing; they are not in the candidate pinned `PlanRel`. Dynamic parameters offer value binding, not a sample-timed event stream. [Reference operator](https://substrait.io/relations/logical_relations/#reference-operator) · [Detached relations](https://substrait.io/serialization/binary_serialization/#detached-relations-and-expressions) · [Dynamic parameters](https://substrait.io/relations/basics/#dynamic-parameters)

## What we're describing

One graph describes a logical output signal. A frame is a time position across its channels. A node's inputs have roles (e.g. `audio` and `cutoff`), and signals have a channel layout and a rate/clock contract. Units and mappings matter: a normalized LFO sample, a value in Hz, and an audio sample might all be floats but aren't interchangeable. Batch boundaries must not affect meaning. Keep the physical Arrow layout, buffer sizes, partitioning, device selection, and actual replacement frame out of the graph definition. A logical sample-rate requirement is reasonable; the session supplies the actual rate in the first slice and rejects incompatible graphs rather than resampling implicitly.

| Concern | Existing behavior and implication |
| --- | --- |
| Preparation | [`PreparedGraph`](../../src/dsl.rs) is a topologically ordered list rooted at `out`, plus a source offset for DSL diagnostics. Share graph validation and lowering between DSL and Substrait; keep source offsets and wire paths in their respective diagnostics. Don't make the internal vector index a public node ID. |
| Timeline | [`ExecutionConfig`](../../src/config.rs) supplies sample rate, batch capacity and absolute start frame. The [live runtime](../../src/runtime/mod.rs) binds execution at the replacement boundary. A finite limit belongs to the preview, and processor history starts fresh on replacement; neither a current cursor nor history should be serialized into a patch. |
| Modulation | The [modulated LPF](../../src/physical/frame/low_pass_filter.rs) consumes aligned normalized control samples and maps them to cutoff Hz with a sample-rate-dependent policy. It operates across different batch boundaries. Preserve this policy for parity; don't silently reinterpret the second input as raw Hz or unbounded values. Keep the port and mapping separate so an explicit Hz policy can be added later. |
| Fan-out | The current [mixer](../../src/physical/frame/mix/mod.rs) calls `execute` for each child; cloned plan handles don't share a stream. Distinguish one graph node referenced twice from two identical nodes. If the wire promises a single execution for a shared stateful producer, implement and test that promise before accepting it. |
| Layout | Current DSP and live rendering are mono. [#7](https://github.com/DerGut/SonicFusion/issues/7) calls for distinct mono/stereo contracts and explicit conversion. Preserve channel roles and synchronization without exposing planar, interleaved, or fixed-size-list storage in the patch. |

## Candidate encodings

The sketches below show semantic shape, not committed syntax or protobuf fields.

### A. Relational spine with typed DSP relations

```text
RelRoot(frame, sample)
  ProjectRel(frame, multiply(sample, fp32(0.25)))
    ExtensionLeafRel(Sine { frequency_hz: 440 })

ExtensionMultiRel(LowPass { cutoff_mapping: DslV1Normalized }, inputs=[
  audio: ExtensionLeafRel(Sine { frequency_hz: 440 }),
  cutoff: ReferenceRel(lfo_ordinal)
])
```

In a real `ProjectRel`, use its emit mapping to keep `frame` and the computed sample, not the unscaled sample. Resolve `multiply` by its extension URN and signature, not a hard-coded function anchor. The LPF extension would define which input positions are `audio` and `cutoff`, how frame alignment works, and the output schema. The reference targets a `PlanRel` within the same plan; `lfo_ordinal` isn't a persistent identity. A bounded offline prototype could explore a relational join for alignment, but a generic join over continuous inputs isn't a substitute for the LPF's streaming cursor contract.

This option exposes more to standard tooling. It works naturally for expressions, single-output sources and transforms, and multi-input feed-forward DSP. A stereo `Pan` extension could take `(frame, sample)` and expose `(frame, left, right)`; those are two fields on one relation, not two independently clocked streams. Splitting into independent effects would use separate projections of the same stereo subtree, with the same fan-out caveat as mono. Joining them again requires frame agreement and an explicit stereo-combine contract. Stable editor identity across reordering would need separate metadata or extension semantics. If supporting standard `ProjectRel` requires more than one strict pattern to recognize SonicFusion gain, compare it to a typed unary `Gain` relation: less general, but potentially smaller and clearer than implementing a general expression evaluator just to multiply.

### B. Native graph (or an opaque Substrait extension for one DSP region)

```text
nodes: [ osc: Sine(440 Hz), sum: Mix, delay: Delay(1 frame, initial=zero) ]
edges: [ osc.signal -> sum.dry, delay.signal -> sum.feedback, sum.signal -> delay.input ]
output: sum.signal
```

Named node/port identities make feedback and editor round-trips direct. They also describe branches like `osc.signal → pan.audio → stereo_out`, with an explicit pan law and left/right roles, or `split.left` and `split.right` feeding separate effects. If an application already exchanges Substrait, the example could appear as `ExtensionSingleRel(FeedbackRegion { sum, delay, internal_edges, output: sum.signal }, input=osc)`, with the external child bound to `sum.dry`. The surrounding plan stays acyclic; only SonicFusion sees and validates the internal loop. Don't add an opaque Substrait envelope just to claim compatibility: if the rest of the plan is always empty, the native payload alone is simpler. Conversely, a genuinely useful relational spine outside a DSP region is a good reason to keep the boundary.

### Why not UDFs for every node?

Scalar expressions are a good fit for pure per-frame math and explicit channel arithmetic. Packing an entire signal into a scalar/list to make an oscillator or filter look like a UDF loses ordinary streaming composition and leaves state and chunking semantics unspecified. An LPF's own history is not a graph cycle; model it as a stateful operator that consumes frames. A graph cycle is a separate scheduling problem: for example, `y[n] = x[n] + g*y[n-1]` uses an explicit delay and an initial value. The pinned schema has no general DSP feedback construct; even relational recursion over sets of rows wouldn't define per-frame ordering, initialization, or execution of feedback. A future causal scheduler should reject instantaneous cycles, for example by validating that the graph is acyclic after removing dependencies that cross an explicit positive delay. A one-frame delay must stay one frame if we change the Arrow batch size. For now, reject all graph cycles rather than inventing a special interpretation for cyclic `ReferenceRel`.

## Pinned implementation reality

- The current repo pins DataFusion `54.1.0` and does not yet depend on Substrait. The matching [`datafusion-substrait` manifest](https://github.com/apache/datafusion/blob/54.1.0/datafusion/substrait/Cargo.toml) uses Rust `substrait` `0.63.0` as its minimum compatible version in that series. The inspected `0.63.0` crate bundles **Substrait spec `0.85.0`** ([vendored spec revision](https://github.com/substrait-io/substrait/tree/2aaae7c0d369a23b5b469badce0644097f1769a1/proto/substrait)). The crate version, wire schema version, and SonicFusion semantic extension version are distinct. Fixture generation must use the same selected wire schema as decoding, not whatever the rolling website shows.
- DataFusion's pinned [logical consumer trait](https://github.com/apache/datafusion/blob/54.1.0/datafusion/substrait/src/logical_plan/consumer/substrait_consumer.rs) and [producer trait](https://github.com/apache/datafusion/blob/54.1.0/datafusion/substrait/src/logical_plan/producer/substrait_producer.rs) have extension hooks. The stock logical conversion entry point accepts just one top-level relation, and the default relation dispatcher doesn't consume `ReferenceRel` ([plan](https://github.com/apache/datafusion/blob/54.1.0/datafusion/substrait/src/logical_plan/consumer/plan.rs), [dispatch](https://github.com/apache/datafusion/blob/54.1.0/datafusion/substrait/src/logical_plan/consumer/rel/mod.rs)). Its [physical consumer](https://github.com/apache/datafusion/blob/54.1.0/datafusion/substrait/src/physical_plan/consumer.rs) isn't a converter for custom synth operators. These limits are not limits of Substrait itself.
- For the spike, use generated Substrait protobuf types and a small, allowlisted lowering path into the shared graph. Don't add a DataFusion logical planner layer only to turn it back into the same synth nodes. Reconsider the logical hooks if we have a concrete need to compose general DataFusion expressions or relational workloads.

## Spike and decision gates

1. **Basic parity.** Check in a fixture and a reproducible way to generate it from the pinned schema (ideally independent of the Rust decoder). Decode `sine(440) → gain(0.25) → out` and render a finite excerpt through the same lowering as DSL. Compare frame coverage and samples at a nonzero origin and two batch capacities. Compare a standard project and a custom unary gain encoding if the project needs excessive machinery. Define the logical frame field explicitly before freezing such a project: core `i64` is not the runtime's `u64`; a signed choice needs range checks and a documented limit.
2. **Modulation and sharing.** Sketch and decode `Sine → LPF(audio, cutoff=LFO) → out`; compare to the existing DSL mapping with mismatched upstream batch boundaries. Then use one referenced producer in two input paths. Verify identity survives decoding (compare graph structure, not protobuf byte equality), check how many streams execute, and reject shared nodes whose required execution semantics we can't provide. Don't hide the distinction by expanding all references into copies.
3. **Evolution probes, design only.** Sketch typed `mono → pan → stereo out`, `stereo → split → independent history → combine`, and the one-frame feedback loop above in both formats. Record where port identity, state scope, mapping, and timing are declared, and what a generic consumer can see. Ensure the current decoder can reject each unsupported feature rather than silently changing it. New DSP is not part of #27.
4. **Validation.** Pin allowed relations, expression patterns, versions, and semantic `Any` types. Reject unknown type URLs/enhancements, unresolved anchors or ordinals, cycles and missing/multiple output roots outside the supported shape. Don't silently discard known fields that change semantics (emit, mappings, modifiers); don't claim a normal protobuf decoder rejects all unknown wire fields. Use bounded input size, depth, nodes and edges; check graph shape and then session-specific constraints such as Nyquist, port compatibility and finite parameters before replacing anything. The decoder should not resolve external code, tables, files or URLs.

**Adopt a Substrait profile** if these probes keep port types and semantics clear and there is a concrete beneficiary of the standard relational structure beyond being able to serialize protobuf. **Prefer a native app payload** if identity, layout and feedback require mostly opaque extension data or if supported consumers still need a bespoke compiler to do anything useful with the entire plan. Either outcome leaves the existing runtime and the frontend's endpoint discovery independent of this choice.

## Local spike results

The opt-in `substrait-spike` feature lowers pinned Substrait protobuf directly to the graph builder used by the DSL. `substrait::build_live_plan` builds an initial plan; `RuntimeControl::submit_substrait_replacement` takes the same preparation, revision and activation path as DSL replacements. Nothing listens on a socket or sets a public wire contract.

- **Simple graph:** `ProjectRel` works for a narrow pattern: select the input sample, multiply it by a required `fp32` literal, and emit only the computed field. The native gain kernel *saturates*; standard arithmetic multiply does not. This profile only accepts factors in `[-1, 1]`, where normalized input cannot saturate. A general SonicFusion gain node would need explicit saturation semantics, not just an alias to standard multiply.
- **Frame representation:** The experiment uses one required `fp32` field named `sample`, with the frame implicit in the row sequence and session origin. That avoids mapping the runtime's `u64` frame counter to core Substrait `i64`. This only works for the supported dense, frame-order-preserving operators; accepting arbitrary relational rewrites would require preserving or representing that timeline explicitly.
- **Modulation and fan-out:** The wire plan can represent both a separate LFO and two ports referencing the same oscillator. The latter lowers to one shared plan node but runs it separately on each input, which is equivalent for the deterministic, frame-indexed oscillator. Reusing a stateful subtree is rejected rather than treated as implicit replay. Parity tests use two session batch sizes; both oscillators use the same capacity within a session. Differently chunked inputs are covered at the existing LPF operator boundary, not by adding a wire-level batching concept.
- **Decision:** Substrait is viable for this narrow acyclic profile. The fixtures don't demonstrate an interoperability benefit that justifies choosing it as the complete frontend patch model yet. Feedback and stereo remain design probes, and [#6](https://github.com/DerGut/SonicFusion/issues/6) still waits on frontend input.

The [text and binary fixtures](../../tests/fixtures/substrait/sonicfusion_spike.proto) can be regenerated independently of Rust with `bash tests/fixtures/substrait/generate.sh /path/to/substrait-0.63.0/substrait/proto`. Pass `--check` before the path to compare without replacing the fixtures. Building the opt-in feature requires `protoc`. Run `cargo test --locked --features substrait-spike --lib --tests` to exercise the fixtures and live replacement.
