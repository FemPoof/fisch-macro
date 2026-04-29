# Security

## Reporting a vulnerability

Found a security issue? Open a [GitHub issue][issues] tagged `security`
with **no reproduction details**, then request a private channel to
share details. Do not post exploit code in public issues.

[issues]: https://github.com/FemPoof/fisch-macro/issues

## What the app does, and what it doesn't

✅ The app:

- Captures the screen region you calibrate (the fishing-bar area only).
- Sends mouse clicks (M1) to the Roblox window.
- Listens for the hotkeys you rebind (default: F3 start/stop).
- Reads/writes config and logs in `%LOCALAPPDATA%\fisch-macro\`.

❌ The app does NOT:

- Phone home. There is **zero outbound network traffic** during normal
  operation — no telemetry, no auto-updates, no crash reports.
- Capture keystrokes. The hotkey listener uses Windows'
  `RegisterHotKey` API, which only fires for the specific keys you
  rebind. It is not a low-level keyboard hook.
- Read your Roblox account credentials, browser data, or any other
  files outside its own app-data folder.
- Modify Roblox's files or memory. It only sends regular mouse input
  through the OS-level input API.
- Require admin rights. If something asks for admin, it's not us.

## License keys

License keys are validated **entirely locally**. The validation logic
hashes your key (SHA-256) and compares it to a list of accepted hashes
baked into the app. The plaintext key never leaves your machine and is
never transmitted over a network.

Your key is stored at:
```
%LOCALAPPDATA%\fisch-macro\config.json
```

If you reset to defaults from the Extras tab, the app will preserve
your license key — paid users don't lose activation when wiping
tunings.

If your key is ever revoked (e.g. for sharing or selling it), the
app will fall back to free-tier behavior on next launch.

## What's in the .exe

The downloadable .exe is a self-contained Windows binary built from
Python source. It bundles the screen-capture libraries (DXCAM /
BetterCam), the GUI framework (PySide6 / Qt), input library
(pydirectinput), and OpenCV.

No part of the binary calls out to a remote server. You can verify
this with a network monitor like Wireshark or by running it in an
offline VM — capture and input both work fine without internet.

## Antivirus warnings

Some antivirus products flag PyInstaller / Nuitka-built binaries as
suspicious because they bundle a Python interpreter and execute
bytecode at startup. This is a generic heuristic, not a real
detection. If your AV flags `fisch-macro.exe`, you can:

1. Verify the SHA-256 hash on the [Releases][releases] page matches
   the file you downloaded (releases are signed with a fixed SHA-256
   per build).
2. Submit the file to [VirusTotal](https://virustotal.com) for a
   second opinion.
3. Add an exception in your AV.

[releases]: https://github.com/FemPoof/fisch-macro/releases
