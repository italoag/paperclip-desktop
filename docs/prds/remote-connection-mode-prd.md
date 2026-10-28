# PRD: Remote Connection Mode for Paperclip Desktop

- **Status:** Draft
- **Owner:** Desktop App Maintainers
- **Branch:** `feature/remote`
- **Last Updated:** April 6, 2026

---

## 1) Summary

Paperclip Desktop currently boots an embedded local Paperclip server and opens it inside Electron. This PRD defines a new **Remote Connection Mode** that allows users to connect the desktop shell to an already-running Paperclip instance, while retaining the current **Local Embedded Mode**.

Remote mode must align with upstream Paperclip's deployment model:

- `local_trusted` is for loopback-only local use
- network-accessible remotes must run in upstream `authenticated` mode
- sign-in is handled by the remote Paperclip instance, not by a desktop-specific credential store

The feature introduces:

1. A launch-time **connection chooser** (`Local` vs `Remote`)
2. A **Remote Connect** flow with URL validation, Paperclip health preflight, and deployment-mode checks
3. **Saved Connections** (CRUD) and **Quick Connect** UX for fast switching
4. Security controls for remote origins in Electron
5. A remote-mode window policy that does not expose privileged local preload APIs to arbitrary remote pages

This enables users to keep all runtime state on a remote machine while using Desktop as a secure native client.

---

## 2) Problem Statement

### Current limitation

Desktop startup assumes local ownership of the server lifecycle:

- find free local port
- spawn bundled server
- wait for local health
- load `http://localhost:<port>`

This blocks a common workflow where users run Paperclip remotely and access it from multiple devices via browser or a tailnet.

### User pain

- Cannot use Desktop as a remote client
- Must use a browser for remote workflows
- No saved-connection workflow for frequently used instances

### Security problem to avoid

A naive "paste any URL into Electron and load it" design would blur the trust boundary between:

- a local Paperclip instance started and controlled by the app
- an arbitrary remote web origin supplied by the user

Remote mode must therefore verify that the target is a compatible Paperclip deployment and must avoid exposing local privileged APIs to remote content.

---

## 3) Goals

### Product goals

1. Let users connect Desktop to a remote Paperclip URL
2. Preserve existing one-click local behavior
3. Provide best-in-class switching UX via saved connection profiles
4. Keep Electron security posture strong in remote mode
5. Reuse upstream Paperclip authentication rather than inventing a desktop-only remote auth layer

### Success metrics (first 60 days)

- **Adoption:** >= 20% of weekly active desktop users create at least 1 remote profile
- **Reliability:** >= 99% successful startup to usable UI (local + remote combined)
- **Usability:** Median time to connect to a saved remote profile <= 3 seconds from app open
- **Stability:** No increase > 5% in renderer crashes/regressions

---

## 4) Non-goals

- Implementing desktop-managed username/password storage for remote Paperclip
- Implementing remote authentication protocols beyond what upstream Paperclip already supports
- Acting as a reverse proxy
- Multi-window simultaneous connections in V1
- Cross-device sync of saved profiles in V1

---

## 5) Personas & Primary Use Cases

### Persona A: Solo remote operator

- Runs Paperclip on a home server or VM
- Uses laptop and phone over Tailscale
- Needs one-click reconnect from desktop

### Persona B: Team member with multiple environments

- Has dev, staging, and prod Paperclip instances
- Needs fast switching without retyping URLs

### Persona C: Existing local-only desktop user

- Wants no change in current one-click local startup

---

## 6) Proposed User Experience

## 6.1 First-run / Launch chooser

On launch, show:

- **Run Local Paperclip** (recommended default)
- **Connect to Remote Paperclip**

Checkbox:

- `Remember my choice and don't ask again`

Advanced:

- `Always show chooser on launch` (in settings)

## 6.2 Remote connect form

Fields:

- Remote URL (required)
- Display name (optional)

Supporting copy:

- Remote mode is for verified Paperclip servers
- Shared or networked remotes should run upstream `authenticated` mode
- The remote server handles sign-in; Desktop does not store remote passwords

Actions:

