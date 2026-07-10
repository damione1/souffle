import { describe, it, expect, beforeEach, vi } from 'vitest';
import { applyTheme } from './theme';

describe('applyTheme', () => {
  beforeEach(() => {
    document.documentElement.className = '';
    localStorage.clear();
  });

  it('dark theme adds dark class and removes light', () => {
    applyTheme('dark');

    expect(document.documentElement.classList.contains('dark')).toBe(true);
    expect(document.documentElement.classList.contains('light')).toBe(false);
  });

  it('light theme adds light class and removes dark', () => {
    applyTheme('light');

    expect(document.documentElement.classList.contains('light')).toBe(true);
    expect(document.documentElement.classList.contains('dark')).toBe(false);
  });

  it('system theme respects prefers-color-scheme dark', () => {
    // Mock matchMedia to return dark preference
    Object.defineProperty(window, 'matchMedia', {
      writable: true,
      value: vi.fn().mockImplementation((query: string) => ({
        matches: query === '(prefers-color-scheme: dark)',
        media: query,
        onchange: null,
        addListener: vi.fn(),
        removeListener: vi.fn(),
        addEventListener: vi.fn(),
        removeEventListener: vi.fn(),
        dispatchEvent: vi.fn(),
      })),
    });

    applyTheme('system');

    expect(document.documentElement.classList.contains('dark')).toBe(true);
    expect(document.documentElement.classList.contains('light')).toBe(false);
  });

  it('system theme respects prefers-color-scheme light', () => {
    Object.defineProperty(window, 'matchMedia', {
      writable: true,
      value: vi.fn().mockImplementation((query: string) => ({
        matches: false,
        media: query,
        onchange: null,
        addListener: vi.fn(),
        removeListener: vi.fn(),
        addEventListener: vi.fn(),
        removeEventListener: vi.fn(),
        dispatchEvent: vi.fn(),
      })),
    });

    applyTheme('system');

    expect(document.documentElement.classList.contains('light')).toBe(true);
    expect(document.documentElement.classList.contains('dark')).toBe(false);
  });

  it('dark theme persists the setting to localStorage', () => {
    applyTheme('dark');

    expect(localStorage.getItem('souffle-theme')).toBe('dark');
  });

  it('light theme persists the setting to localStorage', () => {
    applyTheme('light');

    expect(localStorage.getItem('souffle-theme')).toBe('light');
  });

  it('system theme persists the raw setting, not the resolved value', () => {
    Object.defineProperty(window, 'matchMedia', {
      writable: true,
      value: vi.fn().mockImplementation((query: string) => ({
        matches: query === '(prefers-color-scheme: dark)',
        media: query,
        onchange: null,
        addListener: vi.fn(),
        removeListener: vi.fn(),
        addEventListener: vi.fn(),
        removeEventListener: vi.fn(),
        dispatchEvent: vi.fn(),
      })),
    });

    applyTheme('system');

    expect(localStorage.getItem('souffle-theme')).toBe('system');
  });

  it('handles blocked localStorage gracefully', () => {
    const setItemSpy = vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
      throw new Error('localStorage blocked');
    });

    expect(() => applyTheme('dark')).not.toThrow();
    expect(document.documentElement.classList.contains('dark')).toBe(true);

    setItemSpy.mockRestore();
  });
});
