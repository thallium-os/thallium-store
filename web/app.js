"use strict";

const sessionToken = document.querySelector('meta[name="thallium-session"]').content;
const content = document.getElementById("content");
const statusLine = document.getElementById("status");
const searchForm = document.getElementById("search-form");
const searchInput = document.getElementById("search-input");
const detailsDialog = document.getElementById("details-dialog");
const detailsContent = document.getElementById("details-content");
const confirmDialog = document.getElementById("confirm-dialog");
const toast = document.getElementById("toast");

const state = {
  view: "home",
  discover: [],
  operations: [],
  health: null,
  requestId: 0,
  toastTimer: null,
};

function node(tag, className, text) {
  const element = document.createElement(tag);
  if (className) element.className = className;
  if (text !== undefined && text !== null) element.textContent = String(text);
  return element;
}

function button(label, className, action, disabled = false) {
  const element = node("button", `button ${className || ""}`, label);
  element.type = "button";
  element.disabled = disabled;
  if (action) element.addEventListener("click", action);
  return element;
}

function setStatus(message) {
  statusLine.textContent = message;
}

function notify(message, error = false) {
  toast.textContent = message;
  toast.className = `toast${error ? " error" : ""}`;
  clearTimeout(state.toastTimer);
  state.toastTimer = setTimeout(() => toast.classList.add("hidden"), 4200);
}

async function rpc(method, params = {}) {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), 45000);
  try {
    const response = await fetch("/api", {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        "X-Thallium-Token": sessionToken,
      },
      body: JSON.stringify({ jsonrpc: "2.0", id: ++state.requestId, method, params }),
      signal: controller.signal,
    });
    if (!response.ok) throw new Error(`Backend returned HTTP ${response.status}`);
    const message = await response.json();
    if (message.error) throw new Error(message.error.message || "Backend request failed");
    return message.result;
  } catch (error) {
    const message = error.name === "AbortError" ? "The backend request timed out" : error.message;
    setStatus(message);
    throw new Error(message);
  } finally {
    clearTimeout(timeout);
  }
}

function pageHeading(kicker, title, subtitle) {
  const wrap = node("div", "page-heading");
  const copy = node("div");
  copy.append(node("span", "eyebrow", kicker), node("h1", "", title), node("p", "subtle", subtitle));
  wrap.append(copy);
  return wrap;
}

function emptyState(title, message) {
  const wrap = node("section", "empty-state");
  wrap.append(node("h2", "", title), node("p", "", message));
  return wrap;
}

function loading(message) {
  const wrap = node("section", "loading-state");
  wrap.append(node("span", "spinner"), node("p", "", message));
  content.replaceChildren(wrap);
}

function sourceLabel(source) {
  return ({ system: "APT", apt: "APT", dpkg: "APT", flathub: "Flatpak", flatpak: "Flatpak", github: "GitHub", appimage: "AppImage" })[source] || source || "Package";
}

function normalizedSource(source) {
  if (["apt", "dpkg", "system"].includes(source)) return "system";
  if (["flatpak", "flathub"].includes(source)) return "flathub";
  if (source === "github") return "github";
  return "appimage";
}

function recommendedVariant(app) {
  return app.variants?.find(item => item.id === app.recommended_variant_id) || app.variants?.[0] || null;
}

function iconFor(app, sizeClass = "app-icon") {
  if (app.icon && !app.icon.startsWith("file://")) {
    const image = node("img", sizeClass);
    image.src = app.icon;
    image.alt = "";
    image.loading = "lazy";
    image.addEventListener("error", () => image.replaceWith(iconFallback(app)));
    return image;
  }
  return iconFallback(app);
}

function iconFallback(app) {
  return node("span", "icon-fallback", (app.name || "?").slice(0, 1).toUpperCase());
}

function appCard(app) {
  const card = node("article", "app-card");
  card.tabIndex = 0;
  card.setAttribute("role", "button");
  const variant = recommendedVariant(app);
  card.append(iconFor(app), node("h3", "", app.name), node("p", "", app.summary || "No description available."));
  const meta = node("div", "card-meta");
  meta.append(node("span", "source-pill", sourceLabel(variant?.source)));
  if (app.installed) meta.append(node("span", "installed-pill", "Installed"));
  card.append(meta);
  const open = () => showDetails(app.id);
  card.addEventListener("click", open);
  card.addEventListener("keydown", event => {
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      open();
    }
  });
  return card;
}

function renderCollections(collections) {
  const fragment = document.createDocumentFragment();
  for (const collection of collections) {
    const section = node("section", "collection");
    const heading = node("div", "collection-heading");
    heading.append(node("h2", "", collection.title), node("span", "", collection.subtitle));
    const grid = node("div", "card-grid");
    for (const app of collection.apps || []) grid.append(appCard(app));
    section.append(heading, grid);
    fragment.append(section);
  }
  return fragment;
}