- **Verify Remote**
- **Continue to Sign-In**
- **Connect & Save**

Validation:

- Require `http://` or `https://`
- Normalize trailing slashes
- Reject URLs that embed `username:password@host` credentials
- Warn for non-HTTPS unless hostname matches private or tailnet patterns
- Block malformed URLs before any network call

## 6.3 Remote verification states

The verification step must distinguish at least:

- authenticated Paperclip detected
- authenticated Paperclip detected, sign-in required
- authenticated Paperclip detected, existing session found
- Paperclip detected but running `local_trusted` mode and therefore not eligible for desktop remote mode
- host reachable but not Paperclip
- host unreachable
- TLS or certificate error
- version compatibility warning

## 6.4 Saved Connections (CRUD)

Data shown per profile:

- Name
- URL (origin + host)
- Last connected timestamp
- Last known status
- Optional capability tag, for example `Authenticated`, `Sign-in required`, or `Unsupported`

CRUD actions:

- Add
- Edit
- Rename
- Delete
- Duplicate profile

## 6.5 Quick Connect

Available via:

- App menu (`Connection`)
- Tray menu, when present

Entries:

- Connect Local
- Recent or saved remote profiles
- Manage Connections

## 6.6 Startup behavior

- If `remember last profile` is enabled, app auto-attempts the last used profile
- On failure, show actionable error and offer:
  - `Retry`
  - `Choose another connection`
  - `Switch to Local`

If the remote profile is valid but not signed in:

- load the verified remote origin
- allow the remote Paperclip auth flow to handle sign-in
- return to the app once the remote session exists

---

## 7) Functional Requirements

### FR-1: Connection modes

App must support two exclusive modes:

- `local_embedded`
- `remote_existing`

### FR-2: Mode-aware boot pipeline

- Local mode keeps the current spawn, wait, and load flow
- Remote mode skips local server spawn and navigates only after remote preflight succeeds

### FR-3: Saved profiles

- Persist multiple connection profiles locally
- Support full CRUD operations

### FR-4: Mandatory remote preflight

Before connecting to a remote URL, run preflight checks:

- URL parse and normalize
- network reachability
- Paperclip health probe via `/api/health`
- compatibility inspection of:
  - `status`
  - `deploymentMode`
  - `deploymentExposure`
  - `authReady`
- auth-session probe via `/api/auth/get-session`

The health probe is not optional in V1.

### FR-5: Remote eligibility policy

Desktop remote mode must only allow connecting when the target:

- responds as Paperclip
- is not obviously incompatible
- is in an upstream-supported remote shape

Default V1 policy:

- allow `deploymentMode === "authenticated"` and `authReady === true`
- block `deploymentMode === "local_trusted"` for remote usage
- block non-Paperclip endpoints
- block invalid TLS by default

### FR-6: Auth handling

- Desktop must not implement its own remote username and password exchange
- Desktop must not persist raw remote credentials in connection profiles
- Remote authentication is handled by the remote Paperclip instance and its existing auth routes

### FR-7: Security allowlist

- In local mode, renderer may only navigate to the active localhost origin
- In remote mode, renderer may only navigate to the configured remote origin
- All other links open externally

### FR-8: Preload isolation

- Local mode may use a local preload surface as needed
- Remote mode must not expose privileged local APIs intended for the bundled localhost UI

### FR-9: Local mode compatibility

- Existing local behaviors remain unchanged by default

### FR-10: Telemetry or events

Emit structured events if telemetry exists:

- `connection_mode_selected`
- `remote_preflight_succeeded`
- `remote_preflight_failed`
- `remote_signin_required`
- `connection_succeeded`
- `connection_failed`
- `profile_created`
- `profile_updated`
- `profile_deleted`

---

## 8) Non-functional Requirements

- **Performance:** connection-switch UI action to first paint <= 2.5s on healthy network
- **Reliability:** graceful degradation on unreachable remote endpoints
- **Security:** maintain `contextIsolation=true`, `nodeIntegration=false`, exact-origin checks, and no privileged remote preload surface
- **Maintainability:** mode logic separated into testable modules

---

