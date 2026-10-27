# Kindle battery monitor

Shared language for the battery monitor and its KUAL package.

## Language

**Battery level**:
The Kindle's reported remaining battery charge, expressed as a percentage from 0 to 100.
_Avoid_: Battery health, battery capacity, charge rate

**Battery broadcast**:
A BLE advertisement carrying a sampled Kindle battery level as BTHome service data. A receiver obtains the sample without connecting to the Kindle.
_Avoid_: Battery read request, GATT notification

**Bluetooth alias**:
A user-chosen name included in the Kindle's battery broadcasts, independent of the device name configured in Kindle Settings.
_Avoid_: Kindle device name, Home Assistant entity ID

**KUAL package**:
A distributable Kindle launcher extension containing an application and its launcher controls.
_Avoid_: Firmware update, running monitor

**Automatic startup**:
The saved choice to start the battery monitor after the Kindle boots. It is independent of whether the monitor is running in the current session.
_Avoid_: Running, started

**Monitor session**:
A running instance of the battery monitor between its start and stop. A session can run while automatic startup is disabled.
_Avoid_: Enabled, installed
