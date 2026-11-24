// 随译 · 划词气泡助手（macOS）
//
// 为什么要单独一个进程：
// 要让窗口盖在「别的 App 的全屏空间」上，窗口必须是创建时就带
// .nonactivatingPanel 的 NSPanel，且所属 App 是 accessory（无 Dock 图标）。
// Tauri 的窗口是普通 NSWindow，事后改任何属性都不生效，所以把气泡拆成这个常驻小助手。
//
// 交互（照有道的手感）：
//   1. 划词 → 选区旁边先冒一个 36×36 的小图标；
//   2. 点它、或者鼠标悬停 0.25 秒 → 展开成结果面板（先显示「翻译中」）；
//   3. 面板里原文可编辑，改完点「重新翻译」或按 ⌘↩ 再翻一次；
//   4. 单词（单个英文词）额外显示音标 + 词性 + 多条释义，句子只显示译文。
//
// 与主程序只通过 JSON 文件通信：
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

struct Sense: Decodable {
    var pos: String
    var defs: [String]
}

struct Request: Decodable {
    var seq: Int
    var phase: String
    var title: String
    var source: String
    var text: String
    var loading: Bool
    var error: String?
    var mode: String
    var phonetic: String
    var senses: [Sense]
    var x: Double
    var y: Double
}

let panelWidth: CGFloat = 420
let pad: CGFloat = 18
let ink = NSColor(white: 0.96, alpha: 1)

func writeAction(_ action: String, source: String, text: String, x: Double, y: Double) {
    var dict: [String: Any] = [
        "action": action,
        "ts": Date().timeIntervalSince1970,
        "source": source,
        "text": text,
        "x": x,
        "y": y,
    ]
    dict["title"] = ""
    if let data = try? JSONSerialization.data(withJSONObject: dict),
       let s = String(data: data, encoding: .utf8) {
        try? s.write(to: actionPath, atomically: true, encoding: .utf8)
    }
}

// MARK: - 小图标

final class IconView: NSView {
    var onOpen: (() -> Void)?
    private var hoverTimer: Timer?
    private var tracking: NSTrackingArea?

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.backgroundColor = NSColor(white: 0.11, alpha: 0.96).cgColor
        layer?.cornerRadius = 9
        layer?.borderWidth = 1
        layer?.borderColor = NSColor(white: 1, alpha: 0.16).cgColor
    }

    required init?(coder: NSCoder) { fatalError() }

    override func draw(_ dirtyRect: NSRect) {
        super.draw(dirtyRect)
        let text = "译" as NSString
        let attrs: [NSAttributedString.Key: Any] = [
            .font: NSFont.systemFont(ofSize: 15, weight: .semibold),
            .foregroundColor: NSColor(white: 0.97, alpha: 1),
        ]
        let size = text.size(withAttributes: attrs)
        text.draw(at: NSPoint(x: (bounds.width - size.width) / 2,
                              y: (bounds.height - size.height) / 2),
                  withAttributes: attrs)
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let t = tracking { removeTrackingArea(t) }
        let t = NSTrackingArea(rect: bounds,
                               options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect],
                               owner: self, userInfo: nil)
        addTrackingArea(t)
        tracking = t
    }

    override func mouseEntered(with event: NSEvent) {
        hoverTimer?.invalidate()
        hoverTimer = Timer.scheduledTimer(withTimeInterval: 0.25, repeats: false) { [weak self] _ in
            self?.onOpen?()
        }
    }

    override func mouseExited(with event: NSEvent) {
        hoverTimer?.invalidate()
        hoverTimer = nil
    }

    override func mouseDown(with event: NSEvent) {
        hoverTimer?.invalidate()
        onOpen?()
    }
}

final class IconPanel: NSPanel {
    override var canBecomeKey: Bool { false }
    override var canBecomeMain: Bool { false }
}

/// 图标面板只作展示，不接受键盘
final class PassiveIconPanel: NSPanel {
    override var canBecomeKey: Bool { false }
    override var canBecomeMain: Bool { false }
}

// MARK: - 结果面板

final class BubblePanel: NSPanel {
    // 要能编辑原文，面板就得能成为 key；nonactivating 保证不会把别的 App 顶下去。
    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { false }
}

