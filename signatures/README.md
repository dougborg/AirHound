# Signature packs, research and admission

[Project home](../README.md) · [Documentation index](../docs/README.md) · [Contribution workflow](../CONTRIBUTING.md#adding-device-signatures)

The production source of truth is [builtin.v2.json](builtin.v2.json). Its compiler
generates the shared Rust tables and CI rejects drift. See [pack authoring and
revision rules](../docs/signature-packs.md). The initial compiled profile preserves
existing behavior; every migrated entry remains explicitly unvalidated.

## Research pack

The [research pack](../schemas/examples/research-devices.sigs.json) adds 16
candidate identifiers and six conservative rules for glasses, Axon/TASER
hardware, Tile services and Meshtastic interfaces. It is **offline research data**:
firmware and `hostd` do not load this file or enable these alerts. The compiled
production database is generated from builtin.v2.json; runtime loading is tracked
in #14.

Each identifier has a source, scope and explicit unvalidated status in
[research-provenance.json](research-provenance.json). The packet metadata in
[fixtures/research-devices.json](fixtures/research-devices.json) is synthetic.
The fixtures check rule policy, including shared-vendor false positives; they
are not captured device evidence or measurements of sensitivity/specificity.
The JSON file conforms to the existing signature v1 schema and is checked by
`just check-examples`. Policy and provenance tests run with:

```sh
python3 -m unittest discover -s scripts/tests -p test_signature_candidates.py -v
```

## Admission policy

A manufacturer ID establishes a claimed vendor, not a product. A service UUID
establishes a claimed service, not ownership or use. Names and advertisements
can be edited or spoofed. A positive rule must not imply recording, surveillance,
law-enforcement ownership or that a device is following someone.

The glasses rules require a brand-name clue and a vendor/custom-service clue in
**the same observation**. The Axon rule requires both a company ID and an assigned
service. These deliberately conservative combinations can miss real devices
that do not advertise all fields together. Tile and Meshtastic rules identify
service candidates only, with correspondingly limited labels. No arbitrary
confidence percentage is assigned without a labeled validation dataset.

Before promotion to compiled defaults:

1. Obtain consented labeled captures from a known device, recording model,
   firmware, pairing state, receiver, scan mode and complete AD fields. Include
   non-target devices from the same vendor and near-matching frames.
2. Verify the identifier's context and length. Never search arbitrary packet
   bytes for a short UUID or promote a generic OUI into product identification.
3. Record source revision, upstream license, permitted reuse, limitations and
   model/state coverage. A detector repeating another detector is not an
   independent validation source.
4. Add positive, malformed/truncated and nearby-negative cases to the actual
   parser/filter tests, then test end-to-end on supported receiver backends.
5. Keep ordinary classification separate from alerts and threat scoring. Finish
   confidence/evidence work (#154) before introducing broad vendor candidates.

The new descriptions, compositions and synthetic fixtures are original
AirHound work under the repository MIT license. Only selected factual protocol
identifiers are recorded. No upstream code, binaries or bulk signature tables
were copied or relicensed. Source licenses in the research report describe the
referenced projects; they do not grant permission to copy every embedded dataset.

See [the opportunity matrix](../docs/signature-research.md) for broader uses and
additional protocol work that this v1 candidate pack cannot yet represent safely.
