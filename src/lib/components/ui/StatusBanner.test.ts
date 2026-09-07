import { describe, it, expect, vi } from 'vitest';
import { render, screen } from '@testing-library/svelte';
import StatusBanner from './StatusBanner.svelte';
import { fireEvent } from '@testing-library/svelte';

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

  it('renders an action button when provided and calls it', async () => {
    const onAction = vi.fn();
    render(StatusBanner, {
      props: { message: 'Copied — press ⌘V', actionLabel: 'Repair permission', onAction },
    });

    const button = screen.getByRole('button', { name: 'Repair permission' });
    await fireEvent.click(button);
    expect(onAction).toHaveBeenCalledOnce();
  });

  it('omits the action button when no handler is given', () => {
    render(StatusBanner, { props: { message: 'Just a message', actionLabel: 'Repair' } });
    expect(screen.queryByRole('button')).toBeNull();
  });
});