final class SourceEditor: NSTextView {
    var onSubmit: (() -> Void)?
    /// 面板是 nonactivating 的，点进来要自己 makeKey 才能收到键盘
    override func mouseDown(with event: NSEvent) {
        window?.makeKeyAndOrderFront(nil)
        super.mouseDown(with: event)
    }
    override func keyDown(with event: NSEvent) {
        if event.keyCode == 36 && event.modifierFlags.contains(.command) {
            onSubmit?()
            return
        }
        super.keyDown(with: event)
    }
}

final class BubbleView: NSView, NSTextViewDelegate {
    let header = NSTextField(labelWithString: "")
    let sourceBox = NSScrollView()
    let sourceView = SourceEditor()
    let body = NSTextView()
    let scroll = NSScrollView()
    let copyBtn = NSButton(title: "复制译文", target: nil, action: nil)
    let redoBtn = NSButton(title: "重新翻译", target: nil, action: nil)
    let vocabBtn = NSButton(title: "存生词", target: nil, action: nil)
    let closeBtn = NSButton(title: "关闭", target: nil, action: nil)
    var current: Request?
    /// 用户动过原文就不要再被新结果覆盖
    var userEdited = false

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

        // 可编辑原文
        sourceView.isRichText = false
        sourceView.drawsBackground = false
        sourceView.textContainerInset = NSSize(width: 6, height: 5)
        sourceView.textContainer?.lineFragmentPadding = 2
        sourceView.font = .systemFont(ofSize: 12)
        sourceView.textColor = NSColor(white: 0.92, alpha: 1)
        sourceView.insertionPointColor = NSColor(white: 0.95, alpha: 1)
        sourceView.isAutomaticQuoteSubstitutionEnabled = false
        sourceView.isAutomaticSpellingCorrectionEnabled = false
        sourceView.onSubmit = { [weak self] in self?.doRedo() }
        sourceView.delegate = self
        sourceBox.documentView = sourceView
        sourceBox.hasVerticalScroller = false
        sourceBox.drawsBackground = false
        sourceBox.wantsLayer = true
        sourceBox.layer?.backgroundColor = NSColor(white: 1, alpha: 0.07).cgColor
        sourceBox.layer?.cornerRadius = 6
        addSubview(sourceBox)

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

