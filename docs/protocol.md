# Battery broadcast and radio transaction

The battery source is the integer `battLevel` property of `com.lab126.powerd`. The bundled Rust LIPC implementation speaks DBus directly. The monitor subscribes to `battLevelChanged` and waits on refresh timers or cancellation, adding no polling or suspend inhibitor.

BTHome service UUID is `0xFCD2`. Service bytes are:

```text
D2 FC 44 00 <packet-id> 01 <battery-percent>
```

`0x44` selects unencrypted BTHome v2 with trigger-based device behavior. Object `0x00` is packet ID and `0x01` is battery percentage from zero through 100. The primary advertisement also contains Complete Local Name `Kindle`, encoded as `07 09 4B 69 6E 64 6C 65`. With flags, the advertisement occupies 20 of 31 legacy bytes. The stock stack expands requested flags `0x06` to the observed `0x1E`.

On the qualified firmware, BSA configuration payloads are 832 bytes. Advertising-data mask `0x8102` selects flags, service data, and the explicit name element. This avoids the native name flag, which reads the global Bluetooth name and can consume space before service data. The request selects primary advertisement data, not scan response. Protocol fixtures lock down the recovered encoding.

The monitor requires the stock backend to be absent, acquires BLE-only source 4, and rechecks ownership before publishing. It refuses an existing advertisement, then requests the factory public address for its owned burst. After the five-second timer it stops and verifies advertising, requests stock rotation, rechecks ownership, and releases only source 4. Competing sources remain owned by their original clients. These observations do not provide an atomic system-wide radio lease.

Scheduler/controller delays can extend the RF interval; the timer is not a hard real-time cutoff. Publication or cleanup failure stops the monitor for inspection. The startup job has no respawn directive. A failed battery read is logged and the next sample trigger can retry without fabricating a percentage.

Startup waits on DBus readiness with a finite deadline and cancellation. Upstart 0.6.6 supports the enabled job's boot condition; the disabled template omits that condition while remaining manually startable. `manual` and `.override` are not used. Upgrades preserve automatic startup and the current session independently.
