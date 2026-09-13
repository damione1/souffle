import { commands } from "../types/generated";

/**
 * Wire names of the Tauri commands the test suite mocks.
 *
 * Tests hook `window.__TAURI_INTERNALS__.invoke`, below the generated client,
 * where a command is a bare string. Writing that string by hand means a rename
 * on the Rust side regenerates the client, breaks nothing at compile time, and
 * leaves a mock that silently stops matching: the suite stays green while it
 * has stopped exercising the path it claims to.
 *
 * Keying the table on `keyof typeof commands` fixes the first half: a renamed
 * command removes the key and fails `npm run check` here. `commands.test.ts`
 * fixes the second half by calling each client method under a recording mock
 * and asserting the wire name it actually sends.
 */
export const COMMAND = {
  addDictationEntry: "add_dictation_entry",
  checkForUpdates: "check_for_updates",
  checkSummaryProviders: "check_summary_providers",
  clearDictationHistory: "clear_dictation_history",
  copyText: "copy_text",
  deleteDictationEntry: "delete_dictation_entry",
  deleteMeeting: "delete_meeting",
  deleteModel: "delete_model",
  downloadModel: "download_model",
  exportArchive: "export_archive",
  exportMeetingAudioFilename: "export_meeting_audio_filename",
  exportMeetingAudioToFile: "export_meeting_audio_to_file",
  exportMeetingFilename: "export_meeting_filename",
  exportMeetingPreview: "export_meeting_preview",
  exportMeetingToFile: "export_meeting_to_file",
  frontmostAppName: "frontmost_app_name",
  getAppVersion: "get_app_version",
  getDataStats: "get_data_stats",
  getDiagnosticsText: "get_diagnostics_text",
  getInputSampleRate: "get_input_sample_rate",
  getLogTail: "get_log_tail",
  getMcpSetupInfo: "get_mcp_setup_info",
  getMeeting: "get_meeting",
  getModelStatus: "get_model_status",
  getReleaseNotesForVersion: "get_release_notes_for_version",
  getSettings: "get_settings",
  getShortcuts: "get_shortcuts",
  getTranscriptionCatalog: "get_transcription_catalog",
  learnFromEdit: "learn_from_edit",
  listAudioDevices: "list_audio_devices",
  listCalendars: "list_calendars",
  listDictationEntries: "list_dictation_entries",
  listMeetings: "list_meetings",
  listSnippets: "list_snippets",
  loadModel: "load_model",
  notifyPasteFailed: "notify_paste_failed",
  pasteText: "paste_text",
  pillHold: "pill_hold",
  pillRelease: "pill_release",
  polishDictation: "polish_dictation",
  pullRecommendedOllamaModel: "pull_recommended_ollama_model",
  readFocusedText: "read_focused_text",
  readSelectedText: "read_selected_text",
  requestPermission: "request_permission",
  resetInputSampleRate: "reset_input_sample_rate",
  resumeMeetingRecording: "resume_meeting_recording",
  revealDataDir: "reveal_data_dir",
  saveEditedTranscript: "save_edited_transcript",
  saveMeetingAudioExport: "save_meeting_audio_export",
  saveMeetingExport: "save_meeting_export",
  saveSettings: "save_settings",
  saveShortcuts: "save_shortcuts",
  searchText: "search_text",
  selectAudioDevice: "select_audio_device",
  startMeetingRecording: "start_meeting_recording",
  startTranscription: "start_transcription",
  stopMeetingRecording: "stop_meeting_recording",
  stopTranscription: "stop_transcription",
  summarizeMeeting: "summarize_meeting",
  testMcpConnection: "test_mcp_connection",
  updateDictationEntry: "update_dictation_entry",
} satisfies Partial<Record<keyof typeof commands, string>>;
