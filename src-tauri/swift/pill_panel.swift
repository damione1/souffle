import AppKit
import Foundation
import QuartzCore

// SOU-051: Native HUD pill panel — NSPanel with AppKit content.
// This file owns all UI; Rust drives it through the C bridge (pill_bridge.h).

// ---------------------------------------------------------------------------
// MARK: - Constants
// ---------------------------------------------------------------------------

private let kCompactWidth: CGFloat = 280
private let kCompactHeight: CGFloat = 64
private let kMeetingWidth: CGFloat = 96
private let kMeetingHeight: CGFloat = 44
private let kExpandedWidth: CGFloat = 440
private let kMaxHeight: CGFloat = 200
private let kTopMargin: CGFloat = 40
private let kCornerRadiusFull: CGFloat = 28
private let kCornerRadiusMeet: CGFloat = 22
private let kWaveformBars: Int = 24
private let kMaxLiveLines: Int = 5
private let kLiveFontSize: CGFloat = 13

/// Soufflé accent (`--color-accent` / #e9ae55).
private let kAccent = NSColor(red: 233 / 255, green: 174 / 255, blue: 85 / 255, alpha: 1)

@objc enum PillMode: Int {
    case dictation = 0
    case meeting = 1
    case polishing = 2
}

private func reduceMotionEnabled() -> Bool {
    NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
}

/// Hop to the main thread without deadlocking if we're already on it.
private func onMain(_ body: @escaping () -> Void) {
    if Thread.isMainThread {
        body()
    } else {
        DispatchQueue.main.async(execute: body)
    }
}

// ---------------------------------------------------------------------------
// MARK: - Waveform view
// ---------------------------------------------------------------------------

/// Drawn NSView bars (not CALayer): same geometry as the Svelte pill waveform.
/// A 30 fps tick applies the per-bar sine variation; RMS arrives from Rust.
private final class WaveformView: NSView {
    private var bars: [CGFloat] = Array(repeating: 0.12, count: kWaveformBars)
    private var rms: CGFloat = 0
    private var tick: Timer?

    override var isFlipped: Bool { true }
    override var isOpaque: Bool { false }

    func push(rms raw: Float) {
        // Capture already stores rawRms*8, but conversational speech still
        // lands ~0.1–0.3 — a 2–6px wiggle in this row. Square-root + gain
        // so normal talking fills the bars without clipping a shout.
        let x = max(0, CGFloat(raw))
        rms = min(1, x.squareRoot() * 1.8)
        if reduceMotionEnabled() {
            applyRmsToBars()
        }
    }

    func setActive(_ active: Bool) {
        if active {
            if reduceMotionEnabled() {
                stopTick()
                applyRmsToBars()
            } else {
                startTick()
            }
        } else {
            stopTick()
            rms = 0
            bars = Array(repeating: 0.12, count: kWaveformBars)
            needsDisplay = true
        }
    }

    private func startTick() {
        guard tick == nil else { return }
        let timer = Timer(timeInterval: 1.0 / 30.0, repeats: true) { [weak self] _ in
            self?.tickAnimation()
        }
        RunLoop.main.add(timer, forMode: .common)
        tick = timer
    }

    private func stopTick() {
        tick?.invalidate()
        tick = nil
    }

    private func applyRmsToBars() {
        let target = max(0.08, min(1, rms))
        for i in 0..<kWaveformBars {
            bars[i] = target
        }
        needsDisplay = true
    }

    private func tickAnimation() {
        if reduceMotionEnabled() {
            stopTick()
            applyRmsToBars()
            return
        }
        let t = CACurrentMediaTime()
        for i in 0..<kWaveformBars {
            let variation = sin(t * 5 + Double(i) * 0.5) * 0.15
            let spread = sin(Double(i) * 0.3 + t * 3.3) * 0.1
            let target = max(0.08, min(1, Double(rms) + variation + spread))
            bars[i] += (CGFloat(target) - bars[i]) * 0.45
        }
        needsDisplay = true
    }

