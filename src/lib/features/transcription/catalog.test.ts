import { describe, it, expect } from 'vitest';
import type { TranscriptionCatalog } from '../../types';
import {
  getSelectedTranscriptionBackend,
  getSelectedTranscriptionEngine,
  getSelectedTranscriptionModel,
  toSelectedTranscriptionProfile,
  toSelectedTranscriptionProfileSelection,
  formatSelectedTranscriptionLabel,
} from './catalog';

function backend(id: string, label: string) {
  return {
    id,
    label,
    description: `${label} runtime`,
    recommended: id === 'candle',
    available_in_app: id !== 'mlx',
    availability_note: id === 'mlx' ? `${label} is coming soon.` : null,
    artifacts: [],
  };
}

function model(id: string, label: string, languages: string[]) {
  return {
    id,
    label,
    description: `${label} description`,
    download_size_bytes: 1_000_000_000,
    recommended_memory_bytes: 2_000_000_000,
    supported_languages: languages,
    capabilities: {
      supports_streaming: true,
      supports_batch_transcription: false,
      supports_language_auto_detect: true,
      supports_word_timestamps: true,
      supports_partial_results: true,
    },
    audio_input: {
      sample_rate_hz: 24000,
      channels: 1,
      chunk_size_samples: 1920,
    },
    available_in_app: id !== 'whisper-base',
    availability_note: id === 'whisper-base' ? 'Whisper is coming soon.' : null,
    backends: [backend('candle', 'Candle'), backend('mlx', 'MLX')],
    recommended_backend_id: 'candle',
  };
}

const catalog: TranscriptionCatalog = {
  engines: [
    {
      id: 'kyutai',
      label: 'Kyutai STT',
      description: 'Local Kyutai speech-to-text',
      models: [
        model('stt-1b-en_fr', 'STT 1B EN/FR', ['en', 'fr']),
        model('stt-small', 'STT Small', ['en']),
      ],
    },
    {
      id: 'whisper',
      label: 'Whisper',
      description: 'OpenAI Whisper',
      models: [
        model('whisper-base', 'Whisper Base', ['en']),
      ],
    },
  ],
  selected_engine_id: 'kyutai',
  selected_model_id: 'stt-1b-en_fr',
  selected_backend_id: 'candle',
};

describe('getSelectedTranscriptionEngine', () => {
  it('returns matching engine for valid id', () => {
    const engine = getSelectedTranscriptionEngine(catalog, 'whisper');
    expect(engine).not.toBeNull();
    expect(engine!.id).toBe('whisper');
    expect(engine!.label).toBe('Whisper');
  });

  it('falls back to first engine for unknown id', () => {
    const engine = getSelectedTranscriptionEngine(catalog, 'nonexistent');
    expect(engine).not.toBeNull();
    expect(engine!.id).toBe('kyutai');
  });

  it('returns null for null catalog', () => {
    const engine = getSelectedTranscriptionEngine(null, 'kyutai');
    expect(engine).toBeNull();
  });
});

describe('getSelectedTranscriptionModel', () => {
  it('returns matching model for valid engine and model id', () => {
    const model = getSelectedTranscriptionModel(catalog, 'kyutai', 'stt-small');
    expect(model).not.toBeNull();
    expect(model!.id).toBe('stt-small');
    expect(model!.label).toBe('STT Small');
  });

  it('falls back to first model for unknown model id', () => {
    const model = getSelectedTranscriptionModel(catalog, 'kyutai', 'nonexistent');
    expect(model).not.toBeNull();
    expect(model!.id).toBe('stt-1b-en_fr');
  });
});

describe('toSelectedTranscriptionProfile', () => {
  it('returns correct profile object with resolved labels', () => {
    const profile = toSelectedTranscriptionProfile(catalog, 'kyutai', 'stt-1b-en_fr', 'candle');
    expect(profile).toEqual({
      engine_id: 'kyutai',
      engine_label: 'Kyutai STT',
      model_id: 'stt-1b-en_fr',
      model_label: 'STT 1B EN/FR',
      backend_id: 'candle',
      backend_label: 'Candle',
    });
  });

  it('falls back to raw IDs for null catalog', () => {
    const profile = toSelectedTranscriptionProfile(null, 'raw-engine', 'raw-model', 'raw-backend');
    expect(profile).toEqual({
      engine_id: 'raw-engine',
      engine_label: 'raw-engine',
      model_id: 'raw-model',
      model_label: 'raw-model',
      backend_id: 'raw-backend',
      backend_label: 'raw-backend',
    });
  });
});

describe('getSelectedTranscriptionBackend', () => {
  it('returns matching backend for valid ids', () => {
    const selected = getSelectedTranscriptionBackend(catalog, 'kyutai', 'stt-1b-en_fr', 'mlx');
    expect(selected?.id).toBe('mlx');
  });

  it('falls back to first backend when unknown', () => {
    const selected = getSelectedTranscriptionBackend(catalog, 'kyutai', 'stt-1b-en_fr', 'unknown');
    expect(selected?.id).toBe('candle');
  });

  it('returns exact backend even when unavailable so roadmap states can render', () => {
    const selected = getSelectedTranscriptionBackend(catalog, 'kyutai', 'stt-1b-en_fr', 'mlx');
    expect(selected?.id).toBe('mlx');
  });
});

describe('toSelectedTranscriptionProfileSelection', () => {
  it('returns normalized selection ids', () => {
    const selection = toSelectedTranscriptionProfileSelection(catalog, 'kyutai', 'stt-1b-en_fr', 'mlx');
    expect(selection).toEqual({
      engine_id: 'kyutai',
      model_id: 'stt-1b-en_fr',
      backend_id: 'mlx',
    });
  });
});

describe('formatSelectedTranscriptionLabel', () => {
  it('returns formatted string for valid data', () => {
    const label = formatSelectedTranscriptionLabel(catalog, 'kyutai', 'stt-1b-en_fr', 'candle');
    expect(label).toBe('Kyutai STT \u2022 STT 1B EN/FR \u2022 Candle');
  });

  it('returns raw IDs joined for null catalog', () => {
    const label = formatSelectedTranscriptionLabel(null, 'raw-engine', 'raw-model', 'raw-backend');
    expect(label).toBe('raw-engine \u2022 raw-model \u2022 raw-backend');
  });
});