## 9) Technical Design (High Level)

## 9.1 New core abstractions

### `ConnectionProfile`

```ts
interface ConnectionProfile {
  id: string;
  name: string;
  mode: "local_embedded" | "remote_existing";
  remoteUrl?: string;
  createdAt: string;
  updatedAt: string;
  lastConnectedAt?: string;
  lastHealth?: "healthy" | "auth_required" | "unsupported" | "unreachable" | "unknown";
  lastDeploymentMode?: "local_trusted" | "authenticated" | null;
}
```

### `ConnectionState`

```ts
interface ConnectionState {
  activeProfileId: string | null;
  alwaysShowChooser: boolean;
  autoConnectLastProfile: boolean;
}
```

### `RemotePreflightResult`

```ts
interface RemotePreflightResult {
  ok: boolean;
  normalizedUrl: string;
  origin: string;
  paperclipDetected: boolean;
  deploymentMode: "local_trusted" | "authenticated" | null;
  deploymentExposure: "private" | "public" | null;
  authReady: boolean | null;
  sessionState: "signed_in" | "signed_out" | "unknown";
  reason?:
    | "invalid_url"
    | "unreachable"
    | "tls_error"
    | "not_paperclip"
    | "unsupported_local_trusted"
    | "incompatible_version";
  warning?: string;
}
```

## 9.2 New modules (proposed)

- `src/connection/profiles.ts` - persistence and CRUD
- `src/connection/validate.ts` - URL normalization and remote preflight
- `src/connection/boot.ts` - mode-aware boot orchestration
- `src/connection/window-policy.ts` - allowed-origin and preload policy
- `src/ui/connection/*` - chooser and manage views

## 9.3 Main process refactor

Current flow in `main.ts` should be split into:

- `bootLocal(profile)`
  - existing server startup lifecycle
- `bootRemote(profile)`
  - validate URL
  - run remote preflight
  - create remote-safe window policy
  - load remote URL if eligible

Window creation should accept explicit policy:

```ts
createWindow({
  startUrl,
  allowedOrigins,
  mode,
  preloadVariant,
});
```

## 9.4 Origin policy

Implement strict allowed-origin checks:

- Local mode: allow only `localhost` or `127.0.0.1` on the active port
- Remote mode: allow only the configured remote origin

Any navigation outside the allowed origin:

- prevent in-app navigation
- open in the system browser if safe (`http` or `https`)

## 9.5 Preload policy

- Local mode can keep a desktop preload surface for updater events or local integrations
- Remote mode should use either:
  - no preload, or
  - a minimal remote-safe preload that exposes no privileged local functionality

Remote mode must not expose local updater or app-control APIs to arbitrary remote content.

## 9.6 Persistence

Store profiles and state under app `userData`:

- `connections.json`

Store only:

- profile metadata
- last known health metadata
- local settings
- per-profile or per-origin Electron browser session state needed to preserve upstream remote sign-in continuity

Do not store:

- remote passwords
- raw auth tokens copied out of the browser context

Implementation note:

- if the user asks Desktop to remember an otherwise-unsaved remote choice, Desktop may first persist that verified remote as a normal saved profile so the reconnect target remains stable without storing credentials

---

## 10) Error Handling UX

### Remote URL invalid

- Inline message: `Enter a valid http(s) URL.`

### Host unreachable

- Message and actions: `Retry`, `Edit URL`, `Switch to Local`

### Non-Paperclip endpoint

- Hard block
- Message: `This host does not appear to be a Paperclip server.`

### Remote Paperclip in `local_trusted` mode

- Hard block
- Message: `This Paperclip server is configured for loopback-only local use. Reconfigure the remote server to upstream authenticated mode before using Desktop remote mode.`

### Sign-in required

- Informational state, not an error
- Message: `Verified Paperclip remote detected. Sign in on the remote instance to continue.`

### TLS or certificate errors

- Present explicit warning and remediation guidance
- V1 default: block with clear explanation

### Version mismatch

- Warning, not hard block, unless the incompatibility is known to break startup

---

## 11) Security Considerations

