import Quickshell
import Quickshell.Io
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ShellRoot {
    id: root

    property string backendBinary: Quickshell.env("THALLIUM_STORE_BACKEND") || "thallium-store-backend"
    property string query: ""
    property var results: []
    property var providers: []
    property var discover: []
    property var selectedApp: null
    property var operations: []
    property var installedItems: []
    property var updateItems: []
    property var requestQueue: []
    property bool requestRunning: false
    property string activeMethod: ""
    property string status: "Starting backend"
    property string activeView: "discover"
    // Updates start capped at two rows on the Apps page; the header toggles it.
    property bool updatesExpanded: false
    property bool fakeUniMode: true
    property string uniHealth: ""
    property string storeVersion: ""
    // Screenshot opened over the page; empty means the viewer is closed.
    property string viewerSource: ""
    property int activityFrame: 0
    property var searchCache: ({})
    property string activeSearchQuery: ""
    readonly property int searchCacheTtlMs: 60000
    property bool searchCommitted: false
    property var storeLog: []
    property bool installOpen: false
    property var installApp: null
    property string installVariantId: ""
    property var storeSettings: ({})
    property var cacheInfo: ({ iconBytes: 0, iconCount: 0 })
    property bool setupOpen: false
    property int setupStep: 0

    function pushLog(msg) {
        const stamp = Qt.formatDateTime(new Date(), "HH:mm:ss")
        const next = [stamp + "  " + msg].concat(root.storeLog)
        root.storeLog = next.slice(0, 120)
    }

    // Thallium 81 design tokens — Everforest palette, retro sci-fi HUD grown
    // out of Soviet brutalism. Green is THE accent; blue is retired.
    readonly property color cBase: "#060607"      // void: page, deepest bg
    readonly property color cPanel: "#15191b"     // raised surface: cards
    readonly property color cDim: "#1e2326"       // inputs, insets, controls
    readonly property color cLine: "#3a464c"      // hairlines, borders, dim labels
    readonly property color cDead: "#232a2e"      // dead / inactive segments
    readonly property color cGreen: "#a7c080"     // THE accent: active/focus/values
    readonly property color cGreenSoft: "#83c092" // secondary green (aqua)
    readonly property color cFg: "#d3c6aa"        // body text (Everforest cream)
    readonly property color cMuted: "#8f9a91"     // secondary text
    readonly property color cRed: "#e67e80"       // warnings only
    readonly property color cWarn: "#dbbc7f"      // caution (Everforest yellow)
    readonly property color cBlue: "#a7c080"      // legacy alias -> green accent
    readonly property color cBlueSoft: "#2b3a2e"  // dim green wash (selected/featured)
    readonly property color cPurple: "#d699b6"    // Everforest purple (source hue)
    readonly property string fontBrand: "Unbounded"
    readonly property string fontHuman: "Albert Sans"
    readonly property string fontMono: "JetBrains Mono"

    // Geometry + motion vocabulary
    readonly property int chamfer: 14             // top-right corner cut, px
    readonly property real shear: -0.42           // parallelogram shear for rails/bars
    readonly property int tickW: 2                // green identity tick width
    readonly property real letterSpace: 3.0       // default mono letter-spacing
    readonly property int tFast: 120
    readonly property int tMed: 220
    readonly property int tSlow: 360

    function uiScale() {
        const w = window && window.width ? window.width : 1280
        return Math.max(0.86, Math.min(1.14, w / 1280))
    }

    function pagePad() {
        return Math.round(28 * uiScale())
    }

    // Centered storefront column: slim 4% gutters — content owns the width.
    function contentSideMargin() {
        const w = window && window.width ? window.width : 1280
        return Math.max(pagePad(), Math.round(w * 0.04))
    }

    function actionColumnWidth() {
        const w = window && window.width ? window.width : 1280
        return Math.max(154, Math.min(230, Math.round(w * 0.18)))
    }

    function activeOpsCount() {
        let n = 0
        for (let i = 0; i < operations.length; i++) {
            const s = operations[i].state
            if (s !== "succeeded" && s !== "failed" && s !== "cancelled")
                n++
        }
        return n
    }

    // Operations still in flight. Terminal ones are deliberately not shown:
    // once an install has finished, its row is a log entry, and the Apps page
    // is not a log -- the installed grid below already reflects the outcome.
    function runningOperations() {
        const live = []
        for (let i = 0; i < operations.length; i++) {
            const s = operations[i].state
            if (s !== "succeeded" && s !== "failed" && s !== "cancelled")
                live.push(operations[i])
        }
        return live
    }

    // XDG icon for something already on the machine. Flatpak and apt both name
    // their icon after the package id far more often than after the app's
    // display name, so that is tried first; iconPath's second argument makes a
    // miss return "" instead of a broken-image placeholder.
    function installedIconSource(item) {
        const pkg = packageIdFromInstalled(item)
        const candidates = [pkg, (item.name || "").toLowerCase(), item.name || ""]
        for (let i = 0; i < candidates.length; i++) {
            if (!candidates[i])
                continue
            const path = Quickshell.iconPath(candidates[i], true)
            if (path)
                return path
        }
        return ""
    }

    function cardHeight() {
        return Math.round(126 * uiScale())
    }

    function isBusy() {
        if (requestRunning || requestQueue.length > 0)
            return true
        for (let i = 0; i < operations.length; i++) {
            const state = operations[i].state
            if (state !== "succeeded" && state !== "failed" && state !== "cancelled")
                return true
        }
        return false
    }

    function activityLabel() {
        if (requestRunning) {
            if (activeMethod === "catalog.search")
                return "Searching catalog"
            if (activeMethod === "catalog.appDetails")
                return "Loading details"
            if (activeMethod === "operations.enqueue")
                return "Queueing operation"
            if (activeMethod === "operations.list")
                return "Refreshing queue"
            if (activeMethod === "installed.list")
                return "Reading installed apps"
            if (activeMethod === "updates.list")
                return "Checking updates"
            if (activeMethod === "system.health")
                return "Checking UNI"
            return "Working"
        }
        for (let i = 0; i < operations.length; i++) {
            const state = operations[i].state
            if (state !== "succeeded" && state !== "failed" && state !== "cancelled")
                return operations[i].app_name + " · " + state
        }
        return "Idle"
    }

    function activityGlyph() {
        if (!isBusy())
            return "●"
        const frames = ["◐", "◓", "◑", "◒"]
        return frames[activityFrame % frames.length]
    }

    function request(method, params) {
        const nextQueue = requestQueue.slice()
        nextQueue.push({ method: method, params: params || {} })
        requestQueue = nextQueue
        drainRequests()
    }

    function drainRequests() {
        if (requestRunning || requestQueue.length === 0)
            return

        const item = requestQueue[0]
        requestQueue = requestQueue.slice(1)
        requestRunning = true
        activeMethod = item.method
        if (item.method === "catalog.search")
            activeSearchQuery = item.params.query
        rpc.command = [backendBinary, "--request", item.method, JSON.stringify(item.params)]
        rpc.running = true
    }

    function searchNow() {
        // Never hijack the view — a details page opened just before the
        // debounce fired must stay open. Typing already switches the view.
        if (activeView !== "search")
            return
        const q = query
        const cached = searchCache[q]
        if (cached && (Date.now() - cached.ts) < searchCacheTtlMs) {
            results = cached.results
            providers = cached.providers
            status = results.length + " result" + (results.length === 1 ? "" : "s") + " · cached"
            return
        }
        status = "Searching"
        pushLog("search » " + q)
        // GitHub is the slowest source (network HTTP); skip it while the query
        // is still 1-2 chars so early typing paints from the fast local sources.
        const sources = q.trim().length < 3
            ? ["system", "flathub", "appimage"]
            : ["system", "flathub", "github", "appimage"]
        request("catalog.search", {
            protocol_version: 1,
            query: q,
            sources: sources,
            limit: 40
        })
    }

    function selectApp(app) {
        searchDebounce.stop()
        // Drop the caret out of the search box: while it holds focus, any event
        // that returns focus to the window drags the view back to search.
        searchField.focus = false
        selectedApp = app
        activeView = "details"
        if (app && app.id)
            request("catalog.appDetails", { id: app.id })
    }

    function loadDiscover() {
        request("catalog.discover", {})
    }

    // Rotating featured pool — first two apps of each collection, cycled.
    function featuredPool() {
        let pool = []
        for (let i = 0; i < root.discover.length; i++) {
            const apps = root.discover[i].apps || []
            for (let j = 0; j < Math.min(2, apps.length); j++)
                pool.push(apps[j])
        }
        return pool
    }


    function recommendedVariant(app) {
        if (!app || !app.variants)
            return null
        for (let i = 0; i < app.variants.length; i++) {
            if (app.variants[i].id === app.recommended_variant_id)
                return app.variants[i]
        }
        return app.variants.length > 0 ? app.variants[0] : null
    }

    function enqueueVariant(app, variant, action) {
        if (!app || !variant)
            return
        request("operations.enqueue", {
            app_id: app.id,
            variant_id: variant.id,
            action: action || "install"
        })
        activeView = "installed"
    }

    function enqueueInstall(app) {
        enqueueVariant(app, recommendedVariant(app))
    }

    function operationSource(source) {
        if (source === "apt" || source === "dpkg" || source === "system")
            return "system"
        if (source === "flatpak" || source === "flathub")
            return "flathub"
        if (source === "github")
            return "github"
        if (source === "appimage")
            return "appimage"
        return "system"
    }

    function packageIdFromInstalled(item) {
        if (!item)
            return ""
        if (item.source === "appimage")
            return item.name
        if (item.id && item.id.indexOf("uni:") === 0) {
            const parts = item.id.split(":")
            if (parts.length >= 3)
                return parts.slice(2).join(":")
        }
        if (item.id && item.id.indexOf(":") > 0)
            return item.id.split(":").slice(1).join(":")
        return item.detail || item.name
    }

    function appFromInstalled(item) {
        const source = operationSource(item.source)
        const packageId = packageIdFromInstalled(item)
        const sourceArg = source === "system" ? "apt" : source === "flathub" ? "flatpak" : source
        return {
            id: "installed:" + source + ":" + packageId,
            name: item.name,
            summary: (item.version ? item.version + " · " : "") + item.detail,
            description: "Installed application detected from " + root.sourceLabel(source) + ".",
            developer: null,
            homepage: null,
            license: null,
            icon: null,
            screenshots: [],
            installed: true,
            merge_confidence: 1.0,
            merge_evidence: ["installed state"],
            recommended_variant_id: sourceArg + ":" + packageId,
            variants: [{
                id: sourceArg + ":" + packageId,
                source: source,
                package_id: packageId,
                version: item.version || null,
                trust: source === "flathub" ? "sandboxed" : source === "system" ? "system_access" : "unverified",
                verified: item.managedByUni === true,
                command_preview: "uni remove " + packageId + " --source " + sourceArg,
                ranking_reasons: ["installed source"]
            }]
        }
    }

    function enqueueUninstallInstalled(item) {
        const source = operationSource(item.source)
        const packageId = packageIdFromInstalled(item)
        const sourceArg = source === "system" ? "apt" : source === "flathub" ? "flatpak" : source
        request("operations.enqueue", {
            app_id: "installed:" + source + ":" + packageId,
            variant_id: sourceArg + ":" + packageId,
            action: "remove",
            app_name: item.name,
            package_id: packageId,
            source: source
        })
        activeView = "installed"
    }

    function enqueueUninstall(app) {
        const variant = recommendedVariant(app)
        if (!app || !variant)
            return
        request("operations.enqueue", {
            app_id: app.id,
            variant_id: variant.id,
            action: "remove",
            app_name: app.name,
            package_id: variant.package_id,
            source: variant.source
        })
        activeView = "installed"
    }

    function packageIdFromOperation(operation) {
        if (!operation)
            return ""
        if (operation.variant_id && operation.variant_id.indexOf(":") > 0)
            return operation.variant_id.split(":").slice(1).join(":")
        return operation.app_id || operation.app_name
    }

    function appFromOperation(operation) {
        const source = operationSource(operation.source)
        const packageId = packageIdFromOperation(operation)
        const sourceArg = source === "system" ? "apt" : source === "flathub" ? "flatpak" : source
        return {
            id: operation.app_id || "operation:" + source + ":" + packageId,
            name: operation.app_name,
            summary: root.sourceLabel(source) + " · " + operation.action + " · " + operation.state,
            description: operation.message || "Operation history from the queue.",
            developer: null,
            homepage: null,
            license: null,
            icon: null,
            screenshots: [],
            installed: operation.action === "install" && operation.state === "succeeded",
            merge_confidence: 1.0,
            merge_evidence: ["queue operation"],
            recommended_variant_id: operation.variant_id || sourceArg + ":" + packageId,
            variants: [{
                id: operation.variant_id || sourceArg + ":" + packageId,
                source: source,
                package_id: packageId,
                version: null,
                trust: source === "flathub" ? "sandboxed" : source === "system" ? "system_access" : "unverified",
                verified: false,
                command_preview: "uni " + operation.action + " " + packageId + " --source " + sourceArg,
                ranking_reasons: ["queue operation"]
            }]
        }
    }

    function retryOperation(operation) {
        if (!operation)
            return
        request("operations.enqueue", {
            app_id: operation.app_id,
            variant_id: operation.variant_id,
            action: operation.action,
            app_name: operation.app_name,
            package_id: packageIdFromOperation(operation),
            source: operationSource(operation.source)
        })
        activeView = "installed"
    }

    function cancelOperation(operation) {
        if (!operation)
            return
        request("operations.cancel", {
            operationId: operation.id
        })
        activeView = "installed"
    }

    function operationActionLabel(operation) {
        if (!operation)
            return "Details"
        if (operation.state === "failed")
            return "Retry"
        if (operation.state === "cancelled")
            return "Cancelled"
        if (operation.action === "remove" && operation.state === "succeeded")
            return "Removed"
        if (operation.action === "install" && operation.state === "succeeded")
            return "Installed"
        if (operation.action === "update" && operation.state === "succeeded")
            return "Updated"
        return "Cancel"
    }

    function operationProblem(operation) {
        if (!operation)
            return ""
        if (operation.state === "failed")
            return operation.message && operation.message.length > 0 ? "Problem: " + operation.message : "Problem: install failed without a detailed backend message."
        if (operation.state === "cancelled")
            return "Cancelled: operation was stopped before it finished."
        if (operation.state === "succeeded")
            return operation.action === "remove" ? "Completed: app was removed." : "Completed: app is installed."
        return operation.message || "Working"
    }

    function sourceLabel(source) {
        if (source === "system" || source === "apt" || source === "dpkg")
            return "System"
        if (source === "flathub" || source === "flatpak")
            return "Flathub"
        if (source === "github")
            return "GitHub"
        if (source === "appimage")
            return "AppImage"
        return source || "Unknown"
    }

    function formatLabel(source) {
        if (source === "system" || source === "apt" || source === "dpkg")
            return ".deb (dpkg)"
        if (source === "flathub" || source === "flatpak")
            return "flatpak"
        if (source === "github")
            return "release binary"
        if (source === "appimage")
            return "AppImage"
        return "package"
    }

    function finishSetup() {
        setupOpen = false
        request("settings.set", { first_run_done: true })
    }

    function openInstall(app) {
        if (!app)
            return
        const rec = recommendedVariant(app)
        root.installApp = app
        root.installVariantId = rec ? rec.id : (app.variants.length > 0 ? app.variants[0].id : "")
        root.installOpen = true
    }

    function confirmInstall() {
        if (!root.installApp)
            return
        let chosen = null
        for (let i = 0; i < root.installApp.variants.length; i++) {
            if (root.installApp.variants[i].id === root.installVariantId) {
                chosen = root.installApp.variants[i]
                break
            }
        }
        if (!chosen)
            chosen = recommendedVariant(root.installApp)
        root.installOpen = false
        root.pushLog("install " + root.installApp.name + " via " + root.sourceLabel(chosen.source))
        root.enqueueVariant(root.installApp, chosen)
    }

    function platformLabel(source) {
        if (source === "system" || source === "apt" || source === "dpkg")
            return "Debian / Thallium repositories"
        if (source === "flathub" || source === "flatpak")
            return "Flatpak sandbox"
        if (source === "github")
            return "Curated GitHub release"
        if (source === "appimage")
            return "Portable AppImage"
        return "Package source"
    }

    function trustLabel(trust) {
        if (trust === "sandboxed")
            return "Sandboxed"
        if (trust === "system_access")
            return "System access"
        if (trust === "verified")
            return "Verified"
        if (trust === "unverified")
            return "Unverified"
        return "Unknown trust"
    }

    function trustColor(trust) {
        if (trust === "unverified")
            return cMuted
        if (trust === "system_access")
            return cWarn
        return cGreen
    }

    function commandPreview(variant) {
        return variant && variant.command_preview ? variant.command_preview : "UNI command unavailable"
    }

    // Language segments cycle through the Everforest accents.
    function langColor(i) {
        const palette = [cGreen, cGreenSoft, cWarn, cPurple, cRed, cFg]
        return palette[i % palette.length]
    }

    function formatBytes(bytes) {
        if (!bytes || bytes <= 0)
            return "Not provided"
        if (bytes >= 1073741824)
            return (bytes / 1073741824).toFixed(1) + " GB"
        if (bytes >= 1048576)
            return (bytes / 1048576).toFixed(1) + " MB"
        if (bytes >= 1024)
            return (bytes / 1024).toFixed(1) + " KB"
        return bytes + " B"
    }

    function sizeLabel(app) {
        const variant = recommendedVariant(app)
        if (!variant)
            return "Not provided"
        if (variant.download_size && variant.download_size > 0)
            return formatBytes(variant.download_size)
        if (variant.installed_size && variant.installed_size > 0)
            return formatBytes(variant.installed_size)
        return "Not provided"
    }

    function compactDeveloper(app) {
        if (!app)
            return "Unknown"
        if (app.developer && app.developer.length > 0)
            return app.developer
        const variant = recommendedVariant(app)
        return variant ? sourceLabel(variant.source) : "Unknown"
    }

    function sourceAccent(source) {
        if (source === "system" || source === "apt" || source === "dpkg")
            return cWarn         // native system access — caution (Everforest yellow)
        if (source === "flathub" || source === "flatpak")
            return cGreen        // sandboxed default — THE trusted green
        if (source === "github")
            return cPurple       // curated release (Everforest purple)
        if (source === "appimage")
            return cGreenSoft    // portable (Everforest aqua)
        return cGreen
    }

    function sourceSurface(source) {
        if (source === "system" || source === "apt" || source === "dpkg")
            return "#33301f"
        if (source === "flathub" || source === "flatpak")
            return "#2b3a2e"
        if (source === "github")
            return "#352a33"
        if (source === "appimage")
            return "#243530"
        return cBlueSoft
    }

    function sourceShort(source) {
        if (source === "system" || source === "apt" || source === "dpkg")
            return "APT"
        if (source === "flathub" || source === "flatpak")
            return "FLAT"
        if (source === "github")
            return "GH"
        if (source === "appimage")
            return "AIMG"
        return "PKG"
    }

    function appAccent(app) {
        const variant = recommendedVariant(app)
        return variant ? sourceAccent(variant.source) : cBlue
    }

    function appSurface(app) {
        const variant = recommendedVariant(app)
        return app && app.installed ? "#233024" : variant ? sourceSurface(variant.source) : cBlueSoft
    }

    function joinTags(tags) {
        return tags && tags.length > 0 ? tags.join(", ") : "Not provided"
    }

    Process {
        id: backend
        command: [root.backendBinary]
        running: true
    }

    Process {
        id: rpc
        running: false
        onRunningChanged: {
            if (!running && root.requestRunning) {
                root.requestRunning = false
                root.drainRequests()
            }
        }
        stdout: SplitParser {
            onRead: line => {
                try {
                    const message = JSON.parse(line)
                    if (message.error) {
                        root.status = message.error.message
                        pushLog("✗ " + message.error.message)
                        return
                    }
                    const result = message.result
                    if (!result)
                        return
                    if (result.results !== undefined) {
                        root.results = result.results
                        root.providers = result.providers || []
                        root.status = root.results.length + " result" + (root.results.length === 1 ? "" : "s")
                        pushLog("results « " + root.results.length + " for “" + root.activeSearchQuery + "”")
                        const nextCache = root.searchCache
                        nextCache[root.activeSearchQuery] = {
                            results: result.results,
                            providers: result.providers || [],
                            ts: Date.now()
                        }
                        root.searchCache = nextCache
                    } else if (result.collections !== undefined) {
                        root.discover = result.collections
                        root.status = "Discover ready"
                        pushLog("discover feed ready")
                    } else if (result.items !== undefined && root.activeMethod === "operations.list") {
                        const prevActive = root.activeOpsCount()
                        root.operations = result.items
                        // An operation just finished — installed state changed,
                        // refresh the Apps grid and drop stale search results.
                        if (root.activeOpsCount() < prevActive) {
                            root.searchCache = ({})
                            root.request("installed.list", {})
                        }
                        root.status = "Queue loaded"
                    } else if (result.items !== undefined && root.activeMethod === "installed.list") {
                        root.installedItems = result.items
                        root.status = root.installedItems.length + " installed item" + (root.installedItems.length === 1 ? "" : "s")
                        pushLog("installed « " + root.installedItems.length + " apps")
                    } else if (result.items !== undefined && root.activeMethod === "updates.list") {
                        root.updateItems = result.items
                        root.status = root.updateItems.length + " update" + (root.updateItems.length === 1 ? "" : "s")
                        pushLog("updates « " + root.updateItems.length)
                    } else if (result.id !== undefined && result.variants !== undefined && root.activeMethod === "catalog.appDetails") {
                        root.selectedApp = result
                        root.status = "Details loaded"
                        pushLog("opened " + (result.name || result.id))
                    } else if (result.id && result.state !== undefined) {
                        root.status = result.message || "Queued"
                        pushLog("queue » " + (result.message || result.id))
                        // Install/remove changes installed-state; drop cached
                        // search results so the next search reflects reality.
                        root.searchCache = ({})
                        root.request("operations.list", {})
                    } else if (result.accepted !== undefined && result.operation !== undefined) {
                        root.status = result.message || "Operation updated"
                        root.request("operations.list", {})
                    } else if (result.settings !== undefined) {
                        root.storeSettings = result.settings
                        if (root.activeMethod === "settings.get" && result.settings.first_run_done !== true) {
                            root.setupStep = 0
                            root.setupOpen = true
                        }
                    } else if (result.iconBytes !== undefined) {
                        root.cacheInfo = result
                    } else if (result.status !== undefined) {
                        root.fakeUniMode = result.fakeUni === true
                        root.uniHealth = result.uni || ""
                        root.storeVersion = result.version || ""
                        root.status = result.status + " - " + result.uni
                    }
                } catch (err) {
                    root.status = "Invalid backend response"
                }
            }
        }
        stderr: SplitParser {
            onRead: line => {
                if (line.length > 0) {
                    root.status = line
                    pushLog("· " + line)
                }
            }
        }
    }

    Timer {
        id: searchDebounce
        interval: 250
        repeat: false
        onTriggered: root.searchNow()
    }

    Timer {
        interval: 1400
        repeat: true
        running: true
        onTriggered: {
            if (root.activeView === "installed")
                root.request("operations.list", {})
        }
    }

    Timer {
        interval: 120
        repeat: true
        running: root.isBusy()
        onTriggered: root.activityFrame = root.activityFrame + 1
    }

    // Re-fetch Discover once the daemon's Flathub index has warmed (~7s), so
    // curated cards show real names/summaries instead of prettified ids.
    Timer {
        id: discoverWarmReload
        interval: 8500
        repeat: false
        onTriggered: root.loadDiscover()
    }

    // Auto-advance the featured carousel so it slides between picks.
    Timer {
        interval: 5000
        repeat: true
        running: root.activeView === "discover" && root.discover.length > 0
        onTriggered: {
            const count = root.featuredPool().length
            if (count > 0)
                featuredCarousel.currentIndex = (featuredCarousel.currentIndex + 1) % count
        }
    }

    Timer {
        id: initialLoadDelay
        interval: 650
        repeat: false
        onTriggered: {
            root.request("system.health", {})
            root.request("settings.get", {})
            root.loadDiscover()
            // Installed apps and updates are fetched up front, not only when the
            // Apps tab is clicked. They share one page now, and a page that is
            // empty until you click the tab that is already selected reads as
            // "nothing installed".
            root.request("installed.list", {})
            root.request("updates.list", {})
            discoverWarmReload.start()
        }
    }

    Component.onCompleted: {
        initialLoadDelay.start()
    }

    // Top-bar navigation tab: mono uppercase label, green underline when
    // active, optional live badge (queue count).
    component TopTab: Item {
        id: tab
        property string label: ""
        property string view: ""
        property int badge: 0
        readonly property bool selected: root.activeView === view
            || (view === "search" && root.activeView === "details")

        width: tabRow.implicitWidth
        height: 64

        HoverHandler { id: tabHover }

        Row {
            id: tabRow
            anchors.centerIn: parent
            spacing: 8
            Label {
                text: tab.label.toUpperCase()
                color: tab.selected || tabHover.hovered ? root.cFg : root.cMuted
                font.family: root.fontMono
                font.pixelSize: 12
                font.bold: tab.selected
                font.letterSpacing: 2
                anchors.verticalCenter: parent.verticalCenter
                Behavior on color { ColorAnimation { duration: root.tFast } }
            }
            Rectangle {
                visible: tab.badge > 0
                width: badgeText.implicitWidth + 10
                height: 16
                color: root.cGreen
                anchors.verticalCenter: parent.verticalCenter
                Label {
                    id: badgeText
                    anchors.centerIn: parent
                    text: tab.badge
                    color: root.cBase
                    font.family: root.fontMono
                    font.pixelSize: 10
                    font.bold: true
                }
            }
        }

        Rectangle {
            anchors.bottom: parent.bottom
            anchors.horizontalCenter: parent.horizontalCenter
            width: tab.selected ? tabRow.implicitWidth : 0
            height: 2
            color: root.cGreen
            Behavior on width { NumberAnimation { duration: root.tMed; easing.type: Easing.OutCubic } }
        }

        MouseArea {
            anchors.fill: parent
            cursorShape: Qt.PointingHandCursor
            onClicked: {
                root.activeView = tab.view
                if (tab.view === "installed") {
                    root.request("installed.list", {})
                    root.request("operations.list", {})
                }
                if (tab.view === "installed")
                    root.request("updates.list", {})
                if (tab.view === "search")
                    searchField.forceActiveFocus()
                if (tab.view === "settings") {
                    root.request("system.cacheInfo", {})
                    root.request("settings.get", {})
                }
            }
        }
    }

    // Signature Thallium 81 surface: a rectangle with the top-right corner cut
    // (chamfer) and a green hairline along the cut. Children stack above it.
    component ChamferPanel: Item {
        id: panel
        property color fill: root.cPanel
        property color stroke: root.cLine
        property bool accentEdge: true
        property int cut: root.chamfer
        Canvas {
            anchors.fill: parent
            antialiasing: true
            onWidthChanged: requestPaint()
            onHeightChanged: requestPaint()
            onPaint: {
                const ctx = getContext("2d"); ctx.reset()
                const w = width, h = height, k = panel.cut
                ctx.beginPath()
                ctx.moveTo(0, 0)
                ctx.lineTo(w - k, 0)
                ctx.lineTo(w, k)
                ctx.lineTo(w, h)
                ctx.lineTo(0, h)
                ctx.closePath()
                ctx.fillStyle = panel.fill
                ctx.fill()
                ctx.strokeStyle = panel.stroke
                ctx.lineWidth = 1
                ctx.stroke()
                if (panel.accentEdge) {
                    ctx.strokeStyle = root.cGreen
                    ctx.lineWidth = 1.5
                    ctx.beginPath()
                    ctx.moveTo(w - k, 0)
                    ctx.lineTo(w, k)
                    ctx.stroke()
                }
            }
        }
    }

    // Thallium 81 brand loader — same stroke-by-stroke logo animation as the
    // OS control center (native-QML port of the thallium-81 logo artwork).
    component ThalliumLoader: Item {
        id: loader
        property bool running: true
        property color stroke: root.cGreen
        property real phase: 0
        property real lineScale: 1

        implicitWidth: 72
        implicitHeight: 72

        NumberAnimation on phase {
            from: 0; to: 1; duration: 6000
            loops: Animation.Infinite
            running: loader.running && loader.visible
        }

        Canvas {
            id: loaderCanvas
            anchors.fill: parent
            antialiasing: true

            // x1,y1,x2,y2,drawStart,drawEnd — the original 55 path stages.
            readonly property string geometry: "-85.18,49.18,0,98.36,0,.55;0,-98.36,-85.18,-49.18,0,.55;-85.18,-49.18,-85.18,49.18,0,.55;85.18,49.18,0,98.36,0,.55;85.18,-49.18,85.18,49.18,0,.55;0,-98.36,85.18,-49.18,0,.55;0,-86.08,-62.22,-50.16,.472,1.022;-74.55,43.04,-12.33,78.96,.472,1.022;74.55,-28.81,74.55,43.04,.472,1.022;0,-86.08,65.81,-48.09,.479,1.029;-74.55,-32.95,-74.55,43.04,.479,1.029;74.55,43.04,8.74,81.04,.479,1.029;-9.59,-66.71,.78,-72.70,.703,1.253;-85.18,-49.18,-32.15,-18.56,.799,1.349;0,98.36,0,37.13,.799,1.349;85.18,-49.18,32.15,-18.56,.799,1.349;35.02,-30.31,65.81,-48.09,.977,1.527;-74.55,-32.95,-43.76,-15.17,.977,1.527;8.74,45.48,8.74,81.04,.977,1.527;-63.36,-26.26,-63.36,35.69,.992,1.542;8.94,68,62.58,37.03,.992,1.542;.78,-72.70,54.41,-41.74,.992,1.542;-63.36,35.69,-12.20,65.22,1.015,1.565;62.58,-22.05,62.58,37.03,1.015,1.565;-9.59,-66.71,-50.38,-43.16,1.035,1.585;-62.22,-50.16,-29.42,-31.22,1.095,1.645;-12.33,41.08,-12.33,78.96,1.096,1.646;41.74,-9.87,74.55,-28.81,1.096,1.646;-9.59,-42.66,-9.59,-66.71,1.359,1.909;11.60,-43.83,11.60,-64.76,1.359,1.909;-43.76,11.87,-61.89,22.33,1.359,1.909;32.15,31.96,50.28,42.43,1.359,1.909;-32.15,29.63,-52.96,41.65,1.36,1.91;41.74,13.03,62.55,25.05,1.36,1.91;0,-86.08,0,-24.03,1.381,1.931;-74.55,43.04,-20.81,12.01,1.381,1.931;74.55,43.04,20.81,12.01,1.381,1.931;11.60,-43.83,35.02,-30.31,1.897,2.447;-43.76,-15.17,-43.76,11.87,1.897,2.447;32.15,31.96,8.74,45.48,1.897,2.447;-29.42,-31.22,-9.59,-42.66,1.989,2.539;-32.15,29.63,-12.33,41.08,1.989,2.539;41.74,13.03,41.74,-9.87,1.989,2.539;-32.15,-18.56,11.60,-43.83,2.398,2.948;-43.76,11.87,0,37.13,2.398,2.948;32.15,-18.56,32.15,31.96,2.398,2.948;-32.15,-18.56,-32.15,29.63,2.408,2.958;0,37.13,41.74,13.03,2.408,2.958;-9.59,-42.66,32.15,-18.56,2.408,2.958;0,24.03,20.81,12.01,2.95,3.5;0,-24.03,-20.81,-12.01,2.95,3.5;-20.81,12.01,0,24.03,2.95,3.5;0,-24.03,20.81,-12.01,2.95,3.5;-20.81,-12.01,-20.81,12.01,2.95,3.5;20.81,-12.01,20.81,12.01,2.95,3.5"

            function clamp(v) { return Math.max(0, Math.min(1, v)); }
            onPaint: {
                var c = getContext("2d"); c.reset();
                var scale = Math.min(width, height) / 225;
                var cx = width / 2, cy = height / 2;
                // Original timing: draw 2.17s, hold 1.1s, retract 2.17s, hold .55s.
                var sec = loader.running ? loader.phase * 5.99 : 2.17;
                var sourceT;
                if (sec < 2.17) sourceT = sec / .62;
                else if (sec < 3.27) sourceT = 3.5;
                else if (sec < 5.44) sourceT = (5.44 - sec) / .62;
                else sourceT = 0;

                c.strokeStyle = loader.stroke;
                c.lineWidth = Math.max(1, 2.4 * scale * loader.lineScale);
                c.lineCap = "round"; c.lineJoin = "round";
                var paths = geometry.split(";");
                for (var i = 0; i < paths.length; ++i) {
                    var p = paths[i].split(",").map(Number);
                    var progress = clamp((sourceT - p[4]) / (p[5] - p[4]));
                    // Persistent ghost trace keeps the brand mark identifiable
                    // during the original animation's deliberate empty hold.
                    c.globalAlpha = .08;
                    c.beginPath();
                    c.moveTo(cx + p[0] * scale, cy + p[1] * scale);
                    c.lineTo(cx + p[2] * scale, cy + p[3] * scale);
                    c.stroke();
                    if (progress <= 0) continue;
                    c.globalAlpha = .22 + progress * .78;
                    c.beginPath();
                    c.moveTo(cx + p[0] * scale, cy + p[1] * scale);
                    c.lineTo(cx + (p[0] + (p[2] - p[0]) * progress) * scale,
                             cy + (p[1] + (p[3] - p[1]) * progress) * scale);
                    c.stroke();
                }
                c.globalAlpha = 1;
            }

            Connections {
                target: loader
                function onPhaseChanged() { loaderCanvas.requestPaint(); }
                function onStrokeChanged() { loaderCanvas.requestPaint(); }
                function onRunningChanged() { loaderCanvas.requestPaint(); }
            }
            onWidthChanged: requestPaint()
            onHeightChanged: requestPaint()
        }
    }

    // Machine-voice section header: green tick + mono uppercase label,
    // hairline rule extending to the right edge (HUD readout feel).
    component HudRailHeader: RowLayout {
        property string title: ""
        property string sub: ""
        spacing: 10
        Layout.fillWidth: true
        Rectangle { width: root.tickW; height: 15; radius: 0; color: root.cGreen; Layout.alignment: Qt.AlignVCenter }
        Label {
            text: title.toUpperCase()
            color: root.cFg
            font.family: root.fontMono
            font.pixelSize: Math.round(15 * root.uiScale())
            font.bold: true
            font.letterSpacing: 2
        }
        Label {
            text: sub
            color: root.cLine
            font.family: root.fontMono
            font.pixelSize: 11
            font.letterSpacing: 1
            Layout.alignment: Qt.AlignVCenter
            visible: sub.length > 0
        }
        Rectangle {
            Layout.fillWidth: true
            Layout.alignment: Qt.AlignVCenter
            height: 1
            color: root.cLine
            opacity: 0.45
        }
    }

    // Language breakdown as a ring. A stacked bar reads as one long smear at
    // this width; a ring gives each slice its own arc and leaves the middle
    // free for the dominant language, which is the fact people actually want.
    component LangRing: Item {
        id: ring
        property var languages: []

        Canvas {
            id: ringCanvas
            anchors.fill: parent
            onPaint: {
                const ctx = getContext("2d")
                ctx.reset()
                const cx = width / 2
                const cy = height / 2
                const outer = Math.min(width, height) / 2 - 2
                const inner = outer * 0.58
                let start = -Math.PI / 2
                for (let i = 0; i < ring.languages.length; i++) {
                    const sweep = (ring.languages[i].percent / 100) * Math.PI * 2
                    // Hairline gap between slices, but never so large that a
                    // sub-percent language disappears entirely.
                    const gap = Math.min(0.03, sweep * 0.12)
                    ctx.beginPath()
                    ctx.arc(cx, cy, outer, start + gap / 2, start + sweep - gap / 2, false)
                    ctx.arc(cx, cy, inner, start + sweep - gap / 2, start + gap / 2, true)
                    ctx.closePath()
                    ctx.fillStyle = root.langColor(i)
                    ctx.fill()
                    start += sweep
                }
            }
        }

        Label {
            anchors.centerIn: parent
            text: ring.languages.length > 0 ? Math.round(ring.languages[0].percent) + "%" : ""
            color: root.cFg
            font.family: root.fontBrand
            font.pixelSize: Math.round(18 * root.uiScale())
            font.bold: true
        }

        onLanguagesChanged: ringCanvas.requestPaint()
        onWidthChanged: ringCanvas.requestPaint()
        onHeightChanged: ringCanvas.requestPaint()
    }

    // Source + trust readout: a source-hued dot, mono source tag, trust dot.
    component SourceTag: Row {
        property string source: "flathub"
        property string trust: ""
        spacing: 5
        Rectangle {
            width: 6; height: 6; radius: 0
            color: root.sourceAccent(source)
            anchors.verticalCenter: parent.verticalCenter
        }
        Text {
            text: root.sourceShort(source)
            color: root.sourceAccent(source)
            font.family: root.fontMono
            font.pixelSize: 9
            font.letterSpacing: 1
            font.bold: true
            anchors.verticalCenter: parent.verticalCenter
        }
        Rectangle {
            visible: trust.length > 0
            width: 5; height: 5; radius: 0.5
            color: root.trustColor(trust)
            anchors.verticalCenter: parent.verticalCenter
        }
    }

    component SectionPanel: Rectangle {
        id: panel
        property string title: ""
        property string subtitle: ""

        Layout.fillWidth: true
        color: root.cPanel
        border.color: root.cLine
        radius: 0
        implicitHeight: sectionContent.implicitHeight + 32

        default property alias content: sectionContent.data

        ColumnLayout {
            id: sectionContent
            anchors.fill: parent
            anchors.margins: 14
            spacing: 10

            RowLayout {
                visible: panel.title.length > 0
                Layout.fillWidth: true
                spacing: 9
                Rectangle { width: root.tickW; height: 14; radius: 0; color: root.cGreen; Layout.alignment: Qt.AlignVCenter }
                Label {
                    text: panel.title.toUpperCase()
                    color: root.cFg
                    font.family: root.fontMono
                    font.pixelSize: 14
                    font.bold: true
                    font.letterSpacing: 2
                    Layout.fillWidth: true
                }
            }

            Label {
                visible: panel.subtitle.length > 0
                text: panel.subtitle
                color: root.cMuted
                font.family: root.fontMono
                font.pixelSize: 12
                font.letterSpacing: 1
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
            }
        }
    }

    component StatRow: RowLayout {
        property string name: ""
        property string value: ""
        spacing: 12
        Label {
            text: name.toUpperCase()
            color: root.cLine
            font.family: root.fontMono
            font.pixelSize: 11
            font.letterSpacing: 1
            Layout.preferredWidth: 128
        }
        Label {
            text: value && value.length > 0 ? value : "Not provided"
            color: root.cFg
            font.family: root.fontMono
            font.pixelSize: 13
            wrapMode: Text.WordWrap
            Layout.fillWidth: true
        }
    }

    component MetricBlock: ColumnLayout {
        property string title: ""
        property string value: ""
        property string caption: ""
        spacing: 4
        Layout.fillWidth: true
        Layout.minimumWidth: 86

        Label {
            text: title.toUpperCase()
            color: root.cLine
            font.family: root.fontMono
            font.pixelSize: 10
            font.letterSpacing: 1
            horizontalAlignment: Text.AlignHCenter
            Layout.fillWidth: true
            elide: Text.ElideRight
        }

        Label {
            text: value && value.length > 0 ? value : "Not provided"
            color: root.cFg
            font.family: root.fontMono
            font.pixelSize: Math.round(20 * root.uiScale())
            font.bold: true
            font.letterSpacing: 1
            horizontalAlignment: Text.AlignHCenter
            Layout.fillWidth: true
            elide: Text.ElideRight
        }

        Label {
            text: caption
            color: root.cMuted
            font.family: root.fontHuman
            font.pixelSize: 11
            horizontalAlignment: Text.AlignHCenter
            Layout.fillWidth: true
            elide: Text.ElideRight
        }
    }

    component ActionButton: Rectangle {
        id: action
        property string label: ""
        property color fill: root.cGreen
        property color textColor: root.cBase
        property bool hovered: false
        property bool pressed: false
        property bool active: true
        signal clicked()

        height: 40
        implicitHeight: height
        implicitWidth: actionText.implicitWidth + 28
        color: pressed ? Qt.darker(fill, 1.18) : hovered ? Qt.lighter(fill, 1.08) : fill
        border.color: fill === root.cPanel || fill === root.cDim ? root.cLine : fill
        radius: 0
        scale: pressed ? 0.985 : hovered ? 1.008 : 1.0
        opacity: active ? 1.0 : 0.62

        Behavior on color { ColorAnimation { duration: 120 } }
        Behavior on scale { NumberAnimation { duration: 110; easing.type: Easing.OutCubic } }

        Label {
            id: actionText
            anchors.fill: parent
            anchors.leftMargin: 8
            anchors.rightMargin: 8
            text: action.label
            color: action.textColor
            font.family: root.fontHuman
            font.pixelSize: 13
            font.bold: true
            horizontalAlignment: Text.AlignHCenter
            verticalAlignment: Text.AlignVCenter
            elide: Text.ElideRight
        }

        MouseArea {
            anchors.fill: parent
            enabled: action.active
            hoverEnabled: true
            cursorShape: action.active ? Qt.PointingHandCursor : Qt.ArrowCursor
            onEntered: action.hovered = true
            onExited: {
                action.hovered = false
                action.pressed = false
            }
            onPressed: action.pressed = true
            onReleased: action.pressed = false
            onClicked: action.clicked()
        }
    }

    FloatingWindow {
        id: window
        implicitWidth: 1280
        implicitHeight: 820
        visible: true
        title: "Thallium Store"
        color: root.cBase

        Rectangle {
            anchors.fill: parent
            color: root.cBase

            ColumnLayout {
                anchors.fill: parent
                spacing: 0

                // ── Command bar: brand left, centered HUD tabs, search right ──
                Rectangle {
                    id: topBar
                    Layout.fillWidth: true
                    Layout.preferredHeight: 64
                    color: root.cPanel

                    Rectangle {
                        anchors.bottom: parent.bottom
                        width: parent.width
                        height: 1
                        color: root.cLine
                    }

                    RowLayout {
                        anchors.left: parent.left
                        anchors.leftMargin: 20
                        anchors.verticalCenter: parent.verticalCenter
                        spacing: 12

                        Rectangle {
                            Layout.preferredWidth: 34
                            Layout.preferredHeight: 34
                            color: root.cGreen
                            Label {
                                anchors.centerIn: parent
                                text: "T"
                                color: root.cBase
                                font.family: root.fontBrand
                                font.pixelSize: 17
                                font.bold: true
                            }
                        }

                        ColumnLayout {
                            spacing: 0
                            Label {
                                text: "Thallium"
                                color: root.cFg
                                font.family: root.fontBrand
                                font.pixelSize: 17
                                font.bold: true
                            }
                            Label {
                                text: "STORE"
                                color: root.cMuted
                                font.family: root.fontMono
                                font.pixelSize: 8
                                font.letterSpacing: 4
                            }
                        }
                    }

                    Row {
                        anchors.centerIn: parent
                        spacing: 36
                        // Ribbon steps aside while search is expanded over it.
                        opacity: searchBox.expanded ? 0 : 1
                        enabled: !searchBox.expanded
                        Behavior on opacity { NumberAnimation { duration: root.tMed; easing.type: Easing.OutCubic } }
                        TopTab { label: "Home"; view: "discover" }
                        TopTab { label: "Apps"; view: "installed"; badge: root.activeOpsCount() }
                        TopTab { label: "Search"; view: "search" }
                        TopTab { label: "Settings"; view: "settings" }
                    }

                    // Search: an icon at rest; typing expands it left over the
                    // ribbon tabs, Apple-style.
                    Rectangle {
                        id: searchBox
                        readonly property bool expanded: searchField.activeFocus
                            || root.query.length > 0 || root.activeView === "search"
                        // Centred: it opens over the nav row, in the same place the eye
                        // already is, rather than off in the corner the icon used to sit in.
                        anchors.horizontalCenter: parent.horizontalCenter
                        anchors.verticalCenter: parent.verticalCenter
                        // No resting magnifier in the corner: SEARCH is a nav tab now,
                        // so the icon was a second, quieter control for the same thing.
                        // The field appears when that tab puts the view on search.
                        visible: expanded
                        width: expanded ? Math.min(640, Math.round(window.width * 0.42)) : 36
                        height: 36
                        z: 10
                        color: root.cDim
                        border.color: searchField.activeFocus ? root.cGreen : root.cLine
                        Behavior on width { NumberAnimation { duration: root.tMed; easing.type: Easing.OutCubic } }

                        RowLayout {
                            anchors.fill: parent
                            anchors.leftMargin: searchBox.expanded ? 12 : 0
                            anchors.rightMargin: searchBox.expanded ? 10 : 0
                            spacing: 8
                            Label {
                                text: "⌕"
                                color: searchField.activeFocus ? root.cGreen : root.cMuted
                                font.family: root.fontMono
                                font.pixelSize: 15
                                horizontalAlignment: searchBox.expanded ? Text.AlignLeft : Text.AlignHCenter
                                Layout.fillWidth: !searchBox.expanded
                            }
                            TextField {
                                id: searchField
                                visible: searchBox.expanded
                                Layout.fillWidth: searchBox.expanded
                                placeholderText: "Search apps"
                                text: root.query
                                color: root.cFg
                                placeholderTextColor: root.cMuted
                                background: Rectangle { color: "transparent" }
                                font.family: root.fontHuman
                                font.pixelSize: 13
                                // No onActiveFocusChanged handler on purpose. Steering the view
                                // from focus meant an open app detail page snapped back to search
                                // whenever focus returned to the window -- after a screenshot, a
                                // polkit prompt, a workspace switch -- and the field taking focus
                                // at startup dropped you on a stale search instead of Home.
                                // Clicking the box and typing both switch the view explicitly.
                                onTextChanged: {
                                    if (text === root.query)
                                        return
                                    root.query = text
                                    root.searchCommitted = false
                                    if (root.activeView !== "search")
                                        root.activeView = "search"
                                    searchDebounce.restart()
                                }
                                onAccepted: root.searchCommitted = true
                                Keys.onEscapePressed: {
                                    root.activeView = "discover"
                                    focus = false
                                }
                            }

                            // Clear + exit search.
                            Label {
                                visible: searchBox.expanded
                                text: "✕"
                                color: searchCloseHover.hovered ? root.cFg : root.cMuted
                                font.family: root.fontMono
                                font.pixelSize: 13
                                Layout.alignment: Qt.AlignVCenter
                                HoverHandler { id: searchCloseHover }
                                MouseArea {
                                    anchors.fill: parent
                                    anchors.margins: -8
                                    cursorShape: Qt.PointingHandCursor
                                    onClicked: {
                                        root.query = ""
                                        root.searchCommitted = false
                                        root.results = []
                                        searchDebounce.stop()
                                        searchField.focus = false
                                        root.activeView = "discover"
                                    }
                                }
                            }
                        }

                    }
                }

                StackLayout {
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    // "updates" is no longer a page of its own; it lands on Apps, which carries the section.
                    currentIndex: root.activeView === "details" ? 1 : root.activeView === "settings" ? 2 : (root.activeView === "installed" || root.activeView === "queue" || root.activeView === "updates") ? 3 : 0

                    Item {
                        id: discoverPage
                        readonly property bool onSearch: root.activeView === "search"
                        readonly property bool shown: root.activeView === "discover" || root.activeView === "search"
                        opacity: shown ? 1 : 0
                        transform: Translate {
                            y: discoverPage.shown ? 0 : 12
                            Behavior on y { NumberAnimation { duration: 180; easing.type: Easing.OutCubic } }
                        }
                        Behavior on opacity { NumberAnimation { duration: 180; easing.type: Easing.OutCubic } }

                        Rectangle {
                            anchors.fill: parent
                            color: root.cBase
                        }

                        ColumnLayout {
                            anchors.fill: parent
                            anchors.leftMargin: root.contentSideMargin()
                            anchors.rightMargin: root.contentSideMargin()
                            anchors.topMargin: root.pagePad()
                            anchors.bottomMargin: root.pagePad()
                            spacing: 16

                            ColumnLayout {
                                Layout.fillWidth: true
                                spacing: 3

                                Label {
                                    visible: !discoverPage.onSearch
                                    text: Qt.formatDate(new Date(), "dddd, MMMM d").toUpperCase()
                                    color: root.cGreen
                                    font.family: root.fontMono
                                    font.pixelSize: 11
                                    font.letterSpacing: 3
                                    Layout.fillWidth: true
                                }
                                Label {
                                    text: discoverPage.onSearch ? "Search" : "Home"
                                    color: root.cFg
                                    font.family: root.fontBrand
                                    font.pixelSize: Math.round(34 * root.uiScale())
                                    font.bold: true
                                    Layout.fillWidth: true
                                }
                                Label {
                                    text: discoverPage.onSearch
                                          ? (root.query.length > 0 ? root.results.length + " apps found across Thallium sources." : "Search apt, Flathub, GitHub and AppImage.")
                                          : "Curated picks from Flathub."
                                    color: root.cMuted
                                    font.family: root.fontHuman
                                    font.pixelSize: 15
                                    Layout.fillWidth: true
                                }
                            }

                            RowLayout {
                                Layout.fillWidth: true
                                spacing: 8
                                visible: discoverPage.onSearch && root.query.length > 0
                                Repeater {
                                    model: root.providers
                                    delegate: Rectangle {
                                        height: 26
                                        implicitWidth: providerText.implicitWidth + 18
                                        radius: 0
                                        color: modelData.state === "ready" ? "#233024" : "#332b1a"
                                        border.color: modelData.state === "ready" ? "#4a5f3f" : "#5f4f2a"
                                        Label {
                                            id: providerText
                                            anchors.centerIn: parent
                                            text: root.sourceLabel(modelData.source) + " · " + modelData.state
                                            color: modelData.state === "ready" ? root.cGreen : root.cWarn
                                            font.family: root.fontHuman
                                            font.pixelSize: 12
                                            font.bold: true
                                        }
                                    }
                                }
                                Item { Layout.fillWidth: true }
                            }

                            // Discover landing — curated collections, shown when not searching.
                            ScrollView {
                                id: discoverScroll
                                Layout.fillWidth: true
                                Layout.fillHeight: true
                                clip: true
                                visible: !discoverPage.onSearch
                                ScrollBar.horizontal.policy: ScrollBar.AlwaysOff
                                ScrollBar.vertical.policy: ScrollBar.AlwaysOff

                                ColumnLayout {
                                    width: discoverScroll.availableWidth
                                    spacing: 26

                                    // Featured carousel — full-width editorial cards that slide between picks.
                                    Item {
                                        Layout.fillWidth: true
                                        Layout.preferredHeight: Math.round(236 * root.uiScale()) + 24
                                        visible: root.featuredPool().length > 0

                                        ListView {
                                            id: featuredCarousel
                                            anchors.top: parent.top
                                            anchors.left: parent.left
                                            anchors.right: parent.right
                                            height: Math.round(236 * root.uiScale())
                                            orientation: ListView.Horizontal
                                            snapMode: ListView.SnapOneItem
                                            highlightRangeMode: ListView.StrictlyEnforceRange
                                            highlightMoveDuration: root.tSlow
                                            preferredHighlightBegin: 0
                                            preferredHighlightEnd: 0
                                            clip: true
                                            model: root.featuredPool()
                                            cacheBuffer: 4096

                                            delegate: ChamferPanel {
                                                id: featCard
                                                width: featuredCarousel.width
                                                height: featuredCarousel.height
                                                property var app: modelData
                                                property var variant: root.recommendedVariant(modelData)
                                                fill: root.appSurface(app)
                                                accentEdge: true
                                                cut: 22
                                                clip: true

                                                // Artwork backdrop — first screenshot under an Everforest scrim,
                                                // Apple Today style. Falls back to tint + ghost monogram.
                                                Image {
                                                    id: featShot
                                                    anchors.fill: parent
                                                    source: featCard.app.screenshots && featCard.app.screenshots.length > 0 ? featCard.app.screenshots[0] : ""
                                                    fillMode: Image.PreserveAspectCrop
                                                    asynchronous: true
                                                    cache: true
                                                    smooth: true
                                                    visible: status === Image.Ready
                                                }
                                                Rectangle {
                                                    anchors.fill: parent
                                                    visible: featShot.status === Image.Ready
                                                    gradient: Gradient {
                                                        orientation: Gradient.Horizontal
                                                        GradientStop { position: 0.0; color: "#f5060607" }
                                                        GradientStop { position: 0.5; color: "#c0060607" }
                                                        GradientStop { position: 1.0; color: "#3d060607" }
                                                    }
                                                }
                                                // Re-cut the chamfer notch over the artwork.
                                                Canvas {
                                                    anchors.fill: parent
                                                    visible: featShot.status === Image.Ready
                                                    onWidthChanged: requestPaint()
                                                    onHeightChanged: requestPaint()
                                                    onPaint: {
                                                        const ctx = getContext("2d"); ctx.reset()
                                                        const k = featCard.cut
                                                        ctx.beginPath()
                                                        ctx.moveTo(width - k, 0)
                                                        ctx.lineTo(width, 0)
                                                        ctx.lineTo(width, k)
                                                        ctx.closePath()
                                                        ctx.fillStyle = String(root.cBase)
                                                        ctx.fill()
                                                        ctx.strokeStyle = String(root.cGreen)
                                                        ctx.lineWidth = 1.5
                                                        ctx.beginPath()
                                                        ctx.moveTo(width - k, 0)
                                                        ctx.lineTo(width, k)
                                                        ctx.stroke()
                                                    }
                                                }

                                                // Ghost monogram — fallback identity when no artwork.
                                                Label {
                                                    visible: featShot.status !== Image.Ready
                                                    anchors.right: parent.right
                                                    anchors.rightMargin: Math.round(36 * root.uiScale())
                                                    anchors.verticalCenter: parent.verticalCenter
                                                    text: featCard.app.name.substring(0, 1).toUpperCase()
                                                    color: root.appAccent(featCard.app)
                                                    opacity: 0.09
                                                    font.family: root.fontBrand
                                                    font.pixelSize: Math.round(230 * root.uiScale())
                                                    font.bold: true
                                                }

                                                MouseArea {
                                                    anchors.fill: parent
                                                    cursorShape: Qt.PointingHandCursor
                                                    onClicked: root.selectApp(featCard.app)
                                                }

                                                RowLayout {
                                                    anchors.fill: parent
                                                    anchors.margins: 30
                                                    spacing: 28

                                                    // Icon floats free — no backdrop tile.
                                                    Item {
                                                        Layout.preferredWidth: Math.round(148 * root.uiScale())
                                                        Layout.preferredHeight: Math.round(148 * root.uiScale())
                                                        Layout.alignment: Qt.AlignVCenter
                                                        Label {
                                                            anchors.centerIn: parent
                                                            visible: fIcon.status !== Image.Ready
                                                            text: featCard.app.name.substring(0, 1).toUpperCase()
                                                            color: root.appAccent(featCard.app)
                                                            font.family: root.fontBrand
                                                            font.pixelSize: Math.round(56 * root.uiScale())
                                                            font.bold: true
                                                        }
                                                        Image {
                                                            id: fIcon
                                                            anchors.fill: parent
                                                            anchors.margins: 8
                                                            source: featCard.app.icon ? featCard.app.icon : ""
                                                            fillMode: Image.PreserveAspectFit
                                                            asynchronous: true
                                                            cache: true
                                                            smooth: true
                                                            visible: status === Image.Ready
                                                        }
                                                    }

                                                    ColumnLayout {
                                                        Layout.fillWidth: true
                                                        Layout.minimumWidth: 0
                                                        Layout.alignment: Qt.AlignVCenter
                                                        spacing: 7
                                                        Label {
                                                            text: "▚ FEATURED · " + (featCard.variant ? root.sourceLabel(featCard.variant.source).toUpperCase() : "CURATED")
                                                            color: root.cGreen
                                                            font.family: root.fontMono
                                                            font.pixelSize: 12
                                                            font.bold: true
                                                            font.letterSpacing: 3
                                                        }
                                                        Label {
                                                            text: featCard.app.name
                                                            color: root.cFg
                                                            font.family: root.fontBrand
                                                            font.pixelSize: Math.round(33 * root.uiScale())
                                                            font.bold: true
                                                            elide: Text.ElideRight
                                                            Layout.fillWidth: true
                                                        }
                                                        Label {
                                                            text: featCard.app.summary
                                                            color: root.cFg
                                                            opacity: 0.72
                                                            font.family: root.fontHuman
                                                            font.pixelSize: 15
                                                            wrapMode: Text.WordWrap
                                                            maximumLineCount: 2
                                                            elide: Text.ElideRight
                                                            Layout.fillWidth: true
                                                        }
                                                        RowLayout {
                                                            Layout.topMargin: 6
                                                            spacing: 14
                                                            ActionButton {
                                                                label: featCard.app.installed ? "Installed" : "Get"
                                                                fill: featCard.app.installed ? root.cDim : root.cGreen
                                                                textColor: featCard.app.installed ? root.cMuted : root.cBase
                                                                active: !featCard.app.installed
                                                                height: 36
                                                                Layout.preferredWidth: 104
                                                                onClicked: {
                                                                    if (!featCard.app.installed)
                                                                        root.selectApp(featCard.app)
                                                                }
                                                            }
                                                            Label {
                                                                text: "FREE"
                                                                color: root.cFg
                                                                opacity: 0.5
                                                                font.family: root.fontMono
                                                                font.pixelSize: 10
                                                                font.letterSpacing: 2
                                                            }
                                                        }
                                                    }
                                                }

                                                // HUD index counter, bottom-right.
                                                Label {
                                                    anchors.right: parent.right
                                                    anchors.bottom: parent.bottom
                                                    anchors.margins: 16
                                                    text: (index + 1 < 10 ? "0" : "") + (index + 1) + " / "
                                                          + (featuredCarousel.count < 10 ? "0" : "") + featuredCarousel.count
                                                    color: root.cGreen
                                                    opacity: 0.75
                                                    font.family: root.fontMono
                                                    font.pixelSize: 11
                                                    font.letterSpacing: 2
                                                }
                                            }
                                        }

                                        // page dots
                                        Row {
                                            anchors.top: featuredCarousel.bottom
                                            anchors.topMargin: 12
                                            anchors.horizontalCenter: parent.horizontalCenter
                                            spacing: 7
                                            Repeater {
                                                model: root.featuredPool().length
                                                delegate: Rectangle {
                                                    width: featuredCarousel.currentIndex === index ? 18 : 6
                                                    height: 6
                                                    color: featuredCarousel.currentIndex === index ? root.cGreen : root.cLine
                                                    Behavior on width { NumberAnimation { duration: root.tFast; easing.type: Easing.OutCubic } }
                                                    Behavior on color { ColorAnimation { duration: root.tFast } }
                                                    MouseArea {
                                                        anchors.fill: parent
                                                        anchors.margins: -5
                                                        cursorShape: Qt.PointingHandCursor
                                                        onClicked: featuredCarousel.currentIndex = index
                                                    }
                                                }
                                            }
                                        }
                                    }

                                    Repeater {
                                        model: root.discover
                                        delegate: ColumnLayout {
                                            property var collection: modelData
                                            Layout.fillWidth: true
                                            spacing: 10

                                            HudRailHeader {
                                                title: collection.title
                                                sub: collection.subtitle
                                            }

                                            ListView {
                                                id: railView
                                                Layout.fillWidth: true
                                                Layout.preferredHeight: 246
                                                orientation: ListView.Horizontal
                                                spacing: 14
                                                clip: true
                                                model: collection.apps
                                                // Dynamic card width — fill the row edge-to-edge at any window size;
                                                // when the collection has fewer apps than fit, stretch them instead.
                                                readonly property int cardsVisible: Math.max(3, Math.min(count > 0 ? count : 6, Math.floor((width + spacing) / 200)))
                                                readonly property int cardW: Math.floor((width - (cardsVisible - 1) * spacing) / cardsVisible)
                                                delegate: Rectangle {
                                                    id: railCard
                                                    property var app: modelData
                                                    width: railView.cardW
                                                    height: 234
                                                    radius: 0
                                                    color: railHover.hovered ? root.cDim : root.cPanel
                                                    border.color: railHover.hovered ? root.cGreen : root.cLine

                                                    HoverHandler { id: railHover }
                                                    Behavior on border.color { ColorAnimation { duration: 140 } }
                                                    Behavior on color { ColorAnimation { duration: 140 } }

                                                    MouseArea {
                                                        anchors.fill: parent
                                                        cursorShape: Qt.PointingHandCursor
                                                        onClicked: root.selectApp(railCard.app)
                                                    }

                                                    ColumnLayout {
                                                        anchors.fill: parent
                                                        anchors.margins: 14
                                                        spacing: 10

                                                        // Icon floats free — no backdrop tile.
                                                        Item {
                                                            Layout.fillWidth: true
                                                            Layout.preferredHeight: 108
                                                            Label {
                                                                anchors.centerIn: parent
                                                                visible: railIcon.status !== Image.Ready
                                                                text: railCard.app.name.substring(0, 1).toUpperCase()
                                                                color: root.appAccent(railCard.app)
                                                                font.family: root.fontBrand
                                                                font.pixelSize: 38
                                                                font.bold: true
                                                            }
                                                            Image {
                                                                id: railIcon
                                                                anchors.centerIn: parent
                                                                width: 88
                                                                height: 88
                                                                source: railCard.app.icon ? railCard.app.icon : ""
                                                                fillMode: Image.PreserveAspectFit
                                                                asynchronous: true
                                                                cache: true
                                                                smooth: true
                                                                visible: status === Image.Ready
                                                            }
                                                        }

                                                        Label {
                                                            text: railCard.app.name
                                                            color: root.cFg
                                                            font.family: root.fontHuman
                                                            font.pixelSize: 14
                                                            font.bold: true
                                                            elide: Text.ElideRight
                                                            maximumLineCount: 1
                                                            Layout.fillWidth: true
                                                        }

                                                        Label {
                                                            text: railCard.app.summary || ""
                                                            color: root.cMuted
                                                            font.family: root.fontHuman
                                                            font.pixelSize: 11
                                                            lineHeight: 1.05
                                                            elide: Text.ElideRight
                                                            maximumLineCount: 2
                                                            wrapMode: Text.WordWrap
                                                            Layout.fillWidth: true
                                                            Layout.fillHeight: true
                                                            verticalAlignment: Text.AlignTop
                                                        }

                                                        SourceTag {
                                                            source: railCard.app.variants && railCard.app.variants.length > 0 ? railCard.app.variants[0].source : "flathub"
                                                            trust: railCard.app.variants && railCard.app.variants.length > 0 ? railCard.app.variants[0].trust : ""
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }

                            // Empty search — prompt.
                            Item {
                                Layout.fillWidth: true
                                Layout.fillHeight: true
                                visible: discoverPage.onSearch && root.query.length === 0
                                Label {
                                    anchors.centerIn: parent
                                    text: "TYPE TO SEARCH · APT · FLATHUB · GITHUB · APPIMAGE"
                                    color: root.cLine
                                    font.family: root.fontMono
                                    font.pixelSize: 12
                                    font.letterSpacing: 2
                                }
                            }

                            // No results (search settled, nothing found).
                            Item {
                                Layout.fillWidth: true
                                Layout.fillHeight: true
                                visible: discoverPage.onSearch && root.query.length > 0
                                         && root.results.length === 0
                                         && !root.isBusy() && !searchDebounce.running
                                Label {
                                    anchors.centerIn: parent
                                    text: "NO RESULTS FOR “" + root.query.toUpperCase() + "”"
                                    color: root.cLine
                                    font.family: root.fontMono
                                    font.pixelSize: 12
                                    font.letterSpacing: 2
                                }
                            }

                            // Search pending or in flight, nothing painted yet — brand
                            // loader. Also covers the debounce window so the header
                            // never loses its fill-height sibling (which would let the
                            // ColumnLayout re-center everything mid-keystroke).
                            Item {
                                id: searchLoading
                                Layout.fillWidth: true
                                Layout.fillHeight: true
                                visible: discoverPage.onSearch && root.query.length > 0
                                         && root.results.length === 0
                                         && (root.isBusy() || searchDebounce.running)
                                ColumnLayout {
                                    anchors.centerIn: parent
                                    spacing: 16
                                    ThalliumLoader {
                                        Layout.alignment: Qt.AlignHCenter
                                        Layout.preferredWidth: 84
                                        Layout.preferredHeight: 84
                                        running: searchLoading.visible
                                    }
                                    Label {
                                        text: "SEARCHING SOURCES"
                                        color: root.cMuted
                                        font.family: root.fontMono
                                        font.pixelSize: 10
                                        font.letterSpacing: 3
                                        Layout.alignment: Qt.AlignHCenter
                                    }
                                }
                            }

                            // Typeahead suggestions — quick list while typing (press Enter for full cards).
                            ScrollView {
                                Layout.fillWidth: true
                                Layout.fillHeight: true
                                clip: true
                                visible: discoverPage.onSearch && root.query.length > 0 && !root.searchCommitted && root.results.length > 0

                                ListView {
                                    width: parent.width
                                    model: root.results
                                    spacing: 0
                                    delegate: Rectangle {
                                        id: sugRow
                                        property var app: modelData
                                        width: ListView.view ? ListView.view.width : 0
                                        height: 50
                                        color: sugHover.hovered ? root.cDim : "transparent"

                                        HoverHandler { id: sugHover }
                                        Behavior on color { ColorAnimation { duration: 120 } }
                                        MouseArea {
                                            anchors.fill: parent
                                            cursorShape: Qt.PointingHandCursor
                                            onClicked: root.selectApp(sugRow.app)
                                        }

                                        RowLayout {
                                            anchors.fill: parent
                                            anchors.leftMargin: 6
                                            anchors.rightMargin: 12
                                            spacing: 12
                                            Label {
                                                text: "⌕"
                                                color: root.cMuted
                                                font.family: root.fontMono
                                                font.pixelSize: 15
                                                Layout.preferredWidth: 22
                                                horizontalAlignment: Text.AlignHCenter
                                            }
                                            Label {
                                                text: sugRow.app.name
                                                color: root.cFg
                                                font.family: root.fontHuman
                                                font.pixelSize: 15
                                                elide: Text.ElideRight
                                                Layout.fillWidth: true
                                            }
                                            SourceTag {
                                                source: sugRow.app.variants && sugRow.app.variants.length > 0 ? sugRow.app.variants[0].source : "flathub"
                                                trust: sugRow.app.variants && sugRow.app.variants.length > 0 ? sugRow.app.variants[0].trust : ""
                                            }
                                        }

                                        Rectangle {
                                            anchors.bottom: parent.bottom
                                            width: parent.width
                                            height: 1
                                            color: root.cLine
                                            opacity: 0.4
                                        }
                                    }
                                }
                            }

                            ScrollView {
                                Layout.fillWidth: true
                                Layout.fillHeight: true
                                clip: true
                                visible: discoverPage.onSearch && root.searchCommitted

                                GridView {
                                    id: resultGrid
                                    width: parent.width
                                    height: parent.height
                                    model: root.results
                                    cellWidth: {
                                        const columns = Math.max(1, Math.floor(width / 320))
                                        return Math.floor(width / columns)
                                    }
                                    cellHeight: Math.round(132 * root.uiScale())
                                    delegate: Rectangle {
                                        id: appCard
                                        property var app: modelData

                                        width: resultGrid.cellWidth - 14
                                        height: Math.round(116 * root.uiScale())
                                        color: root.cPanel
                                        border.color: resultHover.hovered ? root.cGreen : root.cLine
                                        radius: 0
                                        opacity: 0
                                        x: 7
                                        y: 4
                                        clip: true

                                        HoverHandler {
                                            id: resultHover
                                        }

                                        Behavior on border.color { ColorAnimation { duration: 140 } }
                                        Behavior on color { ColorAnimation { duration: 140 } }

                                        Component.onCompleted: resultFade.start()

                                        NumberAnimation {
                                            id: resultFade
                                            target: appCard
                                            property: "opacity"
                                            from: 0
                                            to: 1
                                            duration: 180
                                            easing.type: Easing.OutCubic
                                        }

                                        Rectangle {
                                            width: parent.width
                                            height: parent.height
                                            opacity: resultHover.hovered ? 0.16 : 0.08
                                            gradient: Gradient {
                                                orientation: Gradient.Horizontal
                                                GradientStop { position: 0.0; color: root.appSurface(appCard.app) }
                                                GradientStop { position: 1.0; color: "transparent" }
                                            }

                                            Behavior on opacity { NumberAnimation { duration: 140 } }
                                        }

                                        MouseArea {
                                            anchors.fill: parent
                                            cursorShape: Qt.PointingHandCursor
                                            acceptedButtons: Qt.LeftButton
                                            onClicked: root.selectApp(appCard.app)
                                        }

                                        RowLayout {
                                            anchors.fill: parent
                                            anchors.margins: 16
                                            spacing: 14

                                            Rectangle {
                                                Layout.preferredWidth: Math.round(64 * root.uiScale())
                                                Layout.preferredHeight: Math.round(64 * root.uiScale())
                                                color: "transparent"
                                                clip: true
                                                Label {
                                                    anchors.centerIn: parent
                                                    visible: resultIcon.status !== Image.Ready
                                                    text: appCard.app.name.substring(0, 1).toUpperCase()
                                                    color: root.appAccent(appCard.app)
                                                    font.family: root.fontBrand
                                                    font.pixelSize: Math.round(30 * root.uiScale())
                                                    font.bold: true
                                                }
                                                Image {
                                                    id: resultIcon
                                                    anchors.fill: parent
                                                    anchors.margins: 4
                                                    source: appCard.app.icon ? appCard.app.icon : ""
                                                    fillMode: Image.PreserveAspectFit
                                                    asynchronous: true
                                                    cache: true
                                                    smooth: true
                                                    visible: status === Image.Ready
                                                }
                                            }

                                            ColumnLayout {
                                                Layout.fillWidth: true
                                                Layout.minimumWidth: 0
                                                Layout.alignment: Qt.AlignVCenter
                                                spacing: 6
                                                Label {
                                                    text: appCard.app.name
                                                    color: root.cFg
                                                    font.family: root.fontHuman
                                                    font.pixelSize: Math.round(16 * root.uiScale())
                                                    font.bold: true
                                                    elide: Text.ElideRight
                                                    Layout.fillWidth: true
                                                }
                                                Label {
                                                    text: appCard.app.summary
                                                    color: root.cMuted
                                                    font.family: root.fontHuman
                                                    font.pixelSize: Math.round(12 * root.uiScale())
                                                    lineHeight: 0.92
                                                    maximumLineCount: 2
                                                    wrapMode: Text.WordWrap
                                                    elide: Text.ElideRight
                                                    Layout.fillWidth: true
                                                }
                                                RowLayout {
                                                    Layout.fillWidth: true
                                                    spacing: 8
                                                    Rectangle {
                                                        height: 22
                                                        implicitWidth: sourceChip.implicitWidth + 16
                                                        radius: 0
                                                        color: root.cDim
                                                        border.color: root.cLine
                                                        Label {
                                                            id: sourceChip
                                                            anchors.centerIn: parent
                                                            text: root.recommendedVariant(appCard.app) ? root.sourceLabel(root.recommendedVariant(appCard.app).source) : "Source"
                                                            color: root.cMuted
                                                            font.family: root.fontHuman
                                                            font.pixelSize: 10
                                                            font.bold: true
                                                        }
                                                    }
                                                    Item { Layout.fillWidth: true }
                                                }
                                            }

                                            ActionButton {
                                                label: appCard.app.installed ? "Installed" : "Get"
                                                fill: appCard.app.installed ? root.cDim : root.cBlue
                                                textColor: appCard.app.installed ? root.cMuted : "white"
                                                active: !appCard.app.installed
                                                Layout.preferredWidth: 86
                                                Layout.alignment: Qt.AlignRight | Qt.AlignVCenter
                                                onClicked: {
                                                    if (!appCard.app.installed)
                                                        root.enqueueInstall(appCard.app)
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    Item {
                        opacity: root.activeView === "details" ? 1 : 0
                        transform: Translate {
                            y: root.activeView === "details" ? 0 : 12
                            Behavior on y { NumberAnimation { duration: 180; easing.type: Easing.OutCubic } }
                        }
                        Behavior on opacity { NumberAnimation { duration: 180; easing.type: Easing.OutCubic } }
                        ScrollView {
                            id: detailsScroll
                            anchors.fill: parent
                            clip: true
                            contentWidth: availableWidth
                            contentHeight: detailsColumn.implicitHeight + root.pagePad() * 2

                            ColumnLayout {
                                id: detailsColumn
                                x: Math.max(root.pagePad(), Math.round((detailsScroll.availableWidth - width) / 2))
                                y: root.pagePad()
                                width: Math.max(360, Math.min(1000, detailsScroll.availableWidth - root.pagePad() * 2))
                                spacing: 16
                                opacity: root.activeView === "details" ? 1 : 0

                                Behavior on opacity { NumberAnimation { duration: 180; easing.type: Easing.OutCubic } }

                                // Small back control above the banner.
                                ActionButton {
                                    label: "‹ Back"
                                    fill: root.cDim
                                    textColor: root.cFg
                                    Layout.preferredWidth: 84
                                    onClicked: root.activeView = "discover"
                                }


                                // Hero and the stat rail sit on one row, both the same height.
                                RowLayout {
                                    Layout.fillWidth: true
                                    Layout.preferredHeight: Math.max(188, Math.round(210 * root.uiScale()))
                                    spacing: 14

                                    Rectangle {
                                        Layout.fillWidth: true
                                        height: Math.max(188, Math.round(210 * root.uiScale()))
                                        radius: 0
                                        clip: true
                                        color: root.cPanel
                                        border.color: root.cLine
    
                                        gradient: Gradient {
                                            orientation: Gradient.Horizontal
                                            GradientStop { position: 0.0; color: root.selectedApp ? root.appSurface(root.selectedApp) : root.cPanel }
                                            GradientStop { position: 0.55; color: Qt.rgba(0, 0, 0, 0.10) }
                                            GradientStop { position: 1.0; color: "transparent" }
                                        }
    
                                        // Ghost monogram bleeding off the right edge.
                                        Label {
                                            anchors.right: parent.right
                                            anchors.rightMargin: Math.round(24 * root.uiScale())
                                            anchors.verticalCenter: parent.verticalCenter
                                            text: root.selectedApp ? root.selectedApp.name.substring(0, 1).toUpperCase() : ""
                                            color: root.selectedApp ? root.appAccent(root.selectedApp) : root.cGreen
                                            opacity: 0.07
                                            font.family: root.fontBrand
                                            font.pixelSize: Math.round(220 * root.uiScale())
                                            font.bold: true
                                        }
    
                                        RowLayout {
                                            anchors.fill: parent
                                            anchors.margins: 28
                                            spacing: 26
    
                                            Rectangle {
                                                Layout.preferredWidth: Math.round(120 * root.uiScale())
                                                Layout.preferredHeight: Math.round(120 * root.uiScale())
                                                Layout.alignment: Qt.AlignVCenter
                                                color: "transparent"
                                                clip: true
                                                Label {
                                                    anchors.centerIn: parent
                                                    visible: heroIcon.status !== Image.Ready
                                                    text: root.selectedApp ? root.selectedApp.name.substring(0, 1).toUpperCase() : ""
                                                    color: root.selectedApp ? root.appAccent(root.selectedApp) : root.cGreen
                                                    font.family: root.fontBrand
                                                    font.pixelSize: Math.round(54 * root.uiScale())
                                                    font.bold: true
                                                }
                                                Image {
                                                    id: heroIcon
                                                    anchors.fill: parent
                                                    source: root.selectedApp && root.selectedApp.icon ? root.selectedApp.icon : ""
                                                    fillMode: Image.PreserveAspectFit
                                                    asynchronous: true
                                                    cache: true
                                                    smooth: true
                                                    visible: status === Image.Ready
                                                }
                                            }
    
                                            ColumnLayout {
                                                Layout.fillWidth: true
                                                Layout.minimumWidth: 0
                                                Layout.alignment: Qt.AlignVCenter
                                                spacing: 8
                                                Label {
                                                    text: root.selectedApp ? root.selectedApp.name : ""
                                                    color: root.cFg
                                                    font.family: root.fontBrand
                                                    font.pixelSize: Math.round(38 * root.uiScale())
                                                    font.bold: true
                                                    Layout.fillWidth: true
                                                    elide: Text.ElideRight
                                                }
                                                Label {
                                                    text: root.selectedApp ? root.selectedApp.summary : ""
                                                    color: root.cFg
                                                    opacity: 0.86
                                                    font.family: root.fontHuman
                                                    font.pixelSize: 17
                                                    wrapMode: Text.WordWrap
                                                    maximumLineCount: 2
                                                    elide: Text.ElideRight
                                                    Layout.fillWidth: true
                                                }
                                                Label {
                                                    text: {
                                                        const v = root.recommendedVariant(root.selectedApp)
                                                        if (!v) return "Free"
                                                        return "Free · " + root.sourceLabel(v.source) + " · " + root.formatLabel(v.source)
                                                    }
                                                    color: root.cFg
                                                    opacity: 0.62
                                                    font.family: root.fontMono
                                                    font.pixelSize: 12
                                                    font.letterSpacing: 1
                                                    Layout.fillWidth: true
                                                    elide: Text.ElideRight
                                                }
                                            }
    
                                            // Primary action lives in the hero, Apple-style.
                                            ColumnLayout {
                                                Layout.alignment: Qt.AlignVCenter
                                                spacing: 6
                                                ActionButton {
                                                    label: root.selectedApp && root.selectedApp.installed ? "Uninstall" : "Get"
                                                    fill: root.selectedApp && root.selectedApp.installed ? root.cRed : root.cGreen
                                                    textColor: root.selectedApp && root.selectedApp.installed ? "white" : root.cBase
                                                    Layout.preferredWidth: 132
                                                    onClicked: {
                                                        if (root.selectedApp && root.selectedApp.installed)
                                                            root.enqueueUninstall(root.selectedApp)
                                                        else
                                                            root.openInstall(root.selectedApp)
                                                    }
                                                }
                                                Label {
                                                    // Version sits with the action, where the decision
                                                    // is made -- it used to appear only in Sources.
                                                    text: {
                                                        const v = root.recommendedVariant(root.selectedApp)
                                                        if (v && v.version)
                                                            return v.version.charAt(0) === "v" ? v.version : "v" + v.version
                                                        return root.selectedApp && root.selectedApp.installed ? "ON THIS SYSTEM" : "FREE"
                                                    }
                                                    color: root.cFg
                                                    opacity: 0.45
                                                    font.family: root.fontMono
                                                    font.pixelSize: 9
                                                    font.letterSpacing: 2
                                                    Layout.alignment: Qt.AlignHCenter
                                                }
                                            }
                                        }
                                    }

                                    // Vertical stat rail beside the hero: the numbers you check before
                                    // installing, in one column instead of a strip under the banner.
                                        Rectangle {
                                        Layout.preferredWidth: Math.round(220 * root.uiScale())
                                        Layout.fillHeight: true
                                        color: root.cPanel
                                        border.color: root.cLine
                                        radius: 0

                                        ColumnLayout {
                                            anchors.fill: parent
                                            anchors.margins: 16
                                            spacing: 0

                                            Repeater {
                                                model: {
                                                    const v = root.recommendedVariant(root.selectedApp)
                                                    return [
                                                        { t: "RATING",    val: (root.selectedApp && root.selectedApp.rating) ? Math.round(root.selectedApp.rating) + "\u2605" : "\u2014" },
                                                        { t: "TRUST",     val: v ? root.trustLabel(v.trust) : "\u2014" },
                                                        { t: "SOURCE",    val: v ? root.sourceLabel(v.source) : "\u2014" },
                                                        { t: "DEVELOPER", val: root.compactDeveloper(root.selectedApp) || "\u2014" },
                                                        { t: "SIZE",      val: root.sizeLabel(root.selectedApp) }
                                                    ]
                                                }
                                                delegate: ColumnLayout {
                                                    Layout.fillWidth: true
                                                    Layout.fillHeight: true
                                                    spacing: 1
                                                    Label {
                                                        text: modelData.t
                                                        color: root.cMuted
                                                        font.family: root.fontMono
                                                        font.pixelSize: 9
                                                        font.letterSpacing: 1.5
                                                    }
                                                    Label {
                                                        text: modelData.val
                                                        color: root.cFg
                                                        font.family: root.fontBrand
                                                        font.pixelSize: Math.round(15 * root.uiScale())
                                                        font.bold: true
                                                        elide: Text.ElideRight
                                                        Layout.fillWidth: true
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }

                                // Developer / licence / homepage / identifier on one line. These are
                                // reference details, not headline numbers, so they get a strip rather
                                // than a panel of their own.
                                Rectangle {
                                    Layout.fillWidth: true
                                    Layout.preferredHeight: 56
                                    color: root.cPanel
                                    border.color: root.cLine
                                    radius: 0
                                    visible: root.selectedApp !== null

                                    RowLayout {
                                        anchors.fill: parent
                                        anchors.leftMargin: 18
                                        anchors.rightMargin: 18
                                        spacing: 0

                                        Repeater {
                                            model: [
                                                { t: "DEVELOPER",  val: root.compactDeveloper(root.selectedApp) },
                                                { t: "LICENSE",    val: root.selectedApp && root.selectedApp.license ? root.selectedApp.license : "\u2014" },
                                                { t: "HOMEPAGE",   val: root.selectedApp && root.selectedApp.homepage ? root.selectedApp.homepage : "\u2014" },
                                                { t: "IDENTIFIER", val: root.selectedApp ? root.selectedApp.id : "\u2014" }
                                            ]
                                            delegate: RowLayout {
                                                Layout.fillWidth: true
                                                Layout.fillHeight: true
                                                spacing: 0
                                                ColumnLayout {
                                                    Layout.fillWidth: true
                                                    Layout.minimumWidth: 0
                                                    Layout.alignment: Qt.AlignVCenter
                                                    spacing: 2
                                                    Label {
                                                        text: modelData.t
                                                        color: root.cMuted
                                                        font.family: root.fontMono
                                                        font.pixelSize: 9
                                                        font.letterSpacing: 1.5
                                                    }
                                                    Label {
                                                        text: modelData.val || "\u2014"
                                                        color: root.cFg
                                                        font.family: root.fontMono
                                                        font.pixelSize: 11
                                                        elide: Text.ElideRight
                                                        Layout.fillWidth: true
                                                    }
                                                }
                                                Rectangle {
                                                    visible: index < 3
                                                    Layout.preferredWidth: 1
                                                    Layout.preferredHeight: 28
                                                    Layout.rightMargin: 14
                                                    Layout.leftMargin: 14
                                                    Layout.alignment: Qt.AlignVCenter
                                                    color: root.cLine
                                                }
                                            }
                                        }
                                    }
                                }

                                // Preview and composition share a row: a wide gallery beside a compact
                                // language breakdown, rather than two full-width bands stacked.
                                RowLayout {
                                    Layout.fillWidth: true
                                    spacing: 14
                                    visible: (root.selectedApp && root.selectedApp.screenshots && root.selectedApp.screenshots.length > 0)
                                             || (root.selectedApp && root.selectedApp.languages && root.selectedApp.languages.length > 0)

                                    ColumnLayout {
                                        Layout.fillWidth: true
                                        Layout.minimumWidth: 0
                                        spacing: 10
                                        visible: root.selectedApp && root.selectedApp.screenshots && root.selectedApp.screenshots.length > 0

                                        HudRailHeader { title: "Preview" }

                                        ListView {
                                            Layout.fillWidth: true
                                            Layout.preferredHeight: Math.round(210 * root.uiScale())
                                            orientation: ListView.Horizontal
                                            snapMode: ListView.SnapToItem
                                            clip: true
                                            spacing: 14
                                            model: root.selectedApp ? root.selectedApp.screenshots : []
                                            delegate: Rectangle {
                                                width: Math.round(300 * root.uiScale())
                                                height: Math.round(210 * root.uiScale())
                                                color: root.cDim
                                                border.color: shotHover.hovered ? root.cGreen : root.cLine
                                                radius: 0
                                                clip: true
                                                HoverHandler { id: shotHover }
                                                Behavior on border.color { ColorAnimation { duration: 140 } }
                                                Image {
                                                    id: shotImg
                                                    anchors.fill: parent
                                                    anchors.margins: 1
                                                    source: modelData
                                                    fillMode: Image.PreserveAspectCrop
                                                    asynchronous: true
                                                    cache: true
                                                    visible: status === Image.Ready
                                                }
                                                Label {
                                                    anchors.centerIn: parent
                                                    visible: shotImg.status !== Image.Ready
                                                    text: "\u2026"
                                                    color: root.cMuted
                                                    font.family: root.fontMono
                                                    font.pixelSize: 14
                                                }
                                                // Cropped thumbnails cut the screenshot; clicking opens the
                                                // whole thing over the page.
                                                MouseArea {
                                                    anchors.fill: parent
                                                    cursorShape: Qt.PointingHandCursor
                                                    onClicked: root.viewerSource = modelData
                                                }
                                            }
                                        }
                                    }

                                    // Absorbs the slack when there are no screenshots, so the
                                    // composition panel stays a right-hand panel instead of
                                    // stretching across the page with a ring stranded in it.
                                    Item {
                                        Layout.fillWidth: true
                                        visible: !(root.selectedApp && root.selectedApp.screenshots && root.selectedApp.screenshots.length > 0)
                                    }

                                    ColumnLayout {
                                        Layout.preferredWidth: Math.round(330 * root.uiScale())
                                        Layout.maximumWidth: Math.round(330 * root.uiScale())
                                        spacing: 10
                                        visible: root.selectedApp && root.selectedApp.languages && root.selectedApp.languages.length > 0

                                        HudRailHeader { title: "Composition"; sub: "languages" }

                                        Rectangle {
                                            Layout.fillWidth: true
                                            Layout.preferredHeight: Math.round(210 * root.uiScale())
                                            color: root.cPanel
                                            border.color: root.cLine
                                            radius: 0

                                            RowLayout {
                                                anchors.fill: parent
                                                anchors.margins: 16
                                                spacing: 12

                                                // Index first, ring second -- the names are what is read.
                                                ColumnLayout {
                                                    Layout.fillWidth: true
                                                    Layout.minimumWidth: 0
                                                    Layout.alignment: Qt.AlignVCenter
                                                    spacing: 5
                                                    Repeater {
                                                        model: root.selectedApp ? root.selectedApp.languages.slice(0, 6) : []
                                                        delegate: RowLayout {
                                                            Layout.fillWidth: true
                                                            spacing: 6
                                                            Rectangle {
                                                                width: 8; height: 8; radius: 0
                                                                color: root.langColor(index)
                                                                Layout.alignment: Qt.AlignVCenter
                                                            }
                                                            Label {
                                                                text: modelData.name
                                                                color: root.cFg
                                                                font.family: root.fontHuman
                                                                font.pixelSize: 11
                                                                elide: Text.ElideRight
                                                                Layout.fillWidth: true
                                                                Layout.minimumWidth: 0
                                                            }
                                                            Label {
                                                                text: modelData.percent.toFixed(1) + "%"
                                                                color: root.cMuted
                                                                font.family: root.fontMono
                                                                font.pixelSize: 10
                                                                Layout.alignment: Qt.AlignVCenter
                                                            }
                                                        }
                                                    }
                                                }

                                                LangRing {
                                                    Layout.preferredWidth: Math.round(120 * root.uiScale())
                                                    Layout.preferredHeight: Math.round(120 * root.uiScale())
                                                    Layout.alignment: Qt.AlignVCenter
                                                    languages: root.selectedApp ? root.selectedApp.languages : []
                                                }
                                            }
                                        }
                                    }
                                }

                                // Sources — every channel this app can install from.
                                ColumnLayout {
                                    Layout.fillWidth: true
                                    spacing: 10
                                    visible: root.selectedApp && root.selectedApp.variants && root.selectedApp.variants.length > 0

                                    HudRailHeader { title: "Sources"; sub: "install channels" }

                                    Repeater {
                                        model: root.selectedApp ? root.selectedApp.variants : []
                                        delegate: Rectangle {
                                            Layout.fillWidth: true
                                            height: 60
                                            color: root.cPanel
                                            border.color: variantHover.hovered ? root.sourceAccent(modelData.source) : root.cLine
                                            radius: 0

                                            HoverHandler { id: variantHover }
                                            Behavior on border.color { ColorAnimation { duration: 140 } }

                                            RowLayout {
                                                anchors.fill: parent
                                                anchors.leftMargin: 16
                                                anchors.rightMargin: 16
                                                spacing: 14

                                                Rectangle {
                                                    width: 8; height: 8; radius: 0
                                                    color: root.sourceAccent(modelData.source)
                                                    Layout.alignment: Qt.AlignVCenter
                                                }

                                                ColumnLayout {
                                                    Layout.fillWidth: true
                                                    Layout.minimumWidth: 0
                                                    spacing: 3
                                                    Label {
                                                        text: root.sourceLabel(modelData.source) + " · " + root.formatLabel(modelData.source)
                                                              + (modelData.id === root.selectedApp.recommended_variant_id ? "  ★ recommended" : "")
                                                        color: root.cFg
                                                        font.family: root.fontHuman
                                                        font.pixelSize: 13
                                                        font.bold: true
                                                        elide: Text.ElideRight
                                                        Layout.fillWidth: true
                                                    }
                                                    Label {
                                                        text: root.commandPreview(modelData)
                                                        color: root.cMuted
                                                        font.family: root.fontMono
                                                        font.pixelSize: 10
                                                        elide: Text.ElideRight
                                                        Layout.fillWidth: true
                                                    }
                                                }

                                                Label {
                                                    text: modelData.version ? (modelData.version.charAt(0) === "v" ? modelData.version : "v" + modelData.version) : "latest"
                                                    color: root.cMuted
                                                    font.family: root.fontMono
                                                    font.pixelSize: 11
                                                    Layout.alignment: Qt.AlignVCenter
                                                }

                                                Label {
                                                    text: root.trustLabel(modelData.trust).toUpperCase()
                                                    color: root.trustColor(modelData.trust)
                                                    font.family: root.fontMono
                                                    font.pixelSize: 10
                                                    font.letterSpacing: 1
                                                    Layout.alignment: Qt.AlignVCenter
                                                }
                                            }
                                        }
                                    }
                                }

                                Item { Layout.preferredHeight: 4 }
                            }
                        }
                    }

                    Item {
                        opacity: root.activeView === "settings" ? 1 : 0
                        transform: Translate {
                            y: root.activeView === "settings" ? 0 : 12
                            Behavior on y { NumberAnimation { duration: 180; easing.type: Easing.OutCubic } }
                        }
                        Behavior on opacity { NumberAnimation { duration: 180; easing.type: Easing.OutCubic } }
                        ColumnLayout {
                            anchors.fill: parent
                            anchors.leftMargin: root.contentSideMargin()
                            anchors.rightMargin: root.contentSideMargin()
                            anchors.topMargin: root.pagePad()
                            anchors.bottomMargin: root.pagePad()
                            spacing: 14

                            ColumnLayout {
                                Layout.fillWidth: true
                                spacing: 3
                                Label {
                                    text: "Settings"
                                    color: root.cFg
                                    font.family: root.fontBrand
                                    font.pixelSize: Math.round(32 * root.uiScale())
                                    font.bold: true
                                    Layout.fillWidth: true
                                }
                                Label {
                                    text: "Storage, setup and store information."
                                    color: root.cMuted
                                    font.family: root.fontHuman
                                    font.pixelSize: 15
                                    Layout.fillWidth: true
                                }
                            }

                            ScrollView {
                                id: settingsScroll
                                Layout.fillWidth: true
                                Layout.fillHeight: true
                                clip: true
                                ScrollBar.horizontal.policy: ScrollBar.AlwaysOff
                                ScrollBar.vertical.policy: ScrollBar.AlwaysOff

                                ColumnLayout {
                                    width: settingsScroll.availableWidth
                                    spacing: 16

                                    HudRailHeader { title: "Storage" }

                                    Rectangle {
                                        Layout.fillWidth: true
                                        color: root.cPanel
                                        border.color: root.cLine
                                        radius: 0
                                        implicitHeight: 72

                                        RowLayout {
                                            anchors.fill: parent
                                            anchors.leftMargin: 18
                                            anchors.rightMargin: 18
                                            spacing: 14

                                            ColumnLayout {
                                                Layout.fillWidth: true
                                                spacing: 3
                                                Label {
                                                    text: "Icon cache"
                                                    color: root.cFg
                                                    font.family: root.fontHuman
                                                    font.pixelSize: 14
                                                    font.bold: true
                                                }
                                                Label {
                                                    text: (root.cacheInfo.iconBytes > 0 ? root.formatBytes(root.cacheInfo.iconBytes) : "0 B") + " · " + root.cacheInfo.iconCount + " files · ~/.local/share/thallium-store/icons"
                                                    color: root.cMuted
                                                    font.family: root.fontMono
                                                    font.pixelSize: 11
                                                    elide: Text.ElideRight
                                                    Layout.fillWidth: true
                                                }
                                            }

                                            ActionButton {
                                                label: "Clear cache"
                                                fill: root.cDim
                                                textColor: root.cRed
                                                Layout.preferredWidth: 118
                                                onClicked: root.request("system.clearIconCache", {})
                                            }
                                        }
                                    }

                                    HudRailHeader { title: "Setup" }

                                    Rectangle {
                                        Layout.fillWidth: true
                                        color: root.cPanel
                                        border.color: root.cLine
                                        radius: 0
                                        implicitHeight: 72

                                        RowLayout {
                                            anchors.fill: parent
                                            anchors.leftMargin: 18
                                            anchors.rightMargin: 18
                                            spacing: 14

                                            ColumnLayout {
                                                Layout.fillWidth: true
                                                spacing: 3
                                                Label {
                                                    text: "First-launch walkthrough"
                                                    color: root.cFg
                                                    font.family: root.fontHuman
                                                    font.pixelSize: 14
                                                    font.bold: true
                                                }
                                                Label {
                                                    text: "Replay the welcome tour and source overview."
                                                    color: root.cMuted
                                                    font.family: root.fontHuman
                                                    font.pixelSize: 12
                                                    Layout.fillWidth: true
                                                }
                                            }

                                            ActionButton {
                                                label: "Run again"
                                                fill: root.cDim
                                                textColor: root.cGreen
                                                Layout.preferredWidth: 118
                                                onClicked: {
                                                    root.setupStep = 0
                                                    root.setupOpen = true
                                                }
                                            }
                                        }
                                    }

                                    HudRailHeader { title: "About" }

                                    Rectangle {
                                        Layout.fillWidth: true
                                        color: root.cPanel
                                        border.color: root.cLine
                                        radius: 0
                                        implicitHeight: aboutSettingsCol.implicitHeight + 36

                                        ColumnLayout {
                                            id: aboutSettingsCol
                                            anchors.left: parent.left
                                            anchors.right: parent.right
                                            anchors.top: parent.top
                                            anchors.margins: 18
                                            spacing: 10
                                            // Version comes from the backend's own crate version.
                                            // It was a literal here and had been reading 0.1.0 for
                                            // five releases; "MVP" outlived the MVP.
                                            StatRow { name: "Version"; value: root.storeVersion || "—" }
                                            StatRow { name: "Backend"; value: "UNI " + (root.fakeUniMode ? "simulated (fake mode)" : (root.uniHealth || "connected")) }
                                            StatRow { name: "Sources"; value: "APT · Flathub · GitHub · AppImage" }
                                        }
                                    }

                                    Item { Layout.preferredHeight: 4 }
                                }
                            }
                        }
                    }

                    Item {
                        opacity: root.activeView === "installed" ? 1 : 0
                        transform: Translate {
                            y: root.activeView === "installed" ? 0 : 12
                            Behavior on y { NumberAnimation { duration: 180; easing.type: Easing.OutCubic } }
                        }
                        Behavior on opacity { NumberAnimation { duration: 180; easing.type: Easing.OutCubic } }
                        ColumnLayout {
                            anchors.fill: parent
                            anchors.leftMargin: root.contentSideMargin()
                            anchors.rightMargin: root.contentSideMargin()
                            anchors.topMargin: root.pagePad()
                            anchors.bottomMargin: root.pagePad()
                            spacing: 14

                            RowLayout {
                                Layout.fillWidth: true
                                ColumnLayout {
                                    Layout.fillWidth: true
                                    spacing: 3
                                    Label {
                                        text: "Apps"
                                        color: root.cFg
                                        font.family: root.fontBrand
                                        font.pixelSize: Math.round(32 * root.uiScale())
                                        font.bold: true
                                        Layout.fillWidth: true
                                    }
                                    Label {
                                        text: root.installedItems.length + " apps on this system"
                                              + (root.activeOpsCount() > 0 ? " · " + root.activeOpsCount() + " operation" + (root.activeOpsCount() === 1 ? "" : "s") + " running" : "") + "."
                                        color: root.cMuted
                                        font.family: root.fontHuman
                                        font.pixelSize: 15
                                        Layout.fillWidth: true
                                    }
                                }
                                ActionButton {
                                    label: "Refresh"
                                    fill: root.cDim
                                    textColor: root.cBlue
                                    Layout.preferredWidth: Math.min(120, root.actionColumnWidth())
                                    onClicked: {
                                        root.request("installed.list", {})
                                        root.request("operations.list", {})
                                    }
                                }
                            }

                            // Activity — only what is happening right now. Finished operations
                            // used to stay listed here, so the page led with a log of things the
                            // user had already watched finish, pushing the actual apps below it.
                            ColumnLayout {
                                Layout.fillWidth: true
                                spacing: 8
                                visible: root.runningOperations().length > 0

                                HudRailHeader {
                                    title: "Activity"
                                    sub: root.activeOpsCount() + " running"
                                }

                                Repeater {
                                    model: root.runningOperations()
                                    delegate: Rectangle {
                                        id: opRow
                                        property var op: modelData
                                        property bool terminal: op.state === "succeeded" || op.state === "failed" || op.state === "cancelled"
                                        property color stateColor: op.state === "succeeded" ? root.cGreenSoft : op.state === "failed" ? root.cRed : root.cGreen

                                        Layout.fillWidth: true
                                        height: 54
                                        color: root.cPanel
                                        border.color: root.cLine
                                        radius: 0

                                        RowLayout {
                                            anchors.fill: parent
                                            anchors.leftMargin: 16
                                            anchors.rightMargin: 12
                                            spacing: 14

                                            Rectangle {
                                                width: 8; height: 8; radius: 0
                                                color: opRow.stateColor
                                                Layout.alignment: Qt.AlignVCenter
                                            }

                                            Label {
                                                text: opRow.op.app_name || "Unknown"
                                                color: root.cFg
                                                font.family: root.fontHuman
                                                font.pixelSize: 13
                                                font.bold: true
                                                elide: Text.ElideRight
                                                Layout.preferredWidth: 220
                                            }

                                            Label {
                                                text: opRow.op.action + " · " + root.sourceLabel(root.operationSource(opRow.op.source)) + " · " + opRow.op.state
                                                color: opRow.op.state === "failed" ? root.cRed : root.cMuted
                                                font.family: root.fontMono
                                                font.pixelSize: 10
                                                font.letterSpacing: 1
                                                elide: Text.ElideRight
                                                Layout.fillWidth: true
                                            }

                                            ProgressBar {
                                                visible: !opRow.terminal
                                                from: 0
                                                to: 100
                                                value: opRow.op.percent
                                                Layout.preferredWidth: 160
                                            }

                                            ActionButton {
                                                label: root.operationActionLabel(opRow.op)
                                                fill: opRow.op.state === "failed" ? root.cRed : root.cDim
                                                textColor: opRow.op.state === "failed" ? "white" : opRow.terminal ? root.cMuted : root.cFg
                                                active: opRow.op.state === "failed" || !opRow.terminal
                                                height: 32
                                                Layout.preferredWidth: 96
                                                onClicked: {
                                                    if (opRow.op.state === "failed")
                                                        root.retryOperation(opRow.op)
                                                    else if (!opRow.terminal)
                                                        root.cancelOperation(opRow.op)
                                                }
                                            }
                                        }
                                    }
                                }
                            }

                            Rectangle {
                                Layout.fillWidth: true
                                Layout.fillHeight: true
                                color: root.cPanel
                                border.color: root.cLine
                                radius: 0
                                visible: root.installedItems.length === 0
                                ColumnLayout {
                                    anchors.centerIn: parent
                                    width: Math.min(parent.width - 48, 560)
                                    spacing: 8
                                    Label {
                                        text: "No installed applications detected yet."
                                        color: root.cFg
                                        font.family: root.fontHuman
                                        font.pixelSize: 18
                                        horizontalAlignment: Text.AlignHCenter
                                        wrapMode: Text.WordWrap
                                        Layout.fillWidth: true
                                    }
                                    Label {
                                        text: "The MVP reads UNI, Flatpak, AppImage, GitHub, and selected dpkg state."
                                        color: root.cMuted
                                        font.family: root.fontHuman
                                        font.pixelSize: 14
                                        horizontalAlignment: Text.AlignHCenter
                                        wrapMode: Text.WordWrap
                                        Layout.fillWidth: true
                                    }
                                }
                            }

                            // Updates, folded into this page. They were a separate tab, which
                            // meant the answer to "what is on this machine and is any of it out
                            // of date" lived on two screens. Hidden entirely when nothing is
                            // pending, so the page stays about installed apps.
                            ColumnLayout {
                                Layout.fillWidth: true
                                // Height comes from the grid below, which is sized to whole
                                // rows. Setting it here meant guessing the header's height and
                                // being wrong by exactly enough to slice the next row of cards
                                // in half.
                                spacing: 8
                                visible: root.updateItems.length > 0

                                RowLayout {
                                    Layout.fillWidth: true
                                    spacing: 10

                                    HudRailHeader {
                                        Layout.fillWidth: true
                                        title: "Updates"
                                        sub: root.updateItems.length + " available"
                                    }

                                    // Two rows is a deliberate cap so the section does not
                                    // bury the installed apps, but "7 available" above four
                                    // visible cards, with nothing to say the rest exist, is
                                    // just wrong. This says how many are hidden and shows them.
                                    Label {
                                        visible: root.updateItems.length > updatesGrid.columns * 2
                                        text: root.updatesExpanded
                                              ? "SHOW LESS"
                                              : "SHOW ALL " + root.updateItems.length
                                        color: updatesToggle.hovered ? root.cFg : root.cGreen
                                        font.family: root.fontMono
                                        font.pixelSize: 10
                                        font.letterSpacing: 2
                                        Layout.alignment: Qt.AlignVCenter
                                        HoverHandler { id: updatesToggle }
                                        MouseArea {
                                            anchors.fill: parent
                                            anchors.margins: -8
                                            cursorShape: Qt.PointingHandCursor
                                            onClicked: root.updatesExpanded = !root.updatesExpanded
                                        }
                                    }
                                }

                            ScrollView {
                                Layout.fillWidth: true
                                // Exactly two rows of cards, scrolled for the rest. Never a
                                // partial row -- a card cut through the middle reads as a
                                // rendering fault, not as "there is more below".
                                Layout.preferredHeight: Math.round(142 * root.uiScale())
                                    * (root.updatesExpanded
                                       ? Math.ceil(root.updateItems.length / updatesGrid.columns)
                                       : Math.min(2, Math.ceil(root.updateItems.length / updatesGrid.columns)))
                                Behavior on Layout.preferredHeight { NumberAnimation { duration: root.tMed; easing.type: Easing.OutCubic } }
                                clip: true
                                visible: root.updateItems.length > 0

                                GridView {
                                    id: updatesGrid
                                    readonly property int columns: Math.max(1, Math.floor(width / 320))
                                    width: parent.width
                                    height: parent.height
                                    model: root.updateItems
                                    cellWidth: Math.floor(width / columns)
                                    cellHeight: Math.round(142 * root.uiScale())
                                    delegate: Rectangle {
                                        id: updateCard
                                        property var item: modelData
                                        property string itemSource: root.operationSource(item.source)

                                        width: updatesGrid.cellWidth - 14
                                        height: Math.round(126 * root.uiScale())
                                        x: 7
                                        y: 4
                                        color: root.cPanel
                                        border.color: updateHover.hovered ? "#5f7048" : root.cLine
                                        radius: 0
                                        opacity: 0
                                        transform: Translate { id: updateSlide; y: 8 }
                                        clip: true

                                        Component.onCompleted: {
                                            updateFade.start()
                                            updateLift.start()
                                        }

                                        HoverHandler { id: updateHover }
                                        Behavior on border.color { ColorAnimation { duration: 140 } }

                                        NumberAnimation {
                                            id: updateFade
                                            target: updateCard
                                            property: "opacity"
                                            from: 0
                                            to: 1
                                            duration: 180
                                            easing.type: Easing.OutCubic
                                        }

                                        NumberAnimation {
                                            id: updateLift
                                            target: updateSlide
                                            property: "y"
                                            from: 8
                                            to: 0
                                            duration: 180
                                            easing.type: Easing.OutCubic
                                        }

                                        Rectangle {
                                            width: parent.width
                                            height: parent.height
                                            opacity: updateHover.hovered ? 0.16 : 0.08
                                            gradient: Gradient {
                                                orientation: Gradient.Horizontal
                                                GradientStop { position: 0.0; color: root.sourceSurface(updateCard.itemSource) }
                                                GradientStop { position: 1.0; color: "transparent" }
                                            }
                                        }

                                        MouseArea {
                                            anchors.fill: parent
                                            cursorShape: Qt.PointingHandCursor
                                            acceptedButtons: Qt.LeftButton
                                            onClicked: root.selectApp(root.appFromInstalled(updateCard.item))
                                        }

                                        RowLayout {
                                            anchors.fill: parent
                                            anchors.margins: 14
                                            spacing: 12

                                            Rectangle {
                                                Layout.preferredWidth: Math.round(72 * root.uiScale())
                                                Layout.preferredHeight: Math.round(72 * root.uiScale())
                                                radius: 0
                                                color: root.sourceSurface(updateCard.itemSource)
                                                border.color: root.sourceAccent(updateCard.itemSource)
                                                Label {
                                                    anchors.centerIn: parent
                                                    text: updateCard.item.name ? updateCard.item.name.substring(0, 1).toUpperCase() : "U"
                                                    color: root.sourceAccent(updateCard.itemSource)
                                                    font.family: root.fontBrand
                                                    font.pixelSize: Math.round(30 * root.uiScale())
                                                    font.bold: true
                                                }
                                            }

                                            ColumnLayout {
                                                Layout.fillWidth: true
                                                Layout.minimumWidth: 0
                                                Layout.alignment: Qt.AlignVCenter
                                                spacing: 5
                                                Label {
                                                    text: updateCard.item.name || "Unknown update"
                                                    color: root.cFg
                                                    font.family: root.fontHuman
                                                    font.pixelSize: Math.round(15 * root.uiScale())
                                                    font.bold: true
                                                    Layout.fillWidth: true
                                                    elide: Text.ElideRight
                                                }
                                                Label {
                                                    text: (updateCard.item.source ? root.sourceLabel(root.operationSource(updateCard.item.source)) : "Source")
                                                          + " · " + (updateCard.item.current_version || "installed")
                                                          + " → " + (updateCard.item.available_version || "available")
                                                    color: root.cMuted
                                                    font.family: root.fontHuman
                                                    font.pixelSize: 11
                                                    Layout.fillWidth: true
                                                    elide: Text.ElideRight
                                                }
                                                Label {
                                                    text: updateCard.item.detail || "Update metadata from UNI"
                                                    color: root.cMuted
                                                    font.family: root.fontHuman
                                                    font.pixelSize: 11
                                                    Layout.fillWidth: true
                                                    elide: Text.ElideRight
                                                }
                                            }

                                            ColumnLayout {
                                                Layout.preferredWidth: 92
                                                Layout.maximumWidth: 92
                                                Layout.alignment: Qt.AlignRight | Qt.AlignVCenter
                                                spacing: 8
                                                ActionButton {
                                                    label: "Details"
                                                    fill: root.cDim
                                                    textColor: root.cBlue
                                                    Layout.fillWidth: true
                                                    onClicked: root.selectApp(root.appFromInstalled(updateCard.item))
                                                }
                                                ActionButton {
                                                    label: "Update"
                                                    fill: root.cBlue
                                                    textColor: "white"
                                                    Layout.fillWidth: true
                                                    onClicked: {
                                                        root.request("operations.enqueue", {
                                                            app_id: updateCard.item.id || updateCard.item.name,
                                                            variant_id: updateCard.item.variant_id || updateCard.item.id || updateCard.item.name,
                                                            action: "update",
                                                            app_name: updateCard.item.name,
                                                            package_id: root.packageIdFromInstalled(updateCard.item),
                                                            source: root.operationSource(updateCard.item.source)
                                                        })
                                                        root.activeView = "installed"
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            }

                            ScrollView {
                                Layout.fillWidth: true
                                Layout.fillHeight: true
                                clip: true
                                visible: root.installedItems.length > 0

                                GridView {
                                    id: installedGrid
                                    width: parent.width
                                    height: parent.height
                                    model: root.installedItems
                                    cellWidth: {
                                        const columns = Math.max(1, Math.floor(width / 320))
                                        return Math.floor(width / columns)
                                    }
                                    cellHeight: Math.round(142 * root.uiScale())
                                    delegate: Rectangle {
                                        id: installedCard
                                        property var item: modelData
                                        property string packageId: root.packageIdFromInstalled(item)
                                        property string itemSource: root.operationSource(item.source)

                                        width: installedGrid.cellWidth - 14
                                        height: Math.round(126 * root.uiScale())
                                        x: 7
                                        y: 4
                                        color: root.cPanel
                                        border.color: installedHover.hovered ? "#5f7048" : root.cLine
                                        radius: 0
                                        opacity: 0
                                        transform: Translate { id: installedSlide; y: 8 }
                                        clip: true

                                        Component.onCompleted: {
                                            installedFade.start()
                                            installedLift.start()
                                        }

                                        HoverHandler { id: installedHover }
                                        Behavior on border.color { ColorAnimation { duration: 140 } }

                                        NumberAnimation {
                                            id: installedFade
                                            target: installedCard
                                            property: "opacity"
                                            from: 0
                                            to: 1
                                            duration: 180
                                            easing.type: Easing.OutCubic
                                        }

                                        NumberAnimation {
                                            id: installedLift
                                            target: installedSlide
                                            property: "y"
                                            from: 8
                                            to: 0
                                            duration: 180
                                            easing.type: Easing.OutCubic
                                        }

                                        Rectangle {
                                            width: parent.width
                                            height: parent.height
                                            opacity: installedHover.hovered ? 0.16 : 0.08
                                            gradient: Gradient {
                                                orientation: Gradient.Horizontal
                                                GradientStop { position: 0.0; color: installedCard.item.managedByUni ? root.sourceSurface(installedCard.itemSource) : "#332b1a" }
                                                GradientStop { position: 1.0; color: "transparent" }
                                            }
                                        }

                                        MouseArea {
                                            anchors.fill: parent
                                            cursorShape: Qt.PointingHandCursor
                                            acceptedButtons: Qt.LeftButton
                                            onClicked: root.selectApp(root.appFromInstalled(installedCard.item))
                                        }

                                        RowLayout {
                                            anchors.fill: parent
                                            anchors.margins: 14
                                            spacing: 12

                                            Rectangle {
                                                Layout.preferredWidth: Math.round(72 * root.uiScale())
                                                Layout.preferredHeight: Math.round(72 * root.uiScale())
                                                radius: 0
                                                color: installedCard.item.managedByUni ? root.sourceSurface(installedCard.itemSource) : "#332b1a"
                                                border.color: installedCard.item.managedByUni ? root.sourceAccent(installedCard.itemSource) : "#5f4f2a"
                                                clip: true
                                                // Real icon when the machine has one, initial as the
                                                // fallback. Installed cards never even tried to load an
                                                // icon, so a page about apps you already have was a wall
                                                // of letters -- the one place the icon is guaranteed to
                                                // exist locally.
                                                Label {
                                                    anchors.centerIn: parent
                                                    visible: installedIcon.status !== Image.Ready
                                                    text: installedCard.item.name.substring(0, 1).toUpperCase()
                                                    color: installedCard.item.managedByUni ? root.sourceAccent(installedCard.itemSource) : root.cWarn
                                                    font.family: root.fontBrand
                                                    font.pixelSize: Math.round(30 * root.uiScale())
                                                    font.bold: true
                                                }
                                                Image {
                                                    id: installedIcon
                                                    anchors.fill: parent
                                                    anchors.margins: 8
                                                    source: root.installedIconSource(installedCard.item)
                                                    fillMode: Image.PreserveAspectFit
                                                    asynchronous: true
                                                    cache: true
                                                    smooth: true
                                                    visible: status === Image.Ready
                                                }
                                            }

                                            ColumnLayout {
                                                Layout.fillWidth: true
                                                Layout.minimumWidth: 0
                                                Layout.alignment: Qt.AlignVCenter
                                                spacing: 5
                                                Label {
                                                    text: installedCard.item.name
                                                    color: root.cFg
                                                    font.family: root.fontHuman
                                                    font.pixelSize: Math.round(15 * root.uiScale())
                                                    font.bold: true
                                                    Layout.fillWidth: true
                                                    elide: Text.ElideRight
                                                }
                                                Label {
                                                    text: root.sourceLabel(root.operationSource(installedCard.item.source))
                                                          + (installedCard.item.version ? " " + installedCard.item.version : "")
                                                          + " · " + installedCard.packageId
                                                    color: root.cMuted
                                                    font.family: root.fontHuman
                                                    font.pixelSize: 11
                                                    Layout.fillWidth: true
                                                    elide: Text.ElideRight
                                                }
                                                Label {
                                                    text: installedCard.item.detail
                                                    color: root.cMuted
                                                    font.family: root.fontHuman
                                                    font.pixelSize: 11
                                                    Layout.fillWidth: true
                                                    elide: Text.ElideRight
                                                }
                                                Flow {
                                                    Layout.fillWidth: true
                                                    spacing: 8
                                                    Rectangle {
                                                        height: 22
                                                        implicitWidth: sourceChipInstalled.implicitWidth + 14
                                                        radius: 0
                                                        color: root.cDim
                                                        border.color: root.cLine
                                                        Label {
                                                            id: sourceChipInstalled
                                                            anchors.centerIn: parent
                                                            text: root.sourceLabel(root.operationSource(installedCard.item.source))
                                                            color: root.cMuted
                                                            font.family: root.fontHuman
                                                            font.pixelSize: 10
                                                            font.bold: true
                                                        }
                                                    }
                                                    Rectangle {
                                                        height: 22
                                                        implicitWidth: managedChipInstalled.implicitWidth + 14
                                                        radius: 0
                                                        color: installedCard.item.managedByUni ? "#233024" : "#332b1a"
                                                        border.color: installedCard.item.managedByUni ? "#4a5f3f" : "#5f4f2a"
                                                        Label {
                                                            id: managedChipInstalled
                                                            anchors.centerIn: parent
                                                            text: installedCard.item.managedByUni ? "Managed by UNI" : "Detected"
                                                            color: installedCard.item.managedByUni ? root.cGreen : root.cWarn
                                                            font.family: root.fontHuman
                                                            font.pixelSize: 10
                                                            font.bold: true
                                                        }
                                                    }
                                                }
                                            }

                                            ColumnLayout {
                                                Layout.preferredWidth: 92
                                                Layout.maximumWidth: 92
                                                Layout.alignment: Qt.AlignRight | Qt.AlignVCenter
                                                spacing: 8
                                                ActionButton {
                                                    label: "Details"
                                                    fill: root.cDim
                                                    textColor: root.cBlue
                                                    Layout.fillWidth: true
                                                    onClicked: root.selectApp(root.appFromInstalled(installedCard.item))
                                                }
                                                ActionButton {
                                                    label: "Uninstall"
                                                    fill: root.cRed
                                                    textColor: "white"
                                                    Layout.fillWidth: true
                                                    onClicked: root.enqueueUninstallInstalled(installedCard.item)
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                }

            }

            // Screenshot viewer. Gallery thumbnails are cropped to a fixed
            // aspect, so the only way to see a whole screenshot is to open it.
            // Click anywhere or press Escape to close.
            Rectangle {
                anchors.fill: parent
                visible: root.viewerSource !== ""
                color: "#e8060607"
                z: 940

                MouseArea {
                    anchors.fill: parent
                    onClicked: root.viewerSource = ""
                }

                Image {
                    anchors.centerIn: parent
                    width: Math.min(parent.width - 80, sourceSize.width > 0 ? sourceSize.width : parent.width - 80)
                    height: Math.min(parent.height - 120, sourceSize.height > 0 ? sourceSize.height : parent.height - 120)
                    source: root.viewerSource
                    fillMode: Image.PreserveAspectFit
                    asynchronous: true
                    cache: true
                    smooth: true
                }

                Label {
                    anchors.horizontalCenter: parent.horizontalCenter
                    anchors.bottom: parent.bottom
                    anchors.bottomMargin: 34
                    text: "CLICK ANYWHERE TO CLOSE"
                    color: root.cMuted
                    font.family: root.fontMono
                    font.pixelSize: 10
                    font.letterSpacing: 2
                }

                Keys.onEscapePressed: root.viewerSource = ""
                focus: root.viewerSource !== ""
            }

            // First-launch setup — three-step brand walkthrough, shown once.
            Rectangle {
                anchors.fill: parent
                visible: root.setupOpen
                color: "#f0060607"
                z: 950

                MouseArea { anchors.fill: parent }

                Rectangle {
                    anchors.centerIn: parent
                    width: 580
                    height: 440
                    color: root.cPanel
                    border.color: root.cGreen
                    radius: 0

                    ColumnLayout {
                        anchors.fill: parent
                        anchors.margins: 30
                        spacing: 14

                        StackLayout {
                            Layout.fillWidth: true
                            Layout.fillHeight: true
                            currentIndex: root.setupStep

                            // Step 0 — welcome.
                            ColumnLayout {
                                spacing: 12
                                Item { Layout.fillHeight: true }
                                ThalliumLoader {
                                    Layout.alignment: Qt.AlignHCenter
                                    Layout.preferredWidth: 104
                                    Layout.preferredHeight: 104
                                    running: root.setupOpen && root.setupStep === 0
                                }
                                Label {
                                    text: "WELCOME TO"
                                    color: root.cGreen
                                    font.family: root.fontMono
                                    font.pixelSize: 11
                                    font.letterSpacing: 5
                                    Layout.alignment: Qt.AlignHCenter
                                }
                                Label {
                                    text: "Thallium Store"
                                    color: root.cFg
                                    font.family: root.fontBrand
                                    font.pixelSize: 27
                                    font.bold: true
                                    Layout.alignment: Qt.AlignHCenter
                                }
                                Label {
                                    text: "One store for apt, Flathub, GitHub releases and AppImages — merged, ranked by trust, installed through UNI."
                                    color: root.cMuted
                                    font.family: root.fontHuman
                                    font.pixelSize: 13
                                    wrapMode: Text.WordWrap
                                    horizontalAlignment: Text.AlignHCenter
                                    Layout.fillWidth: true
                                }
                                Item { Layout.fillHeight: true }
                            }

                            // Step 1 — sources and trust.
                            ColumnLayout {
                                spacing: 10
                                Label {
                                    text: "SOURCES & TRUST"
                                    color: root.cFg
                                    font.family: root.fontMono
                                    font.pixelSize: 13
                                    font.bold: true
                                    font.letterSpacing: 3
                                }
                                Label {
                                    text: "Every app can come from several channels. Thallium ranks them and recommends the safest."
                                    color: root.cMuted
                                    font.family: root.fontHuman
                                    font.pixelSize: 12
                                    wrapMode: Text.WordWrap
                                    Layout.fillWidth: true
                                }
                                Repeater {
                                    model: [
                                        { s: "flathub",  n: "Flathub",  d: "Sandboxed flatpaks — the default recommendation." },
                                        { s: "system",   n: "System",   d: "Native .deb packages with full system access." },
                                        { s: "github",   n: "GitHub",   d: "Curated release binaries from upstream repos." },
                                        { s: "appimage", n: "AppImage", d: "Portable single-file apps." }
                                    ]
                                    delegate: RowLayout {
                                        Layout.fillWidth: true
                                        spacing: 12
                                        Rectangle {
                                            width: 10; height: 10; radius: 0
                                            color: root.sourceAccent(modelData.s)
                                            Layout.alignment: Qt.AlignVCenter
                                        }
                                        Label {
                                            text: modelData.n
                                            color: root.cFg
                                            font.family: root.fontHuman
                                            font.pixelSize: 13
                                            font.bold: true
                                            Layout.preferredWidth: 92
                                        }
                                        Label {
                                            text: modelData.d
                                            color: root.cMuted
                                            font.family: root.fontHuman
                                            font.pixelSize: 12
                                            wrapMode: Text.WordWrap
                                            Layout.fillWidth: true
                                        }
                                    }
                                }
                                Item { Layout.fillHeight: true }
                            }

                            // Step 2 — cache and privacy.
                            ColumnLayout {
                                spacing: 10
                                Label {
                                    text: "FAST & LOCAL"
                                    color: root.cFg
                                    font.family: root.fontMono
                                    font.pixelSize: 13
                                    font.bold: true
                                    font.letterSpacing: 3
                                }
                                Repeater {
                                    model: [
                                        "App icons are cached on disk after first load, so the store paints instantly offline.",
                                        "Detail pages are enriched from Flathub and GitHub on demand, then cached in the daemon.",
                                        "Network is only touched for search, artwork and metadata — never in the background without you.",
                                        "Storage and cache controls live in Settings."
                                    ]
                                    delegate: RowLayout {
                                        Layout.fillWidth: true
                                        spacing: 10
                                        Rectangle {
                                            width: root.tickW; height: 12; color: root.cGreen
                                            Layout.alignment: Qt.AlignTop
                                            Layout.topMargin: 3
                                        }
                                        Label {
                                            text: modelData
                                            color: root.cMuted
                                            font.family: root.fontHuman
                                            font.pixelSize: 13
                                            wrapMode: Text.WordWrap
                                            Layout.fillWidth: true
                                        }
                                    }
                                }
                                Item { Layout.fillHeight: true }
                            }
                        }

                        RowLayout {
                            Layout.fillWidth: true
                            spacing: 10

                            Row {
                                spacing: 6
                                Layout.alignment: Qt.AlignVCenter
                                Repeater {
                                    model: 3
                                    delegate: Rectangle {
                                        width: root.setupStep === index ? 16 : 6
                                        height: 5
                                        color: root.setupStep === index ? root.cGreen : root.cLine
                                        Behavior on width { NumberAnimation { duration: root.tFast } }
                                    }
                                }
                            }

                            Item { Layout.fillWidth: true }

                            ActionButton {
                                visible: root.setupStep < 2
                                label: "Skip"
                                fill: root.cDim
                                textColor: root.cMuted
                                Layout.preferredWidth: 84
                                onClicked: root.finishSetup()
                            }
                            ActionButton {
                                label: root.setupStep < 2 ? "Continue" : "Get started"
                                fill: root.cGreen
                                textColor: root.cBase
                                Layout.preferredWidth: 128
                                onClicked: {
                                    if (root.setupStep < 2)
                                        root.setupStep = root.setupStep + 1
                                    else
                                        root.finishSetup()
                                }
                            }
                        }
                    }
                }
            }

            // Details loading takeover — whole screen goes dark, the brand
            // logo draws itself, then the finished page fades back in.
            Rectangle {
                id: detailsTakeover
                anchors.fill: parent
                color: root.cBase
                z: 850
                opacity: root.requestRunning && root.activeMethod === "catalog.appDetails" ? 1 : 0
                visible: opacity > 0
                Behavior on opacity { NumberAnimation { duration: root.tMed; easing.type: Easing.OutCubic } }

                MouseArea { anchors.fill: parent }

                ColumnLayout {
                    anchors.centerIn: parent
                    spacing: 20

                    ThalliumLoader {
                        Layout.alignment: Qt.AlignHCenter
                        Layout.preferredWidth: 110
                        Layout.preferredHeight: 110
                        running: detailsTakeover.visible
                    }
                    Label {
                        text: root.selectedApp ? root.selectedApp.name.toUpperCase() : ""
                        color: root.cMuted
                        font.family: root.fontMono
                        font.pixelSize: 11
                        font.letterSpacing: 4
                        Layout.alignment: Qt.AlignHCenter
                    }
                }
            }

            // Boot loading screen — brand logo draws itself until the
            // Discover feed arrives, then fades out.
            Rectangle {
                anchors.fill: parent
                color: root.cBase
                z: 800
                opacity: root.discover.length === 0 ? 1 : 0
                visible: opacity > 0
                Behavior on opacity { NumberAnimation { duration: root.tSlow; easing.type: Easing.OutCubic } }

                ColumnLayout {
                    anchors.centerIn: parent
                    spacing: 22

                    ThalliumLoader {
                        Layout.alignment: Qt.AlignHCenter
                        Layout.preferredWidth: 132
                        Layout.preferredHeight: 132
                        running: root.discover.length === 0
                    }
                    Label {
                        text: "THALLIUM STORE"
                        color: root.cFg
                        font.family: root.fontMono
                        font.pixelSize: 13
                        font.bold: true
                        font.letterSpacing: 6
                        Layout.alignment: Qt.AlignHCenter
                    }
                    Label {
                        text: root.status.toUpperCase()
                        color: root.cMuted
                        font.family: root.fontMono
                        font.pixelSize: 10
                        font.letterSpacing: 2
                        Layout.alignment: Qt.AlignHCenter
                    }
                }
            }

            // Install picker overlay — choose source + version, Modrinth-style.
            Rectangle {
                id: installOverlay
                anchors.fill: parent
                visible: root.installOpen
                color: "#cc07090a"
                z: 900

                MouseArea {
                    anchors.fill: parent
                    onClicked: root.installOpen = false
                }

                Rectangle {
                    anchors.centerIn: parent
                    width: 468
                    height: Math.min(560, bodyCol.implicitHeight + 32)
                    color: root.cPanel
                    border.color: root.cLine
                    radius: 0

                    // swallow clicks so backdrop doesn't close
                    MouseArea { anchors.fill: parent }

                    ColumnLayout {
                        id: bodyCol
                        anchors.fill: parent
                        anchors.margins: 16
                        spacing: 14

                        // Header rail.
                        RowLayout {
                            Layout.fillWidth: true
                            spacing: 8
                            Rectangle { width: root.tickW; height: 15; radius: 0; color: root.cGreen; Layout.alignment: Qt.AlignVCenter }
                            Label {
                                text: "INSTALL"
                                color: root.cFg
                                font.family: root.fontBrand
                                font.pixelSize: 14
                                font.bold: true
                                font.letterSpacing: 1
                            }
                            Label {
                                text: root.installApp ? root.installApp.name : ""
                                color: root.cMuted
                                font.family: root.fontMono
                                font.pixelSize: 12
                                elide: Text.ElideRight
                                Layout.fillWidth: true
                            }
                        }

                        Label {
                            text: "Choose a source and version"
                            color: root.cMuted
                            font.family: root.fontHuman
                            font.pixelSize: 12
                            Layout.fillWidth: true
                        }

                        // Variant list.
                        ListView {
                            id: variantList
                            Layout.fillWidth: true
                            Layout.preferredHeight: Math.min(300, contentHeight)
                            clip: true
                            spacing: 8
                            model: root.installApp ? root.installApp.variants : []
                            delegate: Rectangle {
                                width: variantList.width
                                height: 62
                                property bool selected: modelData.id === root.installVariantId
                                color: selected ? root.cBlueSoft : root.cDim
                                border.color: selected ? root.cGreen : root.cLine
                                radius: 0

                                MouseArea {
                                    anchors.fill: parent
                                    onClicked: root.installVariantId = modelData.id
                                }

                                RowLayout {
                                    anchors.fill: parent
                                    anchors.leftMargin: 12
                                    anchors.rightMargin: 12
                                    spacing: 12

                                    // Radio indicator.
                                    Rectangle {
                                        Layout.preferredWidth: 14
                                        Layout.preferredHeight: 14
                                        Layout.alignment: Qt.AlignVCenter
                                        radius: 0
                                        color: "transparent"
                                        border.color: parent.parent.selected ? root.cGreen : root.cLine
                                        Rectangle {
                                            anchors.centerIn: parent
                                            width: 7; height: 7; radius: 0
                                            color: root.cGreen
                                            visible: parent.parent.parent.selected
                                        }
                                    }

                                    ColumnLayout {
                                        Layout.fillWidth: true
                                        spacing: 2
                                        Label {
                                            text: root.sourceLabel(modelData.source) + "  ·  " + root.formatLabel(modelData.source)
                                            color: root.cFg
                                            font.family: root.fontHuman
                                            font.pixelSize: 13
                                            font.bold: true
                                            elide: Text.ElideRight
                                            Layout.fillWidth: true
                                        }
                                        Label {
                                            text: (modelData.version ? "v" + modelData.version : "latest")
                                                  + "  ·  " + (modelData.download_size > 0 ? root.formatBytes(modelData.download_size) : "size n/a")
                                            color: root.cMuted
                                            font.family: root.fontMono
                                            font.pixelSize: 11
                                            elide: Text.ElideRight
                                            Layout.fillWidth: true
                                        }
                                    }

                                    Label {
                                        text: root.trustLabel(modelData.trust)
                                        color: root.trustColor(modelData.trust)
                                        font.family: root.fontMono
                                        font.pixelSize: 10
                                        font.letterSpacing: 1
                                        Layout.alignment: Qt.AlignVCenter
                                    }
                                }
                            }
                        }

                        Label {
                            text: "You'll be asked for your password to install."
                            color: root.cMuted
                            font.family: root.fontHuman
                            font.pixelSize: 11
                            Layout.fillWidth: true
                        }

                        // Footer actions.
                        RowLayout {
                            Layout.fillWidth: true
                            spacing: 10
                            Item { Layout.fillWidth: true }
                            ActionButton {
                                label: "Cancel"
                                fill: root.cPanel
                                textColor: root.cFg
                                Layout.preferredWidth: 100
                                onClicked: root.installOpen = false
                            }
                            ActionButton {
                                label: "Install"
                                fill: root.cGreen
                                textColor: root.cBase
                                active: root.installVariantId !== ""
                                Layout.preferredWidth: 128
                                onClicked: root.confirmInstall()
                            }
                        }
                    }
                }
            }
        }
    }
}
