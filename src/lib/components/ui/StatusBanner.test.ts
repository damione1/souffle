import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/svelte';
import StatusBanner from './StatusBanner.svelte';

describe('StatusBanner', () => {
  it('renders message text', () => {
    render(StatusBanner, { props: { message: 'Model is loading' } });

    expect(screen.getByText('Model is loading')).toBeTruthy();
  });

  it('applies warning variant outline class', () => {
    const { container } = render(StatusBanner, { props: { message: 'Warning!', variant: 'warning' } });

    const banner = container.querySelector('div');
    expect(banner?.className).toContain('outline-warning/30');
  });

  it('applies danger variant outline class', () => {
    const { container } = render(StatusBanner, { props: { message: 'Error!', variant: 'danger' } });

    const banner = container.querySelector('div');
    expect(banner?.className).toContain('outline-danger/30');
  });

  it('applies default info variant outline class', () => {
    const { container } = render(StatusBanner, { props: { message: 'Info' } });

    const banner = container.querySelector('div');
    expect(banner?.className).toContain('outline-ghost-border');
  });
});