    override func draw(_ dirtyRect: NSRect) {
        guard let ctx = NSGraphicsContext.current?.cgContext else { return }
        let w = bounds.width
        let h = bounds.height
        guard w > 0, h > 0 else { return }

        // Compact dictation only has ~99 pt between title and Stop; 24×3pt
        // bars with 2 pt gaps need 118. Scale to bounds so they never overlap.
        let n = CGFloat(kWaveformBars)
        let scale = min(1, w / (n * 3 + (n - 1) * 2))
        let barWidth = 3 * scale
        let barGap = 2 * scale
        let occupied = n * barWidth + (n - 1) * barGap
        let offsetX = (w - occupied) / 2

        ctx.setFillColor(kAccent.cgColor)
        for i in 0..<kWaveformBars {
            let barH = max(2, bars[i] * (h - 4))
            let x = offsetX + CGFloat(i) * (barWidth + barGap)
            let y = (h - barH) / 2
            let rect = CGRect(x: x, y: y, width: barWidth, height: barH)
            let corner = min(1.5, barWidth / 2, barH / 2)
            ctx.setAlpha(0.4 + bars[i] * 0.6)
            ctx.beginPath()
            ctx.addPath(CGPath(roundedRect: rect, cornerWidth: corner, cornerHeight: corner, transform: nil))
            ctx.fillPath()
        }
        ctx.setAlpha(1)
    }

    deinit {
        stopTick()
    }
}

// ---------------------------------------------------------------------------
// MARK: - Content view
// ---------------------------------------------------------------------------

private final class PillContentView: NSView {
    private let blurView = NSVisualEffectView()
    private let borderView = NSView()
    private let recordingDot = NSView()
    private let modeLabel = NSTextField(labelWithString: "")
    private let liveLabel = NSTextField(wrappingLabelWithString: "")
    private let stopButton = NSButton()
    private let waveform = WaveformView()
    private let spinner = NSProgressIndicator()

    var stopAction: (() -> Void)?
    var currentMode: PillMode = .dictation
    var isExpanded: Bool = false
    var stopLabel: String = "Stop recording"
    var a11yLabel: String = "Dictation in progress"

    override var isFlipped: Bool { true }

    override init(frame: NSRect) {
        super.init(frame: frame)
        setup()
    }

    required init?(coder: NSCoder) { nil }

    private func setup() {
        wantsLayer = true
        layer?.backgroundColor = NSColor.clear.cgColor

        blurView.material = .hudWindow
        blurView.blendingMode = .behindWindow
        blurView.state = .active
        blurView.wantsLayer = true
        blurView.layer?.cornerRadius = kCornerRadiusFull
        blurView.layer?.masksToBounds = true
        addSubview(blurView)

        borderView.wantsLayer = true
        borderView.layer?.backgroundColor = NSColor.clear.cgColor
        borderView.layer?.borderColor = NSColor.white.withAlphaComponent(0.12).cgColor
        borderView.layer?.borderWidth = 1
        borderView.layer?.cornerRadius = kCornerRadiusFull
        borderView.layer?.masksToBounds = true
        addSubview(borderView)

        recordingDot.wantsLayer = true
        recordingDot.layer?.backgroundColor = NSColor.systemRed.cgColor
        recordingDot.layer?.cornerRadius = 5
        recordingDot.setAccessibilityElement(false)
        addSubview(recordingDot)
        animateDot()

        modeLabel.textColor = NSColor.white.withAlphaComponent(0.90)
        modeLabel.font = NSFont.systemFont(ofSize: 12, weight: .medium)
        modeLabel.isEditable = false
        modeLabel.isBezeled = false
        modeLabel.drawsBackground = false
        addSubview(modeLabel)

        liveLabel.textColor = NSColor.white.withAlphaComponent(0.70)
        liveLabel.font = NSFont.systemFont(ofSize: kLiveFontSize)
        liveLabel.maximumNumberOfLines = kMaxLiveLines
        liveLabel.usesSingleLineMode = false
        liveLabel.lineBreakMode = .byWordWrapping
        liveLabel.cell?.wraps = true
        liveLabel.cell?.truncatesLastVisibleLine = true
        liveLabel.isEditable = false
        liveLabel.isSelectable = false
        liveLabel.isBezeled = false
        liveLabel.drawsBackground = false
        liveLabel.isHidden = true
        addSubview(liveLabel)

        waveform.setAccessibilityElement(false)
        addSubview(waveform)

        spinner.style = .spinning
        spinner.controlSize = .small
        spinner.isDisplayedWhenStopped = false
        spinner.isHidden = true
        addSubview(spinner)

        stopButton.bezelStyle = .circular
        stopButton.isBordered = false
        stopButton.wantsLayer = true
        stopButton.layer?.backgroundColor = NSColor.systemRed.withAlphaComponent(0.9).cgColor
        stopButton.layer?.cornerRadius = 13.5
        stopButton.contentTintColor = .white
        stopButton.refusesFirstResponder = false
        stopButton.target = self
        stopButton.action = #selector(didTapStop)
        addSubview(stopButton)

        applyMode(.dictation, title: "Dictating", stopLabel: "Stop recording",
                  a11yLabel: "Dictation in progress", expanded: false)
    }

