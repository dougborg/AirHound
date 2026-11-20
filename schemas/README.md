# Schemas and wire contracts

[Project home](../README.md) · [Documentation index](../docs/README.md) · [Protocol examples and commands](../docs/protocol.md)

These JSON Schemas use draft 2020-12. Schema acceptance describes a record's
structure; it does not prove receiver capability, device identity, delivery or
runtime support for every parsed command.

| Contract | Purpose | Guide |
| --- | --- | --- |
| [signatures.v2](signatures.v2.schema.json) | Production compiled signature profile | [Canonical pack and authoring](../signatures/README.md) |
| [signatures.v1](signatures.v1.schema.json) | Legacy interchange/research examples; not production compiler input | [Research and admission](../signatures/README.md#research-pack) |
| [device-message.v1](device-message.v1.schema.json) | Firmware observations, detection companions, status, command results and counters | [Firmware streaming](../docs/firmware-streaming.md), [evidence](../docs/detection-evidence.md) |
| [host-command.v1](host-command.v1.schema.json) | Commands received by supported runtimes | [Command matrix](../docs/protocol.md#command-results-and-runtime-support) |
| [desktop-ble.v1](desktop-ble.v1.schema.json) | Desktop BLE identity, bounded payloads and evidence | [BLE observation contract](../docs/ble-observation-contract.md) |
| [desktop-ble-packet.v1](desktop-ble-packet.v1.schema.json) | Raw BLE packet and same-packet evidence | [Sniffle/PCAP source](../docs/sniffle-source.md) |
| [desktop-ble-source-status.v1](desktop-ble-source-status.v1.schema.json) | Source lifecycle and queue-drop counters | [Sniffle/PCAP source](../docs/sniffle-source.md) |
| [desktop-remote-id.v1](desktop-remote-id.v1.schema.json) | Decoded BLE Remote ID with snapshot/raw-packet provenance | [Desktop Remote ID](../docs/desktop-remote-id.md) |

[Examples](examples/) illustrate these formats. External receiver formats have
separate contracts: [observation files](../docs/external-wifi-v1.md) and
[live process streams](../docs/external-scanner-stream-v1.md).

## Validate changes

From the repository root, use Python with `check-jsonschema` installed and `just`:

```sh
just check-json
just check-schemas
just check-examples
just check-signatures
```

`check-json` checks formatting under `schemas/`; it is not semantic validation.
`check-signatures` validates the canonical production pack and generated-file
parity. The schema CI job also runs compiler rejection tests and validates
serialized command/evidence fixtures. Keep schemas, examples and actual output
in agreement when changing a contract.

Serial firmware output contains diagnostic logs alongside NDJSON. BLE notifications
need stream reassembly. Desktop stdout is NDJSON, with diagnostics on stderr.
Use the [protocol](../docs/protocol.md) and [streaming](../docs/firmware-streaming.md)
guides before writing a consumer; hostd stdin accepts commands, not sensor events.
