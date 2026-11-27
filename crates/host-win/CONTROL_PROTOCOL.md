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
{"event":"stats","fps":59.9,"bitrate_kbps":14000,"encode_ms":2.3,"rtt_ms":3.1,"encoder":"mf-hevc-hw|mf-h264-hw|openh264","width":2800,"height":1752,"fps_now":59.0,"fps_avg":58.0,"fps_low1":41.2,"frame_ms_mean":17.2,"frame_ms_std":3.5,"frame_ms_var":11.9}
{"event":"devices","items":[{"kind":"usb","name":"Galaxy Tab S10+ (SM-X826B)","serial":"1-5","vid":6353,"pid":11520,"accessory":true,"driver_ok":true,"connected":true}]}
{"event":"log","level":"info|warn|error","msg":"..."}
```

* `status` is emitted on every state change and again when the OTP changes.
  `state=error` with exit code 1 is also emitted on fatal startup failure (`otp` may be "").
  After a client disconnect the host goes back to `waiting` (it never exits by itself).
* `stats` every 1 s while `streaming`; `rtt_ms` is `null` until the client reports one.
  Frame-time fields (added later; clients must default them to 0 when absent) are computed over the
  frames actually SENT to the client (after encode) in a rolling window of the last 10 s / 1200
  frames (`frame_stats.rs`): `fps_now` = frames sent in the last second, `fps_avg` = 1000 / mean
  frame time, `fps_low1` = 1000 / mean of the worst 1 % frame times (at least one sample),
  `frame_ms_mean` / `frame_ms_std` (population σ) in ms and `frame_ms_var` in ms². All 0 until two
  frames were sent. `fps` keeps its old meaning (encoder-side rate of the last window).
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

## Service pipe (lock screen support)

Only a LocalSystem process in the user's console session can capture and control the Winlogon
desktop (lock screen, sign-in, UAC). `RemoteView.exe --service` is a Windows service
(`RemoteViewSvc`, display name "RemoteView Service", auto start, LocalSystem) that starts the very
same engine as SYSTEM and relays the stdin/stdout JSON lines above over a named pipe. The GUI uses
it when the pipe answers and falls back to a plain child process otherwise (same JSON either way,
`HostCmd` / `HostEvent` unchanged).

* Pipe: `\\.\pipe\RemoteViewSvc`, byte mode, one instance (one client at a time; others get
  `ERROR_PIPE_BUSY`), remote clients rejected. ACL (SDDL) `D:(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;IU)`:
  SYSTEM and Administrators full control, interactive users read/write.
* **First line from the client** (UTF-8, `\n`-terminated, <= 16 KiB, within 10 s):
  `{"op":"spawn","args":["--usb","--fps","60","--json-status"]}`. `args` are the engine arguments
  from the top of this document, *without* `--host-worker`.
* The service **validates** `args` against an allow-list (`rv_host_win::service_proto`): only
  `--usb --adb --lan --json-status` and `--display --mode --fps --width --height --quality --codec
  --encoder --bitrate-mbps --otp --monitor <value>` with well-formed values (`--monitor` must be a
  GDI name such as `\\.\DISPLAY1`); anything else (`--host-path`, `--bind`, paths, `--list`,
  `--scan-all`, ...) is refused, and the engine's own parser must accept it as well. `--json-status`
  is added when missing. On refusal / launch failure the service sends one
  `{"event":"status","state":"error",...,"message":"service: ..."}` line and closes the pipe.
* The service then launches **its own exe** with `--host-worker <args>`: a primary token duplicated
  from the service's own SYSTEM token, `TokenSessionId` set to `WTSGetActiveConsoleSessionId()`,
  `CreateProcessAsUserW`, desktop `winsta0\default`, `CREATE_NO_WINDOW`, stdin/stdout (and stderr)
  on anonymous pipes. The environment is the logged-on user's when there is one (same
  `%APPDATA%` / `%LOCALAPPDATA%`), SYSTEM's on the sign-in screen.
* After that the pipe is transparent: worker stdout lines -> client, client lines -> worker stdin.
  stderr lines arrive as non-JSON lines (the GUI shows them as log lines).
* Client disconnects: the service writes `{"cmd":"stop"}` to the worker, waits 3 s, then
  terminates it. Worker exits: the service closes the pipe (the GUI's supervisor restarts it with
  backoff, like a crashed child). Service stop (`sc stop RemoteViewSvc`): same as a disconnect.
* As SYSTEM the worker attaches its capture, cursor, input and monitor threads to the *input
  desktop* (`OpenInputDesktop` + `SetThreadDesktop`) and logs `desktop: Default -> Winlogon`; the
  "locked" gate (`LOCKED_MESSAGE`, input dropped while locked) is disabled in that mode.

Install / uninstall (elevated, one UAC prompt from the GUI):
`RemoteView.exe --install-all <dir|->` installs SudoVDA from `<dir>` (`-` = skip) and creates or
updates the service (`"<this exe>" --service`) and starts it; `RemoteView.exe --uninstall-service`
stops and deletes it.
