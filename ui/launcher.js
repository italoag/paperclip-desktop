const launcher = window.paperclipLauncher;
let selectedCard = "local";
let editingId = null;
let modalReturnView = "saved";
let lastVerification = null;
let lastErrorAction = null;
let pendingDeleteId = null;
let deleteTriggerEl = null;
let snapshot = null;
let requestContentHeightReport = () => {};

const stepElements = {
  init: document.getElementById("step-init"),
  database: document.getElementById("step-database"),
  server: document.getElementById("step-server"),
  ready: document.getElementById("step-ready"),
};
const stepOrder = ["init", "database", "server", "ready"];

function getRemoteRememberChoice() {
  return !!(
    document.getElementById("rememberRemoteForm")?.checked
    || document.getElementById("rememberRemoteChooser")?.checked
  );
}

function setRemoteRememberChoice(checked) {
  const chooser = document.getElementById("rememberRemoteChooser");
  const form = document.getElementById("rememberRemoteForm");
  if (chooser) chooser.checked = !!checked;
  if (form) form.checked = !!checked;
}

function isInsecureHttpUrl(value) {
  return String(value || "").trim().toLowerCase().startsWith("http://");
}

function getRemoteInsecureHttpChoice() {
  return !!document.getElementById("allowInsecureHttp")?.checked;
}

function setRemoteInsecureHttpChoice(checked) {
  const checkbox = document.getElementById("allowInsecureHttp");
  if (checkbox) checkbox.checked = !!checked;
}

function getModalInsecureHttpChoice() {
  return !!document.getElementById("modalAllowInsecureHttp")?.checked;
}

function setModalInsecureHttpChoice(checked) {
  const checkbox = document.getElementById("modalAllowInsecureHttp");
  if (checkbox) checkbox.checked = !!checked;
}

function setInsecureHttpUi(section, required, checked, detailText) {
  const card = document.getElementById(section === "remote" ? "httpWarningCard" : "modalHttpWarningCard");
  const detail = document.getElementById(section === "remote" ? "httpWarningDetail" : "modalHttpWarningDetail");
  const setChoice = section === "remote" ? setRemoteInsecureHttpChoice : setModalInsecureHttpChoice;

  card.classList.toggle("active", required);
  if (detail) {
    detail.textContent = detailText || (
      section === "remote"
        ? "This URL disables TLS. Only continue if you trust the network path to this remote."
        : "Saving this URL means future reconnects can reuse plaintext HTTP unless you switch it back to HTTPS."
    );
  }
  setChoice(required ? checked : false);

  if (section === "remote") {
    syncRemoteActionButtons();
  }
  requestContentHeightReport();
}

function remoteHttpVerificationRequiresAck() {
  const remoteUrl = (document.getElementById("remoteUrl")?.value || "").trim();
  return isInsecureHttpUrl(remoteUrl) && !getRemoteInsecureHttpChoice();
}

function syncRemoteActionButtons(verifying = false) {
  const verifyBtn = document.getElementById("verifyBtn");
  const signinBtn = document.getElementById("signinBtn");
  const saveBtn = document.getElementById("saveBtn");
  const placeholder = document.getElementById("verifyPlaceholder");
  if (!verifyBtn || !signinBtn || !saveBtn) {
    return;
  }

  const requiresHttpAck = remoteHttpVerificationRequiresAck();
  const canProceed = !!(
    lastVerification
    && lastVerification.ok
    && (!lastVerification.insecureTransport || getRemoteInsecureHttpChoice())
  );
  verifyBtn.disabled = verifying || requiresHttpAck;
  verifyBtn.textContent = verifying ? "Verifying..." : "Verify Remote";
  signinBtn.disabled = !canProceed;
  saveBtn.disabled = !canProceed;
  if (placeholder) {
    placeholder.style.display = (lastVerification || verifying || requiresHttpAck) ? "none" : "block";
  }
}

function showView(name) {
  document.querySelectorAll(".view").forEach((view) => {
    view.classList.remove("active");
    view.style.opacity = "0";
  });

  const viewEl = document.getElementById("view-" + name);
  if (viewEl) {
    viewEl.classList.add("active");
    void viewEl.offsetWidth;
    viewEl.style.animation = "none";
    void viewEl.offsetWidth;
    viewEl.style.animation = "fadeIn 0.35s ease-out forwards";
  }

  if (name === "saved") {
    renderConnections();
  }
  if (name === "local-boot") {
    resetLocalBoot();
  }
  updateWindowClose(name);
  focusFirstInteractive(name);
  requestContentHeightReport();
}

function focusFirstInteractive(viewName) {
  requestAnimationFrame(() => {
    switch (viewName) {
      case "chooser": {
        const activeTab = selectedCard === "remote" ? "tab-btn-remote" : "tab-btn-local";
        document.getElementById(activeTab)?.focus();
        break;
      }
      case "remote-form":
        document.getElementById("remoteUrl")?.focus();
        break;
      case "saved": {
        const first = document.querySelector(".conn-item");
        if (first) { first.focus(); }
        else { document.querySelector("#view-saved .btn")?.focus(); }
        break;
      }
      case "error": {
        const firstBtn = document.querySelector("#view-error .btn-row .btn:not([style*='display:none']):not([style*='display: none'])");
        firstBtn?.focus();
        break;
      }
      default:
        break;
    }
  });
}