    var stopControl: NSView { stopButton }

    private func animateDot() {
        recordingDot.layer?.removeAnimation(forKey: "pulse")
        guard !reduceMotionEnabled() else {
            recordingDot.layer?.opacity = 1
            return
        }
        let pulse = CABasicAnimation(keyPath: "opacity")
        pulse.fromValue = 1.0
        pulse.toValue = 0.3
        pulse.duration = 0.9
        pulse.autoreverses = true
        pulse.repeatCount = .infinity
        pulse.timingFunction = CAMediaTimingFunction(name: .easeInEaseOut)
        recordingDot.layer?.add(pulse, forKey: "pulse")
    }

    func applyMode(_ mode: PillMode, title: String, stopLabel: String, a11yLabel: String, expanded: Bool) {
        currentMode = mode
        isExpanded = expanded
        self.stopLabel = stopLabel
        self.a11yLabel = a11yLabel

        let compact = (mode == .meeting && !expanded)
        let radius = compact ? kCornerRadiusMeet : kCornerRadiusFull
        blurView.layer?.cornerRadius = radius
        borderView.layer?.cornerRadius = radius

        recordingDot.isHidden = (mode == .polishing)
        if !recordingDot.isHidden {
            animateDot()
        }

        modeLabel.stringValue = title
        modeLabel.isHidden = compact

        let showWave = (mode == .dictation)
        waveform.isHidden = !showWave
        waveform.setActive(showWave)
        spinner.isHidden = (mode != .polishing)
        if mode == .polishing {
            if reduceMotionEnabled() {
                spinner.stopAnimation(nil)
            } else {
                spinner.startAnimation(nil)
            }
        } else {
            spinner.stopAnimation(nil)
        }

        liveLabel.isHidden = !expanded || mode != .dictation
        stopButton.isHidden = (mode == .polishing)
        stopButton.setAccessibilityLabel(stopLabel)

        setAccessibilityElement(true)
        setAccessibilityRole(.group)
        setAccessibilityLabel(a11yLabel)

        let btnSize: CGFloat = compact ? 24 : 27
        stopButton.layer?.cornerRadius = btnSize / 2
        let imgCfg = NSImage.SymbolConfiguration(pointSize: compact ? 9 : 11, weight: .regular)
        stopButton.image = NSImage(
            systemSymbolName: "stop.fill",
            accessibilityDescription: stopLabel
        )?.withSymbolConfiguration(imgCfg)

        needsLayout = true
    }

    func setLiveText(_ text: String) {
        let expanded = !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        liveLabel.stringValue = text
        applyMode(currentMode, title: modeLabel.stringValue, stopLabel: stopLabel,
                  a11yLabel: a11yLabel, expanded: expanded)
    }

    func pushRMS(_ level: Float) {
        waveform.push(rms: level)
    }

    /// Wrapped-line height for the live tail, capped at 5 lines.
    /// Measured from the string, not `sizeThatFits` (truncating NSTextField
    /// reports a single line and then the window still grew).
    func liveTextHeight(forWidth width: CGFloat) -> CGFloat {
        let text = liveLabel.stringValue
        guard isExpanded, !text.isEmpty, width > 0 else { return 0 }
        let font = liveLabel.font ?? NSFont.systemFont(ofSize: kLiveFontSize)
        // Match the live label's default paragraph style. A 1.15 multiple
        // here made a 1-line tail count as 2 and grew the HUD too early.
        let para = NSMutableParagraphStyle()
        para.lineBreakMode = .byWordWrapping
        let bounds = (text as NSString).boundingRect(
            with: NSSize(width: width, height: CGFloat.greatestFiniteMagnitude),
            options: [.usesLineFragmentOrigin, .usesFontLeading],
            attributes: [.font: font, .paragraphStyle: para]
        )
        let lineH = ceil(font.ascender - font.descender + font.leading)
        let lines = min(CGFloat(kMaxLiveLines), max(1, ceil(bounds.height / max(1, lineH))))
        return lines * lineH
    }