        for b in [copyBtn, redoBtn, vocabBtn, closeBtn] {
            b.bezelStyle = .rounded
            b.controlSize = .small
            b.font = .systemFont(ofSize: 12)
            b.contentTintColor = NSColor(white: 0.92, alpha: 1)
            addSubview(b)
        }
        copyBtn.target = self; copyBtn.action = #selector(doCopy)
        redoBtn.target = self; redoBtn.action = #selector(doRedo)
        vocabBtn.target = self; vocabBtn.action = #selector(doVocab)
        closeBtn.target = self; closeBtn.action = #selector(doClose)
    }

    required init?(coder: NSCoder) { fatalError() }

    func textDidChange(_ notification: Notification) {
        if (notification.object as? NSTextView) === sourceView {
            userEdited = true
        }
    }

    @objc func doCopy() {
        guard let r = current, !r.text.isEmpty else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(r.text, forType: .string)
        copyBtn.title = "已复制"
        writeAction("copy", source: r.source, text: r.text, x: r.x, y: r.y)
    }

    @objc func doVocab() {
        guard let r = current, !r.text.isEmpty else { return }
        vocabBtn.title = "已存入"
        writeAction("vocab", source: sourceView.string, text: r.text, x: r.x, y: r.y)
    }

    @objc func doRedo() {
        guard let r = current else { return }
        let edited = sourceView.string.trimmingCharacters(in: .whitespacesAndNewlines)
        if edited.isEmpty { return }
        redoBtn.title = "翻译中…"
        userEdited = false
        writeAction("retranslate", source: edited, text: "", x: r.x, y: r.y)
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

    /// 词典块（单词模式）
    private func dictText(_ r: Request) -> String {
        guard r.mode == "word", !r.senses.isEmpty || !r.phonetic.isEmpty else { return "" }
        var out = ""
        if !r.phonetic.isEmpty { out += r.phonetic + "\n" }
        for s in r.senses {
            out += s.pos + " · " + s.defs.joined(separator: "；") + "\n"
        }
        return out.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    /// 画内容并把面板高度返回给调用方
    @discardableResult
    func render(_ r: Request) -> CGFloat {
        current = r
        header.stringValue = r.title
        // 编辑框内容只在「用户没改过」时被覆盖，避免翻译回来的结果把用户正在改的文字冲掉
        if !userEdited {
            sourceView.string = r.source
        }
        redoBtn.title = "重新翻译"
        copyBtn.title = "复制译文"
        vocabBtn.title = "存生词"

        let shown: String = r.loading ? "翻译中…" : (r.error ?? r.text)
        let bodyWidth = panelWidth - pad * 2
        let dict = r.loading ? "" : dictText(r)
        let bodyText = dict.isEmpty ? shown : shown + "\n\n" + dict
        let textHeight = BubbleView.measure(bodyText, width: bodyWidth)

        let sourceH: CGFloat = 46
        let chrome: CGFloat = pad + 16 + 8 + sourceH + 10 + 14 + pad + 26
        var height = textHeight + chrome
        height = min(max(height, 186), 540)

        frame = NSRect(x: 0, y: 0, width: panelWidth, height: height)
        header.frame = NSRect(x: pad, y: height - pad - 16, width: bodyWidth, height: 16)
        sourceBox.frame = NSRect(x: pad, y: height - pad - 16 - 8 - sourceH, width: bodyWidth, height: sourceH)
        sourceView.frame = NSRect(x: 0, y: 0, width: bodyWidth - 2, height: sourceH - 2)

        let btnH: CGFloat = 26
        let btnY = pad
        closeBtn.frame = NSRect(x: panelWidth - pad - 52, y: btnY, width: 52, height: btnH)
        vocabBtn.frame = NSRect(x: closeBtn.frame.minX - 8 - 66, y: btnY, width: 66, height: btnH)
        redoBtn.frame = NSRect(x: vocabBtn.frame.minX - 8 - 78, y: btnY, width: 78, height: btnH)
        copyBtn.frame = NSRect(x: redoBtn.frame.minX - 8 - 78, y: btnY, width: 78, height: btnH)
        copyBtn.isHidden = r.loading || r.text.isEmpty
        vocabBtn.isHidden = r.loading || r.text.isEmpty

        let bodyTop = sourceBox.frame.minY - 10
        let bodyBottom = btnY + btnH + 12
        scroll.frame = NSRect(x: pad, y: bodyBottom, width: bodyWidth, height: max(20, bodyTop - bodyBottom))

        let para = NSMutableParagraphStyle()
        para.lineSpacing = 4
        let attr = NSMutableAttributedString(
            string: shown,
            attributes: [
                .font: BubbleView.bodyFont(),
                .foregroundColor: (r.error != nil) ? NSColor.systemRed : ink,
                .paragraphStyle: para,
            ])
        if !dict.isEmpty {
            attr.append(NSAttributedString(string: "\n\n"))
            attr.append(NSAttributedString(
                string: dict,
                attributes: [
                    .font: NSFont.systemFont(ofSize: 12),
                    .foregroundColor: NSColor(white: 0.78, alpha: 1),
                    .paragraphStyle: para,
                ]))
        }
        body.textStorage?.setAttributedString(attr)
        return height
    }
}

// MARK: - 面板与定位

func makePanel(_ w: CGFloat, _ h: CGFloat) -> NSPanel {
    let p = BubblePanel(
        contentRect: NSRect(x: 0, y: 0, width: w, height: h),
        styleMask: [.borderless, .nonactivatingPanel],
        backing: .buffered,
        defer: false)
    p.level = NSWindow.Level(rawValue: 101)
    p.collectionBehavior = [.canJoinAllSpaces, .stationary, .fullScreenAuxiliary]
    p.isFloatingPanel = true
    p.becomesKeyOnlyIfNeeded = false
    p.hidesOnDeactivate = false
    p.isOpaque = false
    p.backgroundColor = .clear
    p.hasShadow = true
    p.isMovableByWindowBackground = true
    p.appearance = NSAppearance(named: .darkAqua)
    p.orderOut(nil)
    return p
}

let panel = makePanel(panelWidth, 200)
let view = BubbleView(frame: NSRect(x: 0, y: 0, width: panelWidth, height: 200))
panel.contentView = view

let iconSize: CGFloat = 36
let iconPanel: NSPanel = {
    let p = PassiveIconPanel(
        contentRect: NSRect(x: 0, y: 0, width: iconSize, height: iconSize),
        styleMask: [.borderless, .nonactivatingPanel],
        backing: .buffered,
        defer: false)
    p.level = NSWindow.Level(rawValue: 101)
    p.collectionBehavior = [.canJoinAllSpaces, .stationary, .fullScreenAuxiliary]
    p.isFloatingPanel = true
    p.becomesKeyOnlyIfNeeded = true
    p.hidesOnDeactivate = false
    p.isOpaque = false
    p.backgroundColor = .clear
    p.hasShadow = true
    p.appearance = NSAppearance(named: .darkAqua)
    p.orderOut(nil)
    return p
}()
let iconView = IconView(frame: NSRect(x: 0, y: 0, width: iconSize, height: iconSize))
iconPanel.contentView = iconView

var lastSeq = -1
var hideTimer: Timer?
var currentRequest: Request?
var currentIcon: Request?

func screenFor(topLeftX: Double, topLeftY: Double) -> NSScreen? {
    let primaryHeight = NSScreen.screens.first?.frame.height ?? 0
    let appkitY = primaryHeight - topLeftY
    for s in NSScreen.screens where s.frame.contains(NSPoint(x: topLeftX, y: appkitY)) { return s }
    return NSScreen.main
}

func place(_ p: NSPanel, width: CGFloat, height: CGFloat, anchoredAt r: Request, offsetX: CGFloat, above: Bool) {
    let screen = screenFor(topLeftX: r.x, topLeftY: r.y) ?? NSScreen.main
    guard let f = screen?.visibleFrame else { return }
    let primaryHeight = NSScreen.screens.first?.frame.height ?? 0
    let cursorY = primaryHeight - r.y
    var x = r.x + Double(offsetX)
    var y = above ? cursorY - Double(height) - 10 : cursorY + 10
    x = min(max(x, f.minX + 8), max(f.minX + 8, f.maxX - Double(width) - 8))
    y = min(max(y, f.minY + 8), max(f.minY + 8, f.maxY - Double(height) - 8))
    p.setFrame(NSRect(x: x, y: y, width: width, height: height), display: true)
    p.orderFrontRegardless()
    p.displayIfNeeded()
}

func openFromIcon() {
    guard let r = currentIcon else { return }
    iconPanel.orderOut(nil)
    writeAction("translate", source: r.source, text: "", x: r.x, y: r.y)
}

iconView.onOpen = { openFromIcon() }

func presentIcon(_ r: Request) {
    currentIcon = r
    hideTimer?.invalidate()
    panel.orderOut(nil)
    place(iconPanel, width: iconSize, height: iconSize, anchoredAt: r, offsetX: 6, above: false)
}

func present(_ r: Request) {
    currentRequest = r
    currentIcon = nil
    iconPanel.orderOut(nil)
    hideTimer?.invalidate()
    let height = view.render(r)
    place(panel, width: panelWidth, height: height, anchoredAt: r, offsetX: 10, above: true)
    if !r.loading {
        hideTimer = Timer.scheduledTimer(withTimeInterval: 30, repeats: false) { _ in
            panel.orderOut(nil)
        }
    }
}

// 点外面就收起来（图标和面板都收）
NSEvent.addGlobalMonitorForEvents(matching: [.leftMouseDown]) { _ in
    let p = NSEvent.mouseLocation
    if panel.isVisible && !panel.frame.contains(p) { panel.orderOut(nil) }
    if iconPanel.isVisible && !iconPanel.frame.contains(p) { iconPanel.orderOut(nil) }
}

Timer.scheduledTimer(withTimeInterval: 0.2, repeats: true) { _ in
    guard let data = try? Data(contentsOf: requestPath),
          let req = try? JSONDecoder().decode(Request.self, from: data) else { return }
    if req.seq == lastSeq { return }
    lastSeq = req.seq
    if req.phase == "icon" { presentIcon(req) } else { present(req) }
}

Timer.scheduledTimer(withTimeInterval: 1.0, repeats: true) { _ in
    if (try? Data(contentsOf: supportDir.appendingPathComponent("bubble-shutdown"))) != nil {
        NSApp.terminate(nil)
    }
}

app.run()
