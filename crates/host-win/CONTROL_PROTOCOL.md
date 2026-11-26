# rv-host control protocol (for the Windows GUI)

Run `rv-host.exe` as a subprocess with `--json-status`:

```
rv-host [--usb|--lan|--adb] [--otp N] [--width W --height H] [--fps F] [--quality 1-100]
        [--codec auto|hevc|h264] [--encoder auto|hw|sw] [--display mirror|extend] [--json-status]
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
{"event":"devices","items":[{"kind":"usb","name":"USB 04e8:6860","serial":"1-5"}]}
{"event":"log","level":"info|warn|error","msg":"..."}
```

* `status` is emitted on every state change and again when the OTP changes.
  `state=error` with exit code 1 is also emitted on fatal startup failure (`otp` may be "").
  After a client disconnect the host goes back to `waiting` (it never exits by itself).
* `stats` every 1 s while `streaming`; `rtt_ms` is `null` until the client reports one.
* `devices` is emitted whenever the list changes (polled every 2 s). `kind` is `usb`
  (AOA candidate; `serial` = `bus-address`) or `adb` (`serial` = adb serial).
* `peer`: `ip:port` on LAN, otherwise the first 6 bytes (hex) of the client's key.

## stdin commands (one JSON object per line)

```json
{"cmd":"set_quality","quality":80}
{"cmd":"set_fps","fps":60}
{"cmd":"set_resolution","width":1920,"height":1080}
{"cmd":"request_idr"}
{"cmd":"new_otp"}
{"cmd":"stop"}
```

Invalid lines produce a `log` warn event and are otherwise ignored. `set_quality` changes the
bitrate live (no re-init when the encoder supports it); `set_fps` / `set_resolution` rebuild
the encoder (the client is told through `StreamConfig`). `new_otp` re-emits `status` with the
new OTP; if a client is already connected it applies to the next connection. `stop` shuts
down (process exits within 2 s at most).
