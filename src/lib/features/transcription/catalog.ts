import type {
  TranscriptionCatalog,
  TranscriptionEngineDescriptor,
  TranscriptionModelDescriptor,
  TranscriptionProfile,
} from "../../types";

export function getSelectedTranscriptionEngine(
  catalog: TranscriptionCatalog | null,
  engineId: string,
): TranscriptionEngineDescriptor | null {
  return (
    catalog?.engines.find((engine) => engine.id === engineId)
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
  return engine?.models.find((model) => model.id === modelId) ?? engine?.models[0] ?? null;
}

export function toSelectedTranscriptionProfile(
  catalog: TranscriptionCatalog | null,
  engineId: string,
  modelId: string,
): TranscriptionProfile {
  const engine = getSelectedTranscriptionEngine(catalog, engineId);
  const model = getSelectedTranscriptionModel(catalog, engineId, modelId);

  return {
    engine_id: engine?.id ?? engineId,
    engine_label: engine?.label ?? engineId,
    model_id: model?.id ?? modelId,
    model_label: model?.label ?? modelId,
  };
}

export function formatSelectedTranscriptionLabel(
  catalog: TranscriptionCatalog | null,
  engineId: string,
  modelId: string,
): string {
  const profile = toSelectedTranscriptionProfile(catalog, engineId, modelId);
  return [profile.engine_label, profile.model_label].filter(Boolean).join(" • ");
}
