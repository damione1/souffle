#ifndef pill_bridge_h
#define pill_bridge_h

// C-compatible declarations for the native HUD pill panel (SOU-051).
// The NSPanel and all AppKit content live entirely in Swift; Rust calls
// these functions to drive visibility and content updates.

#ifdef __cplusplus
extern "C" {
#endif

// ---------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------

/// Create the NSPanel. Safe to call from any thread; hops to the main
/// thread internally. No-op if already created.
void pill_panel_create(void);

// ---------------------------------------------------------------------------
// Visibility
// ---------------------------------------------------------------------------

/// Show or hide the panel. `visible = 1` calls `orderFrontRegardless`;
/// `visible = 0` calls `orderOut:`. Safe from any thread.
void pill_panel_set_visible(int visible);

// ---------------------------------------------------------------------------
// Content
// ---------------------------------------------------------------------------

/// Update the mode, labels, and layout.
/// `mode`: 0 = dictation, 1 = meeting (compact), 2 = polishing
/// `title` / `stop_label` / `a11y_label` are copied immediately (UTF-8);
/// they may be NULL (treated as empty).
void pill_panel_set_mode(int mode, const char* title, const char* stop_label, const char* a11y_label);

/// Update the live dictation text shown below the header row.
/// Pass NULL or empty string to hide the live-text area.
void pill_panel_set_live_text(const char* text);

/// Push a new RMS audio level for the waveform bars (0.0–1.0).
/// May be called from any thread; internally dispatched to the main thread.
void pill_panel_push_rms(float level);

// ---------------------------------------------------------------------------
// Position persistence
// ---------------------------------------------------------------------------

/// Store a restored pill origin (bottom-left, AppKit global points) from the
/// database before the panel is first shown.
void pill_panel_restore_origin(double x, double y);

/// Retrieve the current panel origin after a user drag (for DB persistence).
/// Returns 0 if no origin has been stored yet.
int pill_panel_get_origin(double* out_x, double* out_y);

// ---------------------------------------------------------------------------
// Stop callback
// ---------------------------------------------------------------------------

/// Function pointer type for the stop action. The `recording_mode` argument
/// mirrors the `mode` last passed to `pill_panel_set_mode` so the Rust side
/// can dispatch to dictation stop or the meeting controller.
typedef void (*PillStopCallback)(int recording_mode);

/// Register the callback that fires when the user taps Stop.
/// Replaces any previously registered callback. Pass NULL to clear.
void pill_panel_set_stop_callback(PillStopCallback callback);

#ifdef __cplusplus
}
#endif

#endif /* pill_bridge_h */