    @objc private func didTapStop() {
        stopAction?()
    }

    override func layout() {
        super.layout()

        let w = bounds.width
        let h = bounds.height
        blurView.frame = bounds
        borderView.frame = bounds

        let compact = (currentMode == .meeting && !isExpanded)
        let hPad: CGFloat = compact ? 10 : 16
        let vPad: CGFloat = compact ? 6 : 10
        let btnSize: CGFloat = compact ? 24 : 27
        let dotSize: CGFloat = 10
        let rowH: CGFloat = compact ? 30 : 42
        let headerY = vPad

        recordingDot.frame = CGRect(
            x: hPad,
            y: headerY + (rowH - dotSize) / 2,
            width: dotSize,
            height: dotSize
        )

        let btnX = w - hPad - btnSize
        stopButton.frame = CGRect(
            x: btnX,
            y: headerY + (rowH - btnSize) / 2,
            width: btnSize,
            height: btnSize
        )

        if compact {
            modeLabel.frame = .zero
            waveform.frame = .zero
            spinner.frame = .zero
            liveLabel.frame = .zero
        } else {
            let labelX = hPad + dotSize + 8
            let labelW: CGFloat = 96
            let labelH: CGFloat = 16
            modeLabel.frame = CGRect(
                x: labelX,
                y: headerY + (rowH - labelH) / 2,
                width: labelW,
                height: labelH
            )

            let midX = labelX + labelW + 4
            let midW = max(24, btnX - midX - 4)
            let waveH: CGFloat = 32
            waveform.frame = CGRect(
                x: midX,
                y: headerY + (rowH - waveH) / 2,
                width: midW,
                height: waveH
            )
            spinner.frame = CGRect(
                x: midX + (midW - 16) / 2,
                y: headerY + (rowH - 16) / 2,
                width: 16,
                height: 16
            )

            if !liveLabel.isHidden {
                let liveX = hPad + dotSize + 8
                let liveW = w - liveX - hPad
                let liveY = headerY + rowH + 8
                liveLabel.preferredMaxLayoutWidth = liveW
                liveLabel.frame = CGRect(
                    x: liveX,
                    y: liveY,
                    width: liveW,
                    height: max(0, h - liveY - vPad)
                )
            }
        }
    }

    override func mouseDown(with event: NSEvent) {
        window?.performDrag(with: event)
    }
}

/// Borderless panels return `canBecomeKey == false`. Override so VoiceOver
/// (and a click on Stop) can focus the button without making this the main
/// window. We still never `makeKeyAndOrderFront` on show — that would steal
/// the dictation target.
private final class PillFloatingPanel: NSPanel {
    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { false }
}

// ---------------------------------------------------------------------------
// MARK: - Panel singleton
// ---------------------------------------------------------------------------

private final class PillPanel {
    static let shared = PillPanel()

    private var panel: NSPanel?
    private var contentView: PillContentView?
    /// Bottom-left of the last *user* placement (drag or restore). Resizes
    /// pin the top edge using `lastAppliedSize`, not `panel.frame`, so a
    /// setFrame echo cannot walk the HUD up the screen.
    private var userOrigin: CGPoint?
    private var lastAppliedSize = CGSize(width: kCompactWidth, height: kCompactHeight)
    private var ignoringMove = false
    private let originLock = NSLock()
    private var stopCallback: PillStopCallback?
    private var currentMode: PillMode = .dictation
    private var currentTitle = "Dictating"
    private var currentStopLabel = "Stop recording"
    private var currentA11yLabel = "Dictation in progress"
    private var sessionMaxHeight: CGFloat = kCompactHeight

    private init() {}

    func create() {
        guard panel == nil else { return }

        let initSize = CGSize(width: kCompactWidth, height: kCompactHeight)
        let p = PillFloatingPanel(
            contentRect: CGRect(origin: .zero, size: initSize),
            styleMask: [.borderless, .nonactivatingPanel],
            backing: .buffered,
            defer: false
        )
        p.isFloatingPanel = true
        p.becomesKeyOnlyIfNeeded = true
        p.worksWhenModal = true
        p.isMovableByWindowBackground = true
        p.hasShadow = true
        p.level = NSWindow.Level(rawValue: Int(CGWindowLevelForKey(.statusWindow)))
        p.backgroundColor = .clear
        p.isOpaque = false
        p.hidesOnDeactivate = false
        p.animationBehavior = .none
        p.sharingType = .none
        p.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .ignoresCycle]
        p.orderOut(nil)