function switchTab(tab) {
  selectedCard = tab;
  document.getElementById("tab-btn-local").classList.toggle("active", tab === "local");
  document.getElementById("tab-btn-remote").classList.toggle("active", tab === "remote");
  document.getElementById("tab-btn-local").tabIndex = tab === "local" ? 0 : -1;
  document.getElementById("tab-btn-remote").tabIndex = tab === "remote" ? 0 : -1;
  document.getElementById("tab-local").classList.toggle("active", tab === "local");
  document.getElementById("tab-remote").classList.toggle("active", tab === "remote");
  if (tab === "remote") {
    renderTabRemoteList();
  }
  requestContentHeightReport();
}

function renderChooser() {
  const tab = snapshot && snapshot.state.chooserMode === "remote_existing" ? "remote" : "local";
  switchTab(tab);
  showView("chooser");
}

function showChooser() {
  renderChooser();
  launcher.showChooser();
}

function showChooserRemoteTab() {
  switchTab("remote");
  showView("chooser");
  launcher.setChooserMode("remote_existing");
}

function renderTabRemoteList() {
  const list = document.getElementById("tabRemoteList");
  if (!snapshot) { list.innerHTML = '<div class="tab-empty">No remote connections yet.</div>'; return; }

  const remotes = (snapshot.profiles || []).filter((p) => p.mode === "remote_existing");
  if (!remotes.length) {
    list.innerHTML = '<div class="tab-empty">No remote connections yet.</div>';
    return;
  }

  const globeIcon = '<svg viewBox="0 0 24 24" fill="none" stroke="#a1a1aa" stroke-width="1.5"><circle cx="12" cy="12" r="10"/><line x1="2" y1="12" x2="22" y2="12"/><path d="M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10z"/></svg>';

  list.innerHTML = remotes.map((p) => {
    return '<div class="tab-conn-item" tabindex="0" data-action="quickConnect" data-id="' + escapeAttr(p.id) + '">' +
      '<div class="tab-conn-icon">' + globeIcon + '</div>' +
      '<div class="tab-conn-info">' +
        '<div class="tab-conn-name">' + escapeHtml(p.name) + '</div>' +
        '<div class="tab-conn-url">' + escapeHtml(p.remoteUrl || "") + '</div>' +
      '</div>' +
      '<div class="tab-conn-dot ' + statusClass(p) + '"></div>' +
      '<button class="tab-conn-edit" data-action="openEditModal" data-stop="1" data-id="' + escapeAttr(p.id) + '">Edit</button>' +
    '</div>';
  }).join("");
}

async function launchLocal() {
  const rememberChoice = document.getElementById("rememberLocal").checked;
  await launcher.setChooserMode("local_embedded");
  lastErrorAction = { type: "local", rememberChoice };
  resetLocalBoot();
  showView("local-boot");
  await launcher.connectLocal({ rememberChoice });
}

function openAddRemoteFromChooser() {
  launcher.setChooserMode("remote_existing");
  setInsecureHttpUi("remote", isInsecureHttpUrl(document.getElementById("remoteUrl").value), false);
  resetVerificationUi();
  showView("remote-form");
}

function resetVerificationUi() {
  lastVerification = null;
  document.getElementById("urlError").style.display = "none";
  document.getElementById("urlSuccess").style.display = "none";
  document.getElementById("testStatus").innerHTML = "";
  document.getElementById("statusCard").classList.remove("active");
  document.getElementById("verifyInfo")?.classList.remove("visible");
  setInsecureHttpUi("remote", isInsecureHttpUrl(document.getElementById("remoteUrl").value), getRemoteInsecureHttpChoice());
  syncRemoteActionButtons();
}

function renderStatus(result) {
  const testStatus = document.getElementById("testStatus");
  const card = document.getElementById("statusCard");
  const title = document.getElementById("statusTitle");
  const detail = document.getElementById("statusDetail");
  const meta = document.getElementById("statusMeta");
  const success = document.getElementById("urlSuccess");
  const signinBtn = document.getElementById("signinBtn");
  const saveBtn = document.getElementById("saveBtn");
  const mapped = mapVerificationResult(result);

  testStatus.innerHTML = '<div class="status-badge ' + mapped.badgeClass + '"><div class="dot"></div>' + escapeHtml(mapped.badge) + "</div>";
  card.classList.add("active");
  title.textContent = mapped.title;
  detail.textContent = mapped.detail;
  meta.innerHTML = mapped.meta.map((item) => '<span class="pill">' + escapeHtml(item) + "</span>").join("");
  const tipMeta = document.getElementById("statusTipMeta");
  if (tipMeta) {
    tipMeta.innerHTML = mapped.meta.map((item) => {
      const idx = item.indexOf("=");
      const k = idx === -1 ? item : item.slice(0, idx);
      const v = idx === -1 ? "" : item.slice(idx + 1);
      return '<span class="k">' + escapeHtml(k) + '</span><span class="v">' + escapeHtml(v) + "</span>";
    }).join("");
  }
  document.getElementById("verifyInfo")?.classList.add("visible");

  success.style.display = mapped.success ? "block" : "none";
  signinBtn.textContent = mapped.actionLabel;
  saveBtn.textContent = mapped.saveLabel;
  signinBtn.disabled = !mapped.success;
  saveBtn.disabled = !mapped.success;
  setInsecureHttpUi("remote", result.insecureTransport, getRemoteInsecureHttpChoice(), result.warning);
  syncRemoteActionButtons();
}

