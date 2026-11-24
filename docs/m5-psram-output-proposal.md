# Proposed bounded M5 output buffering

Review proposal only, based on installed source `3fbc1a01faa51473999c25bad0d3b4335dd78a06`. No implementation, build, or device action is authorized by this document. Preserve the existing failed acceptance receipts and their criteria.

## Evidence and limits

The latest 600.061729-second observation (`eb0956f3` receipt, `f8b09191` raw digest) retained a failed verdict: 107,168 BLE and 2,613 Wi-Fi observations, 17 Status reports versus 19 StreamStats/Memory/Serial-output reports, seven output drops including two diagnostic-log drops, and six scan drops. Wi-Fi diagnostic reports numbered 16. Both final 30-second windows and repeated advertisers passed; no runtime fault marker was found. One final partial BLE line was cut off by the observer; the independent audit found no malformed interior JSON.

`src/main.rs:93–114` allocates four `MsgBuffer` output slots in internal RAM. Observation/detection producers, health records, and the logger all compete for these slots using `try_send`; class attribution is absent except for the log subset. `status_task` yields between records, but this does not reserve admission. `output_serial_task` (`main.rs:1021`) calls synchronous `Writer::write`; `serial_output::drain` spins on a busy FIFO for up to 20 ms and yields only after the line. Interrupts remain enabled between short writes, but this blocks progress of other tasks on the same cooperative executor. These are concrete congestion/scheduling mechanisms; the evidence does not identify the precise producer of every dropped record or prove each scan drop had the same cause.

The retained raw stream is 10,796,724 bytes, about 17,995 B/s over the run. Observed Wi-Fi/BLE lines average 97.15 bytes; maximum 201 bytes. The largest device-timestamp bins contain 4,457 observation bytes/100 ms and 23,138 bytes/s. These are post-loss output measurements, not offered-load or radio-arrival bounds. The UART's nominal 460800/8N1 ceiling is 46,080 B/s. Buffering can absorb finite bursts and improve scheduling; it cannot solve sustained overload or establish RF completeness.

`src/memory.rs:17–64` already tests the entire 2 MiB PSRAM mapping, registers a separate external-capability allocator, and initializes the observation table in place. Last sampled external free space was 2,068,472 bytes. Radio/RTOS allocations remain in the separate 64 KiB internal heap. The latest memory samples do not establish safe headroom: the new minimum was 3,364 bytes and recovery to the initial 3,576-byte sample was absent. Do not reinterpret that as a pass or a proven leak.

## Recommended implementation boundary

Use a **fixed 64-slot PSRAM pool**, each payload at most 512 bytes, with eight slots reserved for Status, StreamStats, and command responses. Payload storage is 32 KiB, roughly 1.6% of the observed free PSRAM, plus a small fixed metadata allocation. Ordinary observations/detections and diagnostic forwarding cannot consume the last eight free slots. This is admission reservation, not output reordering: committed records share one FIFO. The in-flight UART record and any reserved producer slots count against the 64-slot bound. The reserve protects periodic/control evidence from ordinary bursts but is not an unconditional delivery guarantee; its exhaustion must remain visible.

Keep slot lengths, kinds, states, free indices, and FIFO indices in the external allocation, accessed only by normal task code. Keep only the pool pointer, wake primitive, minimal ownership state, and counters in internal RAM. Allocate once from `EXTERNAL` during `memory::init`, before runtime ownership or radio startup. Check alignment/size/allocation failure; fail startup before radio if unavailable. Initialize metadata directly through `MaybeUninit`/raw field writes as the observation table does. **Never construct a 32 KiB array/pool value on the boot stack, use `Box::new(large_value)`, or allocate per record.** No general allocator replacement and no PSRAM task stacks.

Metadata operations use fixed-size free-index/FIFO operations, never a scan over payloads. Any critical section covers only bounded metadata transitions; it must exclude payload copying, formatting, logging, allocation, UART polling, and awaits. Reclaim after failure proceeds one slot at a time outside any long IRQ-masked loop.

