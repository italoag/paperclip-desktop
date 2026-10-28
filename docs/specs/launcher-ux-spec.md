# Launcher UX Implementation Spec

Status: Draft
Date: 2026-04-08
Branch: feature/remote-connection-mode

---

## 1. Launcher Window Sizing

### Standalone (no main window exists)

- Width: 560px
- Height: 620px
- Min width: 560px
- Min height: 400px
- Resizable: false
- Maximizable: false
- Fullscreenable: false
- Centered on primary display

### Attached (main window exists, opened as sheet)

No changes from current implementation. Attached dimensions derive from parent bounds with the existing clamping logic.

### Content-height auto-resize

Applies to attached mode only. Standalone uses fixed dimensions above.

---

## 2. Chooser View

### Layout

```
[Logo]
Paperclip

How would you like to start?

[ Run Local (selected) ]  [ Connect Remote ]

[x] Remember my choice and don't ask again

[ Saved Connections ]  [ Continue (primary) ]
```

### Card selection

- Cards are selectable via click or keyboard (see Section 8).
- Exactly one card is selected at all times.
- Selected card has border `#a1a1aa`, background `#1c1c1f`.
- Unselected card has border `#27272a`, background `transparent`.

### Initial state on open

When the chooser opens (fresh launch or navigating back), read `snapshot.state.chooserMode`:

- If `chooserMode === "remote_existing"`: pre-select the Remote card.
- Otherwise: pre-select the Local card.

Read `snapshot.state.alwaysShowChooser` and `snapshot.state.autoConnectLastProfile`:

- Checkbox is checked when `alwaysShowChooser === false && autoConnectLastProfile === true`.
- Checkbox is unchecked in all other cases.

`showChooser()` must call `selectCard(snapshot.state.chooserMode === "remote_existing" ? "remote" : "local")` instead of hardcoding `"local"`.

### Continue button

- If Local is selected: navigate to `local-boot`, call `connectLocal({ rememberChoice })`.
- If Remote is selected: call `setChooserMode("remote_existing")`, navigate to `remote-form`.

### Saved Connections button

Navigate to `saved` view.

---

## 3. Remote Form View

### Layout

```
Connect to Remote Paperclip

[Remote requirements (collapsible)]

Remote URL: [________________________]
  Desktop verifies /api/health and auth readiness before loading a remote origin.
  (error text, hidden by default)
  (success text, hidden by default)

Display name (optional): [________________________]

[verification status badge area]
[status card, hidden by default]

[ Verify Remote ]  [ {action button} (primary, disabled) ]  [ {save button} (disabled) ]

<- Back to chooser
```

### Verify Remote button

**During verification:**
- Button label changes to "Verifying..." and is disabled.
- All other action buttons remain disabled.
- Status badge area shows: `[spinner dot] Verifying remote...` with class `testing`.

**After verification completes:**
- "Verify Remote" button re-enables with label "Verify Remote".
- Requirements box collapses.
- Status card renders per result (see Section 3.1).
- Action buttons enable/disable per result.

**If the URL field is empty when Verify Remote is clicked:** show field error "Enter a valid http(s) URL." and do not call the IPC.

### 3.1 Verification result mapping

Each result maps to three labels: badge, action button, and save button.

| Condition | Badge | Action Button (primary) | Save Button | Buttons enabled |
|---|---|---|---|---|
| `ok && bootstrapStatus === "bootstrap_pending"` | "Verified remote - setup required" (warning) | "Connect Remote" | "Save Connection" | Yes |
| `ok && sessionState === "signed_in"` | "Authenticated Paperclip detected" (healthy) | "Connect Remote" | "Connect & Save" | Yes |
| `ok && sessionState === "signed_out"` | "Verified remote - sign-in required" (warning) | "Connect Remote" | "Save Connection" | Yes |
| `reason === "unsupported_local_trusted"` | "Unsupported remote deployment" (unreachable) | -- | -- | No |
| `reason === "not_paperclip"` | "Host reachable, not Paperclip" (unreachable) | -- | -- | No |
| `reason === "tls_error"` | "TLS validation failed" (unreachable) | -- | -- | No |
| `reason === "unreachable"` | "Host unreachable" (unreachable) | -- | -- | No |
| `reason === "invalid_url"` | (no badge) | -- | -- | No, show field error |

Failed results display the status card with detail text but do not enable action buttons.

### 3.2 Redirect handling

When preflight fails with `reason === "unreachable"` and the normalized URL uses `http:`, the status card detail appends: "If this server redirects HTTP to HTTPS, try entering the HTTPS URL directly."

Implementation: `buildFailure` and `remoteErrorTitle` do not need changes. The hint is added in `mapVerificationResult` for the unreachable case when `result.normalizedUrl` starts with `http:`.

### Action button behavior

**"Connect Remote" (primary):** Calls `connectRemote` with `saveProfile: false`. Navigates to `connecting` view.

**"Save Connection" / "Connect & Save":** Calls `connectRemote` with `saveProfile: true`. Navigates to `connecting` view.