async function verifyRemote() {
  const remoteUrl = document.getElementById("remoteUrl").value.trim();
  resetVerificationUi();

  if (!remoteUrl) {
    document.getElementById("urlError").textContent = "Enter a valid remote URL.";
    document.getElementById("urlError").style.display = "block";
    return;
  }

  if (remoteHttpVerificationRequiresAck()) {
    document.getElementById("urlError").textContent = "Confirm the insecure HTTP warning before verifying this connection.";
    document.getElementById("urlError").style.display = "block";
    return;
  }

  const verifyBtn = document.getElementById("verifyBtn");
  document.getElementById("testStatus").innerHTML = '<div class="status-badge testing"><div class="dot"></div>Verifying remote...</div>';
  syncRemoteActionButtons(true);

  try {
    const result = await launcher.verifyRemote({ remoteUrl });
    lastVerification = result;

    if (result.reason === "invalid_url") {
      document.getElementById("testStatus").innerHTML = "";
      document.getElementById("urlError").textContent = result.detail || "Enter a valid remote URL.";
      document.getElementById("urlError").style.display = "block";
      return;
    }

    setInsecureHttpUi("remote", result.insecureTransport, getRemoteInsecureHttpChoice(), result.warning);
    renderStatus(result);
  } finally {
    verifyBtn.textContent = "Verify Remote";
    syncRemoteActionButtons();
  }
}

async function continueToSignIn() {
  if (!lastVerification || !lastVerification.ok) {
    return;
  }

  const allowInsecureHttp = getRemoteInsecureHttpChoice();
  if (lastVerification.insecureTransport && !allowInsecureHttp) {
    document.getElementById("urlError").textContent = "Confirm the insecure HTTP warning before connecting.";
    document.getElementById("urlError").style.display = "block";
    return;
  }

  const rememberChoice = getRemoteRememberChoice();
  const remoteUrl = document.getElementById("remoteUrl").value.trim();
  const displayName = document.getElementById("displayName").value.trim();
  lastErrorAction = {
    type: "remote",
    saveProfile: false,
    rememberChoice,
    remoteUrl,
    displayName,
    allowInsecureHttp,
  };

  showRemoteConnectingState(lastVerification);
  await launcher.connectRemote({
    remoteUrl,
    displayName,
    saveProfile: false,
    rememberChoice,
    allowInsecureHttp,
  });
}

async function connectAndSave() {
  if (!lastVerification || !lastVerification.ok) {
    document.getElementById("urlError").style.display = "block";
    return;
  }

  const allowInsecureHttp = getRemoteInsecureHttpChoice();
  if (lastVerification.insecureTransport && !allowInsecureHttp) {
    document.getElementById("urlError").textContent = "Confirm the insecure HTTP warning before saving or connecting.";
    document.getElementById("urlError").style.display = "block";
    return;
  }

  const rememberChoice = getRemoteRememberChoice();
  const remoteUrl = document.getElementById("remoteUrl").value.trim();
  const displayName = document.getElementById("displayName").value.trim();
  lastErrorAction = {
    type: "remote",
    saveProfile: true,
    rememberChoice,
    remoteUrl,
    displayName,
    allowInsecureHttp,
  };

  showRemoteConnectingState(lastVerification);
  await launcher.connectRemote({
    remoteUrl,
    displayName,
    saveProfile: true,
    rememberChoice,
    allowInsecureHttp,
  });
}

async function retryLastAction() {
  if (!lastErrorAction) {
    return;
  }

  if (lastErrorAction.type === "local") {
    resetLocalBoot();
    showView("local-boot");
    await launcher.connectLocal({ rememberChoice: !!lastErrorAction.rememberChoice });
    return;
  }

  document.getElementById("remoteUrl").value = lastErrorAction.remoteUrl || "";
  document.getElementById("displayName").value = lastErrorAction.displayName || "";
  setInsecureHttpUi("remote", isInsecureHttpUrl(lastErrorAction.remoteUrl), !!lastErrorAction.allowInsecureHttp);
  showRemoteConnectingState(lastVerification || {
    ok: true,
    insecureTransport: isInsecureHttpUrl(lastErrorAction.remoteUrl),
    origin: "",
    paperclipDetected: true,
    deploymentMode: null,
    deploymentExposure: null,
    authReady: null,
    sessionState: "signed_out",
    bootstrapStatus: null,
    bootstrapInviteActive: null,
    normalizedUrl: lastErrorAction.remoteUrl,
    version: null,
  });
  await launcher.connectRemote({
    remoteUrl: lastErrorAction.remoteUrl,
    displayName: lastErrorAction.displayName,
    saveProfile: !!lastErrorAction.saveProfile,
    rememberChoice: !!lastErrorAction.rememberChoice,
    allowInsecureHttp: !!lastErrorAction.allowInsecureHttp,
  });
}

