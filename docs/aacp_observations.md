# AACP observations (AirPods 4, Linux)

Field notes from AirPods 4 (non-ANC, product ID `0x2019`) connected to a Linux
host running the `linux-rust` app. Packets are shown from the opcode onward
(the `04 00 04 00` header is omitted). Confidence is stated per item; "seen"
means observed repeatedly, "likely" means consistent with every capture so far.

## Decoded and implemented

| Opcode | Meaning | Notes |
|--------|---------|-------|
| `0x63` | Custom EQ | `63 00 05 00 01 <state> <low> <mid> <high>`; state `02` = on, `01` = off; bands 0–100, 50 = flat. Written with the same layout; the device reports the stored value on connect. |
| `0x17` | Head tracking | Start/stop with the motion-service-`0x10` packets (the service-`0x0E` request is refused with status 3). Reports arrive at ~25 Hz, 80 or 81 bytes: a varint counter precedes the fields, so fields sit at fixed offsets **from the end** (quaternion x, y, z as Q15 at offsets 43/45/47 of an 81-byte report). The sensor frame's z axis follows a nod and its y axis a turn. |
| `0x58` | Hi-res microphone | AAC-ELD 64 kHz mono; works on AirPods 4 while A2DP stays up. Stalls after 16 frames when one bud is in the ear and the other charges in a closed case. |
| control `0x3E` | Uplink EQ Bud config | Reported as `02`; encoding not yet known. |

Control commands apply silently: a write is not echoed back; the device reports
the new value on the next connect.

## Connection setup and device switching

These arrive as a burst whenever the AACP link is (re)established — at connect,
when the case lid opens, and when another device takes the AirPods over.

| Opcode | Seen | Likely meaning |
|--------|------|----------------|
| `0x02` | 58-byte TLV list at every setup | Device capabilities. |
| `0x4E` | `4e 00 02 00 …` right after the host sends `0x4D` (feature flags) | Acknowledgement of the feature flags. |
| `0x2B` | List of entries, each a 6-byte address (little-endian) plus counters | Connection history per source device. |
| `0x0C` | `<address LE, 6 bytes> <2 state bytes>`, one per device including the AirPods themselves | Per-device state table for automatic switching. |
| `0x08` | `<u16> <u16>`; second value 1 while the host is not the active audio device or the buds are in the case, 0 while it is | Audio-route state; exact meaning unconfirmed. |
| `0x55` | `55 00 01 01 00 <n>`; `n` = 25 (`0x19`) when idle, 50 (`0x32`) while this host plays media | Host audio-activity level used for switching. Not the volume (volume keys produce nothing here). |
| `0x4C` | `4c 00 03 00 04 02 00` on every play/pause transition, after the host's smart-routing (`0x10`) messages | Acknowledgement of the smart-routing messages. |

Who is using the AirPods is already available from two handled opcodes:
`0x0E` (audio source: this host's address while it plays, all-zero otherwise)
and `0x2E` (connected source devices: only the other device after it takes
over, only this host after it takes them back). `ctl status` reports this as
`audio.owner` (`this-device`, `other-device`, `none`).