Use non-`Clone`, non-`Copy` producer and consumer handles, with private construction and no blanket `Send`/`Sync`. State transitions are `Free → Reserved → Queued → Draining → Free`. A reservation gives exclusive mutable payload access; formatting/serialization occurs outside the metadata lock and without an await. Commit transfers ownership to the queue; failure or reservation `Drop` returns the slot exactly once. A queued slot cannot be freed by the former producer. The UART handle owns its slot across awaits; its `Drop` releases it on normal cancellation/error. Cancellation after a partial line additionally latches UART failure before another writer can run. Panic may abandon slots because the process never resumes; it must not invoke pool cleanup or dereference PSRAM.

Serialization should target a reserved slot directly, eliminating 512-byte message values across awaits. The command task currently holds `MsgBuffer` across `send().await` (`main.rs:1160–1230`); replace that with waiting for a reservation *before* serialization, then immediate commit. Its wait must be cancellation-safe and terminate on writer failure rather than hang forever. Radio/filter producers remain nonblocking. Do not move radio callback/ISR work into this pool; they still enqueue compact scan events.

### Runtime logger and cache restrictions

Do **not** let the global logger write PSRAM directly: dependency logging may occur in interrupt or other non-task contexts. Reuse the present four-slot internal-RAM channel as a dedicated diagnostic staging queue, retaining bounded nonblocking formatting/enqueue behavior and brace/newline sanitization. A small normal-task pump transfers one staged diagnostic into a PSRAM slot and yields; it does not wait for wire transmission. It drains independently of the UART's FIFO and no longer competes with observations for four internal slots. If staging or ordinary pool admission is full, increment the appropriate diagnostic/output counters exactly once; never recursively log the failure. Do not hold a 512-byte staged value over an await: synchronously move/copy it to a reserved PSRAM slot or account the rejection, then yield. Keep the existing four-slot staging capacity until target measurements justify a different value.

This deliberately preserves a bounded internal copy for arbitrary logging contexts rather than assuming an undocumented interrupt-context detector. It does not newly claim that the existing logger/formatting machinery is cache-disabled/IRAM-safe. No PSRAM access is allowed from panic, cache-disabled flash operations, or ISR paths. Firmware currently performs no runtime flash write/OTA operation; adding one later requires explicit exclusion of pool users. Espressif documents that disabling the flash cache also makes external RAM inaccessible ([ESP32 external RAM restrictions](https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-guides/external-ram.html)).

The eight-slot reserve protects the structured Status/StreamStats records and command responses. Memory/Wi-Fi/serial-output diagnostic text still traverses the ordinary logger staging/admission path; the reserve does **not** guarantee its coverage. Those required reports remain independently checked, with counted staging/pool failures and high-water evidence guiding any subsequent design change.

### Cooperative single UART owner

Retain the exact configured UART0 owner and panic handoff. Store only a slot handle, cursor, and fixed deadline in the output future. Copy at most 16 payload bytes from PSRAM into a tiny internal scratch buffer **before** entering the current readiness/write critical section. The existing HAL contract at pinned `esp-hal c8080c3`, `uart/mod.rs:801–825`, says `write_ready=true` makes the next `write` nonblocking; partial progress must still be checked.

On `Blocked`, yield immediately with interrupts enabled. Also yield after at most four successful 16-byte steps (64 bytes) so a continuously ready FIFO cannot monopolize the executor. The prospective cooperative service policy below replaces the original absolute 20 ms record deadline. It starts when transmission begins, not when the record enters the queue. Measure queue residence separately. Do not add blocking flushes or IRQ-masked waits.

Partial hard write error permanently disables normal UART output. An ordinary line timeout instead retains the exclusive draining lease while attempting one LF under a separate fixed 20 ms absolute recovery budget. The original payload is never retried and still counts as one lost record with reason `Timeout`, even if recovery succeeds. FIFO ordering places the LF after the accepted prefix and before any subsequent record. Successful LF recovery releases the slot without closing admission; queued health can then expose the timeout counters. Failure, invalid progress, or cancellation during recovery closes admission; cancellation retains the original timeout attribution rather than counting a second loss. This does not extend the original line deadline or make a timeout pass zero-loss acceptance. Silence in the earlier physical run does not establish that this path was its cause.

Queued rejection/reclamation remains bounded. `custom_pre_backtrace` keeps its existing fatal critical-section ownership transfer and bypasses the pool; the runtime writer must never output after panic. The LF uses the same panic-gated hardware step, never a second UART writer or blocking flush. Preserve newline framing and diagnostic escaping. Target memory and display-overlap latency must be measured again on this revision; no physical recovery result is claimed.

