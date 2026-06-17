import type {
  TranscriptionCatalog,
  TranscriptionEngineDescriptor,
  TranscriptionModelDescriptor,
  TranscriptionProfile,
  TranscriptionProfileSelection,
  TranscriptionRuntimeBackendDescriptor,
} from "../../types";

export function isTranscriptionBackendAvailable(
  backend: TranscriptionRuntimeBackendDescriptor,
): boolean {
  return backend.available_in_app;
}

export function isTranscriptionModelAvailable(
  model: TranscriptionModelDescriptor,
): boolean {
  return model.available_in_app && model.backends.some(isTranscriptionBackendAvailable);
}

export function hasAvailableTranscriptionModel(
  engine: TranscriptionEngineDescriptor,
): boolean {
  return engine.models.some(isTranscriptionModelAvailable);
}

export function getFirstAvailableTranscriptionModel(
  engine: TranscriptionEngineDescriptor | null,
): TranscriptionModelDescriptor | null {
  return engine?.models.find(isTranscriptionModelAvailable) ?? engine?.models[0] ?? null;
}

export function getFirstAvailableTranscriptionBackend(
  model: TranscriptionModelDescriptor | null,
): TranscriptionRuntimeBackendDescriptor | null {
  return model?.backends.find(isTranscriptionBackendAvailable) ?? model?.backends[0] ?? null;
}

export function getSelectedTranscriptionEngine(
  catalog: TranscriptionCatalog | null,
  engineId: string,
): TranscriptionEngineDescriptor | null {
  const exact = catalog?.engines.find((engine) => engine.id === engineId);
  return (
    exact
    ?? catalog?.engines.find(hasAvailableTranscriptionModel)
    ?? catalog?.engines[0]
    ?? null
  );
}

export function getSelectedTranscriptionModel(
  catalog: TranscriptionCatalog | null,
  engineId: string,
  modelId: string,
): TranscriptionModelDescriptor | null {
  const engine = getSelectedTranscriptionEngine(catalog, engineId);
  return (
    engine?.models.find((model) => model.id === modelId)
    ?? getFirstAvailableTranscriptionModel(engine)
    ?? null
  );
}

export function getSelectedTranscriptionBackend(
  catalog: TranscriptionCatalog | null,
  engineId: string,
  modelId: string,
  backendId: string,
): TranscriptionRuntimeBackendDescriptor | null {
  const model = getSelectedTranscriptionModel(catalog, engineId, modelId);
  return (
    model?.backends.find((backend) => backend.id === backendId)
    ?? getFirstAvailableTranscriptionBackend(model)
    ?? null
  );
}

export function toSelectedTranscriptionProfile(
  catalog: TranscriptionCatalog | null,
  engineId: string,
  modelId: string,
  backendId: string,
): TranscriptionProfile {
  const engine = getSelectedTranscriptionEngine(catalog, engineId);
  const model = getSelectedTranscriptionModel(catalog, engineId, modelId);
  const backend = getSelectedTranscriptionBackend(catalog, engineId, modelId, backendId);

  return {
    engine_id: engine?.id ?? engineId,
    engine_label: engine?.label ?? engineId,
    model_id: model?.id ?? modelId,
    model_label: model?.label ?? modelId,
    backend_id: backend?.id ?? backendId,
    backend_label: backend?.label ?? backendId,
  };
}

export function toSelectedTranscriptionProfileSelection(
  catalog: TranscriptionCatalog | null,
  engineId: string,
  modelId: string,
  backendId: string,
): TranscriptionProfileSelection {
  const profile = toSelectedTranscriptionProfile(catalog, engineId, modelId, backendId);
  return {
    engine_id: profile.engine_id,
    model_id: profile.model_id,
    backend_id: profile.backend_id ?? backendId,
  };
}

export interface FlatModelOption {
  key: string;
  engineId: string;
  modelId: string;
  backendId: string;
  label: string;
}

/** All installable (engine, model) pairs flattened for simple pickers,
 * with the backend auto-resolved. */
export function listAvailableModelOptions(
  catalog: TranscriptionCatalog | null,
): FlatModelOption[] {
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
}

export function formatSelectedTranscriptionLabel(
  catalog: TranscriptionCatalog | null,
  engineId: string,
  modelId: string,
  backendId: string,
): string {
  const profile = toSelectedTranscriptionProfile(catalog, engineId, modelId, backendId);
  return [profile.engine_label, profile.model_label, profile.backend_label]
    .filter(Boolean)
    .join(" • ");
}
