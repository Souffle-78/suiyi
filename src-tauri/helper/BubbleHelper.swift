// 随译 · 划词气泡助手（macOS）
//
// 为什么要单独一个进程：
// 要让窗口盖在「别的 App 的全屏空间」上，窗口必须是创建时就带
// .nonactivatingPanel 的 NSPanel，且所属 App 是 accessory（无 Dock 图标）。
// Tauri 的窗口是普通 NSWindow，事后改任何属性都不生效（实测 collectionBehavior
// 读回 0x111 也照样 onActiveSpace=false），所以把气泡拆成这个常驻小助手：
// 它只做一件事——画面板、把结果画出来、把「存生词」写回去。
//
// 与主程序只通过两个 JSON 文件通信（省掉 socket 的复杂度）：
//   ~/Library/Application Support/suiyi/bubble-request.json  ← 主程序写，助手轮询
//   ~/Library/Application Support/suiyi/bubble-action.json   ← 助手写，主程序轮询
//   ~/Library/Application Support/suiyi/bubble-shutdown      ← 主程序写，助手退出
//
// 构建：scripts/make-helper.sh（swiftc），产物随 bundle 一起签名。

import Cocoa

let app = NSApplication.shared
app.setActivationPolicy(.accessory)

let supportDir = FileManager.default
    .homeDirectoryForCurrentUser
    .appendingPathComponent("Library/Application Support/suiyi", isDirectory: true)
try? FileManager.default.createDirectory(at: supportDir, withIntermediateDirectories: true)
let requestPath = supportDir.appendingPathComponent("bubble-request.json")
let actionPath = supportDir.appendingPathComponent("bubble-action.json")
let pidPath = supportDir.appendingPathComponent("bubble-helper.pid")

try? "\(ProcessInfo.processInfo.processIdentifier)".write(to: pidPath, atomically: true, encoding: .utf8)

struct Request: Decodable {
    var seq: Int
    var title: String
    var source: String
    var text: String
    var loading: Bool
    var error: String?
    var x: Double
    var y: Double
}

let panelWidth: CGFloat = 400
let pad: CGFloat = 18

func writeAction(_ action: String, _ req: Request?) {
    var dict: [String: Any] = ["action": action, "ts": Date().timeIntervalSince1970]
    if let r = req {
        dict["source"] = r.source
        dict["text"] = r.text
        dict["title"] = r.title
    }
    if let data = try? JSONSerialization.data(withJSONObject: dict),
       let text = String(data: data, encoding: .utf8) {
        try? text.write(to: actionPath, atomically: true, encoding: .utf8)
    }
}

final class BubblePanel: NSPanel {
    override var canBecomeKey: Bool { false }
    override var canBecomeMain: Bool { false }
}

final class BubbleView: NSView {
    let header = NSTextField(labelWithString: "")
    let source = NSTextField(labelWithString: "")
    let body = NSTextView()
    let scroll = NSScrollView()
    let copyBtn = NSButton(title: "复制译文", target: nil, action: nil)
    let vocabBtn = NSButton(title: "存生词", target: nil, action: nil)
    let closeBtn = NSButton(title: "关闭", target: nil, action: nil)
    var current: Request?

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.backgroundColor = NSColor(white: 0.11, alpha: 0.97).cgColor
        layer?.cornerRadius = 12
        layer?.borderWidth = 1
        layer?.borderColor = NSColor(white: 1, alpha: 0.12).cgColor

        header.font = .systemFont(ofSize: 11, weight: .medium)
        header.textColor = NSColor(white: 1, alpha: 0.6)
        header.lineBreakMode = .byTruncatingTail
        addSubview(header)

        source.font = .systemFont(ofSize: 11)
        source.textColor = NSColor(white: 1, alpha: 0.4)
        source.lineBreakMode = .byTruncatingTail
        addSubview(source)

        body.isEditable = false
        body.isSelectable = true
        body.drawsBackground = false
        body.textContainerInset = NSSize(width: 0, height: 0)
        body.textContainer?.lineFragmentPadding = 0
        scroll.documentView = body
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = false
        scroll.autohidesScrollers = true
        addSubview(scroll)