function configureErrorButtons() {
  const retryBtn = document.getElementById("errorRetryBtn");
  const tryDiffBtn = document.getElementById("errorTryDifferentBtn");
  const switchRemoteBtn = document.getElementById("errorSwitchRemoteBtn");
  const switchLocalBtn = document.getElementById("errorSwitchLocalBtn");

  const isLocal = lastErrorAction && lastErrorAction.type === "local";
  const hasAction = !!lastErrorAction;

  retryBtn.style.display = hasAction ? "" : "none";
  tryDiffBtn.style.display = isLocal ? "none" : "";
  switchRemoteBtn.style.display = isLocal ? "" : "none";
  switchLocalBtn.style.display = isLocal ? "none" : "";

  if (isLocal) {
    switchRemoteBtn.classList.add("primary");
  } else {
    switchRemoteBtn.classList.remove("primary");
  }
}

async function switchToLocal() {
  const rememberChoice = false;
  lastErrorAction = { type: "local", rememberChoice };
  resetLocalBoot();
  showView("local-boot");
  await launcher.connectLocal({ rememberChoice });
}

async function closeLauncherSheet() {
  await launcher.closeSheet();
}

function showRemoteConnectingState(result) {
  const loopState = result.bootstrapStatus === "bootstrap_pending"
    ? "bootstrap_pending"
    : result.sessionState === "signed_out"
      ? "signin_required"
      : null;

  document.getElementById("connectingLabel").textContent =
    loopState === "bootstrap_pending"
      ? "Opening remote setup..."
      : loopState === "signin_required"
        ? "Opening remote sign-in..."
        : "Opening verified remote...";
  document.getElementById("connectingUrl").textContent = result.normalizedUrl || document.getElementById("remoteUrl").value.trim();
  showView("connecting");
}

function updateWindowClose(viewName) {
  const button = document.getElementById("windowCloseBtn");
  const hiddenViews = new Set(["local-boot", "connecting"]);
  const visible = !!(snapshot && snapshot.isAttachedLauncher && !hiddenViews.has(viewName));
  button.classList.toggle("visible", visible);
}

function renderConnections() {
  const list = document.getElementById("connectionsList");
  const profiles = snapshot ? snapshot.profiles : [];

  if (!profiles.length) {
    list.innerHTML = '<div class="empty-state">No saved connections yet.</div>';
    return;
  }

  list.innerHTML = profiles.map((profile) => {
    const icon = profile.mode === "local_embedded"
      ? '<svg viewBox="0 0 24 24" fill="none" stroke="#a1a1aa" stroke-width="1.5"><rect x="2" y="3" width="20" height="14" rx="2"/><line x1="8" y1="21" x2="16" y2="21"/><line x1="12" y1="17" x2="12" y2="21"/></svg>'
      : '<svg viewBox="0 0 24 24" fill="none" stroke="#a1a1aa" stroke-width="1.5"><circle cx="12" cy="12" r="10"/><line x1="2" y1="12" x2="22" y2="12"/><path d="M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10z"/></svg>';
    const lastConnected = profile.lastConnectedAt
      ? '<span class="pill">Last connected: ' + escapeHtml(relativeTime(profile.lastConnectedAt)) + "</span>"
      : "";
    const actions = profile.mode === "remote_existing"
      ? "<div class=\"conn-actions\" data-stop=\"1\">" +
          "<button title=\"Edit\" data-action=\"openEditModal\" data-id=\"" + escapeAttr(profile.id) + "\">&#9998;</button>" +
          "<button title=\"Duplicate\" data-action=\"duplicateConn\" data-id=\"" + escapeAttr(profile.id) + "\">&#10697;</button>" +
          "<button class=\"delete-btn\" title=\"Delete\" data-action=\"deleteConn\" data-id=\"" + escapeAttr(profile.id) + "\">&#10005;</button>" +
        "</div>"
      : "";

    return "<div class=\"conn-item " + (snapshot.activeProfileId === profile.id ? "active-conn" : "") + "\" tabindex=\"0\" data-action=\"quickConnect\" data-id=\"" + escapeAttr(profile.id) + "\">" +
      '<div class="conn-icon">' + icon + "</div>" +
      '<div class="conn-info">' +
        '<div class="conn-name">' + escapeHtml(profile.name) + "</div>" +
        '<div class="conn-url">' + escapeHtml(profile.remoteUrl || "Embedded local server") + "</div>" +
        '<div class="conn-meta"><span class="pill">' + escapeHtml(capabilityLabel(profile)) + "</span>" + lastConnected + "</div>" +
      "</div>" +
      '<div class="conn-status-dot ' + statusClass(profile) + '"></div>' +
      actions +
    "</div>";
  }).join("");
}

function openAddModal() {
  modalReturnView = "saved";
  editingId = null;
  document.getElementById("modalTitle").textContent = "Add Connection";
  document.getElementById("modalName").value = "";
  document.getElementById("modalUrl").value = "";
  document.getElementById("modalError").style.display = "none";
  setInsecureHttpUi("modal", false, false);
  document.getElementById("editModal").classList.add("active");
}

function openEditModal(profileId) {
  const profile = snapshot.profiles.find((candidate) => candidate.id === profileId);
  if (!profile || profile.mode !== "remote_existing") {
    return;
  }

  const activeView = (document.querySelector(".view.active") || {}).id?.replace("view-", "");
  modalReturnView = activeView === "chooser" && selectedCard === "remote" ? "chooser-remote" : "saved";
  editingId = profileId;
  document.getElementById("modalTitle").textContent = "Edit Connection";
  document.getElementById("modalName").value = profile.name;
  document.getElementById("modalUrl").value = profile.remoteUrl || "";
  document.getElementById("modalError").style.display = "none";
  setInsecureHttpUi("modal", isInsecureHttpUrl(profile.remoteUrl), !!profile.allowInsecureHttp);
  document.getElementById("editModal").classList.add("active");
}