Successful resynchronization means LF acceptance into the ordered UART FIFO, not measured wire completion or host receipt. Both absolute budgets are checked when the executor polls the writer; they do not establish a hard scheduling bound while another task occupies the executor. The additional recovery budget never renews on blocked polls or progress.

Normal UART failure/cancellation closes admission, wakes command waiters with an explicit failure, discards queued records with exactly-once accounting, and ensures outstanding reservations cannot later commit into the closed queue. Owned reservations return on cancellation or their next checked operation; no other task steals or aliases a live reservation. A previously BLE-forwarded record that fails UART transmission has separate UART-loss attribution even if BLE enqueue succeeded. Do not report BLE enqueue as over-air delivery. Fatal panic is the deliberate non-returning exception to normal reclamation.

BLE forwarding remains once per admitted JSON record, before UART transmission, using its existing bounded four-record BLE channel and counters. Use a synchronous copy helper so the existing 512-byte BLE copy is not retained across the UART task's await. Diagnostics are not forwarded as observations. Do not change BLE connection cleanup, RF scan parameters, scan-channel capacity, XIAO buffering, XIAO output behavior, or detection policy in this slice.

Post-implementation review correction: closing UART admission must not disable an otherwise working BLE connection. For newly produced structured JSON after permanent UART closure only, serialize synchronously into a local 512-byte buffer and try the existing bounded BLE channel directly. Ordinary pool-full rejection does not use this fallback. There is no UART call, admission reopening, await, or unbounded queue. Account each UART loss once, distinguish BLE enqueue/full/no-client, and never pass diagnostic text through this route. Already queued JSON retains its one normal BLE attempt. The local fallback buffer must remain outside persistent future state; final target stack measurements must include this correction.

The Closed-path handoff first drains older committed JSON in FIFO order before the new fallback record. This takes at most 64 records and never waits, writes UART, or holds a critical section across payload work. Normal pool producers run on the single Embassy executor. A metadata guard also rejects unexpected concurrent/reentrant handoffs or an outstanding draining lease, rather than permitting overtaking; the writer cannot take another record during the handoff. Such a rejection is reported as `HandoffBusy`, with a dedicated counter and a BLE admission drop when a client is connected. Diagnostic text is discarded/accounted without entering BLE. A real failed-writer-yield/new-producer regression verifies the older records are forwarded once and before the new record.

## Truthful diagnostics

Preserve existing `output_drops`, `log_drops` (subset), `ble_output_drops`, and serialization/error meanings. Add bounded cumulative attribution for observation/detection admission, health admission, command admission/cancellation, diagnostic staging/pool rejection, UART timeout/write/disabled outcomes. Avoid double-counting a dropped log in multiple aggregate increments. Counters should saturate rather than silently wrap; expose saturation if needed for a truthful lifetime bound.

Report fixed capacity/reserve, current occupied/queued counts, lifetime occupancy high-water, and maximum completed queue residence. High-water includes reserved and draining slots so capacity is not understated. Put the small diagnostic record in the reserved health class; record its own admission loss if it cannot be published. Its snapshot is explicitly sampled and may lag later losses. Existing StreamStats JSON remains compatible; additive diagnostics must not be mistaken for new radio observations or proof of zero RF loss.

## Required evidence before deployment

1. Portable tests exercise ownership/reuse, cancellation before and after commit, exhaustion, reserved admission, FIFO framing, and exact typed drop reconciliation. An adversarial fake FIFO alternates blocked/partial progress while an independent producer/button sentinel progresses. Verify a 512-byte record completes under the declared scheduling model; blocked output reaches the no-acceptance watchdog and slow positive progress reaches the immutable total limit. Neither failure may splice records. Test panic handoff at each step and no PSRAM use by the logger/panic adapter.
2. A finite burst test must fail the old four-slot implementation and pass the proposed admitted-load envelope without suppressing losses. A sustained overload test must remain bounded and report losses honestly. Exercise health/commands under observation saturation, staging saturation, no BLE client and a slow BLE client. No busy-waiting producer or unbounded queue/allocation is acceptable.
3. Build actual M5 and XIAO CI targets. Inspect linked memory/task sizes and allocation layout, including the diagnostic pump, command-task state, and UART future. Startup stack reservation must remain **at least 7,168 bytes**, unchanged. Reusing the old output channel for diagnostics avoids a second large DRAM staging allocation; removing command-buffer state may recover space, but only the target ELF establishes the net result. If gates fail, redesign placement/state; do not lower thresholds or enlarge the internal heap blindly.
4. Independently review unsafe pool initialization/aliasing, locks, cancellation, IRQ/cache boundaries, parser compatibility, and exact target artifacts before any flash. Existing current-image rollback remains pinned. The next authorized physical check must retain explicit panic rejection, complete periodic coverage, typed output loss, prospective memory review, late continuous BLE/Wi-Fi, buttons, and post-close/lifecycle scope. No new flash is authorized by a promising design or host test.