async function showHome() {
  state.view = "home";
  activateNav("home");
  content.replaceChildren(pageHeading("DISCOVER", "Software worth finding", "APT, Flathub, GitHub releases and AppImages in one trusted catalog."));
  if (state.discover.length) {
    content.append(renderCollections(state.discover));
    return;
  }
  loading("Loading curated applications…");
  try {
    const result = await rpc("catalog.discover");
    state.discover = result.collections || [];
    content.replaceChildren(pageHeading("DISCOVER", "Software worth finding", "APT, Flathub, GitHub releases and AppImages in one trusted catalog."));
    content.append(state.discover.length ? renderCollections(state.discover) : emptyState("Nothing to show yet", "The catalog is still warming up. Try Search or reload Home shortly."));
    setStatus("Discover ready");
  } catch (error) {
    content.replaceChildren(emptyState("Could not load Discover", error.message));
  }
}

async function runSearch(query) {
  const cleanQuery = query.trim();
  if (!cleanQuery) return showHome();
  state.view = "search";
  activateNav("");
  searchInput.value = cleanQuery;
  loading(`Searching for “${cleanQuery}”…`);
  try {
    const result = await rpc("catalog.search", {
      query: cleanQuery,
      sources: ["system", "flathub", "github", "appimage"],
      limit: 60,
      protocol_version: 1,
    });
    const root = document.createDocumentFragment();
    root.append(pageHeading("SEARCH", `Results for “${cleanQuery}”`, `${result.results?.length || 0} applications found across the available sources.`));
    const providers = node("div", "provider-row");
    for (const provider of result.providers || []) {
      providers.append(node("span", `provider ${provider.state || ""}`, `${sourceLabel(provider.source)} · ${provider.state}`));
    }
    root.append(providers);
    if (result.results?.length) {
      const grid = node("div", "card-grid");
      result.results.forEach(app => grid.append(appCard(app)));
      root.append(grid);
    } else {
      root.append(emptyState("No matching applications", "Try a broader name or check whether the source is online."));
    }
    content.replaceChildren(root);
    setStatus(`${result.results?.length || 0} search results`);
  } catch (error) {
    content.replaceChildren(emptyState("Search failed", error.message));
  }
}

async function showDetails(id) {
  detailsContent.replaceChildren(node("section", "loading-state", "Loading application details…"));
  detailsDialog.showModal();
  try {
    const app = await rpc("catalog.appDetails", { id });
    if (!app) throw new Error("Application details are unavailable");
    const hero = node("section", "details-hero");
    const titleRow = node("div", "details-title-row");
    const titleCopy = node("div");
    const title = node("h1", "", app.name);
    title.id = "details-title";
    titleCopy.append(node("span", "eyebrow", app.developer || "APPLICATION"), title, node("p", "subtle", app.summary));
    titleRow.append(iconFor(app), titleCopy);
    hero.append(titleRow);

    const body = node("section", "details-body");
    if (app.screenshots?.length) {
      const shots = node("div", "screenshots");
      app.screenshots.slice(0, 5).forEach(source => {
        const image = node("img");
        image.src = source;
        image.alt = `${app.name} screenshot`;
        image.loading = "lazy";
        shots.append(image);
      });
      body.append(shots);
    }
    body.append(node("p", "details-description", app.description || app.summary || "No description available."));
    body.append(node("h2", "", "Available sources"));
    const variants = node("div", "variant-list");
    for (const variant of app.variants || []) {
      const row = node("div", "variant");
      const copy = node("div");
      copy.append(
        node("strong", "", `${sourceLabel(variant.source)}${variant.id === app.recommended_variant_id ? " · Recommended" : ""}`),
        node("div", "subtle", `${variant.version || "Latest"} · ${String(variant.trust || "unverified").replaceAll("_", " ")}`),
        node("code", "", variant.command_preview || "Command prepared by UNI"),
      );
      const action = app.installed ? "reinstall" : "install";
      row.append(copy, button(app.installed ? "Reinstall" : "Install", "primary", () => requestVariantOperation(app, variant, action)));
      variants.append(row);
    }
    if (!app.variants?.length) variants.append(node("p", "subtle", "No installable source is currently available."));
    body.append(variants);
    detailsContent.replaceChildren(hero, body);
  } catch (error) {
    detailsContent.replaceChildren(emptyState("Could not load details", error.message));
  }
}

function confirmOperation(title, description, command) {
  document.getElementById("confirm-title").textContent = title;
  document.getElementById("confirm-description").textContent = description;
  document.getElementById("confirm-command").textContent = command || "Operation handled by UNI";
  confirmDialog.showModal();
  return new Promise(resolve => {
    confirmDialog.addEventListener("close", () => resolve(confirmDialog.returnValue === "default"), { once: true });
  });
}

