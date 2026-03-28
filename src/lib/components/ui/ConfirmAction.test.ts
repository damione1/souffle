import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte';
import ConfirmAction from './ConfirmAction.svelte';

describe('ConfirmAction', () => {
  it('shows initial trigger button with label', () => {
    render(ConfirmAction, { props: { label: 'Delete', onConfirm: vi.fn() } });

    expect(screen.getByText('Delete')).toBeTruthy();
  });

  it('shows confirmation dialog after clicking trigger', async () => {
    render(ConfirmAction, { props: { label: 'Delete', confirmMessage: 'Really delete?', onConfirm: vi.fn() } });

    await fireEvent.click(screen.getByText('Delete'));

    expect(screen.getByText('Really delete?')).toBeTruthy();
    expect(screen.getByText('Yes')).toBeTruthy();
    expect(screen.getByText('Cancel')).toBeTruthy();
  });

  it('calls onConfirm when confirmed', async () => {
    const onConfirm = vi.fn();
    render(ConfirmAction, { props: { label: 'Delete', onConfirm } });

    await fireEvent.click(screen.getByText('Delete'));
    await fireEvent.click(screen.getByText('Yes'));

    expect(onConfirm).toHaveBeenCalledOnce();
  });

  it('hides confirmation when cancelled', async () => {
    render(ConfirmAction, { props: { label: 'Delete', onConfirm: vi.fn() } });

    await fireEvent.click(screen.getByText('Delete'));
    expect(screen.getByText('Cancel')).toBeTruthy();

    await fireEvent.click(screen.getByText('Cancel'));

    // Should show the trigger button again
    expect(screen.getByText('Delete')).toBeTruthy();
  });

  it('uses custom confirmLabel', async () => {
    render(ConfirmAction, { props: { label: 'Remove', confirmLabel: 'Confirm removal', onConfirm: vi.fn() } });

    await fireEvent.click(screen.getByText('Remove'));

    expect(screen.getByText('Confirm removal')).toBeTruthy();
  });
});
