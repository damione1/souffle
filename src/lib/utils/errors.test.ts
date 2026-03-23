import { describe, it, expect } from 'vitest';
import { errorMessage } from './errors';

describe('errorMessage', () => {
  it('extracts message from Error object', () => {
    expect(errorMessage(new Error('test error'))).toBe('test error');
  });

  it('converts string input', () => {
    expect(errorMessage('string error')).toBe('string error');
  });

  it('converts unknown type', () => {
    expect(errorMessage(42)).toBe('42');
  });

  it('converts null', () => {
    expect(errorMessage(null)).toBe('null');
  });

  it('converts undefined', () => {
    expect(errorMessage(undefined)).toBe('undefined');
  });
});
