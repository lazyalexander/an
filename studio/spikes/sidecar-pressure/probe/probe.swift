import AppKit

let logPath = "~/Desktop/an-agent/An/studio/spikes/sidecar-pressure/probe/probe.log"

func log(_ s: String) {
    let line = s + "\n"
    if let h = FileHandle(forWritingAtPath: logPath) {
        h.seekToEndOfFile()
        h.write(line.data(using: .utf8)!)
        h.closeFile()
    } else {
        try? line.write(toFile: logPath, atomically: true, encoding: .utf8)
    }
}

func subtypeName(_ s: NSEvent.EventSubtype) -> String {
    switch s {
    case .tabletPoint: return "tabletPoint"
    case .tabletProximity: return "tabletProximity"
    case .touch: return "touch"
    default: return "raw(\(s.rawValue))"
    }
}

class ProbeView: NSView {
    override var acceptsFirstResponder: Bool { true }

    private func describe(_ e: NSEvent, _ phase: String) {
        var parts = [phase]
        parts.append("type=\(e.type.rawValue)")
        parts.append("subtype=\(subtypeName(e.subtype))")
        parts.append(String(format: "pressure=%.4f", e.pressure))
        if e.type == .tabletPoint || e.subtype == .tabletPoint || e.subtype == .tabletProximity {
            parts.append("deviceID=\(e.deviceID)")
            parts.append("pointingType=\(e.pointingDeviceType.rawValue)")
            parts.append(String(format: "tilt=(%.3f,%.3f)", e.tilt.x, e.tilt.y))
            parts.append(String(format: "tangential=%.4f", e.tangentialPressure))
            parts.append(String(format: "rotation=%.1f", e.rotation))
            parts.append("abs=\(e.absoluteX),\(e.absoluteY) absZ=\(e.absoluteZ)")
            parts.append("vendor=0x\(String(e.vendorID, radix: 16))")
        }
        log(parts.joined(separator: " "))
    }

    override func mouseDown(with e: NSEvent) { describe(e, "down") }
    override func mouseDragged(with e: NSEvent) { describe(e, "drag") }
    override func mouseUp(with e: NSEvent) { describe(e, "up") }
    override func rightMouseDown(with e: NSEvent) { describe(e, "rdown") }
    override func otherMouseDown(with e: NSEvent) { describe(e, "odown") }
    override func tabletPoint(with e: NSEvent) { describe(e, "tabletPoint") }
    override func tabletProximity(with e: NSEvent) { describe(e, "tabletProximity") }
    override func pressureChange(with e: NSEvent) { describe(e, "pressureChange stage=\(e.stage)") }

    override func draw(_ dirtyRect: NSRect) {
        NSColor.windowBackgroundColor.setFill()
        dirtyRect.fill()
        let msg = "Draw here with Apple Pencil (via Sidecar)"
        let attrs: [NSAttributedString.Key: Any] = [.font: NSFont.systemFont(ofSize: 18)]
        msg.draw(at: NSPoint(x: 20, y: bounds.height - 40), withAttributes: attrs)
    }
}

try? "".write(toFile: logPath, atomically: true, encoding: .utf8)
log("=== probe start \(Date()) ===")

let app = NSApplication.shared
app.setActivationPolicy(.regular)
let window = NSWindow(
    contentRect: NSRect(x: 1800, y: 200, width: 520, height: 320),
    styleMask: [.titled, .closable, .miniaturizable],
    backing: .buffered, defer: false)
window.title = "Sidecar Pressure Probe"
let view = ProbeView(frame: window.contentView!.bounds)
view.autoresizingMask = [.width, .height]
window.contentView!.addSubview(view)
window.makeKeyAndOrderFront(nil)
window.makeFirstResponder(view)
app.activate(ignoringOtherApps: true)
log("window ready")
app.run()