async function enqueue(params, label, command) {
  const accepted = await confirmOperation(`${params.action[0].toUpperCase()}${params.action.slice(1)} ${label}`, `This will ${params.action} ${label} using ${sourceLabel(params.source)}.`, command);
  if (!accepted) return;
  try {
    const operation = await rpc("operations.enqueue", params);
    notify(operation.message || `${label} queued`);
    detailsDialog.close();
    await refreshOperations();
    showActivity();
  } catch (error) {
    notify(error.message, true);
  }
}

function requestVariantOperation(app, variant, action) {
  return enqueue({
    app_id: app.id,
    variant_id: variant.id,
    action,
    app_name: app.name,
    package_id: variant.package_id,
    source: variant.source,
  }, app.name, variant.command_preview);
}

function directPackage(item) {
  let source = normalizedSource(item.source);
  let packageId = item.detail || item.id;
  if (item.id.startsWith("uni:")) packageId = item.id.split(":").slice(2).join(":");
  else if (item.id.includes(":")) packageId = item.id.split(":").slice(1).join(":");
  const commandName = source === "system" ? "apt" : source === "flathub" ? "flatpak" : source;
  return { source, packageId, commandName };
}

async function requestDirectOperation(item, action) {
  const direct = directPackage(item);
  const verb = action === "remove" ? "remove" : "update";
  return enqueue({
    app_id: `direct:${direct.source}:${direct.packageId}`,
    variant_id: `${direct.source}:${direct.packageId}`,
    action,
    app_name: item.name,
    package_id: direct.packageId,
    source: direct.source,
  }, item.name, `uni ${verb} ${direct.packageId} --backend ${direct.commandName}`);
}

async function showInstalled() {
  state.view = "installed";
  activateNav("installed");
  loading("Reading installed applications…");
  try {
    const result = await rpc("installed.list");
    const items = result.items || [];
    const root = document.createDocumentFragment();
    root.append(pageHeading("YOUR SYSTEM", "Installed applications", `${items.length} applications found in UNI and Flatpak.`));
    const stack = node("div", "stack");
    for (const item of items) {
      const card = node("article", "list-card");
      const copy = node("div");
      copy.append(node("h3", "", item.name), node("p", "subtle", `${sourceLabel(item.source)} · ${item.version || "version unavailable"} · ${item.detail || "managed package"}`));
      const actions = node("div", "list-actions");
      actions.append(button("Remove", "danger", () => requestDirectOperation(item, "remove")));
      card.append(copy, actions);
      stack.append(card);
    }
    root.append(items.length ? stack : emptyState("No installed applications found", "Applications managed by UNI or Flatpak will appear here."));
    content.replaceChildren(root);
    setStatus(`${items.length} installed applications`);
  } catch (error) {
    content.replaceChildren(emptyState("Could not read installed apps", error.message));
  }
}

async function showUpdates() {
  state.view = "updates";
  activateNav("updates");
  loading("Checking for updates…");
  try {
    const result = await rpc("updates.list");
    const items = result.items || [];
    const root = document.createDocumentFragment();
    root.append(pageHeading("MAINTENANCE", "Available updates", `${items.length} updates reported by APT, Flatpak and UNI.`));
    const stack = node("div", "stack");
    for (const item of items) {
      const card = node("article", "list-card");
      const copy = node("div");
      copy.append(node("h3", "", item.name), node("p", "subtle", `${sourceLabel(item.source)} · ${item.currentVersion || "current version unknown"} → ${item.availableVersion || "latest"}`));
      card.append(copy, button("Update", "primary", () => requestDirectOperation(item, "update")));
      stack.append(card);
    }
    root.append(items.length ? stack : emptyState("Everything is current", "No available updates were reported."));
    content.replaceChildren(root);
    setStatus(`${items.length} available updates`);
  } catch (error) {
    content.replaceChildren(emptyState("Could not check updates", error.message));
  }
}

const activeStates = new Set(["pending", "resolving", "downloading", "awaiting_authentication", "installing", "finalizing"]);

async function refreshOperations() {
  try {
    const result = await rpc("operations.list");
    state.operations = result.items || [];
    const active = state.operations.filter(operation => activeStates.has(operation.state)).length;
    const badge = document.getElementById("activity-badge");
    badge.textContent = String(active);
    badge.classList.toggle("hidden", active === 0);
    if (state.view === "activity") renderActivity();
  } catch (error) {
    console.warn(error.message);
  }
}

