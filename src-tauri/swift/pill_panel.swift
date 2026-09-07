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
private let kWaveformBars: Int = 12
private let kMaxLiveLines: Int = 5

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
// MARK: - Waveform layer
// ---------------------------------------------------------------------------

/// CALayer RMS bars, centered in the layer (same geometry as the Svelte pill waveform).
private final class WaveformLayer: CALayer {
    private var barLayers: [CALayer] = []
    private var levels: [Float] = Array(repeating: 0.12, count: kWaveformBars)

    override init() {
        super.init()
        setup()
    }

    override init(layer: Any) {
        super.init(layer: layer)
    }

    required init?(coder: NSCoder) { nil }

    private func setup() {
        backgroundColor = NSColor.clear.cgColor
        barLayers = (0..<kWaveformBars).map { _ in
            let layer = CALayer()
            layer.backgroundColor = kAccent.withAlphaComponent(0.7).cgColor
            layer.cornerRadius = 1.5
            addSublayer(layer)
            return layer
        }
    }

    func push(rms: Float) {
        let smoothed = max(0.05, min(1, rms))
        levels.removeFirst()
        levels.append(smoothed)
        setNeedsLayout()
    }

    override func layoutSublayers() {
        super.layoutSublayers()
        let w = bounds.width
        let h = bounds.height
        guard w > 0, h > 0, !barLayers.isEmpty else { return }

        let barWidth: CGFloat = 3
        let totalBars = CGFloat(kWaveformBars)
        let gap = max(2, (w - barWidth * totalBars) / (totalBars + 1))
        let occupied = barWidth * totalBars + gap * (totalBars - 1)
        let offsetX = (w - occupied) / 2

        CATransaction.begin()
        CATransaction.setDisableActions(true)
        for (i, layer) in barLayers.enumerated() {
            let barH = max(4, (h - 4) * CGFloat(levels[i]))
            let x = offsetX + CGFloat(i) * (barWidth + gap)
            let y = (h - barH) / 2
            layer.frame = CGRect(x: x, y: y, width: barWidth, height: barH)
        }
        CATransaction.commit()
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
    let liveLabel = NSTextField(wrappingLabelWithString: "")
    private let stopButton = NSButton()
    private let waveformHost = NSView()
    private let waveform = WaveformLayer()
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
        liveLabel.font = NSFont.systemFont(ofSize: 13)
        liveLabel.maximumNumberOfLines = kMaxLiveLines
        liveLabel.lineBreakMode = .byTruncatingTail
        liveLabel.isEditable = false
        liveLabel.isBezeled = false
        liveLabel.drawsBackground = false
        liveLabel.isHidden = true
        addSubview(liveLabel)

        waveformHost.wantsLayer = true
        waveformHost.layer?.backgroundColor = NSColor.clear.cgColor
        waveformHost.setAccessibilityElement(false)
        waveform.frame = CGRect(x: 0, y: 0, width: 80, height: 24)
        waveformHost.layer?.addSublayer(waveform)
        addSubview(waveformHost)

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
        stopButton.target = self
        stopButton.action = #selector(didTapStop)
        addSubview(stopButton)

        applyMode(.dictation, title: "Dictating", stopLabel: "Stop recording",
                  a11yLabel: "Dictation in progress", expanded: false)
    }

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

        waveformHost.isHidden = (mode != .dictation)
        waveform.isHidden = (mode != .dictation)
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

    func liveTextHeight(forWidth width: CGFloat) -> CGFloat {
        guard isExpanded, !liveLabel.isHidden else { return 0 }
        liveLabel.preferredMaxLayoutWidth = width
        let fitting = liveLabel.sizeThatFits(NSSize(width: width, height: CGFloat.greatestFiniteMagnitude))
        let lineHeight = liveLabel.font?.boundingRectForFont.height ?? 16
        let maxH = lineHeight * CGFloat(kMaxLiveLines) + 4
        return min(maxH, max(lineHeight, fitting.height))
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
            waveformHost.frame = .zero
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
            let waveH: CGFloat = 24
            waveformHost.frame = CGRect(
                x: midX,
                y: headerY + (rowH - waveH) / 2,
                width: midW,
                height: waveH
            )
            waveform.frame = waveformHost.bounds
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

// ---------------------------------------------------------------------------
// MARK: - Panel singleton
// ---------------------------------------------------------------------------

private final class PillPanel {
    static let shared = PillPanel()

    private var panel: NSPanel?
    private var contentView: PillContentView?
    private var storedOrigin: CGPoint?
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
        let p = NSPanel(
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
            let needed = kCompactHeight + liveH + 8
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
            let currentHeight = panel.frame.height > 0 ? panel.frame.height : size.height
            let top = custom.y + currentHeight
            origin = clamp(screen: screen, size: size, x: custom.x, y: top - size.height)
        } else {
            let cx = screen.origin.x + (screen.size.width - size.width) / 2
            let cy = screen.origin.y + screen.size.height - kTopMargin - size.height
            origin = CGPoint(x: cx, y: cy)
        }
        setLockedOrigin(origin)
        panel.setFrame(CGRect(origin: origin, size: size), display: true)
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
        guard let frame = panel?.frame else { return }
        setLockedOrigin(frame.origin)
    }

    private func lockedOrigin() -> CGPoint? {
        originLock.lock()
        defer { originLock.unlock() }
        return storedOrigin
    }

    private func setLockedOrigin(_ origin: CGPoint?) {
        originLock.lock()
        storedOrigin = origin
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