        for b in [copyBtn, vocabBtn, closeBtn] {
            b.bezelStyle = .rounded
            b.controlSize = .small
            b.font = .systemFont(ofSize: 12)
            b.contentTintColor = NSColor(white: 0.92, alpha: 1)
            addSubview(b)
        }
        copyBtn.target = self
        copyBtn.action = #selector(doCopy)
        vocabBtn.target = self
        vocabBtn.action = #selector(doVocab)
        closeBtn.target = self
        closeBtn.action = #selector(doClose)
    }

    required init?(coder: NSCoder) { fatalError() }

    @objc func doCopy() {
        guard let r = current, !r.text.isEmpty else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(r.text, forType: .string)
        copyBtn.title = "已复制"
        writeAction("copy", r)
    }

    @objc func doVocab() {
        guard let r = current, !r.text.isEmpty else { return }
        vocabBtn.title = "已存入"
        writeAction("vocab", r)
    }

    @objc func doClose() {
        window?.orderOut(nil)
    }

    static func bodyFont() -> NSFont { .systemFont(ofSize: 14) }

    static func measure(_ text: String, width: CGFloat) -> CGFloat {
        let para = NSMutableParagraphStyle()
        para.lineSpacing = 4
        let attrs: [NSAttributedString.Key: Any] = [.font: bodyFont(), .paragraphStyle: para]
        let rect = (text as NSString).boundingRect(
            with: NSSize(width: width, height: .greatestFiniteMagnitude),
            options: [.usesLineFragmentOrigin, .usesFontLeading],
            attributes: attrs)
        return ceil(rect.height)
    }

    /// 画内容并把面板高度返回给调用方
    @discardableResult
    func render(_ r: Request) -> CGFloat {
        current = r
        header.stringValue = r.title
        source.stringValue = r.source

        let shown: String = r.loading ? "翻译中…" : (r.error ?? r.text)
        let bodyWidth = panelWidth - pad * 2
        let textHeight = Request.self == Request.self ? BubbleView.measure(shown, width: bodyWidth) : 0
        let chrome: CGFloat = pad + 16 + 6 + 16 + 14 + 12 + 26 + pad
        var height = textHeight + chrome
        height = min(max(height, 132), 460)

        frame = NSRect(x: 0, y: 0, width: panelWidth, height: height)
        header.frame = NSRect(x: pad, y: height - pad - 16, width: bodyWidth, height: 16)
        source.frame = NSRect(x: pad, y: height - pad - 16 - 6 - 16, width: bodyWidth, height: 16)

        let btnH: CGFloat = 26
        let btnY = pad - 2
        closeBtn.frame = NSRect(x: panelWidth - pad - 58, y: btnY, width: 58, height: btnH)
        vocabBtn.frame = NSRect(x: panelWidth - pad - 58 - 8 - 78, y: btnY, width: 78, height: btnH)
        copyBtn.frame = NSRect(x: panelWidth - pad - 58 - 8 - 78 - 8 - 78, y: btnY, width: 78, height: btnH)
        copyBtn.isHidden = r.loading || r.text.isEmpty
        vocabBtn.isHidden = r.loading || r.text.isEmpty

        let bodyTop = source.frame.minY - 12
        let bodyBottom = btnY + btnH + 12
        scroll.frame = NSRect(x: pad, y: bodyBottom, width: bodyWidth, height: max(20, bodyTop - bodyBottom))

        let para = NSMutableParagraphStyle()
        para.lineSpacing = 4
        body.textStorage?.setAttributedString(NSAttributedString(
            string: shown,
            attributes: [
                .font: BubbleView.bodyFont(),
                .foregroundColor: (r.error != nil) ? NSColor.systemRed : NSColor(white: 0.96, alpha: 1),
                .paragraphStyle: para,
            ]))
        return height
    }
}

let panel = BubblePanel(
    contentRect: NSRect(x: 0, y: 0, width: panelWidth, height: 200),
    styleMask: [.borderless, .nonactivatingPanel],
    backing: .buffered,
    defer: false)
panel.level = NSWindow.Level(rawValue: 101)
panel.collectionBehavior = [.canJoinAllSpaces, .stationary, .fullScreenAuxiliary]
panel.isFloatingPanel = true
panel.becomesKeyOnlyIfNeeded = true
panel.hidesOnDeactivate = false
panel.isOpaque = false
panel.backgroundColor = .clear
panel.hasShadow = true
panel.isMovableByWindowBackground = true
panel.appearance = NSAppearance(named: .darkAqua)

let view = BubbleView(frame: NSRect(x: 0, y: 0, width: panelWidth, height: 200))
panel.contentView = view
panel.orderOut(nil)

var lastSeq = -1
var hideTimer: Timer?

func screenFor(topLeftX: Double, topLeftY: Double) -> NSScreen? {
    let primaryHeight = NSScreen.screens.first?.frame.height ?? 0
    let appkitY = primaryHeight - topLeftY
    for s in NSScreen.screens where s.frame.contains(NSPoint(x: topLeftX, y: appkitY)) { return s }
    return NSScreen.main
}

func present(_ r: Request) {
    hideTimer?.invalidate()
    let height = view.render(r)
    let screen = screenFor(topLeftX: r.x, topLeftY: r.y) ?? NSScreen.main
    guard let f = screen?.visibleFrame else { return }
    let primaryHeight = NSScreen.screens.first?.frame.height ?? 0
    let appkitCursorY = primaryHeight - r.y
    var x = r.x + 12
    var y = appkitCursorY - Double(height) - 16
    x = min(max(x, f.minX + 8), max(f.minX + 8, f.maxX - Double(panelWidth) - 8))
    y = min(max(y, f.minY + 8), max(f.minY + 8, f.maxY - Double(height) - 8))
    panel.setFrame(NSRect(x: x, y: y, width: panelWidth, height: height), display: true)
    panel.orderFrontRegardless()
    panel.displayIfNeeded()
    if !r.loading {
        hideTimer = Timer.scheduledTimer(withTimeInterval: 25, repeats: false) { _ in
            panel.orderOut(nil)
        }
    }
}

// 点面板外面就收起
NSEvent.addGlobalMonitorForEvents(matching: [.leftMouseDown]) { _ in
    guard panel.isVisible else { return }
    if !panel.frame.contains(NSEvent.mouseLocation) { panel.orderOut(nil) }
}

Timer.scheduledTimer(withTimeInterval: 0.2, repeats: true) { _ in
    guard let data = try? Data(contentsOf: requestPath),
          let req = try? JSONDecoder().decode(Request.self, from: data) else { return }
    if req.seq == lastSeq { return }
    lastSeq = req.seq
    present(req)
}

Timer.scheduledTimer(withTimeInterval: 1.0, repeats: true) { _ in
    if (try? Data(contentsOf: supportDir.appendingPathComponent("bubble-shutdown"))) != nil {
        NSApp.terminate(nil)
    }
}

app.run()