function closeModal() {
  editingId = null;
  setInsecureHttpUi("modal", false, false);
  document.getElementById("editModal").classList.remove("active");
}

function restoreModalReturnView() {
  if (modalReturnView === "chooser-remote") {
    showChooserRemoteTab();
    return;
  }

  showView("saved");
}

async function saveModal() {
  const name = document.getElementById("modalName").value.trim();
  const remoteUrl = document.getElementById("modalUrl").value.trim();

  try {
    const nextSnapshot = await launcher.saveRemoteProfile({
      profileId: editingId || undefined,
      name,
      remoteUrl,
      allowInsecureHttp: getModalInsecureHttpChoice(),
    });
    applySnapshot(nextSnapshot);
    closeModal();
    restoreModalReturnView();
  } catch (error) {
    document.getElementById("modalError").textContent = error && error.message ? error.message : "Enter a valid remote URL.";
    document.getElementById("modalError").style.display = "block";
  }
}

async function duplicateConn(profileId) {
  const nextSnapshot = await launcher.duplicateProfile(profileId);
  applySnapshot(nextSnapshot);
  showView("saved");
}

function deleteConn(profileId) {
  const profile = snapshot.profiles.find((candidate) => candidate.id === profileId);
  if (!profile) {
    return;
  }

  pendingDeleteId = profileId;
  deleteTriggerEl = document.activeElement;
  document.getElementById("deleteProfileName").textContent = profile.name;
  document.getElementById("deleteModal").classList.add("active");
  document.getElementById("deleteCancelBtn").focus();
}

async function confirmDelete() {
  if (!pendingDeleteId) {
    return;
  }

  const nextSnapshot = await launcher.deleteProfile(pendingDeleteId);
  pendingDeleteId = null;
  document.getElementById("deleteModal").classList.remove("active");
  applySnapshot(nextSnapshot);
  showView("saved");
}

function cancelDelete() {
  pendingDeleteId = null;
  document.getElementById("deleteModal").classList.remove("active");
  if (deleteTriggerEl && typeof deleteTriggerEl.focus === "function") {
    deleteTriggerEl.focus();
  }
  deleteTriggerEl = null;
}

async function quickConnect(profileId) {
  const profile = snapshot.profiles.find((candidate) => candidate.id === profileId);
  if (!profile) {
    return;
  }

  if (profile.mode === "local_embedded") {
    lastErrorAction = { type: "local", rememberChoice: false };
    resetLocalBoot();
    showView("local-boot");
    await launcher.connectSavedProfile({ profileId, rememberChoice: false });
    return;
  }

  const rememberChoice = getRemoteRememberChoice();
  document.getElementById("remoteUrl").value = profile.remoteUrl || "";
  document.getElementById("displayName").value = profile.name;
  setInsecureHttpUi("remote", isInsecureHttpUrl(profile.remoteUrl), !!profile.allowInsecureHttp);
  lastErrorAction = {
    type: "remote",
    saveProfile: false,
    rememberChoice,
    remoteUrl: profile.remoteUrl,
    displayName: profile.name,
    allowInsecureHttp: !!profile.allowInsecureHttp,
  };
  document.getElementById("connectingLabel").textContent = "Opening verified remote...";
  document.getElementById("connectingUrl").textContent = profile.remoteUrl || "";
  showView("connecting");
  await launcher.connectSavedProfile({ profileId, rememberChoice });
}

function resetLocalBoot() {
  document.getElementById("localProgress").style.width = "0%";
  Object.values(stepElements).forEach((step) => {
    step.classList.remove("active", "done");
    step.querySelector(".step-icon").innerHTML = '<div class="pending"></div>';
    const detail = step.querySelector(".step-detail");
    if (detail) {
      detail.remove();
    }
  });
}

function applyBootStatus(payload) {
  showView("local-boot");
  document.getElementById("localProgress").style.width = Math.min(100, Math.max(0, payload.progress || 0)) + "%";
  const currentIndex = stepOrder.indexOf(payload.step);

  stepOrder.forEach((key, index) => {
    const step = stepElements[key];
    const iconContainer = step.querySelector(".step-icon");
    step.classList.remove("active", "done");

    if (index < currentIndex) {
      step.classList.add("done");
      iconContainer.innerHTML = '<span class="check">&#10003;</span>';
      ensureStepDetail(step, "");
      return;
    }

    if (index === currentIndex) {
      step.classList.add("active");
      iconContainer.innerHTML = '<div class="spinner"></div>';
      ensureStepDetail(step, payload.detail || "");
      return;
    }

    iconContainer.innerHTML = '<div class="pending"></div>';
    const detail = step.querySelector(".step-detail");
    if (detail) {
      detail.remove();
    }
  });
}

function ensureStepDetail(step, detailText) {
  if (!detailText) {
    return;
  }

  let detail = step.querySelector(".step-detail");
  if (!detail) {
    detail = document.createElement("div");
    detail.className = "step-detail";
    step.querySelector(".step-label").parentElement.appendChild(detail);
  }
  detail.textContent = detailText;
}