Both buttons read `rememberChoice` from the checkbox at call time.

### Back to chooser

Navigates to `chooser` view. Resets verification state (clears status badge, status card, field errors). Does not clear URL/name fields.

---

## 4. Saved Connections View

### Layout

```
Saved Connections

[ Local (conn-item, single-click to connect) ]
[ My Server (conn-item) ]  [edit] [dup] [delete]
[ Another (conn-item) ]    [edit] [dup] [delete]

[ + Add Connection ]  [ Back ]
```

### Row interaction model

- **Single click** on a connection row triggers `quickConnect(profileId)`.
- **Double-click** is removed. Single click is the only activation gesture.
- Rows have hover state (border `#52525b`, background `#18181b`).
- The currently-connected profile row has class `active-conn` (green border `#4ade80`, background `#0a1a0f`).
- There is no separate "selected but not connected" state. Click = connect.

### Action buttons on rows

Action buttons (`edit`, `duplicate`, `delete`) appear on remote profile rows only. They call `event.stopPropagation()` to prevent row click.

- **Edit:** Opens the edit modal pre-filled with name and URL.
- **Duplicate:** Immediately creates a copy. No confirmation.
- **Delete:** Opens delete confirmation (see Section 5).

### Empty state

When `profiles` array contains only the local profile and no remote profiles:

```
No saved remote connections yet.
```

The local profile row still renders above the empty state message.

### Add Connection button

Opens the add modal (same as current implementation).

### Back button

Navigates to `chooser` view.

---

## 5. Delete Confirmation

When a user clicks the delete button on a saved connection:

### Confirmation dialog

A modal overlay appears with:

```
Delete connection?

"{profile.name}" will be removed. Session data for this
connection will also be cleared. This cannot be undone.

        [ Cancel ]  [ Delete (danger) ]
```

- **Cancel** is the default focused button.
- **Delete** uses class `btn danger` (red text, red border).
- Pressing Escape dismisses the modal (equivalent to Cancel).
- Clicking the overlay backdrop dismisses the modal (equivalent to Cancel).

### On confirm

- Calls `launcher.deleteProfile(profileId)`.
- Applies returned snapshot.
- Re-renders connection list.
- Modal closes.

### On cancel

- Modal closes.
- No state changes.
- Focus returns to the connection list.

---

## 6. Connecting View

### Layout

```
[spinner]
{label}
{url}
```

### Labels

| Condition | Label |
|---|---|
| `bootstrapStatus === "bootstrap_pending"` | "Opening remote setup..." |
| `sessionState === "signed_out"` | "Opening remote sign-in..." |
| `sessionState === "signed_in"` | "Opening verified remote..." |
| Local boot | Not used (local-boot view handles this) |

### Behavior

- Close button is hidden during this view.
- If the connection succeeds, the launcher closes and the main window appears.
- If the connection fails, the view transitions to `error`.

---

## 7. Error View

### Layout

```
[error box]
  {error title}
  {error detail}

[ Retry ]  [ Try Different Connection ]  [ Switch to Local (primary) ]

<- Back to chooser
```

### Button behavior

**Retry:** Calls `retryLastAction()`. If `lastErrorAction` is null, this button is hidden (not shown, not disabled).

**Try Different Connection:** Navigates to `remote-form`. Preserves any previously entered URL/name in the form fields.

**Switch to Local (primary):** Navigates to `local-boot`, calls `connectLocal`.

**Back to chooser:** Navigates to `chooser`.

### Error title/detail mapping

Titles come from `remoteErrorTitle()` (no changes). Detail comes from `result.detail` or the boot error message.

For local boot failures, Retry calls `connectLocal` again. "Try Different Connection" is hidden for local errors (there is no remote form context). Layout for local errors:

```
[error box]
  "Failed to start local Paperclip"
  {error.message}

[ Retry ]  [ Switch to Remote (primary) ]

<- Back to chooser
```

"Switch to Remote" navigates to `remote-form`.

---

## 8. Keyboard and Accessibility

### Global

| Key | Behavior |
|---|---|
| Escape | If a modal is open: close the modal. If the launcher is in attached mode: close the sheet (equivalent to close button). Otherwise: no action. |
| Tab / Shift+Tab | Standard focus cycling through interactive elements in DOM order. |

### Chooser view

| Key | Behavior |
|---|---|
| Arrow Left / Arrow Right | Switch selected card (Local <-> Remote). |
| Enter | Equivalent to clicking "Continue". |

Both cards have `tabindex="0"` and `role="option"`. The card container has `role="listbox"`. Focused card shows a visible focus ring (outline: `2px solid #52525b`, offset `2px`).

### Remote form view

| Key | Behavior |
|---|---|
| Enter (while URL field focused) | Triggers "Verify Remote". |
| Enter (while any button focused) | Activates that button. |

Tab order: URL field -> Display name field -> Verify Remote -> action button -> save button -> Back to chooser.

### Saved connections view

