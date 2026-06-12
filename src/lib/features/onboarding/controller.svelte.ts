import { getAppState } from "../../stores/app.svelte";
import { getTranscriptionCatalog } from "../../api/transcription";
import { saveSettings } from "../../api/settings";
import type { AppSettings, TranscriptionCatalog } from "../../types";
import { errorMessage } from "../../utils";
import {
  getFirstAvailableTranscriptionBackend,
  isTranscriptionModelAvailable,
} from "../transcription/catalog";
import {
  resetTranscriptionRuntimeState,
  startTranscriptionModelDownload,
} from "../transcription/runtime";

export interface OnboardingModelOption {
  key: string;
  engineId: string;
  modelId: string;
  backendId: string;
  label: string;
}

export function createOnboardingController() {
  const app = getAppState();

  let catalog = $state<TranscriptionCatalog | null>(null);
  let selectedKey = $state("");
  let statusMessage = $state("");
  let isStarting = $state(false);

  const options = $derived.by((): OnboardingModelOption[] => {
    if (!catalog) return [];
    return catalog.engines.flatMap((engine) =>
      engine.models.filter(isTranscriptionModelAvailable).map((model) => {
        const backend = getFirstAvailableTranscriptionBackend(model);
        return {
          key: `${engine.id}:${model.id}`,
          engineId: engine.id,
          modelId: model.id,
          backendId: backend?.id ?? "",
          label: `${engine.label} — ${model.label}`,
        };
      }),
    );
  });

  async function mount() {
    try {
      catalog = await getTranscriptionCatalog();
      selectedKey = `${catalog.selected_engine_id}:${catalog.selected_model_id}`;
      if (!options.some((option) => option.key === selectedKey)) {
        selectedKey = options[0]?.key ?? "";
      }
    } catch (e) {
      statusMessage = errorMessage(e);
    }
  }

  /** Persist the chosen model, then download and load it in one go. */
  async function begin() {
    const option = options.find((candidate) => candidate.key === selectedKey);
    if (!option || isStarting) return;

    isStarting = true;
    statusMessage = "";
    try {
      const nextSettings: AppSettings = {
        ...app.settings,
        transcription_engine_id: option.engineId,
        transcription_model_id: option.modelId,
        transcription_backend_id: option.backendId,
      };
      await saveSettings(nextSettings);
      app.settings = nextSettings;
      resetTranscriptionRuntimeState(app);

      await startTranscriptionModelDownload(
        app,
        catalog,
        (message) => {
          statusMessage = message;
        },
        { autoLoad: true },
      );
    } catch (e) {
      statusMessage = errorMessage(e);
      isStarting = false;
    }
  }

  return {
    get app() { return app; },
    get options() { return options; },
    get selectedKey() { return selectedKey; },
    set selectedKey(key: string) { selectedKey = key; },
    get statusMessage() { return statusMessage; },
    get isStarting() { return isStarting; },
    mount,
    begin,
  };
}