function capabilityLabel(profile) {
  if (profile.mode === "local_embedded") {
    return "Local embedded";
  }

  const suffix = profile.allowInsecureHttp ? " / insecure HTTP" : "";
  switch (profile.lastHealth) {
    case "healthy":
      return "Authenticated / session active" + suffix;
    case "auth_required":
      return "Authenticated / sign-in required" + suffix;
    case "unsupported":
      return "Blocked: unsupported remote" + suffix;
    case "unreachable":
      return "Host unreachable" + suffix;
    default:
      return "Pending verification" + suffix;
  }
}

function statusClass(profile) {
  switch (profile.lastHealth) {
    case "healthy":
      return "healthy";
    case "auth_required":
      return "auth-required";
    case "unsupported":
      return "unsupported";
    case "unreachable":
      return "unreachable";
    default:
      return "unknown";
  }
}

function mapVerificationResult(result) {
  if (result.ok && result.bootstrapStatus === "bootstrap_pending") {
    return {
      success: true,
      badgeClass: "warning",
      badge: "Verified remote - setup required",
      title: "Instance setup required",
      detail: mergeDetail(
        "The host is a verified authenticated Paperclip remote, but no instance admin exists yet. Connect Remote to continue through Paperclip's setup flow in-app.",
        result.warning,
      ),
      meta: buildResultMeta(result),
      actionLabel: "Connect Remote",
      saveLabel: "Save Connection",
    };
  }

  if (result.ok && result.sessionState === "signed_in") {
    return {
      success: true,
      badgeClass: "healthy",
      badge: "Authenticated Paperclip detected",
      title: "Verified remote ready",
      detail: mergeDetail(
        "The host looks like a Paperclip instance in authenticated mode, and an active session was found.",
        result.warning,
      ),
      meta: buildResultMeta(result),
      actionLabel: "Connect Remote",
      saveLabel: "Connect & Save",
    };
  }

  if (result.ok && result.sessionState === "signed_out") {
    return {
      success: true,
      badgeClass: "warning",
      badge: "Verified remote - sign-in required",
      title: "Verified remote, no active session",
      detail: mergeDetail(
        "The host looks like an authenticated Paperclip instance. Connect Remote to continue through Paperclip's sign-in flow in-app.",
        result.warning,
      ),
      meta: buildResultMeta(result),
      actionLabel: "Connect Remote",
      saveLabel: "Save Connection",
    };
  }

  const detail = mergeDetail(result.detail || "Could not verify remote.", result.warning);
  if (result.reason === "unsupported_local_trusted") {
    return {
      success: false,
      badgeClass: "unreachable",
      badge: "Unsupported remote deployment",
      title: "Paperclip detected but not eligible",
      detail,
      meta: buildResultMeta(result),
      actionLabel: "Continue to Sign-In",
      saveLabel: "Connect & Save",
    };
  }

  if (result.reason === "not_paperclip") {
    return {
      success: false,
      badgeClass: "unreachable",
      badge: "Host reachable, not Paperclip",
      title: "Non-Paperclip endpoint",
      detail,
      meta: buildResultMeta(result),
      actionLabel: "Continue to Sign-In",
      saveLabel: "Connect & Save",
    };
  }

  if (result.reason === "tls_error") {
    return {
      success: false,
      badgeClass: "unreachable",
      badge: "TLS validation failed",
      title: "Remote certificate rejected",
      detail,
      meta: buildResultMeta(result),
      actionLabel: "Continue to Sign-In",
      saveLabel: "Connect & Save",
    };
  }

  return {
    success: false,
    badgeClass: "unreachable",
    badge: "Host unreachable",
    title: "Could not verify remote",
    detail,
    meta: buildResultMeta(result),
    actionLabel: "Continue to Sign-In",
    saveLabel: "Connect & Save",
  };
}

function buildResultMeta(result) {
  const meta = [];
  if (result.deploymentMode) meta.push("deploymentMode=" + result.deploymentMode);
  if (result.deploymentExposure) meta.push("deploymentExposure=" + result.deploymentExposure);
  if (result.authReady !== null && result.authReady !== undefined) meta.push("authReady=" + result.authReady);
  if (result.bootstrapStatus) meta.push("bootstrapStatus=" + result.bootstrapStatus);
  if (result.bootstrapInviteActive !== null && result.bootstrapInviteActive !== undefined) meta.push("bootstrapInviteActive=" + result.bootstrapInviteActive);
  if (result.sessionState) meta.push("session=" + result.sessionState);
  if (result.version) meta.push("version=" + result.version);
  return meta;
}

function mergeDetail(detail, warning) {
  return warning ? detail + " " + warning : detail;
}

function relativeTime(timestamp) {
  const value = Date.parse(timestamp);
  if (Number.isNaN(value)) {
    return timestamp;
  }

  const diff = Date.now() - value;
  if (diff < 60_000) return "Just now";
  if (diff < 3_600_000) return Math.round(diff / 60_000) + " min ago";
  if (diff < 86_400_000) return Math.round(diff / 3_600_000) + " hr ago";
  return Math.round(diff / 86_400_000) + " day(s) ago";
}