        let cv = PillContentView(frame: NSRect(origin: .zero, size: initSize))
        cv.autoresizingMask = [.width, .height]
        cv.stopAction = { [weak self] in
            guard let self else { return }
            self.stopCallback?(Int32(self.currentMode.rawValue))
        }
        p.contentView = cv
        p.initialFirstResponder = cv.stopControl
        p.setAccessibilityLabel(currentA11yLabel)

        NotificationCenter.default.addObserver(
            self,
            selector: #selector(windowDidMove(_:)),
            name: NSWindow.didMoveNotification,
            object: p
        )

        panel = p
        contentView = cv
        applyFrame(mode: .dictation, expanded: false)
    }

    func setVisible(_ visible: Bool) {
        guard let panel else { return }
        if visible {
            applyFrame(mode: currentMode, expanded: contentView?.isExpanded ?? false)
            panel.orderFrontRegardless()
        } else {
            sessionMaxHeight = kCompactHeight
            lastAppliedSize = CGSize(width: kCompactWidth, height: kCompactHeight)
            contentView?.setLiveText("")
            panel.orderOut(nil)
        }
    }

    private func targetSize(mode: PillMode, expanded: Bool) -> CGSize {
        if mode == .meeting && !expanded {
            sessionMaxHeight = kMeetingHeight
            return CGSize(width: kMeetingWidth, height: kMeetingHeight)
        }
        if expanded, mode == .dictation {
            let liveW = kExpandedWidth - 16 - 10 - 8 - 16
            let liveH = contentView?.liveTextHeight(forWidth: liveW) ?? 0
            // Header (64) + separator padding + wrapped tail. Grow only when
            // the line count changes, not per character.
            let needed = kCompactHeight + 8 + liveH
            sessionMaxHeight = max(sessionMaxHeight, min(kMaxHeight, max(kCompactHeight, needed)))
            return CGSize(width: kExpandedWidth, height: sessionMaxHeight)
        }
        sessionMaxHeight = kCompactHeight
        return CGSize(width: kCompactWidth, height: kCompactHeight)
    }

    private func applyFrame(mode: PillMode, expanded: Bool) {
        guard let panel else { return }
        let size = targetSize(mode: mode, expanded: expanded)
        let screen = placementScreen(for: lockedOrigin())
        let origin: CGPoint
        if let custom = lockedOrigin() {
            let top = custom.y + lastAppliedSize.height
            origin = clamp(screen: screen, size: size, x: custom.x, y: top - size.height)
            // Keep the stored bottom-left in sync with the new size so the
            // next resize still pins the same top. Do not record the default
            // top-center placement as a user origin (SOU-011: a saved default
            // survives a display change and looks "stuck").
            setLockedOrigin(origin)
        } else {
            let cx = screen.origin.x + (screen.size.width - size.width) / 2
            let cy = screen.origin.y + screen.size.height - kTopMargin - size.height
            origin = CGPoint(x: cx, y: cy)
        }

        let sameSize = abs(size.width - lastAppliedSize.width) < 1
            && abs(size.height - lastAppliedSize.height) < 1
        let current = panel.frame
        let sameOrigin = abs(origin.x - current.origin.x) < 1
            && abs(origin.y - current.origin.y) < 1
        if sameSize && sameOrigin {
            return
        }

        ignoringMove = true
        lastAppliedSize = size
        // Explicit animate:false — Reduce Motion and the old tao deadlock.
        panel.setFrame(CGRect(origin: origin, size: size), display: true, animate: false)
        ignoringMove = false
    }

    private func clamp(screen: CGRect, size: CGSize, x: CGFloat, y: CGFloat) -> CGPoint {
        let maxX = screen.origin.x + screen.size.width - size.width
        let maxY = screen.origin.y + screen.size.height - size.height
        return CGPoint(
            x: max(screen.origin.x, min(maxX, x)),
            y: max(screen.origin.y, min(maxY, y))
        )
    }

    private func placementScreen(for origin: CGPoint?) -> CGRect {
        let screens = NSScreen.screens.map(\.frame)
        if let origin,
           let hit = screens.first(where: { $0.contains(origin) }) {
            return hit
        }
        return NSScreen.main?.frame
            ?? NSScreen.screens.first?.frame
            ?? CGRect(x: 0, y: 0, width: 1440, height: 900)
    }

    func setMode(_ mode: PillMode, title: String, stopLabel: String, a11yLabel: String) {
        currentMode = mode
        currentTitle = title
        currentStopLabel = stopLabel
        currentA11yLabel = a11yLabel
        let expanded = contentView?.isExpanded ?? false
        contentView?.applyMode(mode, title: title, stopLabel: stopLabel,
                               a11yLabel: a11yLabel, expanded: expanded)
        panel?.setAccessibilityLabel(a11yLabel)
        applyFrame(mode: mode, expanded: expanded)
    }

    func setLiveText(_ text: String) {
        let expanded = !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        contentView?.setLiveText(text)
        applyFrame(mode: currentMode, expanded: expanded)
    }

    func pushRMS(_ level: Float) {
        contentView?.pushRMS(level)
    }

    func restoreOrigin(x: Double, y: Double) {
        setLockedOrigin(CGPoint(x: x, y: y))
    }

    func getOrigin() -> CGPoint? {
        lockedOrigin()
    }

    func setStopCallback(_ cb: PillStopCallback?) {
        stopCallback = cb
    }

    @objc private func windowDidMove(_ _: Notification) {
        guard !ignoringMove, let frame = panel?.frame else { return }
        setLockedOrigin(frame.origin)
        lastAppliedSize = frame.size
    }

    private func lockedOrigin() -> CGPoint? {
        originLock.lock()
        defer { originLock.unlock() }
        return userOrigin
    }

    private func setLockedOrigin(_ origin: CGPoint?) {
        originLock.lock()
        userOrigin = origin
        originLock.unlock()
    }
}

