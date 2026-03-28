import { describe, it, expect } from 'vitest';
import type { TranscriptionCatalog } from '../../types';
import {
  getSelectedTranscriptionEngine,
  getSelectedTranscriptionModel,
  toSelectedTranscriptionProfile,
  formatSelectedTranscriptionLabel,
} from './catalog';

const catalog: TranscriptionCatalog = {
  engines: [
    {
      id: 'kyutai',
      label: 'Kyutai STT',
      description: 'Local Kyutai speech-to-text',
      supports_streaming: true,
      models: [
        {
          id: 'stt-1b-en_fr',
          label: 'STT 1B EN/FR',
          description: 'English/French model',
          download_size_bytes: 2400000000,
          supported_languages: ['en', 'fr'],
        },
        {
          id: 'stt-small',
          label: 'STT Small',
          description: 'Small model',
          download_size_bytes: 500000000,
          supported_languages: ['en'],
        },
      ],
    },
    {
      id: 'whisper',
      label: 'Whisper',
      description: 'OpenAI Whisper',
      supports_streaming: false,
      models: [
        {
          id: 'whisper-base',
          label: 'Whisper Base',
          description: 'Base model',
          download_size_bytes: 150000000,
          supported_languages: ['en'],
        },
      ],
    },
  ],
  selected_engine_id: 'kyutai',
  selected_model_id: 'stt-1b-en_fr',
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
    const profile = toSelectedTranscriptionProfile(catalog, 'kyutai', 'stt-1b-en_fr');
    expect(profile).toEqual({
      engine_id: 'kyutai',
      engine_label: 'Kyutai STT',
      model_id: 'stt-1b-en_fr',
      model_label: 'STT 1B EN/FR',
    });
  });

  it('falls back to raw IDs for null catalog', () => {
    const profile = toSelectedTranscriptionProfile(null, 'raw-engine', 'raw-model');
    expect(profile).toEqual({
      engine_id: 'raw-engine',
      engine_label: 'raw-engine',
      model_id: 'raw-model',
      model_label: 'raw-model',
    });
  });
});

describe('formatSelectedTranscriptionLabel', () => {
  it('returns formatted string for valid data', () => {
    const label = formatSelectedTranscriptionLabel(catalog, 'kyutai', 'stt-1b-en_fr');
    expect(label).toBe('Kyutai STT \u2022 STT 1B EN/FR');
  });

  it('returns raw IDs joined for null catalog', () => {
    const label = formatSelectedTranscriptionLabel(null, 'raw-engine', 'raw-model');
    expect(label).toBe('raw-engine \u2022 raw-model');
  });
});