function applySnapshot(nextSnapshot) {
  snapshot = nextSnapshot;
  const activeView = (document.querySelector(".view.active") || {}).id?.replace("view-", "");
  if (activeView === "chooser" || !activeView) {
    switchTab(nextSnapshot.state.chooserMode === "remote_existing" ? "remote" : "local");
  }
  setRemoteRememberChoice(
    !nextSnapshot.state.alwaysShowChooser
    && nextSnapshot.state.autoConnectLastProfile
    && nextSnapshot.state.activeProfileId
    && nextSnapshot.state.activeProfileId !== "local_embedded",
  );
  document.getElementById("rememberLocal").checked =
    !nextSnapshot.state.alwaysShowChooser && nextSnapshot.state.autoConnectLastProfile
    && nextSnapshot.state.chooserMode !== "remote_existing";
  renderConnections();
  renderTabRemoteList();
  updateWindowClose(activeView || nextSnapshot.initialView || "chooser");
}

function escapeHtml(value) {
  return String(value)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

function escapeAttr(value) {
  return escapeHtml(value).replaceAll("'", "&#39;");
}


function measureLauncherHeight(windowEl) {
  const styles = window.getComputedStyle(document.body);
  const verticalPadding = Number.parseFloat(styles.paddingTop || "0")
    + Number.parseFloat(styles.paddingBottom || "0");
  // Extra pixels compensate for macOS sheet frame chrome / rounded-corner insets
  return windowEl.getBoundingClientRect().height + verticalPadding + 8;
}

function installContentResizeReporting() {
  const windowEl = document.querySelector(".window");
  if (!windowEl || !launcher.reportContentHeight) {
    return;
  }

  let lastReportedHeight = 0;
  const report = () => {
    const height = Math.ceil(measureLauncherHeight(windowEl));
    if (Math.abs(height - lastReportedHeight) < 2) {
      return;
    }

    lastReportedHeight = height;
    launcher.reportContentHeight(height);
  };

  let settleFrameId = 0;
  let settleTimeoutIds = [];
  const scheduleReport = () => {
    report();

    if (settleFrameId) {
      cancelAnimationFrame(settleFrameId);
    }
    settleTimeoutIds.forEach((timeoutId) => clearTimeout(timeoutId));
    settleTimeoutIds = [];

    settleFrameId = requestAnimationFrame(() => {
      settleFrameId = requestAnimationFrame(() => {
        settleFrameId = 0;
        report();
      });
    });
    settleTimeoutIds = [
      window.setTimeout(report, 80),
      window.setTimeout(report, 220),
    ];
  };

  requestContentHeightReport = scheduleReport;
  new ResizeObserver(report).observe(windowEl);
  scheduleReport();
  window.addEventListener("load", scheduleReport, { once: true });
}

window.paperclipLauncher.onStateChanged((nextSnapshot) => {
  applySnapshot(nextSnapshot);
});

window.paperclipLauncher.onRequestContentHeightSync(() => {
  requestContentHeightReport();
});

window.paperclipLauncher.onNavigate((payload) => {
  if (payload.view === "saved") {
    showView("saved");
    return;
  }

  if (payload.view === "chooser") {
    renderChooser();
    return;
  }

  if (payload.view === "remote-form") {
    if (payload.profile) {
      document.getElementById("remoteUrl").value = payload.profile.remoteUrl || "";
      document.getElementById("displayName").value = payload.profile.name || "";
      setInsecureHttpUi("remote", isInsecureHttpUrl(payload.profile.remoteUrl), !!payload.profile.allowInsecureHttp);
    }
    resetVerificationUi();
    showView("remote-form");
    return;
  }

  if (payload.view === "connecting") {
    document.getElementById("connectingLabel").textContent = payload.label || "Opening verified remote...";
    document.getElementById("connectingUrl").textContent = payload.url || "";
    showView("connecting");
    return;
  }

  if (payload.view === "local-boot") {
    resetLocalBoot();
    showView("local-boot");
  }
});

window.paperclipLauncher.onBootStatus((payload) => {
  applyBootStatus(payload);
});

window.paperclipLauncher.onConnectionError((payload) => {
  document.getElementById("errorTitle").textContent = payload.title;
  document.getElementById("errorDetail").textContent = payload.detail;
  configureErrorButtons();
  showView("error");
});

// ---------------------------------------------------------------------------
// Keyboard handling
// ---------------------------------------------------------------------------

document.addEventListener("keydown", (event) => {
  if (event.key === "Escape") {
    const deleteModal = document.getElementById("deleteModal");
    if (deleteModal.classList.contains("active")) {
      cancelDelete();
      return;
    }

    const editModal = document.getElementById("editModal");
    if (editModal.classList.contains("active")) {
      closeModal();
      return;
    }

    if (snapshot && snapshot.isAttachedLauncher) {
      const activeView = (document.querySelector(".view.active") || {}).id?.replace("view-", "");
      if (activeView !== "local-boot" && activeView !== "connecting") {
        closeLauncherSheet();
      }
    }
    return;
  }

  const activeView = (document.querySelector(".view.active") || {}).id?.replace("view-", "");

  if (activeView === "chooser") {
    if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
      event.preventDefault();
      const next = selectedCard === "local" ? "remote" : "local";
      switchTab(next);
      document.getElementById("tab-btn-" + next).focus();
    } else if (event.key === "Enter" && document.activeElement?.classList.contains("tab-conn-item")) {
      event.preventDefault();
      document.activeElement.click();
    } else if (event.key === "ArrowDown" && selectedCard === "remote") {
      event.preventDefault();
      const items = Array.from(document.querySelectorAll(".tab-conn-item"));
      const idx = items.indexOf(document.activeElement);
      if (items.length) items[Math.min(idx + 1, items.length - 1)].focus();
    } else if (event.key === "ArrowUp" && selectedCard === "remote") {
      event.preventDefault();
      const items = Array.from(document.querySelectorAll(".tab-conn-item"));
      const idx = items.indexOf(document.activeElement);
      if (items.length) items[Math.max(idx - 1, 0)].focus();
    }
    return;
  }

  if (activeView === "saved") {
    const items = Array.from(document.querySelectorAll(".conn-item"));
    const focusedIndex = items.indexOf(document.activeElement);
    if (event.key === "ArrowDown" && items.length) {
      event.preventDefault();
      items[Math.min(focusedIndex + 1, items.length - 1)].focus();
    } else if (event.key === "ArrowUp" && items.length) {
      event.preventDefault();
      items[Math.max(focusedIndex - 1, 0)].focus();
    } else if (event.key === "Enter" && focusedIndex >= 0) {
      event.preventDefault();
      items[focusedIndex].click();
    }
    return;
  }

  if (activeView === "remote-form") {
    if (event.key === "Enter" && document.activeElement?.id === "remoteUrl") {
      event.preventDefault();
      verifyRemote();
    }
    return;
  }
});

