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
    property bool fakeUniMode: true
    property string uniHealth: ""
    property int activityFrame: 0
    property var searchCache: ({})
    property string activeSearchQuery: ""
    readonly property int searchCacheTtlMs: 60000
    property bool sidebarCollapsed: false
    property bool searchCommitted: false
    property var storeLog: []

    function pushLog(msg) {
        const stamp = Qt.formatDateTime(new Date(), "HH:mm:ss")
        const next = [stamp + "  " + msg].concat(root.storeLog)
        root.storeLog = next.slice(0, 120)
    }

    // Thallium 81 design tokens — Everforest palette, retro sci-fi HUD grown
    // out of Soviet brutalism. Green is THE accent; blue is retired.
    readonly property color cBase: "#0d0d0f"      // void: page, deepest bg
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
        const w = window && window.width ? window.width : 1180
        return Math.max(0.86, Math.min(1.14, w / 1180))
    }

    function pagePad() {
        return Math.round(28 * uiScale())
    }

    function actionColumnWidth() {
        const w = window && window.width ? window.width : 1180
        return Math.max(154, Math.min(230, Math.round((w - 238) * 0.22)))
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
        activeView = "search"
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
        activeView = "queue"
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
        activeView = "queue"
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
        activeView = "queue"
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
        activeView = "queue"
    }

    function cancelOperation(operation) {
        if (!operation)
            return
        request("operations.cancel", {
            operationId: operation.id
        })
        activeView = "queue"
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
                        root.operations = result.items
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
                    } else if (result.status !== undefined) {
                        root.fakeUniMode = result.fakeUni === true
                        root.uniHealth = result.uni || ""
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
            if (root.activeView === "queue")
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
            root.loadDiscover()
            discoverWarmReload.start()
        }
    }

    Component.onCompleted: {
        initialLoadDelay.start()
    }

    component NavButton: Rectangle {
        id: nav
        property string label: ""
        property string view: ""
        property string mark: "·"
        property bool hovered: false
        readonly property bool selected: root.activeView === view || (view === "discover" && root.activeView === "details")

        Layout.fillWidth: true
        height: 44
        color: selected ? root.cBlueSoft : hovered ? root.cDim : "transparent"
        border.color: selected ? root.cGreen : "transparent"

        Behavior on color { ColorAnimation { duration: 140 } }

        RowLayout {
            anchors.fill: parent
            anchors.leftMargin: root.sidebarCollapsed ? 0 : 14
            anchors.rightMargin: root.sidebarCollapsed ? 0 : 14
            spacing: 12
            Label {
                text: nav.mark
                color: nav.selected ? root.cGreen : root.cMuted
                font.family: root.fontMono
                font.pixelSize: 17
                Layout.preferredWidth: root.sidebarCollapsed ? -1 : 24
                Layout.fillWidth: root.sidebarCollapsed
                horizontalAlignment: Text.AlignHCenter
            }
            Label {
                visible: !root.sidebarCollapsed
                text: nav.label
                color: nav.selected ? root.cFg : root.cMuted
                font.family: root.fontHuman
                font.pixelSize: 14
                font.bold: nav.selected
                Layout.fillWidth: true
            }
        }

        MouseArea {
            anchors.fill: parent
            hoverEnabled: true
            cursorShape: Qt.PointingHandCursor
            onEntered: nav.hovered = true
            onExited: nav.hovered = false
            onClicked: {
                root.activeView = nav.view
                if (nav.view === "search")
                    searchField.forceActiveFocus()
                if (nav.view === "queue")
                    root.request("operations.list", {})
                if (nav.view === "installed")
                    root.request("installed.list", {})
                if (nav.view === "updates")
                    root.request("updates.list", {})
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

    // Machine-voice section header: green tick + mono uppercase label.
    component HudRailHeader: RowLayout {
        property string title: ""
        property string sub: ""
        spacing: 10
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
        implicitWidth: 1180
        implicitHeight: 760
        visible: true
        title: "Thallium Store"
        color: root.cBase

        Rectangle {
            anchors.fill: parent
            color: root.cBase

            RowLayout {
                anchors.fill: parent
                spacing: 0

                Rectangle {
                    id: sidebar
                    Layout.preferredWidth: root.sidebarCollapsed ? 64 : 246
                    Layout.fillHeight: true
                    color: root.cPanel
                    border.color: root.cLine
                    clip: true
                    Behavior on Layout.preferredWidth { NumberAnimation { duration: root.tMed; easing.type: Easing.OutCubic } }

                    ColumnLayout {
                        anchors.fill: parent
                        anchors.margins: root.sidebarCollapsed ? 11 : 18
                        spacing: 14

                        RowLayout {
                            Layout.fillWidth: true
                            spacing: 12

                            Rectangle {
                                Layout.preferredWidth: 42
                                Layout.preferredHeight: 42
                                color: root.cGreen

                                Label {
                                    anchors.centerIn: parent
                                    text: "T"
                                    color: root.cBase
                                    font.family: root.fontBrand
                                    font.pixelSize: 21
                                    font.bold: true
                                }

                                MouseArea {
                                    anchors.fill: parent
                                    cursorShape: Qt.PointingHandCursor
                                    onClicked: root.sidebarCollapsed = !root.sidebarCollapsed
                                }
                            }

                            ColumnLayout {
                                Layout.fillWidth: true
                                spacing: 0
                                visible: !root.sidebarCollapsed

                                Label {
                                    text: "Thallium"
                                    color: root.cFg
                                    font.family: root.fontBrand
                                    font.pixelSize: 22
                                    font.bold: true
                                    Layout.fillWidth: true
                                }

                                Label {
                                    text: "Store"
                                    color: root.cMuted
                                    font.family: root.fontHuman
                                    font.pixelSize: 13
                                    Layout.fillWidth: true
                                }
                            }

                            Label {
                                visible: !root.sidebarCollapsed
                                text: "«"
                                color: root.cMuted
                                font.family: root.fontMono
                                font.pixelSize: 18
                                MouseArea {
                                    anchors.fill: parent
                                    anchors.margins: -8
                                    cursorShape: Qt.PointingHandCursor
                                    onClicked: root.sidebarCollapsed = true
                                }
                            }
                        }

                        Rectangle { Layout.fillWidth: true; height: 1; color: root.cLine }

                        Label {
                            visible: !root.sidebarCollapsed
                            text: root.status
                            color: root.cMuted
                            font.family: root.fontHuman
                            font.pixelSize: 13
                            wrapMode: Text.WordWrap
                            maximumLineCount: 2
                            elide: Text.ElideRight
                            Layout.fillWidth: true
                        }

                        NavButton { label: "Discover"; view: "discover"; mark: "◆" }
                        NavButton { label: "Search"; view: "search"; mark: "⌕" }
                        NavButton { label: "Queue"; view: "queue"; mark: "▤" }
                        NavButton { label: "Installed"; view: "installed"; mark: "✓" }
                        NavButton { label: "Updates"; view: "updates"; mark: "↻" }

                        // Live store log — what the store is actually doing.
                        Rectangle {
                            Layout.fillWidth: true
                            Layout.fillHeight: true
                            Layout.topMargin: 6
                            visible: !root.sidebarCollapsed
                            color: root.cBase
                            border.color: root.isBusy() ? root.cGreen : root.cLine
                            clip: true

                            ColumnLayout {
                                anchors.fill: parent
                                anchors.margins: 12
                                spacing: 8

                                RowLayout {
                                    Layout.fillWidth: true
                                    spacing: 8
                                    Rectangle { width: root.tickW; height: 12; color: root.cGreen; Layout.alignment: Qt.AlignVCenter }
                                    Label {
                                        text: "LOG"
                                        color: root.cFg
                                        font.family: root.fontMono
                                        font.pixelSize: 12
                                        font.bold: true
                                        font.letterSpacing: 2
                                        Layout.fillWidth: true
                                    }
                                    Label {
                                        text: root.activityGlyph()
                                        color: root.isBusy() ? root.cGreen : root.cMuted
                                        font.family: root.fontMono
                                        font.pixelSize: 12
                                    }
                                }

                                Rectangle { Layout.fillWidth: true; height: 1; color: root.cLine }

                                ListView {
                                    Layout.fillWidth: true
                                    Layout.fillHeight: true
                                    clip: true
                                    model: root.storeLog
                                    spacing: 3
                                    boundsBehavior: Flickable.StopAtBounds
                                    delegate: Label {
                                        width: ListView.view ? ListView.view.width : 0
                                        text: modelData
                                        color: root.cMuted
                                        font.family: root.fontMono
                                        font.pixelSize: 10
                                        elide: Text.ElideRight
                                        maximumLineCount: 1
                                    }
                                }

                                Label {
                                    visible: root.storeLog.length === 0
                                    text: "waiting for events…"
                                    color: root.cLine
                                    font.family: root.fontMono
                                    font.pixelSize: 10
                                    Layout.fillWidth: true
                                }
                            }
                        }
                    }
                }

                StackLayout {
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    currentIndex: root.activeView === "details" ? 1 : root.activeView === "queue" ? 2 : root.activeView === "installed" ? 3 : root.activeView === "updates" ? 4 : 0

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
                            anchors.margins: root.pagePad()
                            spacing: 16

                            RowLayout {
                                Layout.fillWidth: true
                                spacing: 14
                                visible: !discoverPage.onSearch

                                ColumnLayout {
                                    Layout.fillWidth: true
                                    spacing: 3
                                    Label {
                                        text: discoverPage.onSearch ? "Search" : "Discover"
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

                                ActionButton {
                                    label: "Refresh"
                                    fill: root.cDim
                                    textColor: root.cGreen
                                    Layout.preferredWidth: Math.min(120, root.actionColumnWidth())
                                    onClicked: discoverPage.onSearch ? root.searchNow() : root.loadDiscover()
                                }
                            }

                            Rectangle {
                                Layout.fillWidth: true
                                visible: discoverPage.onSearch
                                height: 52
                                radius: 0
                                color: root.cDim
                                border.color: searchField.activeFocus ? root.cGreen : root.cLine
                                clip: true

                                Rectangle {
                                    width: parent.width * 0.28
                                    height: parent.height
                                    radius: 0
                                    color: "#a7c080"
                                    opacity: root.requestRunning && root.activeMethod === "catalog.search" ? 0.16 : 0
                                    x: root.requestRunning && root.activeMethod === "catalog.search" ? ((root.activityFrame * 20) % Math.max(1, parent.width + width)) - width : -width

                                    Behavior on opacity { NumberAnimation { duration: 160 } }
                                }

                                RowLayout {
                                    anchors.fill: parent
                                    anchors.leftMargin: 16
                                    anchors.rightMargin: 16
                                    spacing: 12

                                    Label {
                                        text: "⌕"
                                        color: root.cMuted
                                        font.family: root.fontMono
                                        font.pixelSize: 20
                                    }
                                    TextField {
                                        id: searchField
                                        Layout.fillWidth: true
                                        placeholderText: "Search apps, packages, repositories"
                                        text: root.query
                                        color: root.cFg
                                        placeholderTextColor: root.cMuted
                                        background: Rectangle { color: "transparent" }
                                        font.family: root.fontHuman
                                        font.pixelSize: 16
                                        onTextChanged: {
                                            root.query = text
                                            root.searchCommitted = false
                                            searchDebounce.restart()
                                        }
                                        onAccepted: root.searchCommitted = true
                                        Component.onCompleted: forceActiveFocus()
                                    }
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

                                ColumnLayout {
                                    width: discoverScroll.availableWidth
                                    spacing: 26

                                    // Featured carousel — full-width cards that slide between picks.
                                    Item {
                                        Layout.fillWidth: true
                                        Layout.preferredHeight: Math.round(196 * root.uiScale()) + 24
                                        visible: root.featuredPool().length > 0

                                        ListView {
                                            id: featuredCarousel
                                            anchors.top: parent.top
                                            anchors.left: parent.left
                                            anchors.right: parent.right
                                            height: Math.round(196 * root.uiScale())
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
                                                fill: root.cBlueSoft
                                                accentEdge: true

                                                MouseArea {
                                                    anchors.fill: parent
                                                    cursorShape: Qt.PointingHandCursor
                                                    onClicked: root.selectApp(featCard.app)
                                                }

                                                RowLayout {
                                                    anchors.fill: parent
                                                    anchors.margins: 28
                                                    spacing: 24
                                                    Rectangle {
                                                        Layout.preferredWidth: Math.round(112 * root.uiScale())
                                                        Layout.preferredHeight: Math.round(112 * root.uiScale())
                                                        color: "transparent"
                                                        clip: true
                                                        Label {
                                                            anchors.centerIn: parent
                                                            visible: fIcon.status !== Image.Ready
                                                            text: featCard.app.name.substring(0, 1).toUpperCase()
                                                            color: root.cBlue
                                                            font.family: root.fontBrand
                                                            font.pixelSize: Math.round(48 * root.uiScale())
                                                            font.bold: true
                                                        }
                                                        Image {
                                                            id: fIcon
                                                            anchors.fill: parent
                                                            anchors.margins: 12
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
                                                        spacing: 6
                                                        Label {
                                                            text: "▚ FEATURED"
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
                                                            font.pixelSize: Math.round(30 * root.uiScale())
                                                            font.bold: true
                                                            elide: Text.ElideRight
                                                            Layout.fillWidth: true
                                                        }
                                                        Label {
                                                            text: featCard.app.summary
                                                            color: root.cMuted
                                                            font.family: root.fontHuman
                                                            font.pixelSize: 15
                                                            wrapMode: Text.WordWrap
                                                            maximumLineCount: 2
                                                            elide: Text.ElideRight
                                                            Layout.fillWidth: true
                                                        }
                                                    }
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
                                                Layout.fillWidth: true
                                                Layout.preferredHeight: 162
                                                orientation: ListView.Horizontal
                                                spacing: 12
                                                clip: true
                                                model: collection.apps
                                                delegate: Rectangle {
                                                    id: railCard
                                                    property var app: modelData
                                                    width: 132
                                                    height: 150
                                                    radius: 0
                                                    color: root.cPanel
                                                    border.color: railHover.hovered ? root.cGreen : root.cLine

                                                    HoverHandler { id: railHover }
                                                    Behavior on border.color { ColorAnimation { duration: 140 } }

                                                    MouseArea {
                                                        anchors.fill: parent
                                                        cursorShape: Qt.PointingHandCursor
                                                        onClicked: root.selectApp(railCard.app)
                                                    }

                                                    ColumnLayout {
                                                        anchors.fill: parent
                                                        anchors.margins: 12
                                                        spacing: 8
                                                        Rectangle {
                                                            Layout.alignment: Qt.AlignHCenter
                                                            Layout.preferredWidth: 60
                                                            Layout.preferredHeight: 60
                                                            color: "transparent"
                                                            clip: true
                                                            Label {
                                                                anchors.centerIn: parent
                                                                visible: railIcon.status !== Image.Ready
                                                                text: railCard.app.name.substring(0, 1).toUpperCase()
                                                                color: root.cGreen
                                                                font.family: root.fontBrand
                                                                font.pixelSize: 26
                                                                font.bold: true
                                                            }
                                                            Image {
                                                                id: railIcon
                                                                anchors.fill: parent
                                                                anchors.margins: 2
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
                                                            font.pixelSize: 13
                                                            font.bold: true
                                                            horizontalAlignment: Text.AlignHCenter
                                                            elide: Text.ElideRight
                                                            maximumLineCount: 2
                                                            wrapMode: Text.WordWrap
                                                            Layout.fillWidth: true
                                                            Layout.alignment: Qt.AlignHCenter
                                                        }
                                                        SourceTag {
                                                            Layout.alignment: Qt.AlignHCenter
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
                                x: root.pagePad()
                                y: root.pagePad()
                                width: Math.max(360, detailsScroll.availableWidth - root.pagePad() * 2)
                                spacing: 16
                                opacity: root.activeView === "details" ? 1 : 0

                                Behavior on opacity { NumberAnimation { duration: 180; easing.type: Easing.OutCubic } }

                                Rectangle {
                                    Layout.fillWidth: true
                                    height: Math.max(292, Math.round(320 * root.uiScale()))
                                    color: root.cPanel
                                    border.color: root.cLine
                                    radius: 0
                                    clip: true

                                    ColumnLayout {
                                        anchors.fill: parent
                                        anchors.margins: 24
                                        spacing: 18

                                        RowLayout {
                                            Layout.fillWidth: true
                                            spacing: 12

                                            ActionButton {
                                                label: "Back"
                                                fill: root.cDim
                                                textColor: root.cFg
                                                Layout.preferredWidth: 74
                                                onClicked: root.activeView = "discover"
                                            }

                                            Item { Layout.fillWidth: true }

                                            ActionButton {
                                                label: root.selectedApp && root.selectedApp.installed ? "Uninstall" : "Get"
                                                fill: root.selectedApp && root.selectedApp.installed ? root.cRed : root.cBlue
                                                textColor: "white"
                                                Layout.preferredWidth: 116
                                                onClicked: {
                                                    if (root.selectedApp && root.selectedApp.installed)
                                                        root.enqueueUninstall(root.selectedApp)
                                                    else
                                                        root.enqueueInstall(root.selectedApp)
                                                }
                                            }
                                        }

                                        RowLayout {
                                            Layout.fillWidth: true
                                            spacing: 28

                                            Rectangle {
                                                Layout.preferredWidth: Math.round(120 * root.uiScale())
                                                Layout.preferredHeight: Math.round(120 * root.uiScale())
                                                color: "transparent"
                                                clip: true
                                                Label {
                                                    anchors.centerIn: parent
                                                    visible: heroIcon.status !== Image.Ready
                                                    text: root.selectedApp ? root.selectedApp.name.substring(0, 1).toUpperCase() : ""
                                                    color: root.cBlue
                                                    font.family: root.fontBrand
                                                    font.pixelSize: Math.round(54 * root.uiScale())
                                                    font.bold: true
                                                }
                                                Image {
                                                    id: heroIcon
                                                    anchors.fill: parent
                                                    anchors.margins: 8
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
                                                    font.pixelSize: Math.round(34 * root.uiScale())
                                                    font.bold: true
                                                    Layout.fillWidth: true
                                                    elide: Text.ElideRight
                                                }
                                                Label {
                                                    text: root.selectedApp ? root.selectedApp.summary : ""
                                                    color: root.cMuted
                                                    font.family: root.fontHuman
                                                    font.pixelSize: 17
                                                    wrapMode: Text.WordWrap
                                                    Layout.fillWidth: true
                                                }
                                                Label {
                                                    text: root.selectedApp ? root.compactDeveloper(root.selectedApp) : ""
                                                    color: root.cBlue
                                                    font.family: root.fontHuman
                                                    font.pixelSize: 14
                                                    font.bold: true
                                                    Layout.fillWidth: true
                                                    elide: Text.ElideRight
                                                }
                                            }
                                        }

                                        Rectangle { Layout.fillWidth: true; height: 1; color: root.cLine }

                                        GridLayout {
                                            Layout.fillWidth: true
                                            columns: detailsScroll.availableWidth > 860 ? 5 : 3
                                            rowSpacing: 16
                                            columnSpacing: 20

                                            MetricBlock {
                                                title: "Rating"
                                                value: root.selectedApp && root.selectedApp.rating ? Math.round(root.selectedApp.rating) + "★" : "No rating"
                                                caption: root.selectedApp && root.selectedApp.rating ? "Catalog" : ""
                                            }
                                            MetricBlock {
                                                title: "Source"
                                                value: root.recommendedVariant(root.selectedApp) ? root.sourceLabel(root.recommendedVariant(root.selectedApp).source) : "Unknown"
                                                caption: "Recommended"
                                            }
                                            MetricBlock {
                                                title: "Trust"
                                                value: root.recommendedVariant(root.selectedApp) ? root.trustLabel(root.recommendedVariant(root.selectedApp).trust) : "Unknown"
                                                caption: root.recommendedVariant(root.selectedApp) && root.recommendedVariant(root.selectedApp).verified ? "Verified" : "Metadata"
                                            }
                                            MetricBlock {
                                                title: "Package"
                                                value: root.recommendedVariant(root.selectedApp) ? root.recommendedVariant(root.selectedApp).package_id : ""
                                                caption: root.recommendedVariant(root.selectedApp) ? root.recommendedVariant(root.selectedApp).version || "Version unknown" : ""
                                            }
                                            MetricBlock {
                                                title: "Size"
                                                value: root.sizeLabel(root.selectedApp)
                                                caption: "Download"
                                            }
                                        }
                                    }
                                }

                                SectionPanel {
                                    title: "Preview"
                                    subtitle: "Screenshots appear when catalog metadata includes safe HTTPS image links."
                                    Layout.preferredHeight: Math.max(236, Math.round(254 * root.uiScale()))

                                    ScrollView {
                                        Layout.fillWidth: true
                                        Layout.fillHeight: true
                                        clip: true
                                        ScrollBar.vertical.policy: ScrollBar.AlwaysOff

                                        RowLayout {
                                            spacing: 12
                                            Repeater {
                                                model: root.selectedApp && root.selectedApp.screenshots.length > 0 ? root.selectedApp.screenshots : ["placeholder"]
                                                delegate: Rectangle {
                                                    Layout.preferredWidth: 300
                                                    Layout.preferredHeight: 168
                                                    color: root.cDim
                                                    border.color: root.cLine
                                                    radius: 0
                                                    clip: true

                                                    Image {
                                                        anchors.fill: parent
                                                        anchors.margins: 1
                                                        source: modelData === "placeholder" ? "" : modelData
                                                        fillMode: Image.PreserveAspectCrop
                                                        asynchronous: true
                                                        visible: status === Image.Ready
                                                    }
                                                    ColumnLayout {
                                                        anchors.centerIn: parent
                                                        width: parent.width - 36
                                                        visible: modelData === "placeholder" || shotImage.status === Image.Error
                                                        Label {
                                                            text: "No Preview"
                                                            color: root.cMuted
                                                            font.family: root.fontHuman
                                                            font.pixelSize: 13
                                                            font.bold: true
                                                            horizontalAlignment: Text.AlignHCenter
                                                            Layout.fillWidth: true
                                                        }
                                                        Label {
                                                            text: root.selectedApp ? root.selectedApp.name : ""
                                                            color: root.cMuted
                                                            font.family: root.fontHuman
                                                            font.pixelSize: 13
                                                            horizontalAlignment: Text.AlignHCenter
                                                            Layout.fillWidth: true
                                                        }
                                                    }
                                                    Image {
                                                        id: shotImage
                                                        anchors.fill: parent
                                                        source: modelData === "placeholder" ? "" : modelData
                                                        fillMode: Image.PreserveAspectCrop
                                                        asynchronous: true
                                                        visible: status === Image.Ready
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }

                                SectionPanel {
                                    title: "App Information"
                                    subtitle: root.recommendedVariant(root.selectedApp) && root.recommendedVariant(root.selectedApp).source === "flathub"
                                              ? "Flatpak apps are sandboxed by default. Exact permissions require provider metadata."
                                              : root.recommendedVariant(root.selectedApp) && root.recommendedVariant(root.selectedApp).source === "system"
                                                ? "System packages integrate directly with Debian/Thallium and are not sandboxed."
                                                : root.recommendedVariant(root.selectedApp) && root.recommendedVariant(root.selectedApp).source === "appimage"
                                                  ? "AppImages are portable binaries installed under the user's UNI AppImage directory."
                                                  : "GitHub releases are shown as unverified unless curated by Thallium metadata."
                                    StatRow { name: "Source"; value: root.recommendedVariant(root.selectedApp) ? root.sourceLabel(root.recommendedVariant(root.selectedApp).source) : "" }
                                    StatRow { name: "Package"; value: root.recommendedVariant(root.selectedApp) ? root.recommendedVariant(root.selectedApp).package_id : "" }
                                    StatRow { name: "Version"; value: root.recommendedVariant(root.selectedApp) ? root.recommendedVariant(root.selectedApp).version || "" : "" }
                                    StatRow { name: "Trust"; value: root.recommendedVariant(root.selectedApp) ? root.trustLabel(root.recommendedVariant(root.selectedApp).trust) : "" }
                                    StatRow { name: "Developer"; value: root.selectedApp ? root.selectedApp.developer || "" : "" }
                                    StatRow { name: "Website"; value: root.selectedApp ? root.selectedApp.homepage || root.selectedApp.repository || "" : "" }
                                    StatRow { name: "Size"; value: root.sizeLabel(root.selectedApp) }
                                    StatRow { name: "Command"; value: root.recommendedVariant(root.selectedApp) ? root.commandPreview(root.recommendedVariant(root.selectedApp)) : "" }
                                }

                                SectionPanel {
                                    title: "Available Sources"
                                    subtitle: "Choose the platform UNI should use. The recommended source is selected first."
                                    Repeater {
                                        model: root.selectedApp ? root.selectedApp.variants : []
                                        delegate: Rectangle {
                                            Layout.fillWidth: true
                                            height: Math.max(138, Math.round(132 * root.uiScale()))
                                            color: modelData.id === root.selectedApp.recommended_variant_id ? "#233024" : root.cPanel
                                            border.color: modelData.id === root.selectedApp.recommended_variant_id ? "#4a5f3f" : root.cLine
                                            radius: 0
                                            clip: true

                                            RowLayout {
                                                anchors.fill: parent
                                                anchors.margins: 12
                                                spacing: 14

                                                ColumnLayout {
                                                    Layout.preferredWidth: Math.max(150, Math.round(180 * root.uiScale()))
                                                    spacing: 4
                                                    Label {
                                                        text: root.sourceLabel(modelData.source)
                                                        color: root.cFg
                                                        font.family: root.fontBrand
                                                        font.pixelSize: 17
                                                        font.bold: true
                                                    }
                                                    Label {
                                                        text: root.platformLabel(modelData.source)
                                                        color: root.cMuted
                                                        font.family: root.fontHuman
                                                        font.pixelSize: 12
                                                        wrapMode: Text.WordWrap
                                                        Layout.fillWidth: true
                                                    }
                                                }

                                                ColumnLayout {
                                                    Layout.fillWidth: true
                                                    Layout.minimumWidth: 0
                                                    spacing: 4
                                                    Label {
                                                        text: modelData.package_id
                                                        color: root.cFg
                                                        font.family: root.fontMono
                                                        font.pixelSize: 13
                                                        elide: Text.ElideRight
                                                        Layout.fillWidth: true
                                                    }
                                                    Label {
                                                        text: root.commandPreview(modelData)
                                                        color: root.cMuted
                                                        font.family: root.fontMono
                                                        font.pixelSize: 11
                                                        elide: Text.ElideRight
                                                        Layout.fillWidth: true
                                                    }
                                                    Label {
                                                        text: "Version: " + (modelData.version || "unknown") + " · Download: " + root.formatBytes(modelData.download_size) + " · Installed: " + root.formatBytes(modelData.installed_size)
                                                        color: root.cMuted
                                                        font.family: root.fontMono
                                                        font.pixelSize: 11
                                                        elide: Text.ElideRight
                                                        Layout.fillWidth: true
                                                    }
                                                    Label {
                                                        text: modelData.install_location || root.platformLabel(modelData.source)
                                                        color: root.cMuted
                                                        font.family: root.fontMono
                                                        font.pixelSize: 11
                                                        elide: Text.ElideRight
                                                        Layout.fillWidth: true
                                                    }
                                                    Label {
                                                        text: root.trustLabel(modelData.trust) + (modelData.verified ? " · verified metadata" : " · unverified")
                                                        color: root.trustColor(modelData.trust)
                                                        font.family: root.fontMono
                                                        font.pixelSize: 11
                                                        Layout.fillWidth: true
                                                    }
                                                }

                                                ActionButton {
                                                    label: root.selectedApp && root.selectedApp.installed ? "Uninstall" : modelData.id === root.selectedApp.recommended_variant_id ? "Get" : "Use Source"
                                                    fill: root.selectedApp && root.selectedApp.installed ? root.cRed : modelData.id === root.selectedApp.recommended_variant_id ? root.cGreen : root.cPanel
                                                    textColor: root.selectedApp && root.selectedApp.installed ? "white" : modelData.id === root.selectedApp.recommended_variant_id ? "white" : root.cBlue
                                                    Layout.preferredWidth: Math.min(220, root.actionColumnWidth())
                                                    Layout.alignment: Qt.AlignRight | Qt.AlignVCenter
                                                    onClicked: {
                                                        if (root.selectedApp && root.selectedApp.installed)
                                                            root.enqueueUninstall(root.selectedApp)
                                                        else
                                                            root.enqueueVariant(root.selectedApp, modelData)
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }

                                SectionPanel {
                                    title: "About"
                                    Label {
                                        text: root.selectedApp && root.selectedApp.description ? root.selectedApp.description : "No long description was provided by this source."
                                        color: root.cFg
                                        font.family: root.fontHuman
                                        font.pixelSize: 15
                                        wrapMode: Text.WordWrap
                                        Layout.fillWidth: true
                                    }
                                }
                            }
                        }
                    }

                    Item {
                        opacity: root.activeView === "queue" ? 1 : 0
                        transform: Translate {
                            y: root.activeView === "queue" ? 0 : 12
                            Behavior on y { NumberAnimation { duration: 180; easing.type: Easing.OutCubic } }
                        }
                        Behavior on opacity { NumberAnimation { duration: 180; easing.type: Easing.OutCubic } }
                        ColumnLayout {
                            anchors.fill: parent
                            anchors.margins: root.pagePad()
                            spacing: 14

                            RowLayout {
                                Layout.fillWidth: true
                                ColumnLayout {
                                    Layout.fillWidth: true
                                    spacing: 3
                                    Label {
                                        text: "Queue"
                                        color: root.cFg
                                        font.family: root.fontBrand
                                        font.pixelSize: Math.round(32 * root.uiScale())
                                        font.bold: true
                                        Layout.fillWidth: true
                                    }
                                    Label {
                                        text: root.operations.length + " operations in progress or history."
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
                                    onClicked: root.request("operations.list", {})
                                }
                            }

                            Rectangle {
                                visible: root.operations.length === 0
                                Layout.fillWidth: true
                                Layout.fillHeight: true
                                color: root.cPanel
                                border.color: root.cLine
                                radius: 0
                                Label {
                                    anchors.centerIn: parent
                                    text: "No operations queued."
                                    color: root.cMuted
                                    font.family: root.fontHuman
                                    font.pixelSize: 16
                                }
                            }

                            ScrollView {
                                Layout.fillWidth: true
                                Layout.fillHeight: true
                                clip: true
                                visible: root.operations.length > 0
                                GridView {
                                    id: queueGrid
                                    width: parent.width
                                    height: parent.height
                                    model: root.operations
                                    cellWidth: {
                                        const columns = Math.max(1, Math.floor(width / 320))
                                        return Math.floor(width / columns)
                                    }
                                    cellHeight: Math.round(230 * root.uiScale())
                                    delegate: Rectangle {
                                        id: queueCard
                                        property color stateColor: modelData.state === "succeeded" ? root.cGreenSoft : modelData.state === "failed" ? root.cRed : root.cBlue
                                        property color stateSurface: modelData.state === "succeeded" ? "#233024" : modelData.state === "failed" ? "#33201f" : root.cBlueSoft
                                        property bool terminal: modelData.state === "succeeded" || modelData.state === "failed" || modelData.state === "cancelled"

                                        width: queueGrid.cellWidth - 14
                                        height: Math.round(210 * root.uiScale())
                                        x: 7
                                        y: 4
                                        color: root.cPanel
                                        border.color: queueHover.hovered ? "#5f7048" : root.cLine
                                        radius: 0
                                        opacity: 0
                                        scale: queueHover.hovered ? 1.018 : 1.0
                                        transform: Translate { id: queueSlide; y: queueHover.hovered ? -4 : 8 }
                                        clip: true

                                        HoverHandler { id: queueHover }

                                        Component.onCompleted: {
                                            queueFade.start()
                                            queueLift.start()
                                        }

                                        Behavior on border.color { ColorAnimation { duration: 140 } }
                                        Behavior on scale { NumberAnimation { duration: 140; easing.type: Easing.OutCubic } }

                                        NumberAnimation {
                                            id: queueFade
                                            target: queueCard
                                            property: "opacity"
                                            from: 0
                                            to: 1
                                            duration: 180
                                            easing.type: Easing.OutCubic
                                        }

                                        NumberAnimation {
                                            id: queueLift
                                            target: queueSlide
                                            property: "y"
                                            from: 8
                                            to: 0
                                            duration: 180
                                            easing.type: Easing.OutCubic
                                        }

                                        Rectangle {
                                            width: parent.width
                                            height: parent.height
                                            opacity: queueHover.hovered ? 0.16 : 0.08
                                            gradient: Gradient {
                                                orientation: Gradient.Horizontal
                                                GradientStop { position: 0.0; color: queueCard.stateColor }
                                                GradientStop { position: 1.0; color: "transparent" }
                                            }
                                        }

                                        ColumnLayout {
                                            anchors.fill: parent
                                            anchors.margins: 14
                                            spacing: 8

                                            RowLayout {
                                                Layout.fillWidth: true
                                                spacing: 12

                                                Rectangle {
                                                    Layout.preferredWidth: Math.round(56 * root.uiScale())
                                                    Layout.preferredHeight: Math.round(56 * root.uiScale())
                                                    radius: 0
                                                    color: queueCard.stateSurface
                                                    border.color: queueCard.stateColor

                                                    Label {
                                                        anchors.centerIn: parent
                                                        text: modelData.app_name ? modelData.app_name.substring(0, 1).toUpperCase() : "Q"
                                                        color: queueCard.stateColor
                                                        font.family: root.fontBrand
                                                        font.pixelSize: Math.round(24 * root.uiScale())
                                                        font.bold: true
                                                    }
                                                }

                                                ColumnLayout {
                                                    Layout.fillWidth: true
                                                    Layout.minimumWidth: 0
                                                    spacing: 4

                                                    Label {
                                                        text: modelData.app_name
                                                        color: root.cFg
                                                        font.family: root.fontHuman
                                                        font.pixelSize: Math.round(16 * root.uiScale())
                                                        font.bold: true
                                                        Layout.fillWidth: true
                                                        elide: Text.ElideRight
                                                    }

                                                    Label {
                                                        text: modelData.action + " · " + root.sourceLabel(root.operationSource(modelData.source))
                                                        color: root.cMuted
                                                        font.family: root.fontHuman
                                                        font.pixelSize: 12
                                                        Layout.fillWidth: true
                                                        elide: Text.ElideRight
                                                    }
                                                }

                                                Rectangle {
                                                    Layout.preferredWidth: stateChip.implicitWidth + 18
                                                    Layout.preferredHeight: 24
                                                    radius: 0
                                                    color: queueCard.stateSurface
                                                    border.color: queueCard.stateColor

                                                    Label {
                                                        id: stateChip
                                                        anchors.centerIn: parent
                                                        text: modelData.state
                                                        color: queueCard.stateColor
                                                        font.family: root.fontHuman
                                                        font.pixelSize: 10
                                                        font.bold: true
                                                    }
                                                }
                                            }

                                            ProgressBar {
                                                from: 0
                                                to: 100
                                                value: modelData.percent
                                                Layout.fillWidth: true
                                            }

                                            Label {
                                                text: root.operationProblem(modelData)
                                                color: modelData.state === "failed" ? root.cRed : root.cMuted
                                                font.family: root.fontHuman
                                                font.pixelSize: 12
                                                maximumLineCount: 2
                                                wrapMode: Text.WordWrap
                                                Layout.fillWidth: true
                                                elide: Text.ElideRight
                                            }

                                            RowLayout {
                                                Layout.fillWidth: true
                                                Layout.preferredHeight: 40
                                                spacing: 8

                                                ActionButton {
                                                    label: "Details"
                                                    fill: root.cDim
                                                    textColor: root.cBlue
                                                    Layout.fillWidth: true
                                                    onClicked: root.selectApp(root.appFromOperation(modelData))
                                                }

                                                ActionButton {
                                                    label: root.operationActionLabel(modelData)
                                                    fill: modelData.state === "failed" ? root.cRed : queueCard.terminal ? root.cDim : root.cBlue
                                                    textColor: modelData.state === "failed" ? "white" : queueCard.terminal ? queueCard.stateColor : "white"
                                                    active: modelData.state === "failed" || !queueCard.terminal
                                                    Layout.fillWidth: true
                                                    onClicked: {
                                                        if (modelData.state === "failed")
                                                            root.retryOperation(modelData)
                                                        else if (!queueCard.terminal)
                                                            root.cancelOperation(modelData)
                                                    }
                                                }
                                            }
                                        }
                                    }
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
                            anchors.margins: root.pagePad()
                            spacing: 14

                            RowLayout {
                                Layout.fillWidth: true
                                ColumnLayout {
                                    Layout.fillWidth: true
                                    spacing: 3
                                    Label {
                                        text: "Installed"
                                        color: root.cFg
                                        font.family: root.fontBrand
                                        font.pixelSize: Math.round(32 * root.uiScale())
                                        font.bold: true
                                        Layout.fillWidth: true
                                    }
                                    Label {
                                        text: root.installedItems.length + " apps found on this system."
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
                                    onClicked: root.request("installed.list", {})
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
                                        scale: installedHover.hovered ? 1.018 : 1.0
                                        transform: Translate { id: installedSlide; y: installedHover.hovered ? -4 : 8 }
                                        clip: true

                                        Component.onCompleted: {
                                            installedFade.start()
                                            installedLift.start()
                                        }

                                        HoverHandler { id: installedHover }
                                        Behavior on border.color { ColorAnimation { duration: 140 } }
                                        Behavior on scale { NumberAnimation { duration: 140; easing.type: Easing.OutCubic } }

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
                                                Label {
                                                    anchors.centerIn: parent
                                                    text: installedCard.item.name.substring(0, 1).toUpperCase()
                                                    color: installedCard.item.managedByUni ? root.sourceAccent(installedCard.itemSource) : root.cWarn
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

                    Item {
                        opacity: root.activeView === "updates" ? 1 : 0
                        transform: Translate {
                            y: root.activeView === "updates" ? 0 : 12
                            Behavior on y { NumberAnimation { duration: 180; easing.type: Easing.OutCubic } }
                        }
                        Behavior on opacity { NumberAnimation { duration: 180; easing.type: Easing.OutCubic } }
                        ColumnLayout {
                            anchors.fill: parent
                            anchors.margins: root.pagePad()
                            spacing: 14

                            RowLayout {
                                Layout.fillWidth: true
                                ColumnLayout {
                                    Layout.fillWidth: true
                                    spacing: 3
                                    Label {
                                        text: "Updates"
                                        color: root.cFg
                                        font.family: root.fontBrand
                                        font.pixelSize: Math.round(32 * root.uiScale())
                                        font.bold: true
                                        Layout.fillWidth: true
                                    }
                                    Label {
                                        text: root.updateItems.length + " updates available."
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
                                    onClicked: root.request("updates.list", {})
                                }
                            }

                            Rectangle {
                                visible: root.updateItems.length === 0
                                Layout.fillWidth: true
                                Layout.fillHeight: true
                                color: root.cPanel
                                border.color: root.cLine
                                radius: 0

                                ColumnLayout {
                                    anchors.centerIn: parent
                                    width: Math.min(parent.width - 48, 620)
                                    spacing: 10

                                    Label {
                                        text: "No updates found"
                                        color: root.cFg
                                        font.family: root.fontBrand
                                        font.pixelSize: Math.round(22 * root.uiScale())
                                        horizontalAlignment: Text.AlignHCenter
                                        Layout.fillWidth: true
                                    }

                                    Label {
                                        text: "If UNI does not expose detailed updates yet, Thallium Store shows zero updates instead of guessing from terminal output."
                                        color: root.cMuted
                                        font.family: root.fontHuman
                                        font.pixelSize: 14
                                        horizontalAlignment: Text.AlignHCenter
                                        wrapMode: Text.WordWrap
                                        Layout.fillWidth: true
                                    }
                                }
                            }

                            ScrollView {
                                Layout.fillWidth: true
                                Layout.fillHeight: true
                                clip: true
                                visible: root.updateItems.length > 0

                                GridView {
                                    id: updatesGrid
                                    width: parent.width
                                    height: parent.height
                                    model: root.updateItems
                                    cellWidth: {
                                        const columns = Math.max(1, Math.floor(width / 320))
                                        return Math.floor(width / columns)
                                    }
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
                                        scale: updateHover.hovered ? 1.018 : 1.0
                                        transform: Translate { id: updateSlide; y: updateHover.hovered ? -4 : 8 }
                                        clip: true

                                        Component.onCompleted: {
                                            updateFade.start()
                                            updateLift.start()
                                        }

                                        HoverHandler { id: updateHover }
                                        Behavior on border.color { ColorAnimation { duration: 140 } }
                                        Behavior on scale { NumberAnimation { duration: 140; easing.type: Easing.OutCubic } }

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
                                                        root.activeView = "queue"
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
            }
        }
    }
}