// ---------------------------------------------------------------------------
// MARK: - C bridge (called from Rust via FFI)
// ---------------------------------------------------------------------------

@_cdecl("pill_panel_create")
public func pill_panel_create() {
    onMain { PillPanel.shared.create() }
}

@_cdecl("pill_panel_set_visible")
public func pill_panel_set_visible(_ visible: Int32) {
    onMain { PillPanel.shared.setVisible(visible != 0) }
}

@_cdecl("pill_panel_set_mode")
public func pill_panel_set_mode(
    _ mode: Int32,
    _ title: UnsafePointer<CChar>?,
    _ stopLabel: UnsafePointer<CChar>?,
    _ a11yLabel: UnsafePointer<CChar>?
) {
    let m = PillMode(rawValue: Int(mode)) ?? .dictation
    let titleStr = title.map { String(cString: $0) } ?? ""
    let stopStr = stopLabel.map { String(cString: $0) } ?? ""
    let a11yStr = a11yLabel.map { String(cString: $0) } ?? ""
    onMain { PillPanel.shared.setMode(m, title: titleStr, stopLabel: stopStr, a11yLabel: a11yStr) }
}

@_cdecl("pill_panel_set_live_text")
public func pill_panel_set_live_text(_ cStr: UnsafePointer<CChar>?) {
    let text = cStr.map { String(cString: $0) } ?? ""
    onMain { PillPanel.shared.setLiveText(text) }
}

@_cdecl("pill_panel_push_rms")
public func pill_panel_push_rms(_ level: Float) {
    onMain { PillPanel.shared.pushRMS(level) }
}

@_cdecl("pill_panel_restore_origin")
public func pill_panel_restore_origin(_ x: Double, _ y: Double) {
    onMain { PillPanel.shared.restoreOrigin(x: x, y: y) }
}

@_cdecl("pill_panel_get_origin")
public func pill_panel_get_origin(
    _ outX: UnsafeMutablePointer<Double>?,
    _ outY: UnsafeMutablePointer<Double>?
) -> Int32 {
    guard let pt = PillPanel.shared.getOrigin() else { return 0 }
    outX?.pointee = Double(pt.x)
    outY?.pointee = Double(pt.y)
    return 1
}

@_cdecl("pill_panel_set_stop_callback")
public func pill_panel_set_stop_callback(_ cb: PillStopCallback?) {
    onMain { PillPanel.shared.setStopCallback(cb) }
}
