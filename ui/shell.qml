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

    readonly property color cBase: "#0d0d0f"
    readonly property color cDim: "#1e2326"
    readonly property color cPanel: "#15191b"
    readonly property color cLine: "#3a464c"
    readonly property color cDead: "#232a2e"
    readonly property color cGreen: "#a7c080"
    readonly property color cGreenSoft: "#83c092"
    readonly property color cFg: "#d3c6aa"
    readonly property color cMuted: "#8f9a91"
    readonly property color cRed: "#e67e80"
    readonly property color cWarn: "#dbbc7f"
    readonly property string fontBrand: "Unbounded"
    readonly property string fontHuman: "Albert Sans"
    readonly property string fontMono: "JetBrains Mono"

    function uiScale() {
        const w = window && window.width ? window.width : 1180
        return Math.max(0.86, Math.min(1.14, w / 1180))
    }

    function pagePad() {
        return Math.round(22 * uiScale())
    }

    function actionColumnWidth() {
        const w = window && window.width ? window.width : 1180
        return Math.max(190, Math.min(320, Math.round((w - 238) * 0.30)))
    }

    function cardHeight() {
        return Math.round(116 * uiScale())
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
        rpc.command = [backendBinary, "--request", item.method, JSON.stringify(item.params)]
        rpc.running = true
    }

    function searchNow() {
        status = "Searching"
        activeView = "discover"
        request("catalog.search", {
            protocol_version: 1,
            query: query,
            sources: ["system", "flathub", "github", "appimage"],
            limit: 40
        })
    }

    function selectApp(app) {
        selectedApp = app
        activeView = "details"
        if (app && app.id)
            request("catalog.appDetails", { id: app.id })
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
            return cWarn
        if (trust === "system_access")
            return cRed
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
                        return
                    }
                    const result = message.result
                    if (!result)
                        return
                    if (result.results !== undefined) {
                        root.results = result.results
                        root.providers = result.providers || []
                        root.status = root.results.length + " result" + (root.results.length === 1 ? "" : "s")
                    } else if (result.items !== undefined && root.activeMethod === "operations.list") {
                        root.operations = result.items
                        root.status = "Queue loaded"
                    } else if (result.items !== undefined && root.activeMethod === "installed.list") {
                        root.installedItems = result.items
                        root.status = root.installedItems.length + " installed item" + (root.installedItems.length === 1 ? "" : "s")
                    } else if (result.items !== undefined && root.activeMethod === "updates.list") {
                        root.updateItems = result.items
                        root.status = root.updateItems.length + " update" + (root.updateItems.length === 1 ? "" : "s")
                    } else if (result.id !== undefined && result.variants !== undefined && root.activeMethod === "catalog.appDetails") {
                        root.selectedApp = result
                        root.status = "Details loaded"
                    } else if (result.id && result.state !== undefined) {
                        root.status = result.message || "Queued"
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
                if (line.length > 0)
                    root.status = line
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

    Component.onCompleted: {
        root.request("system.health", {})
        root.searchNow()
    }

    component NavButton: Rectangle {
        id: nav
        property string label: ""
        property string view: ""
        property string mark: "·"
        property bool hovered: false

        Layout.fillWidth: true
        height: 42
        color: root.activeView === view || (view === "discover" && root.activeView === "details") ? root.cDim : hovered ? "#14191b" : "transparent"
        border.color: root.activeView === view || (view === "discover" && root.activeView === "details") ? root.cLine : "transparent"
        radius: 0
        scale: hovered ? 1.01 : 1.0

        Behavior on color { ColorAnimation { duration: 140 } }
        Behavior on scale { NumberAnimation { duration: 120; easing.type: Easing.OutCubic } }

        Rectangle {
            width: 3
            height: parent.height
            color: root.activeView === nav.view || (nav.view === "discover" && root.activeView === "details") ? root.cGreen : "transparent"
        }

        RowLayout {
            anchors.fill: parent
            anchors.leftMargin: 12
            anchors.rightMargin: 10
            spacing: 10
            Label {
                text: nav.mark
                color: root.cGreen
                font.family: root.fontMono
                font.pixelSize: 16
                Layout.preferredWidth: 20
            }
            Label {
                text: nav.label
                color: root.cFg
                font.family: root.fontMono
                font.pixelSize: 13
                font.letterSpacing: 1.6
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
                if (nav.view === "queue")
                    root.request("operations.list", {})
                if (nav.view === "installed")
                    root.request("installed.list", {})
                if (nav.view === "updates")
                    root.request("updates.list", {})
            }
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
        implicitHeight: sectionContent.implicitHeight + 28

        default property alias content: sectionContent.data

        ColumnLayout {
            id: sectionContent
            anchors.fill: parent
            anchors.margins: 14
            spacing: 10

            Label {
                visible: panel.title.length > 0
                text: panel.title
                color: root.cGreen
                font.family: root.fontMono
                font.pixelSize: 13
                font.letterSpacing: 1.8
                Layout.fillWidth: true
            }

            Label {
                visible: panel.subtitle.length > 0
                text: panel.subtitle
                color: root.cMuted
                font.family: root.fontHuman
                font.pixelSize: 13
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
            text: name
            color: root.cMuted
            font.family: root.fontMono
            font.pixelSize: 12
            font.letterSpacing: 1.2
            Layout.preferredWidth: 128
        }
        Label {
            text: value && value.length > 0 ? value : "Not provided"
            color: root.cFg
            font.family: root.fontHuman
            font.pixelSize: 14
            wrapMode: Text.WordWrap
            Layout.fillWidth: true
        }
    }

    component ActionButton: Rectangle {
        id: action
        property string label: ""
        property color fill: root.cGreen
        property color textColor: root.cBase
        property bool hovered: false
        property bool pressed: false
        signal clicked()

        height: 40
        implicitWidth: actionText.implicitWidth + 28
        color: pressed ? Qt.darker(fill, 1.18) : hovered ? Qt.lighter(fill, 1.08) : fill
        border.color: fill
        radius: 0
        scale: pressed ? 0.985 : hovered ? 1.015 : 1.0

        Behavior on color { ColorAnimation { duration: 120 } }
        Behavior on scale { NumberAnimation { duration: 110; easing.type: Easing.OutCubic } }

        Label {
            id: actionText
            anchors.fill: parent
            anchors.leftMargin: 8
            anchors.rightMargin: 8
            text: action.label
            color: action.textColor
            font.family: root.fontMono
            font.pixelSize: 12
            font.bold: true
            font.letterSpacing: 1.4
            horizontalAlignment: Text.AlignHCenter
            verticalAlignment: Text.AlignVCenter
            elide: Text.ElideRight
        }

        MouseArea {
            anchors.fill: parent
            hoverEnabled: true
            cursorShape: Qt.PointingHandCursor
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
                    Layout.preferredWidth: 238
                    Layout.fillHeight: true
                    color: root.cBase
                    border.color: root.cLine

                    ColumnLayout {
                        anchors.fill: parent
                        anchors.margins: 14
                        spacing: 12

                        Label {
                            text: "THALLIUM"
                            color: root.cGreen
                            font.family: root.fontBrand
                            font.pixelSize: 24
                            font.letterSpacing: 2.0
                            Layout.fillWidth: true
                        }

                        Label {
                            text: "STORE"
                            color: root.cFg
                            font.family: root.fontMono
                            font.pixelSize: 13
                            font.letterSpacing: 4.0
                            Layout.fillWidth: true
                        }

                        Rectangle { Layout.fillWidth: true; height: 1; color: root.cLine }

                        Label {
                            text: root.status
                            color: root.cMuted
                            font.family: root.fontHuman
                            font.pixelSize: 13
                            wrapMode: Text.WordWrap
                            Layout.fillWidth: true
                        }

                        NavButton { label: "DISCOVER"; view: "discover"; mark: "▣" }
                        NavButton { label: "QUEUE"; view: "queue"; mark: "▤" }
                        NavButton { label: "INSTALLED"; view: "installed"; mark: "◆" }
                        NavButton { label: "UPDATES"; view: "updates"; mark: "⇧" }

                        Item { Layout.fillHeight: true }

                        Rectangle {
                            Layout.fillWidth: true
                            height: 170
                            color: root.cPanel
                            border.color: root.isBusy() ? root.cGreen : root.cLine
                            clip: true

                            ColumnLayout {
                                anchors.fill: parent
                                anchors.margins: 12
                                spacing: 7

                                RowLayout {
                                    Layout.fillWidth: true
                                    spacing: 10
                                    Label {
                                        text: root.activityGlyph()
                                        color: root.isBusy() ? root.cGreen : root.cMuted
                                        font.family: root.fontMono
                                        font.pixelSize: 16
                                        Layout.preferredWidth: 24
                                    }
                                    Label {
                                        text: "ACTIVITY"
                                        color: root.cGreen
                                        font.family: root.fontMono
                                        font.pixelSize: 12
                                        font.letterSpacing: 1.8
                                        Layout.fillWidth: true
                                    }
                                }

                                Label {
                                    text: root.activityLabel()
                                    color: root.cFg
                                    font.family: root.fontHuman
                                    font.pixelSize: 12
                                    elide: Text.ElideRight
                                    Layout.fillWidth: true
                                }

                                Rectangle {
                                    Layout.fillWidth: true
                                    height: 5
                                    color: root.cDim
                                    border.color: root.cLine
                                    clip: true

                                    Rectangle {
                                        width: parent.width * 0.38
                                        height: parent.height
                                        color: root.cGreen
                                        opacity: root.isBusy() ? 0.9 : 0.25
                                        x: root.isBusy() ? ((root.activityFrame * 17) % Math.max(1, parent.width + width)) - width : 0

                                        Behavior on opacity { NumberAnimation { duration: 160 } }
                                    }
                                }

                                Rectangle { Layout.fillWidth: true; height: 1; color: root.cLine }

                                Label {
                                    text: "MODE"
                                    color: root.cGreen
                                    font.family: root.fontMono
                                    font.pixelSize: 10
                                    font.letterSpacing: 1.6
                                    Layout.fillWidth: true
                                }

                                Label {
                                    text: root.fakeUniMode
                                          ? "Fake UNI progress is active."
                                          : "Real UNI JSON mode through " + (root.uniHealth.length > 0 ? root.uniHealth : "bundled UNI") + "."
                                    color: root.cMuted
                                    font.family: root.fontHuman
                                    font.pixelSize: 11
                                    wrapMode: Text.WordWrap
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
                        opacity: root.activeView === "discover" ? 1 : 0
                        transform: Translate {
                            y: root.activeView === "discover" ? 0 : 12
                            Behavior on y { NumberAnimation { duration: 180; easing.type: Easing.OutCubic } }
                        }
                        Behavior on opacity { NumberAnimation { duration: 180; easing.type: Easing.OutCubic } }
                        ColumnLayout {
                            anchors.fill: parent
                            anchors.margins: root.pagePad()
                            spacing: 16

                            RowLayout {
                                Layout.fillWidth: true
                                spacing: 14

                                ColumnLayout {
                                    Layout.fillWidth: true
                                    spacing: 3
                                    Label {
                                        text: "Discover Applications"
                                        color: root.cFg
                                        font.family: root.fontBrand
                                        font.pixelSize: Math.round(28 * root.uiScale())
                                        Layout.fillWidth: true
                                    }
                                    Label {
                                        text: "APT/System, Flathub, GitHub, and curated AppImage sources merged into one catalog."
                                        color: root.cMuted
                                        font.family: root.fontHuman
                                        font.pixelSize: 14
                                        Layout.fillWidth: true
                                    }
                                }

                                ActionButton {
                                    label: "REFRESH"
                                    fill: root.cDim
                                    textColor: root.cGreen
                                    Layout.preferredWidth: Math.min(120, root.actionColumnWidth())
                                    onClicked: root.searchNow()
                                }
                            }

                            Rectangle {
                                Layout.fillWidth: true
                                height: 48
                                color: root.cDim
                                border.color: root.cLine

                                RowLayout {
                                    anchors.fill: parent
                                    anchors.leftMargin: 12
                                    anchors.rightMargin: 12
                                    spacing: 10

                                    Label {
                                        text: "⌕"
                                        color: root.cGreen
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
                                        font.pixelSize: 17
                                        onTextChanged: {
                                            root.query = text
                                            searchDebounce.restart()
                                        }
                                        Component.onCompleted: forceActiveFocus()
                                    }
                                }
                            }

                            RowLayout {
                                Layout.fillWidth: true
                                spacing: 8
                                Repeater {
                                    model: root.providers
                                    delegate: Rectangle {
                                        height: 26
                                        implicitWidth: providerText.implicitWidth + 18
                                        color: modelData.state === "ready" ? "#1c2b20" : "#2d2618"
                                        border.color: modelData.state === "ready" ? root.cGreen : root.cWarn
                                        Label {
                                            id: providerText
                                            anchors.centerIn: parent
                                            text: root.sourceLabel(modelData.source) + " · " + modelData.state
                                            color: modelData.state === "ready" ? root.cGreen : root.cWarn
                                            font.family: root.fontMono
                                            font.pixelSize: 11
                                            font.letterSpacing: 1.2
                                        }
                                    }
                                }
                                Item { Layout.fillWidth: true }
                            }

                            ScrollView {
                                Layout.fillWidth: true
                                Layout.fillHeight: true
                                clip: true

                                ListView {
                                    id: resultList
                                    model: root.results
                                    spacing: 10
                                    delegate: Rectangle {
                                        id: appCard
                                        property var app: modelData

                                        width: resultList.width
                                        height: Math.max(112, root.cardHeight())
                                        color: root.cPanel
                                        border.color: resultHover.hovered ? root.cGreen : root.cLine
                                        radius: 0
                                        opacity: 0
                                        transform: Translate { id: resultSlide; y: 8 }

                                        HoverHandler {
                                            id: resultHover
                                        }

                                        Component.onCompleted: {
                                            resultFade.start()
                                            resultLift.start()
                                        }

                                        NumberAnimation {
                                            id: resultFade
                                            target: appCard
                                            property: "opacity"
                                            from: 0
                                            to: 1
                                            duration: 180
                                            easing.type: Easing.OutCubic
                                        }

                                        NumberAnimation {
                                            id: resultLift
                                            target: resultSlide
                                            property: "y"
                                            from: 8
                                            to: 0
                                            duration: 180
                                            easing.type: Easing.OutCubic
                                        }

                                        Rectangle {
                                            width: resultHover.hovered ? 7 : 3
                                            height: parent.height
                                            color: appCard.app.installed ? root.cGreenSoft : root.cGreen

                                            Behavior on width { NumberAnimation { duration: 140; easing.type: Easing.OutCubic } }
                                        }

                                        RowLayout {
                                            anchors.fill: parent
                                            anchors.margins: 14
                                            spacing: 14

                                            Rectangle {
                                                Layout.preferredWidth: Math.round(62 * root.uiScale())
                                                Layout.preferredHeight: Math.round(62 * root.uiScale())
                                                color: root.cDim
                                                border.color: root.cLine
                                                Label {
                                                    anchors.centerIn: parent
                                                    text: appCard.app.name.substring(0, 1).toUpperCase()
                                                    color: root.cGreen
                                                    font.family: root.fontBrand
                                                    font.pixelSize: Math.round(24 * root.uiScale())
                                                }
                                            }

                                            ColumnLayout {
                                                Layout.fillWidth: true
                                                Layout.minimumWidth: 0
                                                spacing: 5
                                                Label {
                                                    text: appCard.app.name
                                                    color: root.cFg
                                                    font.family: root.fontHuman
                                                    font.pixelSize: Math.round(18 * root.uiScale())
                                                    font.bold: true
                                                    elide: Text.ElideRight
                                                    Layout.fillWidth: true
                                                }
                                                Label {
                                                    text: appCard.app.summary
                                                    color: root.cMuted
                                                    font.family: root.fontHuman
                                                    font.pixelSize: Math.round(14 * root.uiScale())
                                                    elide: Text.ElideRight
                                                    Layout.fillWidth: true
                                                }
                                                Flow {
                                                    Layout.fillWidth: true
                                                    spacing: 8
                                                    Repeater {
                                                        model: appCard.app.variants
                                                        delegate: Rectangle {
                                                            height: 24
                                                            implicitWidth: sourceChip.implicitWidth + 16
                                                            color: modelData.id === appCard.app.recommended_variant_id ? "#1c2b20" : root.cDim
                                                            border.color: root.cLine
                                                            Label {
                                                                id: sourceChip
                                                                anchors.centerIn: parent
                                                                text: root.sourceLabel(modelData.source)
                                                                color: root.cGreen
                                                                font.family: root.fontMono
                                                                font.pixelSize: 11
                                                            }
                                                        }
                                                    }
                                                }
                                            }

                                            ColumnLayout {
                                                Layout.preferredWidth: root.actionColumnWidth()
                                                Layout.maximumWidth: root.actionColumnWidth()
                                                Layout.alignment: Qt.AlignRight | Qt.AlignVCenter
                                                spacing: 8
                                                ActionButton {
                                                    label: "DETAILS"
                                                    fill: root.cDim
                                                    textColor: root.cGreen
                                                    Layout.fillWidth: true
                                                    onClicked: root.selectApp(appCard.app)
                                                }
                                                ActionButton {
                                                    label: appCard.app.installed ? "OPEN" : "INSTALL"
                                                    fill: root.cGreen
                                                    textColor: root.cBase
                                                    Layout.fillWidth: true
                                                    onClicked: root.enqueueInstall(appCard.app)
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

                                RowLayout {
                                    Layout.fillWidth: true
                                    spacing: 12
                                    ActionButton {
                                        label: "BACK"
                                        fill: root.cDim
                                        textColor: root.cGreen
                                        onClicked: root.activeView = "discover"
                                    }
                                    Item { Layout.fillWidth: true }
                                    ActionButton {
                                        label: root.selectedApp && root.selectedApp.installed ? "UNINSTALL" : "INSTALL RECOMMENDED"
                                        fill: root.selectedApp && root.selectedApp.installed ? root.cRed : root.cGreen
                                        textColor: root.cBase
                                        Layout.preferredWidth: Math.min(260, root.actionColumnWidth())
                                        onClicked: {
                                            if (root.selectedApp && root.selectedApp.installed)
                                                root.enqueueUninstall(root.selectedApp)
                                            else
                                                root.enqueueInstall(root.selectedApp)
                                        }
                                    }
                                }

                                Rectangle {
                                    Layout.fillWidth: true
                                    height: Math.max(154, Math.round(168 * root.uiScale()))
                                    color: root.cPanel
                                    border.color: root.cLine
                                    clip: true

                                    RowLayout {
                                        anchors.fill: parent
                                        anchors.margins: 18
                                        spacing: 18

                                        Rectangle {
                                            Layout.preferredWidth: Math.round(96 * root.uiScale())
                                            Layout.preferredHeight: Math.round(96 * root.uiScale())
                                            color: root.cDim
                                            border.color: root.cLine
                                            Label {
                                                anchors.centerIn: parent
                                                text: root.selectedApp ? root.selectedApp.name.substring(0, 1).toUpperCase() : ""
                                                color: root.cGreen
                                                font.family: root.fontBrand
                                                font.pixelSize: Math.round(34 * root.uiScale())
                                            }
                                        }

                                        ColumnLayout {
                                            Layout.fillWidth: true
                                            Layout.minimumWidth: 0
                                            spacing: 8
                                            Label {
                                                text: root.selectedApp ? root.selectedApp.name : ""
                                                color: root.cFg
                                                font.family: root.fontBrand
                                                font.pixelSize: Math.round(30 * root.uiScale())
                                                Layout.fillWidth: true
                                                elide: Text.ElideRight
                                            }
                                            Label {
                                                text: root.selectedApp ? root.selectedApp.summary : ""
                                                color: root.cMuted
                                                font.family: root.fontHuman
                                                font.pixelSize: 16
                                                wrapMode: Text.WordWrap
                                                Layout.fillWidth: true
                                            }
                                            Label {
                                                text: root.selectedApp ? "Recommended source: " + (root.recommendedVariant(root.selectedApp) ? root.sourceLabel(root.recommendedVariant(root.selectedApp).source) : "None") : ""
                                                color: root.cGreen
                                                font.family: root.fontMono
                                                font.pixelSize: 12
                                                font.letterSpacing: 1.2
                                                Layout.fillWidth: true
                                            }
                                        }
                                    }
                                }

                                SectionPanel {
                                    title: "SCREENSHOTS"
                                    subtitle: "Remote screenshots load when the provider supplies safe HTTPS image metadata."
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
                                                            text: "NO PREVIEW"
                                                            color: root.cGreen
                                                            font.family: root.fontMono
                                                            font.pixelSize: 13
                                                            font.letterSpacing: 2.0
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

                                GridLayout {
                                    Layout.fillWidth: true
                                    columns: detailsScroll.availableWidth > 820 ? 2 : 1
                                    columnSpacing: 16
                                    rowSpacing: 16

                                    SectionPanel {
                                        title: "DETAILS"
                                        Layout.fillWidth: true
                                        StatRow { name: "DEVELOPER"; value: root.selectedApp ? root.selectedApp.developer : "" }
                                        StatRow { name: "LICENSE"; value: root.selectedApp ? root.selectedApp.license : "" }
                                        StatRow { name: "HOMEPAGE"; value: root.selectedApp ? root.selectedApp.homepage : "" }
                                        StatRow { name: "REPOSITORY"; value: root.selectedApp ? root.selectedApp.repository : "" }
                                        StatRow { name: "TAGS"; value: root.selectedApp ? root.joinTags(root.selectedApp.tags) : "" }
                                        StatRow { name: "RATING"; value: root.selectedApp && root.selectedApp.rating ? Math.round(root.selectedApp.rating) + " stars" : "" }
                                        StatRow { name: "APP ID"; value: root.selectedApp ? root.selectedApp.id : "" }
                                        StatRow { name: "MERGE"; value: root.selectedApp ? Math.round(root.selectedApp.merge_confidence * 100) + "% confidence" : "" }
                                    }

                                    SectionPanel {
                                        title: "SECURITY"
                                        Layout.fillWidth: true
                                        Label {
                                            text: root.recommendedVariant(root.selectedApp) ? root.trustLabel(root.recommendedVariant(root.selectedApp).trust) : "Unknown"
                                            color: root.trustColor(root.recommendedVariant(root.selectedApp) ? root.recommendedVariant(root.selectedApp).trust : "")
                                            font.family: root.fontBrand
                                            font.pixelSize: Math.round(22 * root.uiScale())
                                            Layout.fillWidth: true
                                        }
                                        Label {
                                            text: root.recommendedVariant(root.selectedApp) && root.recommendedVariant(root.selectedApp).source === "flathub"
                                                  ? "Flatpak apps are sandboxed by default. Exact permissions require provider metadata."
                                                  : root.recommendedVariant(root.selectedApp) && root.recommendedVariant(root.selectedApp).source === "system"
                                                    ? "System packages integrate directly with Debian/Thallium and are not sandboxed."
                                                    : root.recommendedVariant(root.selectedApp) && root.recommendedVariant(root.selectedApp).source === "appimage"
                                                      ? "AppImages are portable binaries installed under the user's UNI AppImage directory. Treat them like native desktop apps."
                                                      : "GitHub releases are shown as unverified unless curated by Thallium metadata."
                                            color: root.cMuted
                                            font.family: root.fontHuman
                                            font.pixelSize: 14
                                            wrapMode: Text.WordWrap
                                            Layout.fillWidth: true
                                        }
                                    }
                                }

                                SectionPanel {
                                    title: "INSTALLATION"
                                    subtitle: "Exact source and UNI command for this selected application."
                                    StatRow { name: "SOURCE"; value: root.recommendedVariant(root.selectedApp) ? root.sourceLabel(root.recommendedVariant(root.selectedApp).source) : "" }
                                    StatRow { name: "WHERE"; value: root.recommendedVariant(root.selectedApp) ? root.recommendedVariant(root.selectedApp).install_location : "" }
                                    StatRow { name: "PACKAGE"; value: root.recommendedVariant(root.selectedApp) ? root.recommendedVariant(root.selectedApp).package_id : "" }
                                    StatRow { name: "VERSION"; value: root.recommendedVariant(root.selectedApp) ? root.recommendedVariant(root.selectedApp).version : "" }
                                    StatRow { name: "DOWNLOAD"; value: root.recommendedVariant(root.selectedApp) ? root.formatBytes(root.recommendedVariant(root.selectedApp).download_size) : "" }
                                    StatRow { name: "INSTALLED"; value: root.recommendedVariant(root.selectedApp) ? root.formatBytes(root.recommendedVariant(root.selectedApp).installed_size) : "" }
                                    StatRow { name: "COMMAND"; value: root.recommendedVariant(root.selectedApp) ? root.commandPreview(root.recommendedVariant(root.selectedApp)) : "" }
                                }

                                SectionPanel {
                                    title: "SOURCES / PLATFORMS"
                                    subtitle: "Choose the platform UNI should use. The recommended source is selected by safety-first ranking."
                                    Repeater {
                                        model: root.selectedApp ? root.selectedApp.variants : []
                                        delegate: Rectangle {
                                            Layout.fillWidth: true
                                            height: Math.max(138, Math.round(132 * root.uiScale()))
                                            color: modelData.id === root.selectedApp.recommended_variant_id ? "#1c2b20" : root.cDim
                                            border.color: modelData.id === root.selectedApp.recommended_variant_id ? root.cGreen : root.cLine
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
                                                    label: root.selectedApp && root.selectedApp.installed ? "UNINSTALL" : modelData.id === root.selectedApp.recommended_variant_id ? "INSTALL" : "USE SOURCE"
                                                    fill: root.selectedApp && root.selectedApp.installed ? root.cRed : modelData.id === root.selectedApp.recommended_variant_id ? root.cGreen : root.cPanel
                                                    textColor: root.selectedApp && root.selectedApp.installed ? root.cBase : modelData.id === root.selectedApp.recommended_variant_id ? root.cBase : root.cGreen
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
                                    title: "ABOUT"
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
                                Label {
                                    text: "Operation Queue"
                                    color: root.cFg
                                    font.family: root.fontBrand
                                    font.pixelSize: Math.round(28 * root.uiScale())
                                    Layout.fillWidth: true
                                }
                                ActionButton {
                                    label: "REFRESH"
                                    fill: root.cDim
                                    textColor: root.cGreen
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
                                ListView {
                                    id: queueList
                                    model: root.operations
                                    spacing: 10
                                    delegate: Rectangle {
                                        width: queueList.width
                                        height: 100
                                        color: root.cPanel
                                        border.color: root.cLine
                                        Rectangle {
                                            width: 3
                                            height: parent.height
                                            color: modelData.state === "succeeded" ? root.cGreen : modelData.state === "failed" ? root.cRed : root.cWarn
                                        }
                                        ColumnLayout {
                                            anchors.fill: parent
                                            anchors.margins: 12
                                            Label {
                                                text: modelData.app_name + " · " + modelData.action + " · " + modelData.source + " · " + modelData.state
                                                color: root.cFg
                                                font.family: root.fontHuman
                                                font.pixelSize: 16
                                                Layout.fillWidth: true
                                                elide: Text.ElideRight
                                            }
                                            ProgressBar {
                                                from: 0
                                                to: 100
                                                value: modelData.percent
                                                Layout.fillWidth: true
                                            }
                                            Label {
                                                text: modelData.message
                                                color: root.cMuted
                                                font.family: root.fontMono
                                                font.pixelSize: 11
                                                Layout.fillWidth: true
                                                elide: Text.ElideRight
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
                                Label {
                                    text: "Installed"
                                    color: root.cFg
                                    font.family: root.fontBrand
                                    font.pixelSize: Math.round(28 * root.uiScale())
                                    Layout.fillWidth: true
                                }
                                ActionButton {
                                    label: "REFRESH"
                                    fill: root.cDim
                                    textColor: root.cGreen
                                    Layout.preferredWidth: Math.min(120, root.actionColumnWidth())
                                    onClicked: root.request("installed.list", {})
                                }
                            }

                            Rectangle {
                                Layout.fillWidth: true
                                Layout.fillHeight: true
                                color: root.cPanel
                                border.color: root.cLine
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

                                ListView {
                                    id: installedList
                                    model: root.installedItems
                                    spacing: 10
                                    delegate: Rectangle {
                                        id: installedCard
                                        property var item: modelData
                                        property string packageId: root.packageIdFromInstalled(item)

                                        width: installedList.width
                                        height: Math.max(128, Math.round(124 * root.uiScale()))
                                        color: root.cPanel
                                        border.color: installedHover.hovered ? root.cGreen : root.cLine
                                        opacity: 0
                                        transform: Translate { id: installedSlide; y: 8 }

                                        Component.onCompleted: {
                                            installedFade.start()
                                            installedLift.start()
                                        }

                                        HoverHandler { id: installedHover }

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
                                            width: installedHover.hovered ? 7 : 3
                                            height: parent.height
                                            color: installedCard.item.managedByUni ? root.cGreen : root.cWarn
                                            Behavior on width { NumberAnimation { duration: 140; easing.type: Easing.OutCubic } }
                                        }

                                        RowLayout {
                                            anchors.fill: parent
                                            anchors.margins: 12
                                            spacing: 14

                                            Rectangle {
                                                Layout.preferredWidth: Math.round(56 * root.uiScale())
                                                Layout.preferredHeight: Math.round(56 * root.uiScale())
                                                color: root.cDim
                                                border.color: root.cLine
                                                Label {
                                                    anchors.centerIn: parent
                                                    text: installedCard.item.name.substring(0, 1).toUpperCase()
                                                    color: root.cGreen
                                                    font.family: root.fontBrand
                                                    font.pixelSize: Math.round(20 * root.uiScale())
                                                }
                                            }

                                            ColumnLayout {
                                                Layout.fillWidth: true
                                                Layout.minimumWidth: 0
                                                spacing: 5
                                                Label {
                                                    text: installedCard.item.name
                                                    color: root.cFg
                                                    font.family: root.fontHuman
                                                    font.pixelSize: Math.round(17 * root.uiScale())
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
                                                    font.pixelSize: 13
                                                    Layout.fillWidth: true
                                                    elide: Text.ElideRight
                                                }
                                                Label {
                                                    text: installedCard.item.detail
                                                    color: root.cMuted
                                                    font.family: root.fontMono
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
                                                        color: root.cDim
                                                        border.color: root.cLine
                                                        Label {
                                                            id: sourceChipInstalled
                                                            anchors.centerIn: parent
                                                            text: root.sourceLabel(root.operationSource(installedCard.item.source))
                                                            color: root.cGreen
                                                            font.family: root.fontMono
                                                            font.pixelSize: 10
                                                        }
                                                    }
                                                    Rectangle {
                                                        height: 22
                                                        implicitWidth: managedChipInstalled.implicitWidth + 14
                                                        color: installedCard.item.managedByUni ? "#1c2b20" : "#2d2618"
                                                        border.color: installedCard.item.managedByUni ? root.cGreen : root.cWarn
                                                        Label {
                                                            id: managedChipInstalled
                                                            anchors.centerIn: parent
                                                            text: installedCard.item.managedByUni ? "Managed by UNI" : "Detected"
                                                            color: installedCard.item.managedByUni ? root.cGreen : root.cWarn
                                                            font.family: root.fontMono
                                                            font.pixelSize: 10
                                                        }
                                                    }
                                                }
                                            }

                                            ColumnLayout {
                                                Layout.preferredWidth: root.actionColumnWidth()
                                                Layout.maximumWidth: root.actionColumnWidth()
                                                Layout.alignment: Qt.AlignRight | Qt.AlignVCenter
                                                spacing: 8
                                                ActionButton {
                                                    label: "DETAILS"
                                                    fill: root.cDim
                                                    textColor: root.cGreen
                                                    Layout.fillWidth: true
                                                    onClicked: root.selectApp(root.appFromInstalled(installedCard.item))
                                                }
                                                ActionButton {
                                                    label: "UNINSTALL"
                                                    fill: root.cRed
                                                    textColor: root.cBase
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
                                Label {
                                    text: "Updates"
                                    color: root.cFg
                                    font.family: root.fontBrand
                                    font.pixelSize: Math.round(28 * root.uiScale())
                                    Layout.fillWidth: true
                                }
                                ActionButton {
                                    label: "REFRESH"
                                    fill: root.cDim
                                    textColor: root.cGreen
                                    Layout.preferredWidth: Math.min(120, root.actionColumnWidth())
                                    onClicked: root.request("updates.list", {})
                                }
                            }

                            SectionPanel {
                                title: "UPDATE SOURCES"
                                subtitle: "Updates are read from UNI JSON mode. This page stays stable if one provider has no update data."
                                Layout.fillWidth: true
                            }

                            Rectangle {
                                visible: root.updateItems.length === 0
                                Layout.fillWidth: true
                                Layout.fillHeight: true
                                color: root.cPanel
                                border.color: root.cLine

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

                                ListView {
                                    id: updatesList
                                    model: root.updateItems
                                    spacing: 10
                                    delegate: Rectangle {
                                        id: updateCard
                                        property var item: modelData

                                        width: updatesList.width
                                        height: Math.max(112, root.cardHeight())
                                        color: root.cPanel
                                        border.color: updateHover.hovered ? root.cGreen : root.cLine
                                        opacity: 0
                                        transform: Translate { id: updateSlide; y: 8 }

                                        Component.onCompleted: {
                                            updateFade.start()
                                            updateLift.start()
                                        }

                                        HoverHandler { id: updateHover }

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
                                            width: updateHover.hovered ? 7 : 3
                                            height: parent.height
                                            color: root.cWarn
                                            Behavior on width { NumberAnimation { duration: 140; easing.type: Easing.OutCubic } }
                                        }

                                        RowLayout {
                                            anchors.fill: parent
                                            anchors.margins: 12
                                            spacing: 14

                                            Rectangle {
                                                Layout.preferredWidth: Math.round(54 * root.uiScale())
                                                Layout.preferredHeight: Math.round(54 * root.uiScale())
                                                color: root.cDim
                                                border.color: root.cLine
                                                Label {
                                                    anchors.centerIn: parent
                                                    text: updateCard.item.name ? updateCard.item.name.substring(0, 1).toUpperCase() : "U"
                                                    color: root.cGreen
                                                    font.family: root.fontBrand
                                                    font.pixelSize: Math.round(20 * root.uiScale())
                                                }
                                            }

                                            ColumnLayout {
                                                Layout.fillWidth: true
                                                Layout.minimumWidth: 0
                                                spacing: 5
                                                Label {
                                                    text: updateCard.item.name || "Unknown update"
                                                    color: root.cFg
                                                    font.family: root.fontHuman
                                                    font.pixelSize: Math.round(17 * root.uiScale())
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
                                                    font.pixelSize: 13
                                                    Layout.fillWidth: true
                                                    elide: Text.ElideRight
                                                }
                                                Label {
                                                    text: updateCard.item.detail || "Update metadata from UNI"
                                                    color: root.cMuted
                                                    font.family: root.fontMono
                                                    font.pixelSize: 11
                                                    Layout.fillWidth: true
                                                    elide: Text.ElideRight
                                                }
                                            }

                                            ColumnLayout {
                                                Layout.preferredWidth: root.actionColumnWidth()
                                                Layout.maximumWidth: root.actionColumnWidth()
                                                Layout.alignment: Qt.AlignRight | Qt.AlignVCenter
                                                spacing: 8
                                                ActionButton {
                                                    label: "DETAILS"
                                                    fill: root.cDim
                                                    textColor: root.cGreen
                                                    Layout.fillWidth: true
                                                    onClicked: root.selectApp(root.appFromInstalled(updateCard.item))
                                                }
                                                ActionButton {
                                                    label: "UPDATE"
                                                    fill: root.cGreen
                                                    textColor: root.cBase
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