The earlier absolute 20 ms record budget failed on the target. Its receipts remain failed; successful portable scheduling tests are not physical acceptance of the replacement policy.

Decision requested: approve this bounded pool/cooperative-writer implementation scope, or narrow the telemetry/ownership interface before code is written. Remaining output loss and memory findings stay open until measured on a reviewed candidate.


## Prospective cooperative UART service budgets

The completed display/resynchronization observer retained four typed Timeout
losses (three Detection records and one short BLE record), despite continued
output after LF recovery. Its maximum sampled intra-line poll gap was 19,903 us.
This is consistent with multiple scheduling gaps accumulating past the old
20 ms total allowance; it does not identify display execution or FIFO state as
the cause of a particular loss. No earlier failed receipt is reclassified.

The candidate uses two checks before every at-most-16-byte FIFO step:

- A 40,000 us **no-acceptance watchdog**, measured since the last positive,
  validated FIFO acceptance (or the initial drain attempt). Blocked, zero bytes,
  readiness, and yields cannot renew it. It includes unscheduled time, so expiry
  does not prove a physically stalled UART.
- An immutable total service allowance for a validated length `L <= 512`:
  `min(200000, 40000 + ceil(10 * L * 1000000 / 460800) + 20000 * max(0, ceil(L / 64) - 1))`
  microseconds. Arithmetic uses u64 after validating the protocol bound. Empty
  input is a no-op; oversized input fails before FIFO access. The 94/310/512-byte
  allowances are 62,040 / 126,728 / 191,112 us. The wire-time term assumes 8N1;
  delivery returns at FIFO acceptance, not physical wire completion, and never
  deliberately waits for the allowance. Partial writes may require more yields
  than this nominal full-step model and can still exhaust the fixed total.

Elapsed subtraction wraps on the monotonic u64 clock. Equality expires a budget.
The adapter samples immediately after each validated step as well: an interrupted
nonfinal step cannot erase an already expired interval by renewing the watchdog.
If the final step already accepted all bytes (including the framed LF), the record
is complete and must not be retried or falsely counted as an incomplete timeout.
Its elapsed time and completed-overrun count are reported instead. These are
poll-time checks, not preemption or guaranteed wall-clock bounds under starvation.

The single owner, 64-byte/four-step yield quantum, panic exclusion, BLE ordering,
loss attribution, and separate fixed 20 ms LF-only recovery are unchanged. A
payload timeout is still one lost record even after successful LF recovery; no
payload retry or pool reopening occurs. The generic pathological 64-slot service
allowance including LF is at most `64 * (200 + 20) ms = 14.08 s`, not ordinary
latency. Reserved capacity is finite and does not guarantee lossless admission.

Sixteen bytes of bounded timing metadata in the existing PSRAM pool record the
maximum completed-record elapsed time, completed overruns, and the last timeout's
length, accepted prefix length and elapsed time. A separate `UART service:`
health line reports these without payloads or changing the existing pool/loss
line schema; periodic publication yields between lines and retains no 512-byte
buffer across an await. Zero last-timeout fields mean no reported timeout when
the reconciled timeout count is zero. Values saturate and flag saturation.
Queue residence remains commit-to-take time, and intra-line poll gaps do not
measure display-row duration or global button latency.

Prospective physical acceptance still requires zero output loss, full health
coverage, the separate <20 ms intra-line poll-gap responsiveness criterion, and
manual display/button checks. Completed overruns must be reported and assessed,
not hidden as on-time completions. Target stack, heap reservations and memory
gates are unchanged and require the final linked image. This policy does not
modify scan settings, the XIAO output path, or the legacy synchronous writer.
