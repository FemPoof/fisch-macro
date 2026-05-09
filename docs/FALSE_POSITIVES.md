# Antivirus false positives

**TL;DR:** A handful of antivirus heuristics flag the packaged
`fisch-macro.exe`. **None of them detect actual malicious behavior** —
they're all "this binary has structural traits we associate with
malware" warnings, not signature matches against any known threat.
This page explains why it happens, what the labels actually mean, and
how to verify the binary is safe.

---

## The current scan

> The VirusTotal scan below was run on the **v2.3.0** build. The
> v2.4.0 EXE on the [Releases][releases] page has a **different**
> SHA-256 (it's a fresh build with new features) but the same
> structural traits trigger the same heuristics, so the qualitative
> result is expected to match. The hash to verify against your
> download lives on the v2.4.0 release notes itself, not here.

| Field | Value |
|---|---|
| File | `fisch-macro.exe` (v2.3.0) |
| Size | 83.09 MB |
| SHA-256 | `317042e24ca7f002415ae85c9d73bf056aeefb1efb4433613b64632d6a98d160` |
| Result | **7/61** vendors flagged (heuristic / ML only) |
| VirusTotal | https://www.virustotal.com/gui/file/317042e24ca7f002415ae85c9d73bf056aeefb1efb4433613b64632d6a98d160 |

[releases]: https://github.com/FemPoof/fisch-macro/releases

The vendors that flagged it and what their labels actually mean:

| Vendor | Label | What this means |
|---|---|---|
| Antiy-AVL | `Trojan[Packed]/Python.Nuitka` | "It's Nuitka-packed Python" — generic packer flag |
| AVG | `MalwareX-gen [Misc]` | Generic ML heuristic, no specific match |
| Avast | `MalwareX-gen [Misc]` | Same engine as AVG |
| Bkav Pro | `W64.AIDetectMalware` | ML model, no specific family |
| ESET-NOD32 | `Python/Packed.Nuitka.AL Suspicious` | "It's Nuitka-packed Python" |
| Microsoft | `Trojan:Win32/Wacatac.B!ml` | Microsoft's ML catch-all (notorious for false-positives on PyInstaller / Nuitka builds) |
| Trapmine | `Suspicious.low.ml.score` | Low-confidence ML score |

The 54 vendors that **did NOT** flag it include every reputable
signature-based engine: Kaspersky, Bitdefender, Symantec, Sophos,
F-Secure, Trend Micro, Malwarebytes, McAfee, GData, Comodo, etc. They
have full visibility into the binary and don't see anything matching a
known threat.

---

## Why does this happen?

Three structural traits trigger the heuristics:

### 1. The binary is Nuitka-packed

Nuitka compiles the entire Python interpreter, every dependency, and
every `.py` file into one self-extracting executable. To AV
heuristics, "one EXE that unpacks an embedded payload at startup"
looks structurally similar to malware packers (UPX, Themida, etc.) —
even though Nuitka does it for legitimate distribution reasons.

You'll see this exact false-positive on PyInstaller, cx_Freeze,
py2exe, and even Electron apps. It's a known issue across the entire
Python-packaged-as-EXE ecosystem.

### 2. The macro uses input-simulation + screen-capture APIs

To do its job, the macro uses:

- **`pydirectinput`** — Win32 `SendInput` for mouse / keyboard
- **`dxcam` / `bettercam`** — Desktop Duplication API for screen
  capture
- **`RegisterHotKey`** — Win32 global hotkey registration
- **`SetForegroundWindow`** — to focus Roblox

Every one of these is a well-documented Microsoft Win32 API used by
thousands of legitimate apps (OBS uses Desktop Duplication, AutoHotkey
uses `SendInput`, every screen recorder uses `RegisterHotKey`). But
combined in a single binary, they pattern-match against
keylogger / screen-recorder heuristics, especially in ML models that
weren't trained on enough automation tools.

### 3. The binary is unsigned

We don't currently ship with a code-signing certificate. Unsigned
binaries get treated more aggressively by Windows SmartScreen and by
ML heuristics — a signed binary with a valid certificate would clear
many of these flags automatically.

A code-signing cert costs $80–500/year depending on type (OV vs EV)
and we'll add it once usage reaches a level where it's worth the
overhead.

---

## How to verify the binary is safe

If you're not comfortable trusting our word, you have three
independent paths:

### Path 1: Verify the SHA-256

Compare the hash of the EXE you downloaded against the hash on the
release page (and the one above):

```powershell
Get-FileHash fisch-macro.exe -Algorithm SHA256
```

The hash should match exactly. If it doesn't, the file you have is
not the file we shipped — re-download from
[GitHub Releases](https://github.com/FemPoof/fisch-macro/releases).

### Path 2: Run the same VirusTotal scan yourself

Drop the EXE on https://www.virustotal.com/. The result should
match this page (give or take a few engines updating their
heuristics). The reputable signature-based vendors should clean.

### Path 3: Run from source

The most paranoid path: ignore the EXE entirely, clone the repo,
read the source, and run it from Python directly.

```powershell
git clone https://github.com/FemPoof/fisch-macro.git
cd fisch-macro
py -3.11 -m venv .venv
.\.venv\Scripts\Activate.ps1
pip install -e ".[dev]"
python -m fisch_macro
```

Every line of code is in the public repo. The binary is built with
Nuitka from exactly that source — there's nothing in the EXE that
isn't in the repo.

---

## What we're doing about it

1. **False-positive submissions** to the flagging vendors. These
   forms typically take a couple of business days to process; once a
   vendor whitelists a hash the flag clears for that release. Forms:
   - Microsoft: <https://www.microsoft.com/en-us/wdsi/filesubmission>
   - ESET: <https://support.eset.com/en/kb141-submit-a-virus-spyware-or-suspicious-file-or-website>
   - Avast / AVG (shared backend): <https://www.avast.com/false-positive-file-form.php>
   - Antiy-AVL: lower priority; smaller vendor

2. **Code signing** — planned. Will eliminate the
   "unsigned + suspicious APIs" combination that drives the bulk of
   the heuristic flags.

3. **Pinned VirusTotal link** in every GitHub Release so the FP
   conversation happens once, in one place.

---

## What you should NOT do

- **Don't** disable Windows Defender or your AV to run the macro.
  Run from source instead if you don't want to whitelist.
- **Don't** trust the binary blindly because we say so — verify the
  SHA-256, run the scan yourself, or run from source.
- **Don't** download `fisch-macro.exe` from anywhere except the
  official [GitHub Releases](https://github.com/FemPoof/fisch-macro/releases).
  Repackaged binaries circulating on third-party sites are a real
  threat vector that we cannot vouch for.

---

## Adding the EXE to your AV's exclusion list

If you've verified the binary and want to silence the AV warning, add
the EXE (or the folder you put it in) to your AV's exclusion list.
Linking the official docs rather than rewriting them:

- Microsoft Defender: <https://support.microsoft.com/en-us/windows/add-an-exclusion-to-windows-security-811816c0-4dfd-af4a-47e4-c301afe13b26>
- Avast: <https://support.avast.com/en-us/article/Antivirus-scan-exclusions/>
- AVG: <https://support.avg.com/SupportArticleView?l=en&urlname=avg-exceptions>
- ESET: <https://support.eset.com/en/kb2769-exclude-files-or-folders-from-scanning-in-eset-windows-home-products>

Only add an exclusion **after** verifying the SHA-256 and ideally
running the file through VirusTotal yourself.
