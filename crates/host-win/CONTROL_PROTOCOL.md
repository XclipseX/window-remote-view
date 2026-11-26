# rv-host control protocol (for the Windows GUI)

The engine lives in the library (`rv_host_win::engine::run_cli(args)`). Release builds ship a
single `RemoteView.exe`; the GUI starts the engine by re-launching itself as
`RemoteView.exe --host-worker <args below>` (the `rv-host` bin remains for CLI/debug).
Either way it runs as a subprocess with `--json-status`:

```
rv-host [--usb|--lan|--adb] [--otp N] [--width W --height H] [--fps F] [--quality 1-100]
        [--codec auto|hevc|h264] [--encoder auto|hw|sw] [--display mirror|extend] [--monitor NAME] [--json-status]
```

Defaults: link auto (USB if an Android device is attached, else LAN), fps 60, quality 80,
codec/encoder auto, random 6-digit OTP (`--otp` accepts 6-9 digits). `--width/--height`
must be given together (encoded size; never upscaled above the captured size).

With `--json-status`, **stdout carries only JSON lines** (one object per line, UTF-8).
Human logs go to stderr. Closing stdin makes the host exit.

## stdout events

```json
{"event":"status","state":"starting|waiting|connecting|connected|streaming|error","link":"usb|lan|adb|none","peer":"<string or null>","otp":"123456","message":"<vi text>"}
{"event":"stats","fps":59.9,"bitrate_kbps":14000,"encode_ms":2.3,"rtt_ms":3.1,"encoder":"mf-hevc-hw|mf-h264-hw|openh264","width":2800,"height":1752}
{"event":"devices","items":[{"kind":"usb","name":"Galaxy Tab S10+ (SM-X826B)","serial":"1-5","vid":6353,"pid":11520,"accessory":true,"driver_ok":true,"connected":true}]}
{"event":"log","level":"info|warn|error","msg":"..."}
```

* `status` is emitted on every state change and again when the OTP changes.
  `state=error` with exit code 1 is also emitted on fatal startup failure (`otp` may be "").
  After a client disconnect the host goes back to `waiting` (it never exits by itself).
* `stats` every 1 s while `streaming`; `rtt_ms` is `null` until the client reports one.
* `devices` is emitted whenever the list changes (polled every 2 s). `kind` is `usb`
  (AOA candidate; `serial` = `bus-address`) or `adb` (`serial` = adb serial).
* `peer`: `ip:port` on LAN, otherwise the first 6 bytes (hex) of the client's key; replaced by the
  client's friendly name as soon as it sends `DeviceInfo` (hello JSON `device_name`).
* `devices` item fields: `name` = friendly name when known (DeviceInfo, else USB product string,
  else `Vendor vvvv:pppp`), `vid`/`pid`, `accessory` (18D1:2D00/2D01), `driver_ok` (libusb can open
  it; `false` on Windows = WinUSB missing, the GUI then runs the automatic installer), `connected`
  (this device is the live link).
* USB pairing needs no PIN: the host sends a random 8-digit token (new on every host start) as the
  AOA serial and uses it as the session OTP. `status.otp` is always the LAN/adb PIN; the GUI shows
  it only for LAN/adb.
* Every 2 s while a link is open the host logs `link-stats link=… stage=… in=Ndg/NB out=Ndg/NB
  usb_raw_in=… usb_raw_out=… write_errors=… pipe_err=… last_session_err=…` as a `log` event
  (stage: idle (no datagram yet) / handshake / established / streaming).

## stdin commands (one JSON object per line)

```json
{"cmd":"set_quality","quality":80}
{"cmd":"set_fps","fps":60}
{"cmd":"set_resolution","width":1920,"height":1080}
{"cmd":"request_idr"}
{"cmd":"new_otp"}
{"cmd":"connect","serial":"1-5"}
{"cmd":"stop"}
```

`connect` clears the USB retry backoff so the link is attempted immediately (device card button).
Invalid lines produce a `log` warn event and are otherwise ignored. `set_quality` changes the
bitrate live (no re-init when the encoder supports it); `set_fps` / `set_resolution` rebuild
the encoder (the client is told through `StreamConfig`). `new_otp` re-emits `status` with the
new OTP; if a client is already connected it applies to the next connection. `stop` shuts
down (process exits within 2 s at most).