function renderActivity() {
  const root = document.createDocumentFragment();
  root.append(pageHeading("OPERATIONS", "Activity", "Live progress and recent package operations."));
  const stack = node("div", "stack");
  for (const operation of state.operations) {
    const card = node("article", "operation-card");
    const head = node("div", "operation-head");
    head.append(node("h3", "", operation.app_name), node("span", "operation-state", operation.state.replaceAll("_", " ")));
    const progress = node("div", "progress");
    const fill = node("span");
    fill.style.width = `${Math.max(0, Math.min(100, operation.percent || 0))}%`;
    progress.append(fill);
    const foot = node("div", "operation-foot");
    foot.append(node("span", "", operation.message || operation.action), node("span", "", `${operation.percent || 0}%`));
    if (activeStates.has(operation.state)) foot.append(button("Cancel", "danger", () => cancelOperation(operation.id)));
    card.append(head, progress, foot);
    stack.append(card);
  }
  root.append(state.operations.length ? stack : emptyState("No activity yet", "Installs, removals and updates will appear here."));
  content.replaceChildren(root);
  setStatus(`${state.operations.length} recorded operations`);
}

async function showActivity() {
  state.view = "activity";
  activateNav("activity");
  if (!state.operations.length) await refreshOperations();
  renderActivity();
}

async function cancelOperation(id) {
  try {
    const result = await rpc("operations.cancel", { operationId: id });
    notify(result.accepted ? "Cancellation requested" : result.message);
    await refreshOperations();
  } catch (error) {
    notify(error.message, true);
  }
}

async function showSettings() {
  state.view = "settings";
  activateNav("settings");
  loading("Reading store status…");
  try {
    const [health, cache] = await Promise.all([rpc("system.health"), rpc("system.cacheInfo")]);
    state.health = health;
    const root = document.createDocumentFragment();
    root.append(pageHeading("SYSTEM", "Store settings", "Runtime health, cache controls and compatibility information."));
    const grid = node("div", "settings-grid");
    const backend = node("article", "setting-card");
    backend.append(node("span", "eyebrow", "BACKEND"), node("strong", "", `v${health.version}`), node("p", "", `${health.status} · ${health.mode}\nAPT: ${health.apt}\nFlatpak: ${health.flatpak}\nPrivilege: ${health.privilege}`));
    const cacheCard = node("article", "setting-card");
    cacheCard.append(node("span", "eyebrow", "ICON CACHE"), node("strong", "", formatBytes(cache.iconBytes)), node("p", "", `${cache.iconCount} cached files`), button("Clear icon cache", "secondary", clearCache));
    const compatibility = node("article", "setting-card");
    compatibility.append(node("span", "eyebrow", "COMPATIBILITY"), node("strong", "", "Browser frontend"), node("p", "", "Runs as a normal local web application on GNOME, KDE, Xfce, Cinnamon and other Debian desktops. Quickshell and Hyprland are not required."));
    grid.append(backend, cacheCard, compatibility);
    root.append(grid);
    content.replaceChildren(root);
    setStatus("Store status ready");
  } catch (error) {
    content.replaceChildren(emptyState("Could not load settings", error.message));
  }
}

async function clearCache() {
  try {
    const result = await rpc("system.clearIconCache");
    notify(`Icon cache cleared (${result.iconCount} files remain)`);
    showSettings();
  } catch (error) {
    notify(error.message, true);
  }
}

function formatBytes(value) {
  const bytes = Number(value || 0);
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 ** 2) return `${(bytes / 1024).toFixed(1)} KiB`;
  return `${(bytes / 1024 ** 2).toFixed(1)} MiB`;
}

function activateNav(view) {
  document.querySelectorAll("[data-view]").forEach(item => item.classList.toggle("active", item.dataset.view === view));
}

function navigate(view) {
  ({
    home: showHome,
    installed: showInstalled,
    updates: showUpdates,
    activity: showActivity,
    settings: showSettings,
  })[view]?.();
}

document.querySelectorAll("[data-view]").forEach(item => item.addEventListener("click", () => navigate(item.dataset.view)));
searchForm.addEventListener("submit", event => {
  event.preventDefault();
  runSearch(searchInput.value);
});
document.getElementById("details-close").addEventListener("click", () => detailsDialog.close());
detailsDialog.addEventListener("click", event => {
  if (event.target === detailsDialog) detailsDialog.close();
});

async function start() {
  try {
    state.health = await rpc("system.health");
    document.getElementById("simulation-warning").classList.toggle("hidden", !state.health.fakeUni);
    setStatus(`Backend ${state.health.version} ready`);
    await Promise.all([showHome(), refreshOperations()]);
    setTimeout(async () => {
      if (state.view !== "home") return;
      try {
        const result = await rpc("catalog.discover");
        state.discover = result.collections || [];
        showHome();
      } catch (_) {
        // The first successful Discover response remains usable.
      }
    }, 8500);
  } catch (error) {
    content.replaceChildren(emptyState("The store could not start", error.message));
    notify(error.message, true);
  }
}

setInterval(refreshOperations, 1800);
start();
