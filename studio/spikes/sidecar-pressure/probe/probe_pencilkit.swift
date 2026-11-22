import AppKit
import PencilKit

let logPath = "~/Desktop/an-agent/An/studio/spikes/sidecar-pressure/probe/probe-pencilkit.log"

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

class ProbeDelegate: NSObject, PKCanvasViewDelegate {
    var seen = 0

    func canvasViewDrawingDidChange(_ canvasView: PKCanvasView) {
        let strokes = canvasView.drawing.strokes
        guard strokes.count > seen else { return }
        for i in seen..<strokes.count {
            let pts = strokes[i].path.map { $0 }
            let ps = pts.map { Double($0.pressure) }
            let mn = ps.min() ?? -1
            let mx = ps.max() ?? -1
            let mean = ps.isEmpty ? -1 : ps.reduce(0, +) / Double(ps.count)
            let alts = pts.map { Double($0.forceAltitude) }
            let azRange = (pts.map { Double($0.forceAzimuth) }.min() ?? 0,
                           pts.map { Double($0.forceAzimuth) }.max() ?? 0)
            log(String(format: "stroke[%d] points=%d pressure min=%.4f max=%.4f mean=%.4f | altitude min=%.3f max=%.3f | azimuth %.3f..%.3f",
                       i, pts.count, mn, mx, mean,
                       alts.min() ?? 0, alts.max() ?? 0, azRange.0, azRange.1))
            let head = pts.prefix(5).map { String(format: "%.3f", Double($0.pressure)) }.joined(separator: ",")
            let tail = pts.suffix(5).map { String(format: "%.3f", Double($0.pressure)) }.joined(separator: ",")
            log("  head: \(head)")
            log("  tail: \(tail)")
        }
        seen = strokes.count
    }
}

try? "".write(toFile: logPath, atomically: true, encoding: .utf8)
log("=== pencilkit probe start \(Date()) ===")

let app = NSApplication.shared
app.setActivationPolicy(.regular)
let window = NSWindow(
    contentRect: NSRect(x: 1800, y: 200, width: 520, height: 360),
    styleMask: [.titled, .closable, .miniaturizable],
    backing: .buffered, defer: false)
window.title = "PencilKit Pressure Probe"

let canvas = PKCanvasView(frame: window.contentView!.bounds)
canvas.autoresizingMask = [.width, .height]
canvas.tool = PKInkingTool(.pen, color: .black, width: 12)
canvas.backgroundColor = .white
let delegate = ProbeDelegate()
canvas.delegate = delegate
window.contentView!.addSubview(canvas)

let hint = NSTextField(labelWithString: "Draw here with Apple Pencil — stroke stats go to probe-pencilkit.log")
hint.frame = NSRect(x: 8, y: 4, width: 500, height: 18)
hint.font = .systemFont(ofSize: 11)
hint.textColor = .gray
hint.autoresizingMask = [.width, .maxYMargin]
window.contentView!.addSubview(hint)

window.makeKeyAndOrderFront(nil)
window.makeFirstResponder(canvas)
app.activate(ignoringOtherApps: true)
log("window ready")
app.run()
