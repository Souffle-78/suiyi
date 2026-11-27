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
import AVFoundation

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
    var examples: [String]?
    var zh: Bool?
}

struct PhoneticLite: Decodable {
    var label: String
    var text: String
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
    var phonetics: [PhoneticLite]?
    var tags: [String]?
    var ai: String?
    var senses: [Sense]
    var x: Double
    var y: Double
}

let panelWidth: CGFloat = 384
let pad: CGFloat = 16
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
    // 句子模式
    let header = NSTextField(labelWithString: "")
    let body = NSTextView()
    let scroll = NSScrollView()
    let copyBtn = NSButton(title: "复制译文", target: nil, action: nil)
    let redoBtn = NSButton(title: "重新翻译", target: nil, action: nil)
    let vocabBtn = NSButton(title: "存生词", target: nil, action: nil)
    let closeBtn = NSButton(title: "关闭", target: nil, action: nil)
    // 单词模式（有道的查词卡）
    let headword = NSTextField(labelWithString: "")
    let phonLabel = NSTextField(labelWithString: "")
    let speakBtn = NSButton(title: "", target: nil, action: nil)
    let tagsLabel = NSTextField(labelWithString: "")
    let aiBtn = NSButton(title: "", target: nil, action: nil)
    let editBtn = NSButton(title: "", target: nil, action: nil)
    let searchBtn = NSButton(title: "", target: nil, action: nil)
    let hair = NSView()
    /// 单词卡片里原文框默认收起，点铅笔才展开
    var editingSource = false
    let starBtn = NSButton(title: "", target: nil, action: nil)
    let glyphBtn = NSButton(title: "", target: nil, action: nil)
    let xBtn = NSButton(title: "", target: nil, action: nil)
    let moreBtn = NSButton(title: "", target: nil, action: nil)
    // 共用
    let sourceBox = NSScrollView()
    let sourceView = SourceEditor()

    var current: Request?
    var userEdited = false
    var expanded = false
    let speaker = AVSpeechSynthesizer()

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.backgroundColor = NSColor(white: 0.11, alpha: 0.99).cgColor
        layer?.cornerRadius = 12
        layer?.borderWidth = 1
        layer?.borderColor = NSColor(white: 1, alpha: 0.12).cgColor

        header.font = .systemFont(ofSize: 11, weight: .medium)
        header.textColor = NSColor(white: 1, alpha: 0.6)
        header.lineBreakMode = .byTruncatingTail
        addSubview(header)

        headword.font = .systemFont(ofSize: 20, weight: .semibold)
        headword.textColor = .white
        headword.lineBreakMode = .byTruncatingTail
        addSubview(headword)

        phonLabel.font = .systemFont(ofSize: 12.5)
        phonLabel.textColor = NSColor(white: 0.72, alpha: 1)
        phonLabel.lineBreakMode = .byTruncatingTail
        addSubview(phonLabel)

        for (b, symbol, tip, act) in [
            (xBtn, "xmark", "关闭", #selector(doClose)),
            (glyphBtn, "square.on.square", "复制译文", #selector(doCopy)),
            (starBtn, "star", "存生词", #selector(doVocab)),
            (searchBtn, "magnifyingglass", "在主窗口查词", #selector(doSearch)),
        ] as [(NSButton, String, String, Selector)] {
            b.image = NSImage(systemSymbolName: symbol, accessibilityDescription: tip)
            b.imagePosition = .imageOnly
            b.isBordered = false
            b.contentTintColor = NSColor(white: 0.8, alpha: 1)
            b.toolTip = tip
            b.target = self
            b.action = act
            addSubview(b)
        }
        speakBtn.image = NSImage(systemSymbolName: "speaker.wave.2", accessibilityDescription: "朗读")
        speakBtn.imagePosition = .imageOnly
        speakBtn.isBordered = false
        speakBtn.contentTintColor = NSColor(white: 0.7, alpha: 1)
        speakBtn.toolTip = "朗读"
        speakBtn.target = self
        speakBtn.action = #selector(doSpeak)
        addSubview(speakBtn)
        tagsLabel.font = .systemFont(ofSize: 10.5)
        tagsLabel.textColor = NSColor(white: 0.55, alpha: 1)
        tagsLabel.isHidden = true
        addSubview(tagsLabel)
        aiBtn.image = NSImage(systemSymbolName: "sparkles", accessibilityDescription: "AI 精解")
        aiBtn.isBordered = false
        aiBtn.contentTintColor = NSColor(white: 0.86, alpha: 1)
        aiBtn.target = self
        aiBtn.action = #selector(doAI)
        addSubview(aiBtn)

        editBtn.image = NSImage(systemSymbolName: "square.and.pencil", accessibilityDescription: "编辑原文")
        editBtn.imagePosition = .imageOnly
        editBtn.isBordered = false
        editBtn.contentTintColor = NSColor(white: 0.7, alpha: 1)
        editBtn.toolTip = "编辑原文"
        editBtn.target = self
        editBtn.action = #selector(doToggleEdit)
        addSubview(editBtn)

        hair.wantsLayer = true
        hair.layer?.backgroundColor = NSColor(white: 1, alpha: 0.10).cgColor
        addSubview(hair)

        // 可编辑原文（两种模式都有）
        sourceView.isRichText = false
        sourceView.drawsBackground = false
        sourceView.textContainerInset = NSSize(width: 6, height: 5)
        sourceView.textContainer?.lineFragmentPadding = 2
        sourceView.font = .systemFont(ofSize: 12)
        sourceView.textColor = NSColor(white: 0.9, alpha: 1)
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

        for b in [copyBtn, redoBtn, vocabBtn, closeBtn, moreBtn] {
            b.bezelStyle = .rounded
            b.controlSize = .small
            b.font = .systemFont(ofSize: 12)
            b.contentTintColor = NSColor(white: 0.9, alpha: 1)
            addSubview(b)
        }
        copyBtn.target = self; copyBtn.action = #selector(doCopy)
        redoBtn.target = self; redoBtn.action = #selector(doRedo)
        vocabBtn.target = self; vocabBtn.action = #selector(doVocab)
        closeBtn.target = self; closeBtn.action = #selector(doClose)
        moreBtn.bezelStyle = .inline
        moreBtn.isBordered = false
        moreBtn.contentTintColor = NSColor(white: 0.75, alpha: 1)
        moreBtn.target = self
        moreBtn.action = #selector(doMore)
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
        glyphBtn.toolTip = "已复制"
        writeAction("copy", source: sourceView.string, text: r.text, x: r.x, y: r.y)
    }

    @objc func doVocab() {
        FileHandle.standardError.write("click star btn\n".data(using: .utf8)!)
        guard let r = current, !r.text.isEmpty else { return }
        vocabBtn.title = "已存入"
        starBtn.contentTintColor = NSColor.systemYellow
        writeAction("vocab", source: sourceView.string, text: r.text, x: r.x, y: r.y)
    }

    @objc func doAI() {
        guard let r = current else { return }
        aiBtn.contentTintColor = NSColor.systemTeal
        writeAction("ai", source: r.source, text: r.text, x: r.x, y: r.y)
    }

    @objc func doSearch() {
        guard let r = current else { return }
        writeAction("open_main", source: r.source, text: r.text, x: r.x, y: r.y)
        // 送进主窗口后收起卡片，避免压在主窗口上
        window?.orderOut(nil)
    }

    @objc func doSpeak() {
        guard let r = current else { return }
        let word = sourceView.string.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !word.isEmpty else { return }
        let utter = AVSpeechUtterance(string: word)
        utter.voice = AVSpeechSynthesisVoice(language: "en-US")
        utter.rate = 0.46
        speaker.stopSpeaking(at: .immediate)
        speaker.speak(utter)
        _ = r
    }

    @objc func doToggleEdit() {
        FileHandle.standardError.write("click pencil btn\n".data(using: .utf8)!)
        editingSource.toggle()
        guard let r = current else { return }
        let h = render(r)
        if let win = window as? NSPanel {
            var f = win.frame
            f.origin.y += (f.height - h)
            f.size.height = h
            if let screen = win.screen ?? NSScreen.main {
                let vf = screen.visibleFrame
                f.origin.y = min(max(f.origin.y, vf.minY + 8), vf.maxY - h - 8)
            }
            win.setFrame(f, display: true)
        } else {
            window?.setContentSize(NSSize(width: panelWidth, height: h))
        }
        if editingSource {
            window?.makeKeyAndOrderFront(nil)
            window?.makeFirstResponder(sourceView)
            sourceView.setSelectedRange(NSRange(location: 0, length: sourceView.string.count))
        }
    }

    @objc func doMore() {
        FileHandle.standardError.write("click more btn\n".data(using: .utf8)!)
        expanded.toggle()
        guard let r = current else { return }
        let h = render(r)
        if let win = window as? NSPanel {
            var f = win.frame
            f.origin.y += (f.height - h)
            f.size.height = h
            if let screen = win.screen ?? NSScreen.main {
                let vf = screen.visibleFrame
                f.origin.y = min(max(f.origin.y, vf.minY + 8), vf.maxY - h - 8)
            }
            win.setFrame(f, display: true)
        } else {
            window?.setContentSize(NSSize(width: panelWidth, height: h))
        }
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
        FileHandle.standardError.write("click close btn\n".data(using: .utf8)!)
        window?.orderOut(nil)
    }

    static func bodyFont() -> NSFont { .systemFont(ofSize: 14) }

    static func measure(_ attr: NSAttributedString, width: CGFloat) -> CGFloat {
        let box = NSTextContainer(size: NSSize(width: width, height: .greatestFiniteMagnitude))
        box.lineFragmentPadding = 0
        let layout = NSLayoutManager()
        layout.addTextContainer(box)
        let storage = NSTextStorage(attributedString: attr)
        storage.addLayoutManager(layout)
        layout.ensureLayout(for: box)
        return ceil(layout.usedRect(for: box).height)
    }

    /// 单词释义正文（有道那种：词性一行，释义按序号排，例句灰色斜体）
    /// noun → n. / verb → v. 这类缩写，和有道词典一致
    private func shortPos(_ pos: String) -> String {
        switch pos.lowercased() {
        case "noun": return "n."
        case "verb": return "v."
        case "adjective": return "adj."
        case "adverb": return "adv."
        case "preposition": return "prep."
        case "pronoun": return "pron."
        case "conjunction": return "conj."
        case "interjection": return "int."
        case "determiner": return "det."
        case "numeral": return "num."
        case "article": return "art."
        case "abbreviation": return "abbr."
        default: return pos
        }
    }

    private func wordBody(_ r: Request) -> NSAttributedString {
        let out = NSMutableAttributedString()
        let para = NSMutableParagraphStyle()
        para.lineSpacing = 4

        if !r.text.isEmpty {
            let lead = NSMutableParagraphStyle()
            lead.lineSpacing = 3
            lead.paragraphSpacing = 10
            out.append(NSAttributedString(
                string: r.text + "\n",
                attributes: [.font: NSFont.systemFont(ofSize: 15, weight: .semibold),
                             .foregroundColor: ink,
                             .paragraphStyle: lead]))
        }

        // 中文释义：一行一个词性，全部给出来，卡片内部滚动
        let zhSenses = r.senses.filter { $0.zh == true }
        if !zhSenses.isEmpty {
            for s in zhSenses {
                let head = s.pos.isEmpty ? "" : shortPos(s.pos) + " "
                let line = head + s.defs.joined(separator: "；")
                let p2 = NSMutableParagraphStyle()
                p2.lineSpacing = 3
                p2.paragraphSpacing = 6
                let str = NSMutableAttributedString(
                    string: line + "\n",
                    attributes: [.font: NSFont.systemFont(ofSize: 12.5),
                                 .foregroundColor: NSColor(white: 0.93, alpha: 1),
                                 .paragraphStyle: p2])
                if !s.pos.isEmpty, let r0 = str.string.range(of: head) {
                    str.addAttribute(.foregroundColor,
                                     value: NSColor(white: 0.60, alpha: 1),
                                     range: NSRange(r0, in: str.string))
                    str.addAttribute(.font,
                                     value: NSFont.systemFont(ofSize: 11.5, weight: .semibold),
                                     range: NSRange(r0, in: str.string))
                }
                out.append(str)
            }
        } else {
            // 英英兜底：编号释义 + 例句（只有拿不到中文释义时才走这里）
            let senses = expanded ? r.senses : Array(r.senses.prefix(2))
            for s in senses {
                let shownDefs = expanded ? s.defs : Array(s.defs.prefix(2))
                let shownEx = expanded ? (s.examples ?? []) : Array((s.examples ?? []).prefix(1))
                let posPara = NSMutableParagraphStyle()
                posPara.lineSpacing = 2
                posPara.paragraphSpacingBefore = 6
                out.append(NSAttributedString(
                    string: shortPos(s.pos) + "\n",
                    attributes: [.font: NSFont.systemFont(ofSize: 11.5, weight: .semibold),
                                 .foregroundColor: NSColor(white: 0.62, alpha: 1),
                                 .paragraphStyle: posPara]))
                for (i, def) in shownDefs.enumerated() {
                    out.append(NSAttributedString(
                        string: "\(i + 1). " + def + "\n",
                        attributes: [.font: NSFont.systemFont(ofSize: 12),
                                     .foregroundColor: NSColor(white: 0.93, alpha: 1),
                                     .paragraphStyle: para]))
                }
                for ex in shownEx {
                    out.append(NSAttributedString(
                        string: "例：" + ex + "\n",
                        attributes: [.font: NSFontManager.shared.convert(NSFont.systemFont(ofSize: 11), toHaveTrait: .italicFontMask),
                                     .foregroundColor: NSColor(white: 0.66, alpha: 1),
                                     .paragraphStyle: para]))
                }
            }
        }

        if let ai = r.ai, !ai.isEmpty {
            let p3 = NSMutableParagraphStyle()
            p3.lineSpacing = 3
            p3.paragraphSpacingBefore = 12
            out.append(NSAttributedString(
                string: "AI 精解\n",
                attributes: [.font: NSFont.systemFont(ofSize: 11, weight: .semibold),
                             .foregroundColor: NSColor.systemTeal,
                             .paragraphStyle: p3]))
            out.append(NSAttributedString(
                string: ai + "\n",
                attributes: [.font: NSFont.systemFont(ofSize: 12.5),
                             .foregroundColor: NSColor(white: 0.92, alpha: 1),
                             .paragraphStyle: para]))
        }
        return out
    }

    /// 画内容并把面板高度返回给调用方
    @discardableResult
    func render(_ r: Request) -> CGFloat {
        current = r
        if !userEdited { sourceView.string = r.source }
        redoBtn.title = "重新翻译"
        copyBtn.title = "复制译文"
        vocabBtn.title = "存生词"

        let bodyWidth = panelWidth - pad * 2
        let isWord = r.mode == "word"
        let sourceH: CGFloat = 34

        // 显示哪一组控件
        for v in [header, headword, phonLabel, speakBtn, tagsLabel, aiBtn, editBtn, starBtn, searchBtn, glyphBtn, xBtn, moreBtn, hair,
                  copyBtn, vocabBtn, closeBtn] as [NSView] {
            v.isHidden = true
        }
        header.isHidden = !isWord ? false : true
        scroll.isHidden = false
        sourceBox.isHidden = isWord && !editingSource
        redoBtn.isHidden = isWord && !editingSource
        editBtn.isHidden = !isWord

        let hasZh = r.senses.contains { $0.zh == true }
        let hasMore = !hasZh && (r.senses.count > 2
            || r.senses.contains { $0.defs.count > 2 }
            || r.senses.contains { ($0.examples ?? []).count > 1 })
        moreBtn.isHidden = !(isWord && (hasMore || expanded))
        moreBtn.title = expanded ? "收起 ‹" : "更多释义 ›"

        let attr: NSAttributedString
        if isWord {
            headword.stringValue = r.source
            headword.isHidden = false
            let phs = r.phonetics ?? []
            if phs.isEmpty {
                phonLabel.stringValue = r.phonetic
            } else {
                phonLabel.stringValue = phs.map { p in
                    p.label.isEmpty ? p.text : "\(p.label) \(p.text)"
                }.joined(separator: "    ")
            }
            phonLabel.isHidden = phonLabel.stringValue.isEmpty
            let tags = r.tags ?? []
            tagsLabel.stringValue = tags.isEmpty ? "" : tags.joined(separator: " · ")
            tagsLabel.isHidden = tags.isEmpty
            speakBtn.isHidden = false
            for b in [aiBtn, starBtn, searchBtn, glyphBtn, xBtn] { b.isHidden = false }
            hair.isHidden = false

            attr = wordBody(r)
        } else {
            header.stringValue = r.title
            header.isHidden = false
            let shown: String = r.loading ? "翻译中…" : (r.error ?? r.text)
            let para = NSMutableParagraphStyle()
            para.lineSpacing = 4
            attr = NSAttributedString(
                string: shown,
                attributes: [
                    .font: BubbleView.bodyFont(),
                    .foregroundColor: (r.error != nil) ? NSColor.systemRed : ink,
                    .paragraphStyle: para,
                ])
            for b in [copyBtn, vocabBtn, closeBtn] { b.isHidden = false }
        }

        let textHeight = BubbleView.measure(attr, width: bodyWidth)
        let wordSourceChrome: CGFloat = (isWord && editingSource) ? (8 + sourceH) : 0
        let tagsExtra: CGFloat = (isWord && !tagsLabel.isHidden) ? 16 : 0
        let topChrome: CGFloat = isWord
            ? (pad + 30 + 6 + 18 + tagsExtra + 8 + wordSourceChrome + 10)
            : (pad + 16 + 8 + sourceH + 10)
        let bottomChrome: CGFloat = isWord ? (pad + 26) : (pad + 26 + pad / 2)
        var height = topChrome + textHeight + bottomChrome
        height = min(max(height, 150), 520)

        frame = NSRect(x: 0, y: 0, width: panelWidth, height: height)

        if isWord {
            let iconY = height - pad - 24
            xBtn.frame = NSRect(x: panelWidth - pad - 22, y: iconY, width: 22, height: 22)
            glyphBtn.frame = NSRect(x: xBtn.frame.minX - 6 - 22, y: iconY, width: 22, height: 22)
            searchBtn.frame = NSRect(x: glyphBtn.frame.minX - 6 - 22, y: iconY, width: 22, height: 22)
            starBtn.frame = NSRect(x: searchBtn.frame.minX - 6 - 22, y: iconY, width: 22, height: 22)
            aiBtn.frame = NSRect(x: starBtn.frame.minX - 6 - 22, y: iconY, width: 22, height: 22)
            headword.frame = NSRect(x: pad, y: iconY - 4, width: aiBtn.frame.minX - 12 - pad, height: 30)

            let phonY = height - pad - 30 - 6 - 18
            let tagsH: CGFloat = tagsLabel.isHidden ? 0 : 16
            tagsLabel.frame = NSRect(x: pad, y: phonY - 4 - 14, width: bodyWidth, height: 14)
            hair.frame = NSRect(x: pad, y: phonY - 10 - tagsH, width: bodyWidth, height: 1)
            phonLabel.frame = NSRect(x: pad, y: phonY, width: bodyWidth - 26, height: 18)
            let phonWidth = min(phonLabel.intrinsicContentSize.width, bodyWidth - 70)
            speakBtn.frame = NSRect(x: pad + phonWidth + 8, y: phonY - 1, width: 20, height: 20)
            editBtn.frame = NSRect(x: speakBtn.frame.maxX + 4, y: phonY - 1, width: 20, height: 20)
            if editingSource {
                sourceBox.frame = NSRect(x: pad, y: phonY - 8 - sourceH, width: bodyWidth, height: sourceH)
                sourceView.frame = NSRect(x: 0, y: 0, width: bodyWidth - 2, height: sourceH - 2)
            }

            let btnY = pad

            moreBtn.frame = NSRect(x: panelWidth - pad - 110, y: btnY, width: 110, height: 26)
            moreBtn.alignment = .right
            redoBtn.frame = NSRect(x: pad, y: btnY, width: 78, height: 26)

            let bodyBottom = btnY + 26 + 10
            let bodyTop = (editingSource ? sourceBox.frame.minY : phonY - tagsH) - 16
            let bodyHeight = max(24, bodyTop - bodyBottom)
            scroll.frame = NSRect(x: pad, y: bodyBottom, width: bodyWidth, height: bodyHeight)
            body.frame = NSRect(x: 0, y: 0, width: bodyWidth, height: max(textHeight, bodyHeight))
            scroll.hasVerticalScroller = textHeight > bodyHeight + 1
        } else {
            header.frame = NSRect(x: pad, y: height - pad - 16, width: bodyWidth, height: 16)
            sourceBox.frame = NSRect(x: pad, y: height - pad - 16 - 8 - sourceH, width: bodyWidth, height: sourceH)
            sourceView.frame = NSRect(x: 0, y: 0, width: bodyWidth - 2, height: sourceH - 2)

            let btnY = pad
            closeBtn.frame = NSRect(x: panelWidth - pad - 52, y: btnY, width: 52, height: 26)
            vocabBtn.frame = NSRect(x: closeBtn.frame.minX - 8 - 66, y: btnY, width: 66, height: 26)
            redoBtn.frame = NSRect(x: vocabBtn.frame.minX - 8 - 78, y: btnY, width: 78, height: 26)
            copyBtn.frame = NSRect(x: redoBtn.frame.minX - 8 - 78, y: btnY, width: 78, height: 26)
            copyBtn.isHidden = r.loading || r.text.isEmpty
            vocabBtn.isHidden = r.loading || r.text.isEmpty

            let bodyBottom = btnY + 26 + 12
            let bodyTop = sourceBox.frame.minY - 10
            let bodyHeight = max(20, bodyTop - bodyBottom)
            scroll.frame = NSRect(x: pad, y: bodyBottom, width: bodyWidth, height: bodyHeight)
            body.frame = NSRect(x: 0, y: 0, width: bodyWidth, height: max(textHeight, bodyHeight))
            scroll.hasVerticalScroller = textHeight > bodyHeight + 1
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
    FileHandle.standardError.write("icon frame \(iconPanel.frame)\n".data(using: .utf8)!)
}

func present(_ r: Request) {
    currentRequest = r
    currentIcon = nil
    iconPanel.orderOut(nil)
    hideTimer?.invalidate()
    let height = view.render(r)
    place(panel, width: panelWidth, height: height, anchoredAt: r, offsetX: 10, above: true)
    FileHandle.standardError.write("panel frame \(panel.frame) screen \(panel.screen?.frame.debugDescription ?? "?")\n".data(using: .utf8)!)
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