1. Keep Electron hardening defaults (`contextIsolation`, no node integration)
2. Restrict in-app navigation to the active allowed origin set
3. Validate and sanitize remote URLs
4. Never expose local privileged APIs to arbitrary remote pages
5. Require remote Paperclip verification before loading remote content
6. Reuse upstream authentication rather than storing remote passwords locally
7. Keep updater concerns isolated from remote browsing concerns
8. Keep local and remote session data partitioned by profile or origin where practical

---

## 12) Analytics / Observability (Optional)

Track events if a telemetry framework exists:

- `connection_mode_selected`
- `remote_preflight_attempted`
- `remote_preflight_succeeded`
- `remote_preflight_failed`
- `remote_signin_required`
- `connection_succeeded`
- `connection_failed`
- `profile_created`
- `profile_updated`
- `profile_deleted`
- `quick_connect_used`

Also add structured logs in the main process for boot path, preflight result, and failure reasons.

---

## 13) QA Strategy

### Unit tests

- URL normalization and validation
- profile CRUD operations
- allowed-origin policy checks
- remote preflight eligibility logic

### Integration tests

- local boot path remains unaffected
- remote boot path skips local spawn
- remote mode blocks `local_trusted`
- remote mode blocks non-Paperclip endpoints
- remote mode allows authenticated Paperclip and routes to sign-in when unsigned

### Manual matrix

- macOS Intel and Apple Silicon
- online and offline states
- reachable authenticated tailnet endpoint
- reachable `local_trusted` endpoint over network
- invalid certificates
- malformed URLs
- signed-in and signed-out authenticated instances

---

## 14) Rollout Plan

### Phase 1 (MVP)

- Mode chooser
- Remote preflight
- Remote mode safety policy
- Manual URL connect, no saved profiles required

### Phase 2

- Saved profiles CRUD
- Quick connect menu
- Auto-connect last profile

### Phase 3

- Polishing: recents, richer diagnostics, compatibility warnings

Feature flag recommendation:

- Ship behind `remoteModeEnabled` for initial beta

---

## 15) Risks and Mitigations

1. **Security regressions from remote navigation**
   - Mitigation: exact-origin allowlist, remote-safe preload policy, mandatory preflight

2. **User confusion between local and remote state**
   - Mitigation: always show active profile badge and clear mode labeling

3. **Connection instability on mobile or hotspot networks**
   - Mitigation: resilient retry UX, clear errors, quick fallback to local

4. **Version mismatch with remote Paperclip**
   - Mitigation: lightweight compatibility warning, not hard block by default

5. **Users trying to connect to arbitrary web pages**
   - Mitigation: block non-Paperclip endpoints and unsupported deployment modes

---

## 16) Acceptance Criteria

1. User can choose Local or Remote on launch
2. User can input a remote URL, verify it, and connect successfully when it is a supported remote Paperclip deployment
3. Remote mode does not spawn a local Paperclip server process
4. User can create, edit, and delete saved remote profiles
5. User can quick-connect to saved profiles from app menu or tray
6. Out-of-origin in-app navigation is blocked and opened externally
7. Remote mode does not expose local privileged preload APIs to the remote page
8. Existing local startup behavior works unchanged when Local is selected

---

## 17) Open Questions

1. Should HTTP on private or tailnet hosts require an explicit acknowledgment every time or only on first save?
2. How should version compatibility with remote Paperclip versions be detected reliably?
3. Should remote-mode browser sessions be isolated per saved profile or shared per origin?
4. Should profile storage be plain JSON or migrated into an existing store abstraction?

---

## 18) Implementation Checklist

- [ ] Define profile, state, and preflight result types
- [ ] Build persistence layer and migration path
- [ ] Build launch chooser UI
- [ ] Build remote connect UI with verification states
- [ ] Refactor boot logic into mode-specific paths
- [ ] Implement mode-specific window and preload policies
- [ ] Implement required remote health and session preflight
- [ ] Block unsupported remote deployment modes
- [ ] Add Saved Connections CRUD UI
- [ ] Add Quick Connect in app menu and tray menu
- [ ] Add automated tests
- [ ] Add docs and release notes