| Key | Behavior |
|---|---|
| Arrow Up / Arrow Down | Move focus between connection rows. |
| Enter | Activate focused connection (equivalent to single click). |

Connection rows have `tabindex="0"`. Focused row shows visible focus ring.

### Modals (edit, delete, add)

| Key | Behavior |
|---|---|
| Escape | Close modal, restore focus to the element that opened it. |
| Enter | Activate the primary/default button in the modal. |
| Tab | Cycle within modal only (focus trap). |

Click on backdrop closes the modal.

### Focus restoration

When a view transition occurs, focus moves to the first interactive element in the new view. When a modal closes, focus returns to the trigger element.

---

## 9. Implementation Checklist

Each item is a discrete code change. Items are ordered by dependency.

### Window sizing

- [ ] Change standalone launcher dimensions in `createLauncherWindow` from `1440x900` / `minWidth 900` / `minHeight 600` to `560x620` / `minWidth 560` / `minHeight 400`.

### Chooser persistence

- [ ] In `showChooser()`, replace `selectCard("local")` with `selectCard(snapshot && snapshot.state.chooserMode === "remote_existing" ? "remote" : "local")`.
- [ ] In `applySnapshot()`, add: `selectCard(nextSnapshot.state.chooserMode === "remote_existing" ? "remote" : "local")` after setting `snapshot`.

### Saved connections: single-click

- [ ] Change `ondblclick="quickConnect(...)"` to `onclick="quickConnect(...)"` on connection rows in `renderConnections()`.

### Delete confirmation

- [ ] Add a delete confirmation modal to the HTML (alongside the existing edit modal).
- [ ] Add `pendingDeleteId` variable to track which profile is being deleted.
- [ ] `deleteConn(profileId)`: instead of calling `launcher.deleteProfile` directly, set `pendingDeleteId = profileId`, populate modal text with profile name, show the delete modal, focus the Cancel button.
- [ ] `confirmDelete()`: calls `launcher.deleteProfile(pendingDeleteId)`, applies snapshot, closes modal, re-renders connections.
- [ ] `cancelDelete()`: closes modal, clears `pendingDeleteId`, restores focus.

### Remote verification loading state

- [ ] In `verifyRemote()`, before the `await`: set Verify Remote button to `disabled`, change its label to "Verifying...".
- [ ] In `verifyRemote()`, in the `finally` (or after await): restore label to "Verify Remote", re-enable the button.
- [ ] If URL field is empty, show field error and return early without calling IPC.

### Redirect hint

- [ ] In `mapVerificationResult`, for the default (unreachable) case: if `result.normalizedUrl` starts with `"http:"`, append to detail: " If this server redirects HTTP to HTTPS, try entering the HTTPS URL directly."

### Error view: conditional buttons

- [ ] Track `lastErrorAction.type` to determine which buttons to show.
- [ ] If `lastErrorAction` is null or undefined: hide the Retry button.
- [ ] If `lastErrorAction.type === "local"`: hide "Try Different Connection", show "Switch to Remote" as primary.
- [ ] If `lastErrorAction.type === "remote"`: show "Try Different Connection", show "Switch to Local" as primary.
- [ ] Rename middle button from "Choose another connection" to "Try Different Connection".

### Keyboard: global Escape

- [ ] Add `document.addEventListener("keydown", ...)` handler.
- [ ] On Escape: if delete modal is open, close it. Else if edit modal is open, close it. Else if `snapshot.isAttachedLauncher` and current view is not `local-boot` or `connecting`, call `closeLauncherSheet()`.

### Keyboard: chooser cards

- [ ] Add `tabindex="0"` and `role="option"` to both card divs. Add `role="listbox"` to `.chooser-cards`.
- [ ] Add `keydown` listener on `.chooser-cards`: ArrowLeft selects local, ArrowRight selects remote. Enter calls `chooserContinue()`.
- [ ] Add CSS `.chooser-card:focus-visible { outline: 2px solid #52525b; outline-offset: 2px; }`.

### Keyboard: saved connections

- [ ] Add `tabindex="0"` to each `.conn-item` in `renderConnections()`.
- [ ] Add `keydown` listener on `.connections-list`: ArrowUp/ArrowDown move focus between rows. Enter calls `quickConnect` on focused row.
- [ ] Add CSS `.conn-item:focus-visible { outline: 2px solid #52525b; outline-offset: 2px; }`.

### Keyboard: modals focus trap

- [ ] On modal open: record the trigger element. Move focus to first focusable element in modal (or Cancel button for delete modal).
- [ ] On Tab inside modal: trap focus within modal elements.
- [ ] On modal close: restore focus to recorded trigger element.
- [ ] On backdrop click: close modal.

### Keyboard: remote form Enter

- [ ] Add `keydown` listener on `#remoteUrl`: Enter triggers `verifyRemote()`.

### Focus on view transition

- [ ] In `showView(name)`, after the view becomes active, focus the first interactive element: chooser -> first card, remote-form -> URL input, saved -> first connection row or Add button, error -> first visible button.
