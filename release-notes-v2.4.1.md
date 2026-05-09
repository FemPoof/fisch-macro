# fisch-macro v2.4.1

Hotfix release. Three UX bugs in v2.4.0 that surfaced once users started downloading the binary. No detection / controller / capture changes — same fishing pipeline, same overnight numbers (3,864 cycles, 97.4% catch rate).

## What's fixed

### 1. License-activation flow

The "Pro features unlocked" success dialog was modal and rendered on top of a still-not-yet-refreshed Settings tab. Users saw "Free Tier" + no Pro pill *behind* the success dialog and concluded activation had silently failed. Multiple users reported the issue with screenshots showing exactly that mid-modal moment.

Fixes in `SettingsTab._activate_license`:

- **Status refresh + Pro-pill update + Developer-tab refresh now run BEFORE showing the success dialog.** The post-activation state is visible underneath the confirmation rather than hidden by it.
- **Dev keys no longer get a hardware binding stored.** The previous flow called `compute_binding(key)` for every valid key including dev keys. Harmless to validation (`is_premium_unlocked` short-circuits dev keys) but it produced a stale binding hash on disk that contradicted the multi-machine guarantee dev keys are supposed to give.
- **Dialog message branches by key type.** Dev keys get a "multi-machine, no binding" message that mentions the Developer tab; regular keys keep the per-machine message.
- **Improved error-path message.** "Key not recognized" warnings now point at the most common cause (stray characters / whitespace from a chat-app paste).

Two regression tests added: `test_dev_key_activation_does_not_store_binding` (verifies `binding=""` after activation + multi-machine validity) and `test_regular_key_activation_stores_binding` (regular keys still produce a per-machine binding).

### 2. Modal dialog dark-theme styling

`QMessageBox`, `QInputDialog`, and `QDialog` were never covered by the QSS in `theme.py`. Every modal in the app (license activation success, "key not recognized" warnings, "Clear zone?" confirmations, Add Rod input prompt, every other `QMessageBox.warning/information/question` call) rendered with a stark white Windows-default background against the otherwise-dark UI. Looked like a different application's dialog had popped up.

Fix: explicit dark-theme rules added at the top of the stylesheet for `QDialog`, `QMessageBox`, `QMessageBox QLabel`, `QInputDialog`, `QInputDialog QLabel`. The cascade picks up the existing global QPushButton rules once the dialog's own background flips to `Colors.BG`, so OK / Cancel buttons (which were already rendering correctly) stay correct.

### 3. Premium tabs hidden from free-tier users

Perfect Cast, Buffs, and Reconnect previously sat in the tab bar with a locked-state `PremiumTab` placeholder rendered inside them when premium wasn't active. Users found that confusing — "why is this tab here if I can't use it?".

Fix: `MainWindow._refresh_premium_tabs()` adds / removes those tabs from the QTabWidget based on `is_premium_unlocked()`. Mirrors the Developer-tab pattern (constructed once, attached / detached on license change). Free-tier users now see exactly four tabs: Main / Cast / Fish / Settings. Premium users see seven: Main / Cast / **Perfect Cast** / Fish / **Buffs** / **Reconnect** / Settings. Dev-key users get **Developer** appended to the end.

Each premium tab keeps its internal locked-state placeholder as dead-code safety net in case a future code path adds the tab without checking premium.

## Upgrading

Drop-in replacement for v2.4.0. No config changes needed; settings carry forward verbatim. If you previously activated a dev key, the stale hardware binding will clear itself on first launch (`ensure_license_binding` already had this fixup wired; v2.4.1 just stops creating it in the first place).

## Tests

484 tests pass on this release. New coverage: 2 regression tests for the activation flow.