// ---------------------------------------------------------------------------
// Modal focus trap
// ---------------------------------------------------------------------------

function trapFocusInModal(modalEl, event) {
  const focusable = modalEl.querySelectorAll("button:not([disabled]), input:not([disabled]), [tabindex]:not([tabindex='-1'])");
  if (!focusable.length) return;

  const first = focusable[0];
  const last = focusable[focusable.length - 1];

  if (event.shiftKey && document.activeElement === first) {
    event.preventDefault();
    last.focus();
  } else if (!event.shiftKey && document.activeElement === last) {
    event.preventDefault();
    first.focus();
  }
}

document.addEventListener("keydown", (event) => {
  if (event.key !== "Tab") return;

  const deleteModal = document.getElementById("deleteModal");
  if (deleteModal.classList.contains("active")) {
    trapFocusInModal(deleteModal, event);
    return;
  }

  const editModal = document.getElementById("editModal");
  if (editModal.classList.contains("active")) {
    trapFocusInModal(editModal, event);
    return;
  }
});

// ---------------------------------------------------------------------------
// Event delegation
// ---------------------------------------------------------------------------
//
// The Electron build wired buttons with inline `onclick` attributes. Those are
// blocked outright by this window's `script-src 'self'` policy, so every
// trigger now carries `data-action` and is dispatched from here.
//
// This also removed a whole class of risk: ids used to be escaped into
// generated JavaScript source with a bespoke `escapeJsSingleQuote` helper, and
// they now travel as ordinary attribute values instead.

const ACTIONS = {
  cancelDelete,
  closeLauncherSheet,
  closeModal,
  confirmDelete,
  connectAndSave,
  continueToSignIn,
  deleteConn,
  duplicateConn,
  launchLocal,
  openAddModal,
  openAddRemoteFromChooser,
  openEditModal,
  quickConnect,
  retryLastAction,
  saveModal,
  showChooser,
  showChooserRemoteTab,
  showView,
  switchTab,
  switchToLocal,
  verifyRemote,
};

function installActionDelegation() {
  document.addEventListener("click", (event) => {
    // Overlays close only when the overlay itself is clicked, never when the
    // click merely started inside the dialog it contains.
    const backdrop = event.target.closest("[data-backdrop-action]");
    if (backdrop && event.target === backdrop) {
      ACTIONS[backdrop.dataset.backdropAction]?.();
      return;
    }

    const trigger = event.target.closest("[data-action], [data-stop]");
    if (!trigger) return;

    // `closest` already resolves to the innermost action, which is what the old
    // `event.stopPropagation()` calls achieved. A `data-stop` element with no
    // action of its own is a boundary: clicks on it must not fall through to an
    // enclosing row.
    if (!trigger.dataset.action) return;

    const action = ACTIONS[trigger.dataset.action];
    if (!action) {
      console.warn("[launcher] unknown data-action:", trigger.dataset.action);
      return;
    }

    const argument = trigger.dataset.id ?? trigger.dataset.arg;
    if (argument === undefined) {
      action();
    } else {
      action(argument);
    }
  });
}

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

document.addEventListener("DOMContentLoaded", async () => {
  document.getElementById("remoteUrl")?.addEventListener("input", (event) => {
    const remoteUrl = event.target.value;
    setInsecureHttpUi("remote", isInsecureHttpUrl(remoteUrl), getRemoteInsecureHttpChoice());
    resetVerificationUi();
  });
  document.getElementById("allowInsecureHttp")?.addEventListener("change", () => {
    document.getElementById("urlError").style.display = "none";
    syncRemoteActionButtons();
  });
  document.getElementById("modalUrl")?.addEventListener("input", (event) => {
    setInsecureHttpUi("modal", isInsecureHttpUrl(event.target.value), getModalInsecureHttpChoice());
    document.getElementById("modalError").style.display = "none";
  });
  document.getElementById("modalAllowInsecureHttp")?.addEventListener("change", () => {
    document.getElementById("modalError").style.display = "none";
  });

  installActionDelegation();

  const initialSnapshot = await launcher.bootstrap();
  applySnapshot(initialSnapshot);
  showView(initialSnapshot.initialView || "chooser");
  installContentResizeReporting();
});
