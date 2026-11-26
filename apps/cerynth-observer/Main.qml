import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ApplicationWindow {
    id: root
    width: 1120; height: 760; visible: true
    title: "Cerynth Observer"
    color: "#10161e"
    palette.window: "#10161e"; palette.base: "#19232f"; palette.button: "#263746"
    palette.buttonText: "#d7e9f2"; palette.text: "#d7e9f2"; palette.highlight: "#286078"
    palette.windowText: "#d7e9f2"; palette.highlightedText: "#e5f6ff"
    component PageTab: TabButton {
        contentItem: Label { text: parent.text; color: parent.checked ? "#c9f2ff" : "#a8c4d4"; horizontalAlignment: Text.AlignHCenter; verticalAlignment: Text.AlignVCenter }
        background: Rectangle { color: parent.checked ? "#244152" : "#1b2835"; radius: 3 }
    }
    readonly property var view: monitor.data
    function count(list) { return list ? list.length : 0 }
    function bytes(value) { return value === null || value === undefined ? "unavailable" : (Number(value)/1048576).toFixed(1)+" MiB" }
    header: ToolBar {
        background: Rectangle { color: "#19232f" }
        RowLayout {
            anchors.fill: parent; anchors.margins: 12
            Label { text: "CERYNTH  /  OBSERVER"; color: "#b9e6f3"; font.bold: true; Layout.fillWidth: true }
            Label { text: root.view.stale ? "STALE · service unavailable" : "LIVE · owner session"; color: root.view.stale ? "#f3c277" : "#8cdbc3"; textFormat: Text.PlainText }
            Button { text: "Refresh"; enabled: !monitor.refreshing; onClicked: monitor.refresh() }
        }
    }
    ColumnLayout {
        anchors.fill: parent; anchors.margins: 20; spacing: 14
        Label { text: "Agent activity"; color: "#eef5fa"; font.pixelSize: 28 }
        Label { text: "Experimental read-only client · kernel limits and cooperative GPU reservations are labelled separately"; color: "#9aaebe"; wrapMode: Text.WordWrap; Layout.fillWidth: true }
        Label { visible: root.view.stale; text: root.view.agentError || "Connecting to the private owner service…"; color: "#f3c277"; textFormat: Text.PlainText }
        TabBar {
            id: tabs; Layout.fillWidth: true
            PageTab { text: "Sessions · "+root.count(root.view.sessions) }
            PageTab { text: "Timeline · "+root.count(root.view.events) }
            PageTab { text: "GPU · "+root.count(root.view.jobs)+" jobs" }
        }
        StackLayout {
            currentIndex: tabs.currentIndex; Layout.fillWidth: true; Layout.fillHeight: true
            RowLayout {
                ListView {
                    id: sessions; clip: true; Layout.preferredWidth: 540; Layout.fillHeight: true; spacing: 8
                    model: root.view.sessions || []
                    delegate: ItemDelegate {
                        required property var modelData
                        width: sessions.width; height: 82
                        onClicked: monitor.selectedSession=modelData.id
                        background: Rectangle { color: monitor.selectedSession===parent.modelData.id ? "#244152" : "#1b2835"; radius: 7 }
                        contentItem: Column {
                            spacing: 6
                            Label { text: parent.parent.modelData.spec.name; textFormat: Text.PlainText; color: "#e7f1f6"; font.bold: true }
                            Label { text: parent.parent.modelData.state+"  ·  "+parent.parent.modelData.spec.harness; textFormat: Text.PlainText; color: "#a8c4d4" }
                            Label { text: parent.parent.modelData.boundary; textFormat: Text.PlainText; color: "#72d3db" }
                        }
                    }
                    Label { anchors.centerIn: parent; visible: sessions.count===0; text: "No managed sessions"; color: "#9aaebe" }
                }
                ColumnLayout {
                    Layout.fillWidth: true; Layout.fillHeight: true; spacing: 14
                    Label { text: "Resources"; color: "#eef5fa"; font.pixelSize: 21 }
                    Label { text: !root.view.resources ? "Hierarchy state unavailable" : root.view.resources.hierarchy_enabled ? "Nested systemd accounting enabled" : "Nested hierarchy disabled"; color: "#a8c4d4" }
                    Label { text: "Aggregate RAM: "+root.bytes(root.view.accounting ? root.view.accounting.memory_current_bytes : null); color: "#eef5fa" }
                    Label { text: "Kernel maximum: "+root.bytes(root.view.accounting ? root.view.accounting.memory_max_bytes : null); color: "#a8c4d4" }
                    Label { text: !root.view.resources ? "Pressure: unavailable" : root.view.resources.pressure_blocks_new_work ? "Pressure: new work queued" : "Pressure: no current block"; color: "#a8c4d4" }
                    Label { text: !root.view.inference ? "Inference coordinator unavailable" : "Inference: "+root.view.inference.active_requests+" active, "+root.view.inference.admitted_and_waiting_requests+" admitted or waiting"; color: !root.view.inference ? "#f3c277" : "#8cdbc3" }
                    Label { text: !root.view.inference ? "Model hold state unavailable" : "Model residency holds: "+root.view.inference.model_residency_holds; color: "#a8c4d4" }
                    Label { text: "Select a session to inspect aggregate kernel usage. Missing measurements remain unavailable."; color: "#9aaebe"; wrapMode: Text.WordWrap; Layout.fillWidth: true }
                    Item { Layout.fillHeight: true }
                }
            }
            ColumnLayout {
                Label { text: root.view.eventsPartial ? "Bounded view; additional session events omitted" : "Cursor-based events; retention gaps are explicit"; color: "#9aaebe" }
                ListView {
                    id: timeline; clip: true; Layout.fillWidth: true; Layout.fillHeight: true; spacing: 4
                    model: root.view.events || []
                    delegate: Label {
                        required property var modelData
                        width: timeline.width; height: 36; textFormat: Text.PlainText
                        text: modelData.category+"  ·  "+modelData.name+"  ·  "+(modelData.attributes ? modelData.attributes.source : "unknown")
                        color: modelData.category==="security" ? "#e6bf85" : modelData.category==="resource" ? "#8cdbc3" : "#bed0dc"
                    }
                }
            }
            ColumnLayout {
                Label { text: "GPU admission is cooperative. No generic hard VRAM partition is claimed."; color: "#f3c277"; wrapMode: Text.WordWrap; Layout.fillWidth: true }
                Repeater {
                    model: root.view.devices || []
                    delegate: Label {
                        required property var modelData
                        text: modelData.name+"  ·  "+root.bytes(modelData.used_bytes)+" / "+root.bytes(modelData.total_bytes)
                        textFormat: Text.PlainText; color: "#b9e6f3"
                    }
                }
                ListView {
                    id: jobs; Layout.fillWidth: true; Layout.fillHeight: true; clip: true
                    model: root.view.jobs || []
                    delegate: Label {
                        required property var modelData
                        width: jobs.width; height: 38; textFormat: Text.PlainText; color: "#bed0dc"
                        text: modelData.state+"  ·  "+root.bytes(modelData.request.memory_bytes)+"  ·  priority "+modelData.request.priority
                    }
                }
            }
        }
        Label { text: "No management changes are sent by this client. Full approval and session-control UX remains a separate desktop component."; color: "#8197a8"; wrapMode: Text.WordWrap; Layout.fillWidth: true }
    }
}
